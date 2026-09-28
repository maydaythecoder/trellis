//! A dense multi-pane chat interface over a local, OpenAI-compatible model.
//!
//! Three parts, each standalone:
//!
//! - `model` — the client, and a mock so the rest runs with nothing
//!   installed;
//! - `tui` — the interface, an Elm loop whose `update` is pure;
//! - `commands` — the slash-command system, including `/eng`, whose loop
//!   safety is enforced by `commands::runner` rather than by prompt text.

pub mod commands;
pub mod model;
pub mod tui;

/// Removes the hosted-key variables from a child's environment.
///
/// trellis does this with `trellis_core::secrets::scrub` (ADR 0024 §3), but
/// this crate depends on nothing in the workspace by design, so it removes
/// the two default key variables itself. It reads `OPENAI_API_KEY` for its
/// own requests, and a checker it runs that printed its environment would
/// show the key in the interface. Names a `[[providers]]` entry adds are
/// trellis configuration this crate never reads.
pub fn scrub_keys(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd.env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
}

/// `git` with a planted `core.fsmonitor` and hooks turned off, keys
/// removed: this crate's copy of `trellis_vcs::command`, for the reason
/// `scrub_keys` is a copy (it depends on nothing in the workspace). The
/// `-c` values are [`GIT_OVERRIDES`]; the workspace's source scan
/// (`every_git_spawn_goes_through_trellis_vcs`) allows this one site.
pub fn git_command() -> std::process::Command {
    let mut c = std::process::Command::new("git");
    c.args(GIT_OVERRIDES);
    scrub_keys(&mut c);
    c
}

/// A copy of `trellis_vcs::OVERRIDES`, written the same way so the two can
/// be compared as text: `chat_s_git_overrides_are_trellis_vcs_s` in
/// `trellis-cli`'s `tests/hygiene.rs` fails when they differ (review round
/// 11). A copy that drifted would leave this crate running a planted
/// `core.fsmonitor` while the rest of the workspace does not.
pub const GIT_OVERRIDES: [&str; 4] = ["-c", "core.fsmonitor=", "-c", NO_HOOKS];

#[cfg(windows)]
const NO_HOOKS: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const NO_HOOKS: &str = "core.hooksPath=/dev/null";
