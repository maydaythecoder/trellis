# ADR 0002 — Edge confidence model, and the thresholds that govern it

**Status:** Accepted
**Date:** 2026-09-18

## Context

Complete call-graph recall is impossible in Python. Dynamic dispatch, decorator
indirection, `getattr`, and conditional imports make roughly three edges in ten
statically invisible. This is a property of the language, not a tooling gap.

Yang et al. (MSR '22, over 3 million Python files) found genuinely unanalyzable
constructs like `eval` to be rare, while decorators and context managers are
widespread — so misses should be boring and common rather than exotic. The
benchmark candidates bear this out: 20–41% of definitions are decorated, while
`eval`/`exec` appears 0–9 times per repository.

## Decision

Every edge carries one of three outcomes, never two:

- `Resolved(SymbolId)` — exactly one first-party definition matches and nothing
  nearer shadows it.
- `Candidates(SmallVec<SymbolId>)` — a known finite set. All recorded, none
  chosen.
- `Unresolved(Reason)` — with a reason code from a closed enum.

Every run reports coverage. The harness never emits a bare "complete".

**Thresholds, chosen 2026-09-18, before any measurement exists:**

| | Value | Consequence |
|---|---|---|
| v1.0 gate | recall >= 85% | Ship the blast-radius claim |
| S1 fallback trigger | recall < 85% | Adopt a semantic backstop |
| **S16 pivot trigger** | **recall < 60%** | **The blast-radius guarantee dies** |

Recall is measured over call sites whose *true callee is first-party*, with
`Candidates` counted as **not** resolved. Full definition in
`BENCHMARK_AND_EVAL.md` §2.

## Alternatives considered

**Two-valued (resolved / unresolved).** Simpler, and wrong. It forces every
ambiguous site into one bucket: called "resolved" it is an unsound edge, called
"unresolved" it discards real information the user could act on. `Candidates`
is where a conditional import or a re-export chain honestly lands.

**A numeric confidence score per edge.** Rejected. A float invites thresholding
("show me edges above 0.8"), which reintroduces exactly the silent-miss problem
the model exists to prevent, and no principled way exists to calibrate the
score in the first place.

**Set the thresholds after measuring.** Rejected, emphatically. A threshold
chosen after seeing the number is a threshold chosen to be met. The brief calls
for writing the pivot trigger while it is cheap, and cheap means before.

## Why

A tool that confidently reports a blast radius missing the thing that breaks is
worse than no tool at all. The confidence model is the mechanism that prevents
it, and it only works if every consumer is forced to see coverage — hence
`RadiusReport` owning `Coverage` by value with no constructor that omits it,
and `Coverage::resolved_fraction()` returning `Option` so an empty sample can
never render as 100%.
*Amended by ADR 0014:* a radius carries `RadiusCoverage` (two facts, no
ratio); `Coverage::resolved_fraction` remains for the index only.

## What changes our mind

- If the `Reason` distribution turns out to be dominated by a variant we did
  not anticipate, add it — the enum is closed so that additions are deliberate,
  not to keep it small.
- If real usage shows users treating `Candidates` as equivalent to `Resolved`,
  the presentation is wrong even though the model is right. Fix the
  presentation (S6), not the model.
