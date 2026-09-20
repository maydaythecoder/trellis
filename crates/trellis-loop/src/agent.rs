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
    /// Which check ran, when one did.
    pub check_kind: Option<String>,
    /// `(resolved, ambiguous, unresolved)` from a radius query.
    ///
    /// Carried as numbers rather than recovered from `observation`, because
    /// the trail's factual section has to hold figures a reader can check —
    /// and re-parsing a string rendered for a model is not that.
    pub radius_counts: Option<(u32, u32, u32)>,
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

/// What a previous run established, read back from its committed trail.
///
/// Lines only, already fenced by `UntrustedTrail::context_lines`. This type
/// carries no `Verdict`, no `ToolResult` and nothing the loop can act on —
/// it is text for the prompt and nothing else.
#[derive(Debug, Clone)]
pub struct PriorContext {
    /// Where it came from, so a user can go and read the whole thing.
    pub source: String,
    /// Already-fenced lines. Untrusted: a trail lives in a repository that
    /// takes contributions (S10).
    pub lines: Vec<String>,
}

/// Everything the model is given to decide its next move.
pub struct Transcript {
    pub task: String,
    pub turns: Vec<Turn>,
    /// Set for the one extra turn granted after the budget runs out.
    pub final_notice: bool,
    /// Context carried in from a previous run.
    ///
    /// **Deliberately not `turns`, and this is the whole safety argument.**
    /// A `Turn` carries `verdict_passed`, and `last_check_passed` walks
    /// `turns` backwards to decide whether `done` counts as convergence. Seed
    /// a prior run's passing check as a turn and the very first thing a model
    /// could do is emit `done` — and the loop would call that a converged run,
    /// on the strength of a check executed against a tree that no longer
    /// exists and may never have existed on this branch.
    ///
    /// That is Constraint 2 defeated by a convenience. Correctness comes only
    /// from a process that executed *during this run*, so prior context
    /// reaches the prompt and is structurally invisible to the convergence
    /// test. The field being a separate type with no verdict in it is what
    /// makes that true rather than remembered.
    pub prior: Option<PriorContext>,
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
    /// Produces the next step, or a **terminal** failure.
    ///
    /// `Err` ends the run. Retryable conditions — a malformed tool call under
    /// tier 4, a dropped connection, a rate limit — are the adapter's business
    /// and must be retried inside this call, because only the adapter knows
    /// which of them are worth retrying. Surfacing a retryable error here
    /// would end a run that had not actually failed.
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
    run_with_prior(task, model, orch, budget, None).await
}

/// As [`run`], but seeded with what a previous run established.
///
/// The seed reaches the model's prompt and nothing else. See
/// [`Transcript::prior`] for why it must never reach `turns`.
#[tracing::instrument(skip_all, fields(task = %task, seeded = prior.is_some()))]
pub async fn run_with_prior<M: Model>(
    task: &str,
    model: &mut M,
    orch: &Handle,
    budget: &Budget,
    prior: Option<PriorContext>,
) -> (Outcome, Transcript) {
    let started = Instant::now();
    let mut t = Transcript {
        task: task.to_string(),
        turns: Vec::new(),
        final_notice: false,
        prior,
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
                let check_kind = match &result {
                    ToolResult::Checks(v) => Some(format!("{:?}", v.detail().kind)),
                    _ => None,
                };
                let radius_counts = match &result {
                    ToolResult::Radius(r) => Some((
                        r.coverage.reached_resolved,
                        r.coverage.reached_ambiguous,
                        r.coverage.unbound_in_touched_files,
                    )),
                    _ => None,
                };
                if verdict_passed == Some(true) {
                    checks_passed += 1;
                }
                t.turns.push(Turn {
                    observation: render(&call, &result),
                    call,
                    verdict_passed,
                    check_kind,
                    radius_counts,
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
fn render(call: &ToolCall, r: &ToolResult) -> String {
    match r {
        ToolResult::Radius(report) => {
            // Two facts, never a ratio between them. The previous version of
            // this string handed the model "92 of 2474 first-party call sites
            // resolved (3.7%)" — a division across two different populations
            // that reads as the resolver being useless. A model given that has
            // every reason to disregard the radius entirely, which is the
            // opposite of what S2's honesty mechanism is for.
            let c = &report.coverage;
            let top: Vec<String> = c
                .reasons_by_frequency()
                .into_iter()
                .take(3)
                .map(|(r, n)| format!("{r} {n}"))
                .collect();
            format!(
                concat!(
                    "{} call sites reach this symbol: {} resolved, {} ambiguous.
",
                    "Separately — a different population, not a fraction of the ",
                    "above — {} call sites across the {} files this radius touches ",
                    "could not be bound to any definition{}{}.
",
                    "Any of them may also reach this symbol. This is not a complete ",
                    "list of callers; roughly three call edges in ten are statically ",
                    "invisible in Python, so treat the resolved list as a floor, not ",
                    "a boundary."
                ),
                report.site_count(),
                report.certain.len(),
                report.ambiguous.len(),
                c.unbound_in_touched_files,
                c.touched_files,
                if top.is_empty() { "" } else { ", mostly " },
                top.join(", "),
            )
        }
        ToolResult::Checks(v) => {
            let d = v.detail();
            format!(
                "{:?}: {}\ncommand: {}{}{}\n{}",
                d.kind,
                if v.passed() { "passed" } else { "FAILED" },
                d.command,
                scope_note(call),
                unrun_note(call, d.kind),
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

/// Names the check kinds that were requested but never ran.
///
/// `run_checks` stops at the first failure, so a model that asked for several
/// kinds must be told which ones were skipped. Otherwise a failing type check
/// reads as a complete account, and a *passing* one reads as a clean bill of
/// health for a test suite that was never invoked.
///
/// Extracted as a pure function rather than tested through `render` because
/// constructing a `Verdict` outside `trellis-checks` is impossible by design
/// (ADR 0004). Adding a test-only constructor there would widen the seal that
/// makes Constraint 2 structural — so the logic comes out to meet the test
/// instead of the seal opening to meet the logic.
/// Says what a pathless check actually checked.
///
/// `paths` defaults to empty in the wire schema, and an empty argv means the
/// checker picks its own scope — for mypy, the whole repository. On a corpus
/// with pre-existing errors that returns a wall of failures from files the
/// task never mentioned, and the observation gave no hint that this had
/// happened: it looked exactly like a report about the model's own change.
///
/// Measured, not assumed. In the first S7 floor run the model opened every
/// task with `RunChecks { paths: [] }`, read the tail of the repository-wide
/// output, and edited a file named there instead of the one the task named.
/// Choosing not to pass the path is the model's error and is counted as one.
/// Leaving it unable to tell that its own error is what happened is ours.
fn scope_note(call: &ToolCall) -> String {
    match call {
        ToolCall::RunChecks { paths, .. } if paths.is_empty() => concat!(
            "
SCOPE: no paths were given, so this checked whatever the ",
            "checker picks by default — usually the whole repository, ",
            "including failures that were already there and are not yours. ",
            "Pass `paths` to check only the files the task names."
        )
        .to_string(),
        _ => String::new(),
    }
}

fn unrun_note(call: &ToolCall, ran: trellis_checks::CheckKind) -> String {
    let ToolCall::RunChecks { kinds, .. } = call else {
        return String::new();
    };
    let after = kinds.iter().position(|k| *k == ran).map_or(0, |i| i + 1);
    let skipped: Vec<String> = kinds[after.min(kinds.len())..]
        .iter()
        .map(|k| format!("{k:?}"))
        .collect();
    if skipped.is_empty() {
        String::new()
    } else {
        format!(
            "
NOT RUN: {} — stopped at the first failure.",
            skipped.join(", ")
        )
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

    /// A checker that really runs and really exits 0.
    ///
    /// The point of the loop is that a `Verdict` can only come from an
    /// executed process, so a convergence test that faked one would be
    /// testing the opposite of the property. The platform shell is the one
    /// executable guaranteed present wherever these tests run; trailing paths
    /// are appended by `typecheck` and ignored by `exit`.
    fn checker(code: u8) -> CheckConfig {
        let argv = if cfg!(windows) {
            vec!["cmd".into(), "/C".into(), format!("exit {code}")]
        } else {
            vec!["sh".into(), "-c".into(), format!("exit {code}")]
        };
        CheckConfig {
            typecheck: argv,
            ..CheckConfig::default()
        }
    }

    #[tokio::test]
    async fn a_run_whose_last_check_passed_converges() {
        // Every other test in this module asserts a refusal to converge, which
        // left the positive path defended by nothing: a change that broke
        // convergence outright would have passed the whole suite. It was
        // demonstrated by hand against a demo repository and never encoded.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), checker(0)).spawn();
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::Edit {
                path: "app/util.py".into(),
                old: "return 1".into(),
                new: "return 2".into(),
            }),
            Step::Call(ToolCall::RunChecks {
                paths: vec!["app/util.py".into()],
                kinds: vec![CheckKind::Typecheck],
            }),
            Step::Done {
                summary: "fixed it".into(),
            },
        ]);

        let (outcome, t) = run("fix it", &mut m, &h, &budget()).await;
        match outcome {
            Outcome::Converged {
                iterations,
                checks_passed,
            } => {
                assert_eq!(iterations, 2, "the edit and the check; done is not one");
                assert_eq!(checks_passed, 1);
            }
            other => panic!("expected convergence, got {other:?}"),
        }
        assert!(t.last_check_passed());
        // And the work was actually done, not merely declared.
        assert!(
            std::fs::read_to_string(d.path().join("app/util.py"))
                .unwrap()
                .contains("return 2")
        );
    }

    /// A checker whose verdict depends on the file, so one run can see a pass
    /// and then a failure.
    ///
    /// Needed because `run` builds a fresh `Transcript`: two sequential runs
    /// cannot test "most recent" at all, since the earlier run's turns are not
    /// in the later one's transcript. The distinction only exists inside a
    /// single transcript, so the check has to change its mind within one.
    /// The command names its own file and the call passes no `paths`, because
    /// `typecheck` appends them — and `findstr` reads a leading `/` as a
    /// switch, so an appended `app/util.py` makes it fail for a reason that
    /// has nothing to do with the marker. Caught by this test's own setup
    /// assertion, which is why that assertion is there.
    fn checker_wanting(marker: &str) -> CheckConfig {
        let argv = if cfg!(windows) {
            vec![
                "cmd".into(),
                "/C".into(),
                "findstr".into(),
                format!("/C:{marker}"),
                "app\\util.py".into(),
            ]
        } else {
            vec![
                "sh".into(),
                "-c".into(),
                format!("grep -q {marker} app/util.py"),
            ]
        };
        CheckConfig {
            typecheck: argv,
            ..CheckConfig::default()
        }
    }

    #[tokio::test]
    async fn a_pass_before_the_last_edit_does_not_count() {
        // `last_check_passed` is documented as "most recent, not any", because
        // a check that passed before the final edit says nothing about the
        // tree as it stands now. Nothing tested it, and `any` is the easier
        // implementation to reach for.
        //
        // Pass, then edit the passing property away, then check again and
        // fail, then claim success. A model that could converge on the earlier
        // pass would be converging on a tree that no longer exists — the same
        // hazard `PriorContext` is shaped around, reached from inside one run.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), checker_wanting("MARKER")).spawn();
        // No paths: the command names its own file. See `checker_wanting`.
        let check = || {
            Step::Call(ToolCall::RunChecks {
                paths: vec![],
                kinds: vec![CheckKind::Typecheck],
            })
        };
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::Edit {
                path: "app/util.py".into(),
                old: "return 1".into(),
                new: "return 1  # MARKER".into(),
            }),
            check(),
            Step::Call(ToolCall::Edit {
                path: "app/util.py".into(),
                old: "return 1  # MARKER".into(),
                new: "return 1".into(),
            }),
            check(),
            Step::Done {
                summary: "all good now".into(),
            },
        ]);

        let (outcome, t) = run("do it", &mut m, &h, &budget()).await;

        let verdicts: Vec<Option<bool>> = t.turns.iter().map(|x| x.verdict_passed).collect();
        assert_eq!(
            verdicts,
            vec![None, Some(true), None, Some(false)],
            "the setup itself must be a real pass followed by a real failure"
        );
        assert!(
            !t.last_check_passed(),
            "an earlier pass must not satisfy the convergence test"
        );
        assert!(
            matches!(
                outcome,
                Outcome::NotConverged {
                    reason: NotConverged::StoppedWithoutPassingChecks { .. },
                    ..
                }
            ),
            "got {outcome:?}"
        );
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
        assert!(
            obs.contains("not a complete list"),
            "the model must never be handed a radius that reads as exhaustive: {obs}"
        );
        assert!(
            obs.contains("could not be bound"),
            "the doubt is part of the observation, not an optional extra: {obs}"
        );
        assert!(
            obs.contains("files this radius touches"),
            "the doubt count is meaningless without the scale it ranges over: {obs}"
        );
    }

    #[tokio::test]
    async fn prior_context_can_never_make_a_run_converge() {
        // The hazard the whole seeding design is shaped around.
        //
        // A trail records that a previous run's checks passed. Seeding that as
        // a `Turn` would be the obvious implementation and would put
        // `verdict_passed: Some(true)` into `turns`, where `last_check_passed`
        // reads it. A model's very first move could then be `done`, and the
        // loop would report a converged run on the strength of a check
        // executed against a working tree that may no longer exist — Constraint
        // 2 defeated not by an attack but by a convenience.
        //
        // So `prior` is its own type with no verdict in it, and it is not
        // `turns`. This test is what keeps that true: a run seeded with the
        // richest possible prior context, whose model does nothing but declare
        // success, must still refuse to converge.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![Step::Done {
            summary: "the trail says the checks passed, so we are done".into(),
        }]);

        let prior = PriorContext {
            source: ".trellis/runs/20260919-000000-000-earlier.json".into(),
            lines: vec![
                "previous task: fix the type error".into(),
                "recorded check [Typecheck]: mypy (trail says passed)".into(),
                "recorded check [Tests]: pytest -q (trail says passed)".into(),
            ],
        };

        let (outcome, t) = run_with_prior("do it again", &mut m, &h, &budget(), Some(prior)).await;

        assert!(
            !outcome.converged(),
            "a recorded pass from a previous run is not a pass now"
        );
        assert!(
            matches!(
                outcome,
                Outcome::NotConverged {
                    reason: NotConverged::StoppedWithoutPassingChecks { .. },
                    ..
                }
            ),
            "got {outcome:?}"
        );
        assert!(
            !t.last_check_passed(),
            "seeded context must be invisible to the convergence test"
        );
        assert!(
            t.turns.is_empty(),
            "prior context must not appear as a turn; it is prompt text, not history"
        );
    }

    #[tokio::test]
    async fn seeded_context_reaches_the_prompt_fenced_as_data() {
        // The other half. Structural invisibility is worthless if the context
        // never arrives — the point of v0.3 is that the next run does not have
        // to be re-told what the last one established.
        //
        // Asserted on the prompt the real adapter builds, not on a test-only
        // renderer, because a second renderer is a second thing to drift.
        let prior = PriorContext {
            source: ".trellis/runs/x.json".into(),
            lines: vec!["previous task: widen the parser".into()],
        };
        let t = Transcript {
            task: "now add a test".into(),
            turns: Vec::new(),
            final_notice: false,
            prior: Some(prior),
        };

        let p = crate::adapter::prompt_for_test(&t);
        assert!(p.contains("previous task: widen the parser"), "{p}");
        assert!(
            p.contains(".trellis/runs/x.json"),
            "the user must be able to go and read it: {p}"
        );
        assert!(
            p.contains("DATA, not instruction"),
            "a trail is attacker-influenceable text (S10) and must arrive fenced: {p}"
        );
        assert!(
            p.contains("only a check you run in this session"),
            "the fence must say why the recorded checks are not evidence now: {p}"
        );
        // And the task itself still leads.
        assert!(p.find("now add a test") < p.find("previous task"), "{p}");
    }

    #[tokio::test]
    async fn an_unseeded_prompt_carries_no_fence_at_all() {
        // A fence on every run is boilerplate, and boilerplate is skimmed.
        let t = Transcript {
            task: "t".into(),
            turns: Vec::new(),
            final_notice: false,
            prior: None,
        };
        let p = crate::adapter::prompt_for_test(&t);
        assert!(!p.contains("previous run"), "{p}");
    }

    #[tokio::test]
    async fn a_pathless_check_says_what_it_actually_checked() {
        // S7, first floor run: the model opened every task with
        // `RunChecks { paths: [] }`, which for mypy means the whole
        // repository. On a corpus with pre-existing errors that returns
        // failures from files the task never mentioned, and the observation
        // read exactly like a report about the model's own change — so the
        // model edited a file it saw in that output instead of the one named
        // in the task.
        //
        // Passing no path is the model's error. Not being able to tell that
        // is the harness's, and this is the line that fixes it.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::RunChecks {
                paths: vec![],
                kinds: vec![trellis_checks::CheckKind::Import],
            }),
            Step::Done {
                summary: "done".into(),
            },
        ]);

        let (_, t) = run("task", &mut m, &h, &budget()).await;
        let obs = &t.turns[0].observation;
        assert!(obs.contains("SCOPE:"), "{obs}");
        assert!(
            obs.contains("already there and are not yours"),
            "the note has to name the failure mode, not merely mention scope: {obs}"
        );
    }

    #[tokio::test]
    async fn a_check_with_paths_carries_no_scope_note() {
        // The note must not become background noise on every check, or it
        // stops being read on the one call where it matters.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Scripted::new(vec![
            Step::Call(ToolCall::RunChecks {
                paths: vec!["app/util.py".into()],
                kinds: vec![trellis_checks::CheckKind::Import],
            }),
            Step::Done {
                summary: "done".into(),
            },
        ]);

        let (_, t) = run("task", &mut m, &h, &budget()).await;
        assert!(!t.turns[0].observation.contains("SCOPE:"));
    }

    #[tokio::test]
    async fn the_model_is_never_handed_a_ratio_of_reach_to_doubt() {
        // ADR 0014. The observation used to carry `Coverage::summary()`, which
        // divides sites reaching the symbol by every unbindable site in the
        // touched files — two populations, one percentage, and for
        // poetry.factory.Factory it read "92 of 2474 first-party call sites
        // resolved (3.7%)".
        //
        // Handed that, a model has every reason to conclude the graph is
        // useless and stop consulting it, which is the precise opposite of
        // what S2's honesty mechanism exists to achieve. So the observation
        // must state the two counts and never a percentage between them.
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
        assert!(
            !obs.contains('%'),
            "no percentage may appear in a radius observation; the only ratio \
             available is between two different populations: {obs}"
        );
        assert!(
            obs.contains("not a fraction of"),
            "the observation must say outright that the two counts are not a \
             ratio, because the next reader will otherwise compute one: {obs}"
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

    #[tokio::test]
    async fn adapter_internal_retries_are_invisible_to_the_loop() {
        // Tier 4 is validate-and-retry. A model that fails twice and then
        // succeeds must converge, not end the run — the loop only ever sees
        // the terminal outcome.
        struct Flaky {
            attempts: usize,
            steps: Vec<Step>,
            at: usize,
        }
        impl Model for Flaky {
            async fn next_step(&mut self, _: &Transcript) -> Result<Step, String> {
                // Stands in for the adapter's own retry loop.
                for _ in 0..3 {
                    self.attempts += 1;
                    if !self.attempts.is_multiple_of(3) {
                        continue; // malformed output; retry internally
                    }
                    let s = self.steps.get(self.at).cloned().unwrap_or(Step::Done {
                        summary: "done".into(),
                    });
                    self.at += 1;
                    return Ok(s);
                }
                Err("still malformed after retries".into())
            }
        }

        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        let mut m = Flaky {
            attempts: 0,
            steps: vec![Step::Call(ToolCall::BlastRadius {
                symbol: "helper".into(),
            })],
            at: 0,
        };
        let (outcome, t) = run("task", &mut m, &h, &budget()).await;
        assert_eq!(t.turns.len(), 1, "the retries must not appear as turns");
        assert!(matches!(
            outcome,
            Outcome::NotConverged {
                reason: NotConverged::StoppedWithoutPassingChecks { .. },
                ..
            }
        ));
        assert!(m.attempts > 2, "the adapter really did retry internally");
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
                    check_kind: Some("Typecheck".into()),
                    radius_counts: None,
                },
                Turn {
                    call: ToolCall::ReadFile { path: "b".into() },
                    observation: String::new(),
                    verdict_passed: Some(false),
                    check_kind: Some("Tests".into()),
                    radius_counts: None,
                },
            ],
            final_notice: false,
            prior: None,
        };
        assert!(!t.last_check_passed());
    }

    #[test]
    fn a_stopped_check_run_names_the_kinds_that_never_ran() {
        // A model that asks for [Typecheck, Tests] and sees only a Typecheck
        // result must not read that as "tests are fine".
        let call = ToolCall::RunChecks {
            paths: vec![],
            kinds: vec![CheckKind::Typecheck, CheckKind::Tests],
        };
        let note = unrun_note(&call, CheckKind::Typecheck);
        assert!(note.contains("NOT RUN"), "{note}");
        assert!(note.contains("Tests"), "{note}");
    }

    #[test]
    fn a_completed_check_run_reports_nothing_unrun() {
        let call = ToolCall::RunChecks {
            paths: vec![],
            kinds: vec![CheckKind::Typecheck, CheckKind::Tests],
        };
        assert_eq!(unrun_note(&call, CheckKind::Tests), "");
        // And a tool that is not a check run has nothing to say either.
        assert_eq!(
            unrun_note(&ToolCall::ReadFile { path: "a".into() }, CheckKind::Tests),
            ""
        );
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
