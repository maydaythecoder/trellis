# ADR 0004 — The grounding contract

**Status:** Accepted
**Date:** 2026-09-18

## Context

Huang et al. (ICLR 2024, DeepMind) found that without external feedback LLMs
struggle to self-correct and often degrade: GPT-4 on GSM8K fell from 95.5% to
91.5% to 89.0% across two rounds of intrinsic self-correction. A companion
result found models specifically bad at *detecting* errors, though capable of
fixing one they are pointed at. The MAST study (Cemri et al., 2025) annotated
1,642 multi-agent traces and found 41–86.7% failure rates across seven
frameworks, concluding better base models will not fix it.

Chen et al. (ICLR 2024) is the counterpart: self-debugging works consistently
and substantially **when anchored to unit-test execution**, and ablations show
the code execution is what does the work.

## Decision

Correctness verdicts come only from things that execute: type checker, test
runner, import check. The model reads their output and decides what to do next.
It never issues a pass verdict.

Enforced in the type system:

- `trellis_checks::Verdict` has a private `passed` field and a private `Seal`
  field. Three `pub(crate)` constructors, each taking a `ProcessOutput`.
- `Verdict` derives `Serialize` but **not** `Deserialize`. A derived
  `Deserialize` is a public constructor accepting arbitrary JSON and would
  defeat the seal entirely.
- `trellis_loop::ToolResult` has **no pass variant**. The only claim-bearing
  variant is `Checks(Verdict)`.
- A process that failed to spawn yields `exit_code: None`, which never reads as
  success.

### A pass describes the tree it ran against (amended 2026-09-27)

A `Verdict` is evidence about one tree. Convergence reads
`Transcript::last_check_passed`, which walks the run's turns from the end:
a verdict answers, but **a turn that may have changed the tree, reached
before any verdict, answers `false`**. The first version skipped every
verdict-less turn, so pass, then edit, then done converged on bytes no
check had seen. Found in review of PR #1; pre-existing since v0.2.

"May have changed the tree" is decided by the orchestrator, which owns the
only write path, and is conservative: an `edit` or `write_file` that reached
a write (whether or not it then succeeded), any `bash` or custom tool that
ran, and any turn around which a `pre_tool` or `post_tool` hook changed the
worktree (ADR 0023, "After a pass"). Only tools known to be read-only are
exempt. The match is exhaustive, so a new tool does not compile until
someone decides whether it writes. A run stopped this way ends
`ChangedAfterLastCheck`, whose explanation says the pass went stale rather
than that there was none.

### A check that does not finish fails (amended 2026-09-29, #19)

A hung mypy or pytest had no deadline: the loop reads its wall-clock budget
between turns, so one check could hold a run, and `trellis check`, for as
long as it hung. Every grounded check (type check, tests, import) now runs
under a bound:

- **`[checks] timeout_s`**, default **1800** seconds
  (`trellis_checks::DEFAULT_CHECK_TIMEOUT_S`). Where the repository's and the
  user's `config.toml` both set it, the lower wins, as for the budget: a
  cloned repository may shorten it, not lengthen the user's.
- In a run, the bound is `min(timeout_s, what is left of the wall-clock
  budget)`, measured when the loop dispatches the call and moved on by the
  time a person took to approve it (the budget does not charge that wait,
  so the check's share of it may not either). `trellis check` and the
  convergence audit use `timeout_s` alone.
- On expiry the check's whole process tree is killed with
  `trellis_core::process::kill_tree`, the helper `bash` and hooks already
  used (moved there from `trellis-loop`, not copied; its reach and limits
  are ADR 0023 §3's: on Unix the check leads its own process group, on
  Windows `taskkill /T` walks the tree). The result is
  `ProcessOutput { exit_code: None, stderr: "timed out after N s; the
  check's process tree was killed", .. }`, so the failing `Verdict` comes
  from the existing `pub(crate)` constructors. `passed`, the `Seal`, the
  constructors and the absent `Deserialize` are unchanged.
  *Amended by ADR 0035 (#172):* on Windows the checker runs in a Job
  Object, terminated however the check ends (on a timeout it replaces
  `taskkill /T`); on Unix the timeout path is unchanged.
- A check that exits but whose output is still held open at the bound (by
  something it left running) also yields `exit_code: None`: a verdict
  resting on cut output is not a pass. That case kills nothing, because the
  checker has been reaped and its group id may no longer be its own.
  *Amended by ADR 0035 (#172):* after any exit, the Windows job is
  terminated and the Unix group (held by a group holder) is killed, and a
  passing check whose tree changed after its exit was observed fails.
- A check whose run is dropped (interrupted, cancelled) has its checker process
  killed with it (the process only, not its tree) rather than left running
  unowned.

The bound can only turn a pass into a failure, never the reverse, so it
adds no way to converge.

## Alternatives considered

**A code-review guideline plus a lint.** Rejected. Under delivery pressure a
guideline is the first thing to soften, and this is the property the product
rests on. A guideline also produces no artefact in a diff.

**A runtime assertion** that verdicts originate from checks. Rejected: it fires
in production rather than at compile time, and can be deleted as easily as it
was added.

**Letting the model produce an advisory verdict alongside the real one.**
Rejected: the two would be rendered together and collapse in the user's mind.
The `Reviewed` audit mode already covers the legitimate version of this need,
explicitly labelled as a weak signal.

## Why

To let the model say "looks fine", a contributor must add a `ToolResult`
variant *and* widen the seal in a different crate. Both are loud in a diff.
That visibility is the entire mechanism — the goal is not to make the erosion
impossible but to make it impossible to do *quietly*.

This is also what makes small open-weight models viable. A design resting
correctness on model judgment is hardest to run on weak models; a design
resting it on a type checker barely cares which model you use.

## What changes our mind

Nothing short of the research being overturned. If a future result shows
reliable intrinsic self-verification, this ADR is reopened — but the bar is a
replicated result, not a strong impression from a new model release.
