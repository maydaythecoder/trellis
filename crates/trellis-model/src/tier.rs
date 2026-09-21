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
    /// Batches continuously, but the runtime does not report how much.
    ///
    /// vLLM, SGLang and ollama batch by design and expose no slot count, so
    /// "does it batch" and "how much" have different answers — and
    /// `Unknown` could only say the first one, wrongly. `doctor` said
    /// *"unknown; multi-agent mode runs agents sequentially"* on one line
    /// and *"batches continuously, so multi-agent mode is supported in
    /// principle"* two paragraphs later, leaving the reader to work out
    /// which was true.
    ///
    /// Still yields one agent: an unmeasured capacity is not a number to
    /// invent, and that half of the old line was right.
    ContinuousUnmeasured,
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
            BatchingCapability::SingleStream
            | BatchingCapability::ContinuousUnmeasured
            | BatchingCapability::Unknown => 1,
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
    /// `None` until a runtime has actually been probed.
    ///
    /// Not defaulted to a tier. `doctor` printed `ValidateAndRetry` when no
    /// `--model` was given while its own footer said tier was "reported as
    /// unknown rather than guessed" — so the page contradicted itself, and
    /// the half a user reads as a measurement was the untrue half. Which tier
    /// is active is the one thing S17 asks this command to report, and
    /// "unknown" is the honest answer before anything has been asked.
    pub tier: Option<AdapterTier>,
    pub batching: BatchingCapability,
    /// The configured type checker's version, or `None` when it could not be
    /// run at all.
    ///
    /// A version rather than a yes: `04-v1.0-harness.md` asks doctor to print
    /// `pyright 1.1.x`, and the reason is diagnostic. "found" cannot
    /// distinguish the checker the user configured from a different one
    /// earlier on `PATH`, and a verdict is only as good as the process that
    /// produced it.
    pub typecheck: Option<String>,
    pub tests: Option<String>,
    /// Whether a coverage plugin is available to the test runner.
    ///
    /// Not required, and not a check TRELLIS runs. It is reported because
    /// `Verified` versus `Reviewed` (S3) is the distinction the product turns
    /// on, and without coverage data nothing can ever populate `Verified` —
    /// so a user should know that before reading `0 verified` as a result.
    pub coverage_available: bool,
}

impl Probe {
    /// Lines for the doctor output. Plain statements, no reassurance.
    pub fn report(&self) -> Vec<String> {
        let mut out = vec![
            format!("model:      {}", self.model_id),
            format!("runtime:    {}", self.runtime),
            match self.tier {
                Some(t) => format!("tier:       {t:?} — {}", t.degradation()),
                None => "tier:       not probed; pass --model to find out".to_string(),
            },
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
            BatchingCapability::ContinuousUnmeasured => {
                "batching:   continuous, concurrency not reported; `--agents auto` stays at 1"
                    .to_string()
            }
            BatchingCapability::Unknown => "batching:   unknown; treated as single-stream, \
                 multi-agent mode runs agents sequentially"
                .to_string(),
        });

        // Every line that can be wrong says what to do about it. A diagnostic
        // that reports a problem without a next step has only told the user
        // they are stuck.
        out.push(match &self.typecheck {
            Some(v) => format!("typecheck:  {v}"),
            None => concat!(
                "typecheck:  NOT FOUND — no typecheck verdict can be produced, so no run ",
                "can converge.
",
                "            Install it, or set [checks] typecheck in .trellis/config.toml ",
                "to the command you use."
            )
            .to_string(),
        });
        out.push(match (&self.tests, self.coverage_available) {
            (Some(v), true) => format!("tests:      {v}, coverage available"),
            (Some(v), false) => format!(
                concat!(
                    "tests:      {}; no coverage plugin, so Verified stays 0 and ",
                    "every covered\n            call reads as Reviewed (S3)"
                ),
                v
            ),
            (None, _) => concat!(
                "tests:      NOT FOUND — the grounding contract has nothing to ground on.
",
                "            Install it, or set [checks] tests in .trellis/config.toml. A ",
                "repository
            without a test suite is a stated v1.0 non-goal."
            )
            .to_string(),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unprobed() -> Probe {
        Probe {
            model_id: "not configured".into(),
            runtime: "not configured".into(),
            tier: None,
            batching: BatchingCapability::Unknown,
            typecheck: Some("mypy 1.0".into()),
            tests: Some("pytest 8.0".into()),
            coverage_available: true,
        }
    }

    #[test]
    fn an_unprobed_tier_is_reported_as_unprobed_not_as_a_default() {
        // `doctor` with no --model printed `tier: ValidateAndRetry` while its
        // own footer said tier was "reported as unknown rather than guessed".
        // The page contradicted itself, and the half a reader takes as a
        // measurement was the untrue half.
        //
        // Which tier is active is the one thing S17 asks this command to
        // report. A default printed in its place is worse than no answer,
        // because tier 4 is a real state a runtime can be in and the reader
        // has no way to tell the guess from the finding.
        let lines = unprobed().report();
        let tier = lines
            .iter()
            .find(|l| l.starts_with("tier:"))
            .expect("a tier line");
        assert!(tier.contains("not probed"), "{tier}");
        assert!(
            !tier.contains("ValidateAndRetry") && !tier.contains("RuntimeConstrained"),
            "no tier may be named before one has been measured: {tier}"
        );
    }

    #[test]
    fn a_probed_tier_is_reported_with_what_it_costs() {
        // The other direction: once measured, the tier must be named *and*
        // carry its degradation, because "tier 4" means nothing to a reader
        // who does not already know the model.
        let mut p = unprobed();
        p.tier = Some(AdapterTier::ValidateAndRetry);
        let lines = p.report();
        let tier = lines
            .iter()
            .find(|l| l.starts_with("tier:"))
            .expect("a tier line");
        assert!(tier.contains("ValidateAndRetry"), "{tier}");
        assert!(
            tier.contains("format"),
            "a tier must arrive with what it costs: {tier}"
        );
    }

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
            AdapterTier::ValidateAndRetry
                .degradation()
                .contains("retried"),
            "the doctor line must name what degrades, not reassure"
        );
    }

    /// The seventh recurrence of the `cargo fmt` string-continuation defect
    /// landed in this file, and was caught by reading terminal output rather
    /// than by a test. Every other place it appeared got a local assertion;
    /// this one covers the whole report at once.
    ///
    /// A backslash-continued literal that loses its escape folds the source
    /// indentation into the string, leaving runs of spaces mid-sentence in
    /// text a user reads while trying to work out why their run will not
    /// start.
    #[test]
    fn no_report_line_carries_folded_indentation() {
        let cases = [
            Probe {
                model_id: "m".into(),
                runtime: "r".into(),
                tier: Some(AdapterTier::ValidateAndRetry),
                batching: BatchingCapability::SingleStream,
                typecheck: Some("mypy 1.0".into()),
                tests: Some("pytest 8.0".into()),
                coverage_available: false,
            },
            Probe {
                model_id: "m".into(),
                runtime: "r".into(),
                tier: None,
                batching: BatchingCapability::Unknown,
                typecheck: None,
                tests: None,
                coverage_available: true,
            },
        ];
        for p in cases {
            for line in p.report() {
                for seg in line.lines() {
                    // Past the label column. `model:      m` is deliberate
                    // alignment; a run of spaces after that is the defect,
                    // and so is one on an indented continuation line.
                    let body = seg
                        .split_once(':')
                        .map_or(seg, |(_, rest)| rest)
                        .trim_start();
                    assert!(
                        !body.contains("  "),
                        "folded indentation in a user-facing line: {body:?}"
                    );
                }
            }
        }
    }

    /// Every line that can be wrong has to say what to do about it.
    ///
    /// `04-v1.0-harness.md`: "Every line that can be wrong should say what to
    /// do about it." A diagnostic that reports a problem and stops there has
    /// only told the user they are stuck.
    #[test]
    fn a_missing_tool_says_what_to_do_about_it() {
        let p = Probe {
            model_id: "m".into(),
            runtime: "r".into(),
            tier: None,
            batching: BatchingCapability::Unknown,
            typecheck: None,
            tests: None,
            coverage_available: false,
        };
        let text = p.report().join(
            "
",
        );
        assert!(text.contains("NOT FOUND"), "{text}");
        assert!(
            text.contains(".trellis/config.toml"),
            "a missing checker must name where to configure one: {text}"
        );
        assert!(
            text.matches(".trellis/config.toml").count() >= 2,
            "both the checker and the test runner need the remedy, not just              whichever is listed first: {text}"
        );
    }

    #[test]
    fn only_measured_continuous_batching_supports_multi_agent() {
        // S18. `supports_multi_agent -> false` survived, and false is the
        // safe direction — which is exactly why it hid. The claim that needs
        // defending is the *true* one: multi-agent is enabled only by a
        // measured continuous-batching runtime, never by a default and never
        // by not knowing.
        assert!(
            BatchingCapability::Continuous {
                measured_concurrency: 4
            }
            .supports_multi_agent()
        );
        assert!(!BatchingCapability::SingleStream.supports_multi_agent());
        assert!(
            !BatchingCapability::Unknown.supports_multi_agent(),
            "not knowing is not permission"
        );
    }
}

#[cfg(test)]
mod batching_report_tests {
    use super::*;

    /// The batching line, for a given capability.
    fn line(b: BatchingCapability) -> String {
        let probe = Probe {
            model_id: "m".into(),
            runtime: "r".into(),
            tier: None,
            batching: b,
            typecheck: Some("mypy 1.0".into()),
            tests: Some("pytest 8.0".into()),
            coverage_available: true,
        };
        probe
            .report()
            .into_iter()
            .find(|l| l.starts_with("batching:"))
            .expect("the report always has a batching line")
    }

    #[test]
    fn a_runtime_that_batches_but_reports_no_count_is_not_called_single_stream() {
        // `doctor` against ollama said "unknown; treated as single-stream,
        // multi-agent mode runs agents sequentially" on one line, and
        // "batches continuously, so multi-agent mode is supported in
        // principle" two paragraphs below it. Both from the same probe.
        //
        // The type could not hold the real answer: "does it batch" and "how
        // much" have different ones, and `Unknown` could only express the
        // first, wrongly.
        let l = line(BatchingCapability::ContinuousUnmeasured);
        assert!(l.contains("continuous"), "{l}");
        assert!(
            !l.contains("single-stream"),
            "it batches; saying otherwise contradicts the note below it: {l}"
        );
        assert!(
            !l.contains("sequentially"),
            "and that was the half that was wrong: {l}"
        );
        assert!(
            l.contains("not reported"),
            "the unknown half still has to be said: {l}"
        );
    }

    #[test]
    fn an_unmeasured_capacity_still_yields_one_agent() {
        // The half of the old line that was right. A capacity nobody
        // measured is not a number to invent, and inventing one produces a
        // slower run the user cannot explain.
        assert_eq!(BatchingCapability::ContinuousUnmeasured.auto_agents(), 1);
        assert_eq!(BatchingCapability::Unknown.auto_agents(), 1);
        assert_eq!(BatchingCapability::SingleStream.auto_agents(), 1);
        assert_eq!(
            BatchingCapability::Continuous {
                measured_concurrency: 4
            }
            .auto_agents(),
            4
        );
    }

    #[test]
    fn genuinely_unknown_still_reads_as_unknown() {
        // The new variant must not swallow the old one: a runtime that
        // could not be probed at all is a different state from one that
        // batches without saying how much.
        let l = line(BatchingCapability::Unknown);
        assert!(l.contains("unknown"), "{l}");
        assert!(!l.contains("continuous"), "{l}");
    }
}
