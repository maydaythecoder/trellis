# ADR 0003 — Degradation policy

**Status:** Accepted
**Date:** 2026-09-18

## Context

Partial coverage is the normal case, not the exception. If roughly three edges
in ten are invisible, then most non-trivial blast radii will carry unresolved
entries. The product's honesty depends on surfacing them; its usability depends
on not drowning the user in them. These pull against each other.

## Decision

1. **Never suppress, always aggregate.** An unresolved site is never dropped
   from the record. It may be collapsed in the display, with a count always
   visible.
2. **Coverage travels with every radius.** No API returns reach without limits.
3. **Two audit modes, never merged.** `Verified` (an executed check covered it)
   and `Reviewed` (a model read it and did not object) are counted separately
   everywhere — trail, CLI, approval screen. `ModeCounts` has no `total()`.
4. **Three exits from a partial-coverage screen**, and the one that improves
   the repository is the cheapest to take:
   - proceed, with gaps recorded in the trail;
   - inspect candidates;
   - **add a test**, converting a `Reviewed` site into a `Verified` one.
5. **Design for the tenth run in an hour, not the first.** Repeated reasons
   aggregate across a session rather than re-reporting per run.

## Alternatives considered

**A single coverage percentage.** Rejected: it collapses `Candidates` and
`Unresolved`, which have different remedies — one you inspect, the other you
test.

**Suppressing low-value reasons by default** (e.g. hiding
`DecoratorIndirection` once it is known to dominate). Rejected: the dominant
reason is the one most likely to hide the edge that breaks. Aggregate it,
do not hide it.

**Blocking on any unresolved edge.** Rejected: under Constraint 1 this blocks
almost every real change, which trains users to bypass the tool.

## Why

The realistic failure of this design is not a component breaking. It is the
tenth partial-coverage run in an hour training the user to click through the
honesty mechanism without reading it (S6). Aggregation plus a cheap remediation
path is the defence.

## What changes our mind

Watching a real user on a real run. This ADR is designed from first principles
and is the one most likely to be wrong in ways only observation reveals.

## Open

Whether repeat suppression is per-session or persisted in `.trellis/`.
Persisting is more useful and is also a way to permanently hide a warning.
→ `OPEN_QUESTIONS.md` Q-6.
