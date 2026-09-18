//! The four adapter tiers, and what degrades on each.
//!
//! `trellis doctor` reports the active tier because the difference between
//! tier 3 and tier 4 is otherwise invisible to a user, and tier 4 is
//! materially worse (S17).

use serde::{Deserialize, Serialize};

/// How TRELLIS gets a well-formed tool call out of a model.
///
/// Ordered best to worst. Selected by probing model and runtime at startup,
/// never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AdapterTier {
    /// The model's own function-calling API. Used when present.
    Native,
    /// The compiled grammar is handed to the runtime's guided-decoding
    /// parameter (llama.cpp, vLLM, SGLang). Their embedded llguidance does
    /// the masking — the same engine, so the grammar artifact is identical
    /// to the one tier 3 would use locally.
    RuntimeConstrained,
    /// TRELLIS owns the sampling loop and applies the mask itself. The tier
    /// that makes any model work regardless of what it was trained for, and
    /// the default for local open weights. Needs logit access, hence
    /// in-process inference (S17).
    HarnessConstrained,
    /// Opaque API with neither native tools nor guided decoding: llguidance
    /// validates returned text and we retry on failure. Lossy and slow;
    /// the user is warned loudly.
    ValidateAndRetry,
}

impl AdapterTier {
    /// What the user loses on this tier. Shown by `trellis doctor`.
    pub fn degradation(self) -> &'static str {
        match self {
            AdapterTier::Native => "none: the model's own tool-calling API is in use",
            AdapterTier::RuntimeConstrained => {
                "none for correctness; grammar compilation happens in the serving runtime"
            }
            AdapterTier::HarnessConstrained => {
                "none for correctness; adds roughly 50us/token of mask computation"
            }
            AdapterTier::ValidateAndRetry => {
                "malformed tool calls are retried, not prevented; slower, \
                 and a run can fail on format alone"
            }
        }
    }

    /// Whether output is well-formed by construction rather than by luck.
    ///
    /// This is the property that makes small quantized models viable:
    /// quantization degrades format reliability faster than it degrades
    /// reasoning, so a 14B quant may reason adequately about the code and
    /// still emit malformed tool calls. Constraint deletes that failure
    /// mode; it does not improve the model's judgment (S7).
    pub fn is_constrained(self) -> bool {
        matches!(
            self,
            AdapterTier::RuntimeConstrained | AdapterTier::HarnessConstrained
        )
    }
}

/// Whether the serving backend does continuous batching.
///
/// S18: N agents against one local model queue on one GPU. llama.cpp and
/// Ollama are built for batch-size-1 single-user work and degrade under
/// concurrency; vLLM and SGLang do continuous batching. A user running four
/// agents against Ollama sees no speedup, may see worse, and blames
/// TRELLIS. So this is probed and reported rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchingCapability {
    /// Continuous batching: multi-agent mode is supported.
    Continuous {
        /// Measured concurrent-request capacity, the basis for
        /// `--agents auto`. Never derived from CPU count.
        measured_concurrency: u32,
    },
    /// Batch-size-1 oriented. Multi-agent mode degrades to sequential
    /// agents and says so.
    SingleStream,
    /// Could not be determined. Treated as `SingleStream`, and reported as
    /// unknown rather than as a measurement.
    Unknown,
}

impl BatchingCapability {
    /// Agent count to use for `--agents auto`.
    ///
    /// Anything but measured continuous batching yields 1: pretending
    /// otherwise produces a slower run the user cannot explain.
    pub fn auto_agents(self) -> u32 {
        match self {
            BatchingCapability::Continuous {
                measured_concurrency,
            } => measured_concurrency.max(1),
            BatchingCapability::SingleStream | BatchingCapability::Unknown => 1,
        }
    }

    pub fn supports_multi_agent(self) -> bool {
        matches!(self, BatchingCapability::Continuous { .. })
    }
}

/// What `trellis doctor` found.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Probe {
    pub model_id: String,
    pub runtime: String,
    pub tier: AdapterTier,
    pub batching: BatchingCapability,
    /// Present only when the repository's configured checks were found.
    pub typecheck_found: bool,
    pub tests_found: bool,
}

impl Probe {
    /// Lines for the doctor output. Plain statements, no reassurance.
    pub fn report(&self) -> Vec<String> {
        let mut out = vec![
            format!("model:      {}", self.model_id),
            format!("runtime:    {}", self.runtime),
            format!("tier:       {:?} — {}", self.tier, self.tier.degradation()),
        ];

        out.push(match self.batching {
            BatchingCapability::Continuous {
                measured_concurrency,
            } => format!(
                "batching:   continuous, measured concurrency {measured_concurrency}; \
                 multi-agent supported"
            ),
            BatchingCapability::SingleStream => "batching:   single-stream; multi-agent mode runs \
                 agents sequentially"
                .to_string(),
            BatchingCapability::Unknown => "batching:   unknown; treated as single-stream, \
                 multi-agent mode runs agents sequentially"
                .to_string(),
        });

        out.push(format!(
            "typecheck:  {}",
            if self.typecheck_found {
                "found"
            } else {
                "NOT FOUND — typecheck verdicts unavailable"
            }
        ));
        out.push(format!(
            "tests:      {}",
            if self.tests_found {
                "found"
            } else {
                "NOT FOUND — TRELLIS requires a test suite (v1 non-goal: repos without one)"
            }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_agents_never_guesses_above_one_without_a_measurement() {
        assert_eq!(BatchingCapability::SingleStream.auto_agents(), 1);
        assert_eq!(BatchingCapability::Unknown.auto_agents(), 1);
        assert_eq!(
            BatchingCapability::Continuous {
                measured_concurrency: 8
            }
            .auto_agents(),
            8
        );
    }

    #[test]
    fn unknown_batching_does_not_claim_multi_agent_support() {
        assert!(!BatchingCapability::Unknown.supports_multi_agent());
        assert!(!BatchingCapability::SingleStream.supports_multi_agent());
    }

    #[test]
    fn validate_and_retry_is_reported_as_unconstrained() {
        assert!(!AdapterTier::ValidateAndRetry.is_constrained());
        assert!(AdapterTier::HarnessConstrained.is_constrained());
        assert!(
            AdapterTier::ValidateAndRetry.degradation().contains("retried"),
            "the doctor line must name what degrades, not reassure"
        );
    }
}
