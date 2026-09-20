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
