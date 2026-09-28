//! The tool contract.
//!
//! One flat tool list, one model, iterating. Blast radius and checks are
//! tools the model calls, not stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

use serde::Serialize;
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
