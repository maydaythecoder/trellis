//! The interface: Model / Message / Update / View.
//!
//! The split is load-bearing rather than stylistic. `app` holds the state
//! and the one function that changes it, and that function is synchronous
//! and does no IO — so every behaviour in this interface is testable with no
//! terminal. `events` is the only module that `.await`s. `view` is a pure
//! function of the state.
//!
//! **Nothing here may write to stdout.** Stdout belongs to the display, and
//! a stray `println!` corrupts it in a way that reads as a rendering bug.
//! `tracing` goes to a file subscriber and to the in-app log pane; there is
//! a test in `chat.rs` that greps this directory for print macros.

pub mod app;
pub mod events;
pub mod message;
pub mod terminal;
pub mod theme;
pub mod view;
pub mod widgets;
