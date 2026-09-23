//! Model adapter tiers, grammar compilation, and the runtime client.
//!
//! Tiers 2 and 4 are real: `runtime` speaks to llama.cpp, vLLM and SGLang,
//! putting the JSON Schema in whichever key that runtime reads. Tier 3 — the
//! harness owning the sampling loop — is **deliberately not built**, because
//! llguidance is already the grammar engine inside those runtimes, so for the
//! ones local users actually run, tier 2 delivers what tier 3 would with no
//! inference code here at all. See `docs/adr/0013-inference-path.md`, which
//! settled the question S17 left open.

pub mod catalog;
#[cfg(feature = "grammar")]
mod grammar;
mod runtime;
mod spec;
mod tier;

pub use catalog::{CATALOG, CatalogEntry, DEFAULT};
#[cfg(feature = "grammar")]
pub use grammar::{GrammarError, ToolGrammar};
pub use runtime::{Completion, Dialect, RuntimeClient, RuntimeError};
pub use spec::{ModelSpec, SpecError};
pub use tier::{AdapterTier, BatchingCapability, Probe};
