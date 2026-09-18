//! Model adapter tiers and grammar compilation.
//!
//! Status: the tier taxonomy, probe result, and schema-to-grammar path are
//! real. The sampling loop is not — it needs logit access, which means
//! in-process inference, which is an open decision (S17). See
//! `docs/ADRs/0006-adapter-tiers-and-inference-path.md`, which is
//! deliberately left `Proposed` until v0.2 measures the options.

pub mod grammar;
pub mod tier;

pub use grammar::{GrammarError, ToolGrammar};
pub use tier::{AdapterTier, BatchingCapability, Probe};
