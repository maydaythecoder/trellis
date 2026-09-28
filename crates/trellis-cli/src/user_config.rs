//! The user-level configuration directory and how its `config.toml` layers
//! under the repository's (ADR 0025 §7).
//!
//! The rule in one line: the repository wins field by field, except where a
//! field is a restriction. There the two files add up, and neither can relax
//! the other.

use crate::config::{
    BudgetSection, ChecksSection, ContextSection, ControlSection, GovernanceSection, ModelSection,
    RepoConfig,
};
use std::ffi::OsString;
use std::path::PathBuf;

/// The user config directory, from the environment.
///
/// `None` in this crate's unit tests. A developer's own file would otherwise
/// change what every test that loads a config sees, and a test that passes on
/// one machine and fails on another is measuring the machine. The resolution
/// itself is [`resolve`], which is tested directly.
pub(crate) fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    resolve(&|k| std::env::var_os(k), cfg!(windows))
}

/// `<dir>/config.toml`.
pub(crate) fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("config.toml"))
}

/// Environment variables only; no `dirs` crate. First match wins:
/// `TRELLIS_CONFIG_DIR`, then `%APPDATA%\trellis` on Windows, then
/// `$XDG_CONFIG_HOME/trellis`, then `$HOME/.config/trellis`.
///
/// An empty variable counts as unset: `XDG_CONFIG_HOME=` is how a shell
/// clears one, and joining `trellis` onto an empty path would read a
/// relative `trellis/` in whatever directory the command ran in.
pub(crate) fn resolve(env: &dyn Fn(&str) -> Option<OsString>, windows: bool) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = get("TRELLIS_CONFIG_DIR") {
        return Some(d);
    }
    if windows && let Some(d) = get("APPDATA") {
        return Some(d.join("trellis"));
    }
    if let Some(d) = get("XDG_CONFIG_HOME") {
        return Some(d.join("trellis"));
    }
    get("HOME").map(|h| h.join(".config").join("trellis"))
}

/// The repository's configuration over the user's.
///
/// Struct-update syntax at the end on purpose: a section added later (W-B's
/// `[tools]` and `[hooks]`) comes from the repository alone until someone
/// decides how it layers, rather than compiling into a merge nobody chose.
pub(crate) fn layer(repo: RepoConfig, user: RepoConfig) -> RepoConfig {
    RepoConfig {
        checks: either(repo.checks, user.checks, |r, u| ChecksSection {
            typecheck: r.typecheck.or(u.typecheck),
            tests: r.tests.or(u.tests),
            python: r.python.or(u.python),
        }),
        budget: either(repo.budget, user.budget, |r, u| BudgetSection {
            // Lower wins: ADR 0008, a runaway loop spends the user's money,
            // and a cloned repository must not raise a cap they set.
            max_iterations: lower(r.max_iterations, u.max_iterations),
            max_wall_clock_s: lower(r.max_wall_clock_s, u.max_wall_clock_s),
            max_tokens: lower(r.max_tokens, u.max_tokens),
            regime: r.regime.or(u.regime),
        }),
        model: either(repo.model, user.model, |r, u| ModelSection {
            spec: r.spec.or(u.spec),
            unconstrained: r.unconstrained.or(u.unconstrained),
        }),
        context: either(repo.context, user.context, |r, u| ContextSection {
            vault: r.vault.or(u.vault),
            focus: r.focus.or(u.focus),
            files: r.files.or(u.files),
        }),
        governance: either(repo.governance, user.governance, |r, u| GovernanceSection {
            deny_write: union(r.deny_write, u.deny_write),
        }),
        control: either(repo.control, user.control, |r, u| ControlSection {
            // Approve is the restriction, so either file asking for it gets
            // it. An unknown word is kept as written from whichever file has
            // it, so `RepoConfig::control` still refuses it by name.
            mode: match (r.mode, u.mode) {
                (Some(a), Some(b)) if a == "auto" => Some(b),
                (Some(a), _) => Some(a),
                (None, b) => b,
            },
            approve: union(r.approve, u.approve),
        }),
        agents: repo.agents.or(user.agents),
        ..repo
    }
}

fn either<T>(r: Option<T>, u: Option<T>, both: impl FnOnce(T, T) -> T) -> Option<T> {
    match (r, u) {
        (Some(r), Some(u)) => Some(both(r, u)),
        (r, u) => r.or(u),
    }
}

fn lower<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn union(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            for x in b {
                if !a.contains(&x) {
                    a.push(x);
                }
            }
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

/// The dotted path of the first thing in `v` that looks like a key, if any.
///
/// A name test and a value test. Names: `api_key`, `apikey`, `key`,
/// `secret`, `token`, `password`, or anything ending `_key`, `_secret`,
/// `_token` or `_password`. `max_tokens` ends `_tokens` and is not matched.
/// Values: any string starting `sk-`, which is how the two most common
/// providers' keys begin, wherever it sits.
///
/// Returns the path, never the value: the refusal names where to look, and a
/// message that quoted the key would put it in a terminal's scrollback.
pub(crate) fn key_in(v: &toml::Value) -> Option<String> {
    fn keyish(name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        matches!(
            n.as_str(),
            "api_key" | "apikey" | "key" | "secret" | "token" | "password"
        ) || ["_key", "_secret", "_token", "_password", "-key", "-token"]
            .iter()
            .any(|s| n.ends_with(s))
    }
    fn walk(v: &toml::Value, at: &str) -> Option<String> {
        match v {
            toml::Value::String(s) if s.trim_start().starts_with("sk-") => Some(at.to_string()),
            toml::Value::Table(t) => t.iter().find_map(|(k, x)| {
                let here = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                if keyish(k) {
                    Some(here)
                } else {
                    walk(x, &here)
                }
            }),
            toml::Value::Array(xs) => xs
                .iter()
                .enumerate()
                .find_map(|(i, x)| walk(x, &format!("{at}[{i}]"))),
            _ => None,
        }
    }
    walk(v, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ControlMode;
    use std::path::Path;

    fn files(repo: &str, user: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        if !repo.is_empty() {
            std::fs::write(d.path().join(".trellis/config.toml"), repo).unwrap();
        }
        let u = d.path().join("user.toml");
        std::fs::write(&u, user).unwrap();
        (d, u)
    }

    fn load(repo: &str, user: &str) -> RepoConfig {
        let (d, u) = files(repo, user);
        RepoConfig::load_with_user(d.path(), Some(&u)).unwrap()
    }

    #[test]
    fn the_repository_wins_field_by_field_and_the_user_fills_the_rest() {
        let c = load(
            "[checks]\ntypecheck = [\"pyright\"]\n[model]\nspec = \"scripted:a.json\"\n",
            "[checks]\ntests = [\"pytest\", \"-q\"]\ntypecheck = [\"mypy\"]\n\
             [model]\nspec = \"scripted:b.json\"\nunconstrained = true\n",
        );
        assert_eq!(c.checks().typecheck, ["pyright"]);
        assert_eq!(c.checks().tests, ["pytest", "-q"]);
        assert_eq!(c.model().as_deref(), Some("scripted:a.json"));
        assert!(c.unconstrained(), "a field the repository left unset");
    }

    #[test]
    fn a_user_file_cannot_relax_the_repositorys_approve_mode() {
        // The risk: a user-level `mode = "auto"` turning off the approval a
        // repository's owner asked for, on a machine the owner never sees.
        let c = load(
            "[control]\nmode = \"approve\"\napprove = [\"edit\"]\n",
            "[control]\nmode = \"auto\"\napprove = []\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.mode, ControlMode::Approve);
        assert_eq!(dial.approve, ["edit"]);
    }

    #[test]
    fn a_user_file_can_add_approval_the_repository_did_not_ask_for() {
        let c = load(
            "[control]\nmode = \"auto\"\n",
            "[control]\nmode = \"approve\"\napprove = [\"read_file\"]\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.mode, ControlMode::Approve);
        assert_eq!(dial.approve, ["read_file"]);
        // A flag is a person deciding for one run, and still wins.
        assert_eq!(
            c.control(Some(ControlMode::Auto)).unwrap().mode,
            ControlMode::Auto
        );
    }

    #[test]
    fn approval_lists_and_denied_paths_add_up_and_never_shrink() {
        let c = load(
            "[control]\napprove = [\"edit\"]\n[governance]\ndeny_write = [\"vendor/\"]\n",
            "[control]\nmode = \"approve\"\napprove = [\"run_checks\", \"edit\"]\n\
             [governance]\ndeny_write = [\"*.lock\"]\n",
        );
        assert_eq!(c.control(None).unwrap().approve, ["edit", "run_checks"]);
        let rules: Vec<String> = c
            .governance()
            .rules()
            .iter()
            .map(|r| r.as_written())
            .collect();
        assert_eq!(rules, ["vendor/", "*.lock"]);
    }

    #[test]
    fn a_budget_ceiling_is_the_lower_of_the_two() {
        // ADR 0008: a cloned repository must not raise a cap the user set on
        // their own spend, and the user cannot lift the repository's either.
        let c = load(
            "[budget]\nmax_tokens = 2000000\nmax_iterations = 10\n",
            "[budget]\nmax_tokens = 1000000\nmax_iterations = 40\nmax_wall_clock_s = 60\n",
        );
        let b = c.budget();
        assert_eq!(b.max_tokens, 1_000_000);
        assert_eq!(b.max_iterations, 10);
        assert_eq!(b.max_wall_clock, std::time::Duration::from_secs(60));
    }

    #[test]
    fn context_files_can_be_turned_off_from_either_file() {
        let c = load("", "[context]\nfiles = false\n");
        assert_eq!(c.context.as_ref().and_then(|s| s.files), Some(false));
        let c = load("[context]\nfiles = true\n", "[context]\nfiles = false\n");
        assert_eq!(c.context.as_ref().and_then(|s| s.files), Some(true));
    }

    #[test]
    fn a_key_in_either_file_is_refused_and_the_value_is_not_repeated() {
        // ADR 0010. The message names where the key is so it can be removed,
        // and never the key, which would land in scrollback and logs.
        for (repo, user) in [
            ("[model]\napi_key = \"sk-live-abcdef\"\n", ""),
            (
                "",
                "[model]\nspec = \"x\"\nanthropic_token = \"abc123secret\"\n",
            ),
            ("", "[providers]\nheaders = [\"sk-ant-0123456789\"]\n"),
        ] {
            let (d, u) = files(repo, user);
            let e = RepoConfig::load_with_user(d.path(), Some(&u))
                .expect_err("a key must refuse the load");
            let msg = format!("{e:#}");
            assert!(msg.contains("ADR 0010"), "{msg}");
            for secret in ["sk-live-abcdef", "abc123secret", "sk-ant-0123456789"] {
                assert!(!msg.contains(secret), "the value leaked: {msg}");
            }
        }
    }

    #[test]
    fn budget_fields_that_merely_mention_tokens_are_not_keys() {
        let c = load("[budget]\nmax_tokens = 5\n", "");
        assert_eq!(c.budget().max_tokens, 5);
    }

    #[test]
    fn a_malformed_user_file_is_an_error_naming_it() {
        let (d, u) = files("", "[control\nmode=");
        let e = RepoConfig::load_with_user(d.path(), Some(&u)).unwrap_err();
        assert!(format!("{e:#}").contains("user.toml"), "{e:#}");
    }

    #[test]
    fn an_absent_user_file_is_the_repository_alone() {
        let d = tempfile::tempdir().unwrap();
        let c = RepoConfig::load_with_user(d.path(), Some(Path::new("/nonexistent/x.toml")));
        assert!(c.is_ok());
    }

    #[test]
    fn the_directory_comes_from_the_environment_in_a_fixed_order() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        let all = env(&[
            ("TRELLIS_CONFIG_DIR", "/t"),
            ("APPDATA", "/a"),
            ("XDG_CONFIG_HOME", "/x"),
            ("HOME", "/h"),
        ]);
        assert_eq!(resolve(&all, true), Some(PathBuf::from("/t")));
        let no_override = env(&[("APPDATA", "/a"), ("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(
            resolve(&no_override, true),
            Some(PathBuf::from("/a").join("trellis"))
        );
        assert_eq!(
            resolve(&no_override, false),
            Some(PathBuf::from("/x").join("trellis"))
        );
        let home = env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(
            resolve(&home, false),
            Some(PathBuf::from("/h").join(".config").join("trellis"))
        );
        assert_eq!(resolve(&env(&[]), false), None);
    }
}
