//! Shared identifiers, interning, and error types.
//!
//! This crate is deliberately dependency-light and knows nothing about
//! parsing, models, or processes. Everything else in the workspace may
//! depend on it; it depends on nothing in the workspace.

pub mod ids;
pub mod interner;

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
