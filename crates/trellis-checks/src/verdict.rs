//! The grounding contract.
//!
//! Constraint 2 of the brief: an LLM cannot verify its own work without an
//! external signal. Huang et al. (ICLR 2024) measured intrinsic
//! self-correction *degrading* accuracy across rounds; Chen et al. (ICLR
//! 2024) measured self-debugging working when anchored to test execution.
//! The difference is whether something ran.
//!
//! So: a `Verdict` is only constructible from the output of a process that
//! actually ran. The rule is enforced here, in the type system, and not in
//! a prompt or a review checklist, because under delivery pressure a
//! guideline is the first thing to soften. See
//! `docs/adr/0004-grounding-contract.md`.
//!
//! To let a model emit a pass verdict, a contributor would have to make the
//! `Seal` field public or add a constructor to this file. Both are visible
//! in a diff. That visibility is the entire mechanism.

use crate::ProcessOutput;

/// Private witness that a `Verdict` was built inside this crate.
///
/// `Seal` is private to the module and has no public constructor, so
/// `Verdict` cannot be built by struct-literal syntax from anywhere else,
/// including elsewhere in this crate.
#[derive(Debug, Clone, Copy)]
struct Seal;

/// What kind of executed check produced a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckKind {
    Typecheck,
    Tests,
    Import,
    /// A governance audit: the repository's own rules, evaluated
    /// mechanically. Sealed like any other check because convergence reads
    /// verdicts — a violation that never became one is a violation a model
    /// can converge straight past.
    Governance,
}

/// The executed check's own account of what happened.
///
/// `detail` is what the model reads to decide its next move, so it carries
/// the tool's structured output where one exists rather than a summary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckDetail {
    pub kind: CheckKind,
    /// The command line that ran, for the trail and for the user.
    pub command: String,
    pub exit_code: Option<i32>,
    /// Count of distinct failures where the tool reports one. This is the
    /// input to the check-recovery-rate metric (S7 metric 1): whether an
    /// iteration reduced the failure count.
    pub failure_count: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Proof that a check executed and what it concluded.
///
/// Constructible only from a `ProcessOutput`, only inside this crate.
///
/// `Serialize` but deliberately **not** `Deserialize`. A derived
/// `Deserialize` would be a public constructor taking arbitrary JSON, which
/// forges a passing verdict from outside the crate and defeats the seal
/// entirely. It is also precisely the hole S10 warns about: the trail is
/// committed to a repository that may take contributions, so a verdict read
/// back from it is untrusted input. The trail records verdicts; it never
/// reconstitutes one. `trellis-trail` reads them back as inert
/// `RecordedCheck` data that carries no authority.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Verdict {
    passed: bool,
    detail: CheckDetail,
    #[serde(skip)]
    _seal: Seal,
}

impl Verdict {
    pub(crate) fn from_typecheck(o: ProcessOutput) -> Self {
        Self::from_output(CheckKind::Typecheck, o)
    }

    pub(crate) fn from_tests(o: ProcessOutput) -> Self {
        Self::from_output(CheckKind::Tests, o)
    }

    pub(crate) fn from_governance(o: ProcessOutput) -> Self {
        Self::from_output(CheckKind::Governance, o)
    }

    pub(crate) fn from_import(o: ProcessOutput) -> Self {
        Self::from_output(CheckKind::Import, o)
    }

    fn from_output(kind: CheckKind, o: ProcessOutput) -> Self {
        // A check passes when the process it ran exited zero. Nothing here
        // consults a model, and there is no other path to `passed: true`.
        let passed = o.exit_code == Some(0);
        Self {
            passed,
            detail: CheckDetail {
                kind,
                command: o.command,
                exit_code: o.exit_code,
                failure_count: o.failure_count,
                stdout: o.stdout,
                stderr: o.stderr,
                duration_ms: o.duration_ms,
            },
            _seal: Seal,
        }
    }

    pub fn passed(&self) -> bool {
        self.passed
    }

    pub fn detail(&self) -> &CheckDetail {
        &self.detail
    }

    pub fn kind(&self) -> CheckKind {
        self.detail.kind
    }
}

/// How well a given call site in a blast radius is actually covered.
///
/// S3: "the callers still work" means two different things, and collapsing
/// them into one number is how a user ends up feeling lied to the first
/// time something breaks after a clean run. These are counted separately
/// in the trail and shown separately on the approval screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AuditMode {
    /// An executed check covered this call site and passed. Strong signal.
    Verified,
    /// No check reached it; a model read it and did not object. Weak signal,
    /// and labelled as one everywhere it appears.
    Reviewed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32) -> ProcessOutput {
        ProcessOutput {
            command: "pytest -q".into(),
            exit_code: Some(code),
            failure_count: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 1,
        }
    }

    #[test]
    fn verdict_tracks_process_exit_status() {
        assert!(Verdict::from_tests(out(0)).passed());
        assert!(!Verdict::from_tests(out(1)).passed());
        assert!(!Verdict::from_typecheck(out(2)).passed());
        assert_eq!(Verdict::from_import(out(0)).kind(), CheckKind::Import);
    }

    #[test]
    fn a_process_that_never_ran_cannot_pass() {
        let never_ran = ProcessOutput {
            exit_code: None,
            ..out(0)
        };
        assert!(
            !Verdict::from_tests(never_ran).passed(),
            "absence of an exit code must never read as success"
        );
    }
}
