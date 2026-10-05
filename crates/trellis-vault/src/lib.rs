//! Validating an Apex-format note vault and compiling one note's context.
//!
//! A vault is a directory of Markdown notes. Each note's frontmatter is
//! exactly two keys, `title` and `parent`; `parent` is `null` or
//! `"[[stem]]"` naming exactly one other note, and the parent graph is
//! acyclic. `manifest.json` is the sorted list of every `.md` path under the
//! vault. [`validate`] checks all of that; [`compile`] turns one focus note
//! into a context block (ADR 0017).
//!
//! The behaviour is held to shared golden files under `tests/fixtures/`,
//! produced by the TypeScript prototype (ADR 0017); they are the
//! specification. Where the two languages differ in a primitive — string
//! ordering, what counts as whitespace — this crate does what JavaScript
//! does, because the goldens are the contract and a divergence that only
//! shows on unusual input is the kind nobody finds until it matters.
//!
//! Beyond the run gate, the crate reads a vault the way Apex Notes reads one
//! ([`Vault`], [`Graph`]), searches it ([`search`]), keeps `dates.json`
//! ([`dates`]) and edits it ([`edit`]) — ADR 0022. Reading is lenient;
//! the gate stays strict about the two-key schema.

mod compile;
pub mod dates;
pub mod edit;
mod graph;
mod js;
mod reader;
mod refs;
pub mod search;
mod validate;

pub use compile::{Compiled, DEFAULT_CAP, compile, compile_with_dates};
pub use graph::{Graph, Issue, Resolved, Resolver};
pub use reader::{Note, Vault, notes_root};
pub use validate::{Stats, validate};

use std::path::PathBuf;

/// Why a vault could not be validated or compiled.
///
/// Each failure a caller must handle differently has its own variant. An
/// over-cap vault is valid and an unknown stem is a typo; reporting either as
/// "invalid vault" sends the user looking for a problem that is not there.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// The vault breaks one or more format rules. Every violation found is
    /// listed, not just the first: fixing a vault one error per run is a tax.
    #[error("invalid vault:\n{}", violations.join("\n"))]
    Invalid {
        violations: Vec<String>,
        /// Counted even when invalid, so a caller can say how far off the
        /// manifest is.
        stats: Stats,
    },

    /// The context is larger than the cap with every reference body already
    /// dropped. The lineage is never truncated, so there is nothing left to
    /// drop.
    #[error(
        "context for {stem} is {minimum} bytes with every reference body dropped; cap is {cap}"
    )]
    OverCap {
        stem: String,
        minimum: usize,
        cap: usize,
    },

    #[error("unknown stem: {0}")]
    UnknownStem(String),

    #[error("ambiguous stem: {stem} ({})", paths.join(", "))]
    AmbiguousStem { stem: String, paths: Vec<String> },

    /// An edit that would break the vault or cannot mean anything: a parent
    /// under its own child, a note linking to itself, `init` over an
    /// existing vault. Nothing was written.
    #[error("{0}")]
    Refused(String),

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
