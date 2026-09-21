//! A terminal interface, split so that the part with no terminal in it can
//! be tested without one.
//!
//! `app` holds the state and the only function that changes it; `view` draws
//! and mutates nothing; `events` is the only place that awaits. That split is
//! what lets `update` be tested directly and `render` be tested against a
//! `TestBackend`, neither of which needs a tty.

pub mod app;
pub mod events;
pub mod message;
pub mod mock;
pub mod terminal;
pub mod view;
