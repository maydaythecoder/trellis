//! The tool contract.
//!
//! One flat tool list, one model, iterating. Blast radius and checks are
//! tools the model calls, not stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trellis_checks::Verdict;
use trellis_index::RadiusReport;

/// What a tool hands back to the model.
///
/// There is deliberately **no pass variant**. A correctness claim can only
/// travel as `ToolResult::Checks(Verdict)`, and a `Verdict` is
/// constructible only inside `trellis-checks`, only from a process that
/// ran. So there is no code path by which the model's own output becomes a
/// verdict — not because a guideline forbids it, but because the types do
/// not permit it (Constraint 2, S4).
///
/// A contributor wanting to let the model say "looks fine" has to add a
/// variant here *and* widen the seal in `trellis-checks`. Both are loud in
/// a diff. That is the mechanism.
#[derive(Debug, Serialize)]
pub enum ToolResult {
    /// Blast radius, with its coverage attached.
    Radius(RadiusReport),
    /// The only way a correctness claim enters the loop.
    Checks(Verdict),
    FileContent(String),
    Edit(EditOutcome),
    /// What a listing or a command printed: `grep`, `find`, `ls`, `bash`, a
    /// custom tool, a created file (ADR 0023). Text for the model to read.
    /// An exit code of 0 is in here as a fact about a process, and it is
    /// not a correctness claim: only `Checks` is.
    Output(String),
    /// A tool failed. Note that this is *not* a correctness verdict either:
    /// a failed tool says nothing about whether the code is right.
    Error(String),
}

impl ToolResult {
    /// Whether this result carries a passing correctness claim.
    ///
    /// Everything that is not a `Verdict` answers `false`, including
    /// successful edits and successful file reads. Writing an edit is not
    /// evidence the edit was correct.
    pub fn is_passing_check(&self) -> bool {
        match self {
            ToolResult::Checks(v) => v.passed(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EditOutcome {
    pub path: String,
    pub applied: bool,
    /// Byte range replaced, for the trail.
    pub span: (usize, usize),
    /// The lines it changed, for the run's `turn` event (#802). Not
    /// serialized: a `post_tool` hook is sent this result as JSON, and its
    /// input is unchanged by a field only the event reads.
    #[serde(skip)]
    pub change: crate::gate::EditDiff,
}

// One step of the model's plan (ADR 0058 §5). Model output: wherever it is
// shown or stored it is quoted as data, and its `state` is the model's own
// account of its progress, from which nothing converges and no count is
// made. Inlined, both types, so the schema gains no root `$defs`: every
// run's grammar is built from that root, and an opt-in tool must leave the
// measured one byte-identical. The doc comment is the schema's description,
// which a model reads.
/// One step: what it is, and whether it is to do, current or done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PlanStep {
    pub text: String,
    pub state: PlanState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum PlanState {
    Todo,
    Current,
    Done,
}

/// The most steps a plan keeps. The Plan card's rule: a plan over eight
/// steps is a sign the task should have been split.
pub(crate) const MAX_PLAN_STEPS: usize = 8;

/// The longest step text kept, in characters. It is drawn on one line of a
/// checklist and sent in every `plan` event.
pub(crate) const MAX_PLAN_STEP_CHARS: usize = 200;

/// A plan as kept: at most [`MAX_PLAN_STEPS`] steps, each cut to
/// [`MAX_PLAN_STEP_CHARS`], and how much was cut to get there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptPlan {
    pub steps: Vec<PlanStep>,
    /// Steps past the eighth, dropped.
    pub steps_cut: usize,
    /// Kept steps whose text was shortened.
    pub texts_cut: usize,
}

/// Cuts a plan to what is kept. Cut, not refused (ADR 0058 §5): the steps
/// within the limits are still the model's plan, and the result says what
/// was cut. One function, so the tool's result, the `plan` event and the
/// trail's target cannot disagree about what was kept.
pub(crate) fn keep_plan(steps: &[PlanStep]) -> KeptPlan {
    let mut texts_cut = 0;
    let kept = steps
        .iter()
        .take(MAX_PLAN_STEPS)
        .map(|s| {
            if s.text.chars().count() <= MAX_PLAN_STEP_CHARS {
                return s.clone();
            }
            texts_cut += 1;
            PlanStep {
                text: s.text.chars().take(MAX_PLAN_STEP_CHARS).collect(),
                state: s.state,
            }
        })
        .collect();
    KeptPlan {
        steps: kept,
        steps_cut: steps.len().saturating_sub(MAX_PLAN_STEPS),
        texts_cut,
    }
}

impl KeptPlan {
    /// The tool's result text: what was kept, and what was cut.
    pub(crate) fn observation(&self) -> String {
        let mut out = format!(
            "plan recorded: {} step{} kept",
            self.steps.len(),
            if self.steps.len() == 1 { "" } else { "s" }
        );
        if self.steps_cut > 0 {
            out.push_str(&format!(
                "; {} step{} past the first {MAX_PLAN_STEPS} cut. A plan over {MAX_PLAN_STEPS} \
                 steps is a sign the task should be split",
                self.steps_cut,
                if self.steps_cut == 1 { "" } else { "s" }
            ));
        }
        if self.texts_cut > 0 {
            out.push_str(&format!(
                "; {} step text{} cut to {MAX_PLAN_STEP_CHARS} characters",
                self.texts_cut,
                if self.texts_cut == 1 { "" } else { "s" }
            ));
        }
        out.push_str(". It is shown to the person as your words; it is not a check.");
        for s in &self.steps {
            let mark = match s.state {
                PlanState::Done => "[x]",
                PlanState::Current => "[>]",
                PlanState::Todo => "[ ]",
            };
            // Quoted: model output, read back into later prompts.
            out.push_str(&format!("\n{mark} {:?}", s.text));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_but_a_verdict_carries_a_passing_claim() {
        assert!(!ToolResult::FileContent("def f(): pass".into()).is_passing_check());
        assert!(
            !ToolResult::Edit(EditOutcome {
                path: "a.py".into(),
                applied: true,
                span: (0, 10),
                change: crate::gate::EditDiff {
                    lines_added: 1,
                    lines_removed: 1,
                    start_line: 1,
                    diff: String::new(),
                },
            })
            .is_passing_check(),
            "a successful edit is not evidence the edit was correct"
        );
        assert!(!ToolResult::Error("boom".into()).is_passing_check());
        assert!(
            !ToolResult::Output("exit 0\nall tests passed".into()).is_passing_check(),
            "a command that says it passed is not a check that passed"
        );
    }

    /// Compile-time guard, not a runtime one. If someone adds a variant to
    /// `ToolResult` that could express success, this match stops compiling
    /// and they have to come here and think about it.
    #[test]
    fn tool_result_variants_are_accounted_for() {
        fn exhaustive(r: &ToolResult) -> &'static str {
            match r {
                ToolResult::Radius(_) => "no claim: a radius is a map, not a verdict",
                ToolResult::Checks(_) => "the only claim-bearing variant",
                ToolResult::FileContent(_) => "no claim",
                ToolResult::Edit(_) => "no claim",
                ToolResult::Output(_) => "no claim: a command's output, whatever its exit code",
                ToolResult::Error(_) => "no claim",
            }
        }
        assert_eq!(exhaustive(&ToolResult::Error("x".into())), "no claim");
    }
}
