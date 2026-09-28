//! Which environment variables a child process must not inherit
//! (ADR 0010, ADR 0024 §3).
//!
//! trellis reads a hosted provider's key from its own environment, and a
//! child process inherits that environment unless each spawn removes the
//! variable. A command that prints its environment (a check, `bash`, a
//! custom tool, a hook, a process agent) would otherwise put the key in an
//! observation, and from there in the log and the trail.
//!
//! Removal is per spawn, with `Command::env_remove`. The process-wide
//! `std::env::remove_var` would be simpler, but it is `unsafe` in edition
//! 2024 and the workspace forbids `unsafe`; it would also take the key from
//! trellis itself, which still needs it for its own requests.
//!
//! Two names are removed whether or not anything was registered, so a spawn
//! is safe even on a path where registration was missed. Every other name is
//! the `key_env` of a `[[providers]]` entry, registered by the binary once it
//! has read the effective configuration.

use std::sync::RwLock;

/// Removed from every child's environment unconditionally: the default key
/// variables of the two hosted protocols.
pub const BUILT_IN_KEY_VARS: [&str; 2] = ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"];

static REGISTERED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Adds variable names to the set every scrubbed spawn removes.
///
/// Appends; never replaces. The binary registers from the configuration at
/// start and again wherever it resolves a provider, and the second call must
/// not undo the first. Blank names are ignored.
pub fn register_key_vars<I, S>(names: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    // A poisoned lock means a panic mid-push; the Vec is still a valid list
    // of names, and refusing to add to it would only remove protection.
    let mut set = REGISTERED.write().unwrap_or_else(|e| e.into_inner());
    for n in names {
        let n = n.as_ref().trim();
        if !n.is_empty() && !set.iter().any(|s| s == n) {
            set.push(n.to_string());
        }
    }
}

/// Every name a scrubbed spawn removes: the built-ins, then the registered.
pub fn key_vars() -> Vec<String> {
    let set = REGISTERED.read().unwrap_or_else(|e| e.into_inner());
    let mut all: Vec<String> = BUILT_IN_KEY_VARS.iter().map(|s| s.to_string()).collect();
    for n in set.iter() {
        if !all.contains(n) {
            all.push(n.clone());
        }
    }
    all
}

/// Removes every key variable from `cmd`'s environment.
///
/// Call it after any `env`/`envs` on the same command, so nothing set there
/// can put a key back. A tokio command is scrubbed through
/// `tokio::process::Command::as_std_mut`.
pub fn scrub(cmd: &mut std::process::Command) -> &mut std::process::Command {
    for n in key_vars() {
        cmd.env_remove(n);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removed(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_built_in_key_variables_are_removed_even_when_nothing_was_registered() {
        // The guarantee that makes a missed registration a smaller leak
        // rather than the whole one.
        let mut c = std::process::Command::new("x");
        scrub(&mut c);
        let r = removed(&c);
        for n in BUILT_IN_KEY_VARS {
            assert!(r.iter().any(|k| k.eq_ignore_ascii_case(n)), "{r:?}");
        }
    }

    #[test]
    fn a_registered_variable_is_removed_and_set_env_cannot_put_it_back() {
        register_key_vars(["TRELLIS_CORE_TEST_KEY", " ", "TRELLIS_CORE_TEST_KEY"]);
        let mut c = std::process::Command::new("x");
        c.env("TRELLIS_CORE_TEST_KEY", "sk-canary");
        scrub(&mut c);
        assert!(
            removed(&c)
                .iter()
                .any(|k| k.eq_ignore_ascii_case("TRELLIS_CORE_TEST_KEY"))
        );
        assert_eq!(
            key_vars()
                .iter()
                .filter(|k| *k == "TRELLIS_CORE_TEST_KEY")
                .count(),
            1,
            "registered once, however often it is named"
        );
        assert!(!key_vars().iter().any(|k| k.trim().is_empty()));
    }
}
