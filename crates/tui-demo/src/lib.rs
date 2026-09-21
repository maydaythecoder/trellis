//! A standalone Ratatui interface, driven by a mock event source.
//!
//! Nothing here depends on the rest of the workspace. That is deliberate:
//! the interface can be run, resized, panicked and exited without a model, a
//! serving runtime or a repository, which is what makes its terminal
//! handling testable at all.
//!
//! Wiring it to real components is a separate job.

pub mod tui;
