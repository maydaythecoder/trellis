//! The call graph: high precision, explicitly incomplete.
//!
//! This crate depends on no model and never will. See
//! `docs/ADRs/0001-resolver-scope.md`.

pub mod coverage;
pub mod graph;
pub mod parse;
pub mod radius;

pub use coverage::Coverage;
pub use graph::{CallSiteEntry, Edge, FileEntry, Index, Reason, SymbolEntry, SymbolKind, Target};
pub use parse::{CallFact, CalleeShape, DefFact, FileFacts, ImportFact};
pub use radius::RadiusReport;
