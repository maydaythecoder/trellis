//! The call graph: high precision, explicitly incomplete.
//!
//! This crate depends on no model and never will. See
//! `docs/adr/0001-resolver-scope.md`.

pub mod cache;
pub mod coverage;
pub mod graph;
pub mod incremental;
pub mod parse;
pub mod radius;
pub mod resolve;

pub use coverage::Coverage;
pub use graph::{
    CallSiteEntry, CalleeKind, Edge, FileEntry, Index, Reason, SymbolEntry, SymbolKind, Target,
};
pub use incremental::Session;
pub use parse::{
    BindingFact, CallFact, CalleeShape, DefFact, FileFacts, ImportFact, TypeFact,
    collect as parse_facts,
};
pub use radius::{DoubtSite, FileDoubt, RadiusReport, doubt_in_files};
pub use resolve::{
    absolute_module, build, build_cached, build_from_facts, build_from_sources, discover,
    module_path_of, read_sources,
};
