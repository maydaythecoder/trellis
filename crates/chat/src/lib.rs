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
