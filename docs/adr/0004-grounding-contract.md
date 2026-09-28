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
