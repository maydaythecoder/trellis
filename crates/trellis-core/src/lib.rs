//! Shared identifiers, interning, and error types.
//!
//! This crate is deliberately dependency-light and knows nothing about
//! parsing or models. Of processes it knows one thing: which environment
//! variables a child must not inherit (`secrets`), because every crate that
//! spawns one needs that list and all of them may depend on this crate.
//! Of paths it knows which ones a repository may hand to the prompt or to
//! a write (`paths`), for the same reason. And how to kill a child and what
//! it started (`process`), which the tool runner and the checks both need.
//! Everything else in the workspace may depend on it; it depends on nothing
//! in the workspace.

mod ids;
pub mod interner;
pub mod paths;
pub mod process;
pub mod secrets;

pub use ids::{CallSiteId, FileId, PartitionId, RunId, SymbolId};
pub use interner::Interner;

/// Errors that cross crate boundaries.
///
/// Library crates return these; the binary wraps them with `anyhow` for
/// display. See `docs/CLAUDE.md` for the convention.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("path is outside the repository root: {0}")]
    OutsideRoot(String),

    #[error("index is stale: {0}")]
    StaleIndex(String),

    #[error("worktree is not clean; TRELLIS requires a clean worktree before a run")]
    DirtyWorktree,

    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("io error: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;
