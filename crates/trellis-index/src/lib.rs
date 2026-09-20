//! The call graph: high precision, explicitly incomplete.
//!
//! This crate depends on no model and never will. See
//! `docs/ADRs/0001-resolver-scope.md`.

pub mod coverage;
pub mod graph;
pub mod parse;
pub mod radius;
pub mod resolve;

pub use coverage::Coverage;
pub use graph::{
    CallSiteEntry, CalleeKind, Edge, FileEntry, Index, Reason, SymbolEntry, SymbolKind, Target,
};
pub use parse::{BindingFact, CallFact, CalleeShape, DefFact, FileFacts, ImportFact};
pub use radius::RadiusReport;
pub use resolve::{build, build_from_sources, discover, module_path_of};
