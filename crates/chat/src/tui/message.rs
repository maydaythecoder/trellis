//! Everything that can reach the state, as one enum.
//!
//! One inbound type is what lets the event loop be a `select!` over sources
//! that know nothing about each other: the terminal, the model stream, the
//! tick. Adding a source means adding a variant, not a branch in the
//! drawing code.

use crate::commands::Command;
use ratatui::crossterm::event::KeyEvent;

#[derive(Debug, Clone)]
pub enum Message {
    Key(KeyEvent),
    Resize(u16, u16),
    /// Drives elapsed time and the spinner. Nothing else — see the note on
    /// redraw-on-change in `events.rs`.
    Tick,
    /// One chunk of assistant text.
    StreamToken(String),
    /// The stream ended normally.
    StreamDone,
    ToolCalled {
        name: String,
    },
    Error(String),
    /// A parsed slash command. `update` turns this into an `Effect`; it
    /// never carries one out.
    Command(Command),
    /// The runtime answering a command: one line for the conversation pane.
    Notice(String),
    /// The custom half of the registry, after a `/reload`.
    CommandsLoaded {
        specs: Vec<crate::commands::CommandSpec>,
        errors: Vec<String>,
    },
    /// Context accounting from the runtime, which owns the tokeniser.
    ContextUsed {
        tokens: u32,
    },
    Quit,
}
