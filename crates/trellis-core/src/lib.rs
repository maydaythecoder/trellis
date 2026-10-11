//! Shared identifiers and interning.
//!
//! This crate is deliberately dependency-light and knows nothing about
//! parsing or models. Of processes it knows one thing: which environment
//! variables a child must not inherit (`secrets`), because every crate that
//! spawns one needs that list and all of them may depend on this crate.
//! Of paths it knows which ones a repository may hand to the prompt or to
//! a write (`paths`), for the same reason. And how to kill a child and what
//! it started (`process`), which the tool runner and the checks both need.
//! And how to print repository text without letting it drive the terminal
//! (`text`), which the loop and the binary both do.
//! Everything else in the workspace may depend on it; it depends on nothing
//! in the workspace.

mod ids;
pub mod interner;
pub mod paths;
pub mod process;
pub mod secrets;
pub mod text;

pub use ids::{CallSiteId, FileId, PartitionId, RunId, SymbolId};
pub use interner::Interner;
