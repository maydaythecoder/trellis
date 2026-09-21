//! The slash-command system: specs, parsing, completion, and the registry.
//!
//! Parsing happens here and produces a `Command`. Nothing in this module
//! performs a side effect — a `Command` is a *request*, handed to the
//! runtime by `AppState::update` as an `Effect`. That split is what keeps
//! `update` synchronous and testable with no terminal and no filesystem.

pub mod eng;
pub mod loader;
pub mod runner;

use std::path::PathBuf;

/// What a command takes after its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    /// `/context`, `/clear` — anything after the name is a mistake.
    None,
    /// `/add <path>` — one required argument.
    One(&'static str),
    /// `/steer <direction>` — the rest of the line, verbatim.
    Rest(&'static str),
    /// `/git <branch|diff|status>` — one of a fixed set.
    OneOf(&'static str, &'static [&'static str]),
}

/// A command the interface offers, and what it costs to run it.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub name: String,
    pub description: String,
    pub args: ArgSpec,
    /// Requires an explicit `y` before it runs. Never remembered — see the
    /// note on `Confirm` in `app.rs`.
    pub destructive: bool,
    /// Loaded from `.harness/commands/`, rather than compiled in. Shown
    /// differently in the popup, because a custom command is a prompt
    /// someone else may have written.
    pub custom: bool,
}

impl CommandSpec {
    const fn builtin(
        name: &'static str,
        description: &'static str,
        args: ArgSpec,
        destructive: bool,
    ) -> BuiltinSpec {
        BuiltinSpec {
            name,
            description,
            args,
            destructive,
        }
    }
}

/// The compile-time form, so `BUILTINS` can be a `const`.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub args: ArgSpec,
    pub destructive: bool,
}

impl From<&BuiltinSpec> for CommandSpec {
    fn from(b: &BuiltinSpec) -> Self {
        CommandSpec {
            name: b.name.to_string(),
            description: b.description.to_string(),
            args: b.args,
            destructive: b.destructive,
            custom: false,
        }
    }
}

/// Which git snapshot to pull in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRef {
    Branch,
    Diff,
    Status,
}

impl GitRef {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "branch" => Ok(GitRef::Branch),
            "diff" => Ok(GitRef::Diff),
            "status" => Ok(GitRef::Status),
            other => Err(format!(
                "`{other}` is not a git snapshot. Use branch, diff or status."
            )),
        }
    }
}

/// A parsed command. Requests, never actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Add(PathBuf),
    Context,
    Clear,
    Compact,
    Git(GitRef),
    Steer(String),
    Stop,
    Approve,
    Agent(String),
    Delegate(String),
    Eng(String),
    Reload,
    Custom { name: String, args: Vec<String> },
}

impl Command {
    /// Whether running this needs an explicit yes.
    pub fn destructive(&self) -> bool {
        matches!(self, Command::Clear | Command::Compact)
    }
}

/// Every built-in, in the order the popup shows them.
pub const BUILTINS: &[BuiltinSpec] = &[
    CommandSpec::builtin(
        "add",
        "inject a file, folder or glob into context",
        ArgSpec::One("path"),
        false,
    ),
    CommandSpec::builtin(
        "context",
        "show what is loaded, with token cost per item",
        ArgSpec::None,
        false,
    ),
    // Destructive: history is the only record of what was decided and why,
    // and nothing else in the session holds a copy.
    CommandSpec::builtin("clear", "drop conversation history", ArgSpec::None, true),
    CommandSpec::builtin(
        "compact",
        "summarise history to reclaim window space",
        ArgSpec::None,
        true,
    ),
    CommandSpec::builtin(
        "git",
        "pull a git snapshot into context",
        ArgSpec::OneOf("ref", &["branch", "diff", "status"]),
        false,
    ),
    CommandSpec::builtin(
        "steer",
        "inject a mid-run course correction",
        ArgSpec::Rest("direction"),
        false,
    ),
    CommandSpec::builtin(
        "stop",
        "kill running child processes and halt the loop",
        ArgSpec::None,
        false,
    ),
    CommandSpec::builtin("approve", "pass a pending gate", ArgSpec::None, false),
    CommandSpec::builtin(
        "agent",
        "swap model or persona at runtime",
        ArgSpec::One("name"),
        false,
    ),
    CommandSpec::builtin(
        "delegate",
        "spawn an isolated sub-agent",
        ArgSpec::Rest("task"),
        false,
    ),
    CommandSpec::builtin(
        "eng",
        "generate a self-verifying loop as a new slash command",
        ArgSpec::One("name"),
        false,
    ),
    CommandSpec::builtin("reload", "rescan .harness/commands/", ArgSpec::None, false),
];

/// Built-ins plus whatever `.harness/commands/` currently holds.
#[derive(Debug, Clone)]
pub struct Registry {
    specs: Vec<CommandSpec>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::builtins_only()
    }
}

impl Registry {
    pub fn builtins_only() -> Self {
        Registry {
            specs: BUILTINS.iter().map(CommandSpec::from).collect(),
        }
    }

    /// Replaces the custom half, keeping built-ins. A loaded file can never
    /// shadow a built-in: `register_custom` drops any whose name collides,
    /// because a dropped-in `clear.md` that quietly replaced `/clear` is a
    /// command doing something other than what its name has always meant.
    pub fn register_custom(&mut self, loaded: Vec<CommandSpec>) -> Vec<String> {
        self.specs.retain(|s| !s.custom);
        let mut rejected = Vec::new();
        for spec in loaded {
            if BUILTINS.iter().any(|b| b.name == spec.name) {
                rejected.push(spec.name);
                continue;
            }
            self.specs.push(spec);
        }
        rejected
    }

    pub fn all(&self) -> &[CommandSpec] {
        &self.specs
    }

    pub fn find(&self, name: &str) -> Option<&CommandSpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    /// Specs whose name starts with `prefix`, for the popup. `prefix` is
    /// what follows the `/`, so an empty one lists everything.
    pub fn complete(&self, prefix: &str) -> Vec<&CommandSpec> {
        let mut v: Vec<&CommandSpec> = self
            .specs
            .iter()
            .filter(|s| s.name.starts_with(prefix))
            .collect();
        // Built-ins first, then alphabetical, so the list does not reorder
        // itself as custom commands are loaded and unloaded.
        v.sort_by(|a, b| a.custom.cmp(&b.custom).then_with(|| a.name.cmp(&b.name)));
        v
    }

    /// Parses a whole input line beginning with `/`.
    ///
    /// Returns `Err` with something a user can act on. An unparseable
    /// command must not fall through to the model as a prompt — a typo
    /// would otherwise become a turn.
    pub fn parse(&self, line: &str) -> Result<Command, String> {
        let line = line.trim();
        let body = line
            .strip_prefix('/')
            .ok_or_else(|| "not a command".to_string())?;
        let (name, rest) = match body.split_once(char::is_whitespace) {
            Some((n, r)) => (n, r.trim()),
            None => (body, ""),
        };
        if name.is_empty() {
            return Err("type a command name after the `/`".into());
        }

        let spec = self
            .find(name)
            .ok_or_else(|| format!("unknown command `/{name}`. Type `/` to see the list."))?;

        // Arity is checked against the spec rather than per command, so a
        // new command cannot forget to do it.
        match spec.args {
            ArgSpec::None if !rest.is_empty() => {
                return Err(format!("`/{name}` takes no arguments"));
            }
            ArgSpec::One(what) | ArgSpec::Rest(what) if rest.is_empty() => {
                return Err(format!("`/{name}` needs a {what}"));
            }
            ArgSpec::OneOf(what, allowed) => {
                if rest.is_empty() {
                    return Err(format!("`/{name}` needs a {what}: {}", allowed.join(", ")));
                }
                if !allowed.contains(&rest) {
                    return Err(format!(
                        "`{rest}` is not a {what}. Use {}.",
                        allowed.join(", ")
                    ));
                }
            }
            _ => {}
        }

        Ok(match name {
            "add" => Command::Add(PathBuf::from(rest)),
            "context" => Command::Context,
            "clear" => Command::Clear,
            "compact" => Command::Compact,
            "git" => Command::Git(GitRef::parse(rest)?),
            "steer" => Command::Steer(rest.to_string()),
            "stop" => Command::Stop,
            "approve" => Command::Approve,
            "agent" => Command::Agent(rest.to_string()),
            "delegate" => Command::Delegate(rest.to_string()),
            "eng" => {
                // The name becomes a filename and a command name, so it is
                // constrained here rather than at the point it is written.
                // A `/eng ../../etc/passwd` must fail as a *parse*, not as a
                // filesystem error after the loop has been described.
                if !eng::is_valid_name(rest) {
                    return Err(format!(
                        "`{rest}` is not a usable command name. Letters, digits, \
                         `-` and `_`, up to 40 characters."
                    ));
                }
                Command::Eng(rest.to_string())
            }
            "reload" => Command::Reload,
            other => Command::Custom {
                name: other.to_string(),
                args: rest.split_whitespace().map(str::to_string).collect(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_with_no_arguments_refuses_arguments() {
        let r = Registry::builtins_only();
        assert_eq!(r.parse("/context").unwrap(), Command::Context);
        assert!(r.parse("/context extra").is_err());
    }

    #[test]
    fn a_command_needing_an_argument_says_which_one() {
        let r = Registry::builtins_only();
        let e = r.parse("/add").unwrap_err();
        assert!(e.contains("path"), "got {e}");
    }

    #[test]
    fn a_fixed_set_argument_lists_the_set_when_it_is_wrong() {
        let r = Registry::builtins_only();
        assert_eq!(r.parse("/git diff").unwrap(), Command::Git(GitRef::Diff));
        let e = r.parse("/git sideways").unwrap_err();
        assert!(e.contains("branch") && e.contains("status"), "got {e}");
    }

    #[test]
    fn an_unknown_command_is_an_error_and_not_a_prompt() {
        // Falling through to the model would turn a typo into a turn — a
        // request the user never made, against a budget they are paying.
        let r = Registry::builtins_only();
        let e = r.parse("/contxt").unwrap_err();
        assert!(e.contains("unknown command"), "got {e}");
    }

    #[test]
    fn the_rest_of_the_line_reaches_steer_verbatim() {
        let r = Registry::builtins_only();
        assert_eq!(
            r.parse("/steer stop rewriting the tests").unwrap(),
            Command::Steer("stop rewriting the tests".into())
        );
    }

    #[test]
    fn eng_refuses_a_name_that_is_a_path() {
        // The name becomes a filename. Caught at parse time, so it fails
        // before anything is described, let alone written.
        let r = Registry::builtins_only();
        for bad in ["../escape", "a/b", "a\\b", "", "with space"] {
            assert!(
                r.parse(&format!("/eng {bad}")).is_err(),
                "`{bad}` was accepted"
            );
        }
        assert_eq!(
            r.parse("/eng test-loop").unwrap(),
            Command::Eng("test-loop".into())
        );
    }

    #[test]
    fn completion_filters_by_prefix_and_keeps_a_stable_order() {
        let r = Registry::builtins_only();
        let names: Vec<&str> = r.complete("c").iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["clear", "compact", "context"]);
        assert_eq!(r.complete("").len(), BUILTINS.len());
        assert!(r.complete("zzz").is_empty());
    }

    #[test]
    fn a_custom_command_cannot_shadow_a_builtin() {
        // A dropped-in `clear.md` that replaced `/clear` would be a command
        // doing something other than what its name has always meant, with
        // the user's credentials, in the user's repository.
        let mut r = Registry::builtins_only();
        let rejected = r.register_custom(vec![
            CommandSpec {
                name: "clear".into(),
                description: "not the real one".into(),
                args: ArgSpec::None,
                destructive: false,
                custom: true,
            },
            CommandSpec {
                name: "test-loop".into(),
                description: "fine".into(),
                args: ArgSpec::None,
                destructive: false,
                custom: true,
            },
        ]);
        assert_eq!(rejected, vec!["clear".to_string()]);
        assert_eq!(r.parse("/clear").unwrap(), Command::Clear);
        assert!(matches!(
            r.parse("/test-loop").unwrap(),
            Command::Custom { .. }
        ));
    }

    #[test]
    fn reloading_replaces_the_custom_set_rather_than_appending() {
        let mut r = Registry::builtins_only();
        let one = |n: &str| CommandSpec {
            name: n.into(),
            description: String::new(),
            args: ArgSpec::None,
            destructive: false,
            custom: true,
        };
        r.register_custom(vec![one("gone")]);
        r.register_custom(vec![one("kept")]);
        assert!(r.find("gone").is_none(), "a deleted file must disappear");
        assert!(r.find("kept").is_some());
    }

    #[test]
    fn destructive_is_declared_on_the_commands_that_lose_work() {
        assert!(Command::Clear.destructive());
        assert!(Command::Compact.destructive());
        assert!(!Command::Context.destructive());
        assert!(!Command::Add(PathBuf::from("x")).destructive());
    }
}
