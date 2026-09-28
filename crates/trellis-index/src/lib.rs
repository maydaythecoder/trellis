//! The call graph: high precision, explicitly incomplete.
//!
//! This crate depends on no model and never will. See
//! `docs/adr/0001-resolver-scope.md`.

pub mod cache;
mod coverage;
mod delta;
mod graph;
mod incremental;
pub mod modules;
pub mod parse;
pub mod radius;
mod resolve;

pub use coverage::Coverage;
pub use delta::{ChangeKind, IndexedSources, SymbolChange, symbol_delta};
pub use graph::{
    CallSiteEntry, CalleeKind, Edge, FileEntry, Index, Reason, SymbolEntry, SymbolKind, Target,
};
pub use incremental::Session;
pub use modules::{ModuleGraph, ModuleNode, module_graph};
pub use parse::{
    BindingFact, CallFact, CalleeShape, DefFact, FileFacts, ImportFact, TypeFact,
    collect as parse_facts,
};
pub use radius::{
    CallerSite, DoubtSite, FileDoubt, RadiusReport, callers_of_files, doubt_in_files,
};
pub use resolve::{
    absolute_module, build, build_cached, build_from_facts, build_from_sources, discover,
    discover_relative, module_path_of, read_sources,
};
