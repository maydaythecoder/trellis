//! Model adapter tiers, grammar compilation, and the runtime client.
//!
//! Tiers 2 and 4 are real: `runtime` speaks to llama.cpp, vLLM and SGLang,
//! putting the JSON Schema in whichever key that runtime reads. Tier 3 — the
//! harness owning the sampling loop — is **deliberately not built**, because
//! llguidance is already the grammar engine inside those runtimes, so for the
//! ones local users actually run, tier 2 delivers what tier 3 would with no
//! inference code here at all. See `docs/adr/0013-inference-path.md`, which
//! settled the question S17 left open.
//!
//! Tier 1 is `hosted`: a provider's own tool-calling API over HTTPS, with
//! TLS behind the `hosted` cargo feature (ADR 0024).

pub mod catalog;
pub mod colibri;
#[cfg(feature = "grammar")]
mod grammar;
mod hosted;
mod image;
pub mod pull;
mod runtime;
mod secret;
mod spec;
mod tier;
mod transport;

pub use catalog::{CATALOG, CatalogEntry, DEFAULT};
#[cfg(feature = "grammar")]
pub use grammar::{GrammarError, ToolGrammar};
pub use hosted::{
    Conversation, Exchange, HostedClient, MAX_ATTEMPTS, MAX_RETRY_WAIT, NativeCall, NativeTool,
    OVERLOADED_WAITS, Protocol, Reply, Thinking, deadline_after, every_turn_replayed,
    is_context_overflow, parse_reply, request_body, retry_delay, status_retry_delay, thinking_sent,
};
pub use image::{Image, ImageError, MAX_IMAGE_BYTES, redact_images};
pub use pull::{PullError, Pulled};
pub use runtime::{Completion, Dialect, RuntimeClient, RuntimeError};
pub use secret::ApiKey;
pub use spec::{ModelSpec, SpecError};
pub use tier::{AdapterTier, BatchingCapability, Probe};
pub use transport::{is_loopback, split_authority};

/// Feeds untrusted bytes to the hosted client's HTTP response parser, for the
/// `http_response` fuzz target (issue #97). A provider or proxy controls those
/// bytes, so the parser must return `Err` on anything malformed, never panic.
///
/// Compiled only under `cfg(fuzzing)`, which cargo-fuzz sets for every crate it
/// builds: the ordinary build keeps `parse_response` crate-private and gains no
/// public item and no Cargo feature (ADR 0027, decision log row #97).
#[cfg(fuzzing)]
pub fn fuzz_parse_response(raw: &[u8]) {
    let _ = transport::parse_response(raw);
}
