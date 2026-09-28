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
