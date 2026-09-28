//! An API key, held so that it cannot be printed by accident (ADR 0010).
//!
//! No `Display`, no `Serialize`, and a `Debug` that names only where the key
//! came from. A `tracing` field, a `{:?}` in an error, a derived `Debug` on a
//! struct that holds one: each of those would otherwise put the value in a
//! log line, and a log line is a file someone attaches to a bug report.
//!
//! The value leaves this type in exactly one place, [`ApiKey::expose`], which
//! is crate-private and called only while writing a request's header block.

/// A key read from an environment variable.
#[derive(Clone)]
pub struct ApiKey {
    value: String,
    /// The variable it came from, e.g. `ANTHROPIC_API_KEY`. Safe to print,
    /// and what every message names instead of the value.
    source: String,
}

impl ApiKey {
    /// Reads `var` from the environment. `None` when unset or blank.
    ///
    /// Environment only: ADR 0024 narrows ADR 0010 so no file trellis reads
    /// can hold a key.
    pub fn from_env(var: &str) -> Option<Self> {
        let value = std::env::var(var).ok()?;
        let value = value.trim().to_string();
        if value.is_empty() {
            return None;
        }
        Some(Self {
            value,
            source: var.to_string(),
        })
    }

    /// For tests: a key with a stated source, never read from anywhere.
    #[doc(hidden)]
    pub fn for_test(value: &str, source: &str) -> Self {
        Self {
            value: value.to_string(),
            source: source.to_string(),
        }
    }

    /// The variable the key came from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The value, for the header block and nothing else.
    pub(crate) fn expose(&self) -> &str {
        &self.value
    }

    /// `text` with every occurrence of the key replaced by a mention of its
    /// source. Applied to any provider text that becomes an error message,
    /// in case a provider echoes what it was sent.
    pub(crate) fn scrub(&self, text: &str) -> String {
        if self.value.len() < 4 {
            return text.to_string();
        }
        text.replace(&self.value, &format!("<key from {}>", self.source))
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiKey(from {})", self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_names_the_source_and_never_the_value() {
        // A derived Debug on anything holding a key is the easiest way for
        // one to reach a log line; this one cannot.
        let k = ApiKey::for_test("sk-very-secret-value", "OPENAI_API_KEY");
        let shown = format!("{k:?}");
        assert!(shown.contains("OPENAI_API_KEY"), "{shown}");
        assert!(!shown.contains("very-secret"), "{shown}");
    }

    #[test]
    fn scrub_removes_every_occurrence() {
        let k = ApiKey::for_test("sk-abcdef", "K");
        let s = k.scrub("got sk-abcdef, and again sk-abcdef.");
        assert!(!s.contains("sk-abcdef"), "{s}");
        assert!(s.contains("<key from K>"), "{s}");
    }
}
