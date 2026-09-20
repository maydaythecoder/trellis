//! The loop: one model, a flat tool list, iterating.
//!
//! Not a committee and not a pipeline. The model chooses a tool each turn and
//! reads what comes back; blast radius and checks are tools it calls, not
//! stages written around it (ADR 0005).
//!
//! **Where Constraint 2 is enforced.** The model never reports success. It can
//! say it is finished, and the loop still refuses to call that convergence
//! unless a `Verdict` from a process that actually ran came back passing. The
//! model's opinion is an input to the loop's decision; it is not the decision.
//!
//! Concretely: `Step::Done` is routed through `Transcript::last_check_passed`,
//! which reads `Turn::verdict_passed`, which is `Some` only for
//! `ToolResult::Checks(Verdict)` — and a `Verdict` is constructible only
//! inside `trellis-checks`, only from a process that ran. So the chain from
//! "the model claims success" to "the run converged" passes through an
//! executed check or it does not complete.

use crate::budget::{Budget, Exhaustion, Spend};
use crate::orchestrator::Handle;
use crate::tools::ToolResult;
use std::time::Instant;
use trellis_checks::CheckKind;

/// What the model asks for on a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    BlastRadius {
        symbol: String,
    },
    ReadFile {
        path: String,
    },
    Edit {
        path: String,
        old: String,
        new: String,
    },
    RunChecks {
        paths: Vec<String>,
        kinds: Vec<CheckKind>,
    },
}

/// The model's move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Call(ToolCall),
    /// The model believes the work is done. **This is not a verdict** — see
    /// the module docs. The loop decides what it means.
    Done {
        summary: String,
    },
}

/// One completed turn, as the model will see it next time round.
#[derive(Debug, Clone)]
pub struct Turn {
    pub call: ToolCall,
    /// The tool's output, rendered for the model to read.
    pub observation: String,
    /// `Some(true)` only when a real `Verdict` came back passing. Every other
    /// outcome — a successful edit, a successful file read — is `None`.
    /// Writing a file is not evidence the write was correct.
    pub verdict_passed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotConverged {
    /// The model stopped while the last check was failing, or before any
    /// check had run at all.
    StoppedWithoutPassingChecks {
        summary: String,
    },
    BudgetExhausted(Exhaustion),
    ModelFailed(String),
}

impl NotConverged {
    /// A sentence the user can act on. Failure has to be legible, not just
    /// detected.
    pub fn explain(&self) -> String {
        match self {
            NotConverged::StoppedWithoutPassingChecks { summary } => format!(
                "the model stopped without a passing check. Its own summary was: {summary}. \
                 Nothing was accepted, because only an executed check can accept it."
            ),
            NotConverged::BudgetExhausted(e) => format!("budget exhausted: {}", e.explain()),
            NotConverged::ModelFailed(e) => format!("the model could not continue: {e}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Converged {
        iterations: u32,
        /// How many executed checks passed over the run.
        checks_passed: u32,
    },
    NotConverged {
        reason: NotConverged,
        iterations: u32,
    },
}

impl Outcome {
    pub fn converged(&self) -> bool {
        matches!(self, Outcome::Converged { .. })
    }
}

/// Everything the model is given to decide its next move.
pub struct Transcript {
    pub task: String,
    pub turns: Vec<Turn>,
    /// Set for the one extra turn granted after the budget runs out.
    pub final_notice: bool,
}

impl Transcript {
    /// Whether the most recent executed check passed.
    ///
    /// Deliberately "most recent" rather than "any": a check that passed
    /// before the last edit says nothing about the tree as it stands now.
    pub fn last_check_passed(&self) -> bool {
        self.turns
            .iter()
            .rev()
            .find_map(|t| t.verdict_passed)
            .unwrap_or(false)
    }
}

/// The model behind the loop. Implemented by the real adapter and, in tests,
/// by a script — which is what lets the loop's control flow be tested with no
/// inference anywhere near it.
pub trait Model {
    fn next_step(
        &mut self,
        t: &Transcript,
    ) -> impl std::future::Future<Output = Result<Step, String>> + Send;

    /// Tokens consumed so far, for the budget. Local adapters may report 0;
    /// the wall-clock and iteration ceilings still apply (S8).
    fn tokens_used(&self) -> u64 {
        0
    }
}

/// Drives the loop until it converges, the model stops, or a budget runs out.
///
/// Returns the outcome and the transcript; the caller owns what happens next,
/// because rollback is the orchestrator's business and reporting is the
/// trail's.
#[tracing::instrument(skip_all, fields(task = %task))]
pub async fn run<M: Model>(
    task: &str,
    model: &mut M,
    orch: &Handle,
    budget: &Budget,
) -> (Outcome, Transcript) {
    let started = Instant::now();
    let mut t = Transcript {
        task: task.to_string(),
        turns: Vec::new(),
        final_notice: false,
    };
    let mut iterations: u32 = 0;
    let mut checks_passed: u32 = 0;
    // The one extra turn the budget grants on exhaustion, so the model can
    // leave the tree in a describable state instead of being cut mid-edit.
    let mut escalated = false;

    loop {
        let spend = Spend {
            iterations,
            elapsed: started.elapsed(),
            tokens: model.tokens_used(),
        };
        if let Some(e) = budget.exceeded(&spend) {
            if escalated {
                tracing::warn!(exhaustion = ?e, "budget exhausted after escalation");
                return (
                    Outcome::NotConverged {
                        reason: NotConverged::BudgetExhausted(e),
                        iterations,
                    },
                    t,
                );
            }
            tracing::info!(exhaustion = ?e, "budget exhausted; escalating once");
            escalated = true;
            t.final_notice = true;
        }

        let step = match model.next_step(&t).await {
            Ok(s) => s,
            Err(e) => {
                return (
                    Outcome::NotConverged {
                        reason: NotConverged::ModelFailed(e),
                        iterations,
                    },
                    t,
                );
            }
        };

        match step {
            Step::Done { summary } => {
                // The whole architecture is this branch. The model says it is
                // finished; the loop asks the checks.
                return if t.last_check_passed() {
                    (
                        Outcome::Converged {
                            iterations,
                            checks_passed,
                        },
                        t,
                    )
                } else {
                    (
                        Outcome::NotConverged {
                            reason: NotConverged::StoppedWithoutPassingChecks { summary },
                            iterations,
                        },
                        t,
                    )
                };
            }
            Step::Call(call) => {
                iterations += 1;
                let result = dispatch(orch, &call).await;
                let verdict_passed = match &result {
                    ToolResult::Checks(v) => Some(v.passed()),
                    _ => None,
                };
                if verdict_passed == Some(true) {
                    checks_passed += 1;
                }
                t.turns.push(Turn {
                    call,
                    observation: render(&result),
                    verdict_passed,
                });
            }
        }
    }
}

async fn dispatch(orch: &Handle, call: &ToolCall) -> ToolResult {
    match call {
        ToolCall::BlastRadius { symbol } => orch.blast_radius(symbol).await,
        ToolCall::ReadFile { path } => orch.read_file(path).await,
        ToolCall::Edit { path, old, new } => orch.edit(path, old, new).await,
        ToolCall::RunChecks { paths, kinds } => orch.run_checks(paths.clone(), kinds.clone()).await,
    }
}

/// Renders a tool result for the model.
///
/// A radius renders its coverage alongside its reach, always — there is no
/// path by which the model sees who calls a symbol without also seeing how
/// much the graph could not resolve (S2).
fn render(r: &ToolResult) -> String {
    match r {
        ToolResult::Radius(report) => format!(
            "reached by {} call sites ({} resolved, {} ambiguous)\ncoverage: {}\n\
             This is not a complete list of callers; roughly three call edges in ten \
             are statically invisible in Python.",
            report.site_count(),
            report.certain.len(),
            report.ambiguous.len(),
            report.coverage.summary(),
        ),
        ToolResult::Checks(v) => {
            let d = v.detail();
            format!(
                "{:?}: {}\ncommand: {}\n{}",
                d.kind,
                if v.passed() { "passed" } else { "FAILED" },
                d.command,
                tail(&d.stdout, 40)
            )
        }
        ToolResult::FileContent(s) => s.clone(),
        ToolResult::Edit(e) => format!(
            "edited {} (applied: {}, bytes {}..{})",
            e.path, e.applied, e.span.0, e.span.1
        ),
        ToolResult::Error(e) => format!("error: {e}"),
    }
}

/// Check output is read from the bottom: the failure summary lives at the end.
fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::Orchestrator;
    use trellis_checks::CheckConfig;

    /// A model that plays a fixed script. The loop's control flow is the thing
    /// under test, so the model must be boring and deterministic.
    struct Scripted {
        steps: Vec<Step>,
        at: usize,
        seen_final_notice: bool,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps,
                at: 0,
                seen_final_notice: false,
            }
        }
    }

    impl Model for Scripted {
        async fn next_step(&mut self, t: &Transcript) -> Result<Step, String> {
            if t.final_notice {
                self.seen_final_notice = true;
            }
            let s = self.steps.get(self.at).cloned().unwrap_or(Step::Done {
                summary: "script exhausted".into(),
            });
            self.at += 1;
            Ok(s)
        }
    }

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        d
    }

    fn budget() -> Budget {
        Budget {
            max_iterations: 10,
            ..Budget::default()
        }
    }

    #[tokio::test]
    async fn a_model_that_declares_success_without_checks_does_not_converge() {
        // The single most important test in this crate. An agent that says
        // "done" having run nothing must not be recorded as having succeeded.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![Step::Done {
            summary: "looks fine to me".into(),
        }]);

        let (outcome, _) = run("task", &mut m, &h, &budget()).await;
        assert!(!outcome.converged());
        match outcome {
            Outcome::NotConverged {
                reason: NotConverged::StoppedWithoutPassingChecks { summary },
                ..
            } => assert_eq!(summary, "looks fine to me"),
            other => panic!("expected a refusal to converge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn work_without_a_check_still_does_not_converge() {
        // Editing files and reading a radius is activity, not evidence.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::BlastRadius {
                symbol: "helper".into(),
            }),
            Step::Call(ToolCall::Edit {
                path: "app/util.py".into(),
                old: "return 1".into(),
                new: "return 2".into(),
            }),
            Step::Done {
                summary: "edited it".into(),
            },
        ]);

        let (outcome, t) = run("task", &mut m, &h, &budget()).await;
        assert!(!outcome.converged());
        assert_eq!(t.turns.len(), 2);
        assert!(
            t.turns.iter().all(|turn| turn.verdict_passed.is_none()),
            "neither a radius nor an edit may carry a correctness claim"
        );
        // And the edit really happened — the loop refuses to *call it success*,
        // it does not refuse to do the work.
        let after = std::fs::read_to_string(d.path().join("app/util.py")).unwrap();
        assert!(after.contains("return 2"));
    }

    #[tokio::test]
    async fn a_radius_observation_always_carries_its_coverage() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::BlastRadius {
                symbol: "helper".into(),
            }),
            Step::Done {
                summary: "done".into(),
            },
        ]);

        let (_, t) = run("task", &mut m, &h, &budget()).await;
        let obs = &t.turns[0].observation;
        assert!(obs.contains("coverage:"), "{obs}");
        assert!(
            obs.contains("not a complete list"),
            "the model must never be handed a radius that reads as exhaustive: {obs}"
        );
    }

    #[tokio::test]
    async fn an_exhausted_budget_escalates_once_then_stops() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        // A model that never stops on its own.
        let steps = std::iter::repeat_n(
            Step::Call(ToolCall::BlastRadius {
                symbol: "helper".into(),
            }),
            50,
        )
        .collect();
        let mut m = Scripted::new(steps);

        let tight = Budget {
            max_iterations: 3,
            ..Budget::default()
        };
        let (outcome, t) = run("task", &mut m, &h, &tight).await;

        match outcome {
            Outcome::NotConverged {
                reason: NotConverged::BudgetExhausted(_),
                iterations,
            } => {
                // Three within budget, plus exactly one escalation turn.
                assert_eq!(iterations, 4, "escalation grants one turn, not an appeal");
            }
            other => panic!("expected budget exhaustion, got {other:?}"),
        }
        assert!(
            m.seen_final_notice,
            "the model must be told the turn is its last, or escalation is just a \
             wasted iteration"
        );
        assert_eq!(t.turns.len(), 4);
    }

    #[tokio::test]
    async fn a_model_error_is_reported_not_swallowed() {
        struct Broken;
        impl Model for Broken {
            async fn next_step(&mut self, _: &Transcript) -> Result<Step, String> {
                Err("adapter returned malformed output".into())
            }
        }
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let (outcome, _) = run("task", &mut Broken, &h, &budget()).await;
        match outcome {
            Outcome::NotConverged {
                reason: NotConverged::ModelFailed(e),
                ..
            } => assert!(e.contains("malformed")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_the_most_recent_check_counts() {
        // A check that passed before the last edit says nothing about the
        // tree as it now stands.
        let t = Transcript {
            task: "t".into(),
            turns: vec![
                Turn {
                    call: ToolCall::ReadFile { path: "a".into() },
                    observation: String::new(),
                    verdict_passed: Some(true),
                },
                Turn {
                    call: ToolCall::ReadFile { path: "b".into() },
                    observation: String::new(),
                    verdict_passed: Some(false),
                },
            ],
            final_notice: false,
        };
        assert!(!t.last_check_passed());
    }

    #[test]
    fn a_failure_explains_itself() {
        let r = NotConverged::StoppedWithoutPassingChecks {
            summary: "I think it works".into(),
        };
        let e = r.explain();
        assert!(e.contains("only an executed check"), "{e}");
    }
}
