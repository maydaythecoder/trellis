# ADR 0016 — Narrowing the partitionability test, and staging the plan

**Status:** Accepted.
**Date:** 2026-09-21
**Amends:** S19's patch 1 as implemented, and the flat output of
`partition::plan`. Does not amend S19's patches 2 and 3, which are unchanged
and now carry more weight.

## Context

`05-v1.1-multi-agent.md` says the conservative version ships first and
"loosening it is an experiment to run with measurements in hand, not a
starting position". The measurements were taken first
(`docs/bench/decomposition-20260921.md`).

On poetry 2.5.0 a two-symbol change partitioned **4.0%** of the time. The
cause was not the milestone's hypothesis of an interconnected core:

```
symbol partitionable (S19's stated rule): 98.1%
UNIT  partitionable (radius report):       2.9%
```

The gap is `add_touched_file_misses`, which cleared `partitionable` when any
unbound call site sat in any file the radius touched. Broken down:

```
the root's own file:             96.9%
a file defining a radius symbol: 25.5%
a file holding a caller site:    26.0%
```

**96.9% of units were disqualified by their own file.** 271 of poetry's 451
files hold an unbound call somewhere, so almost every symbol's own file
disqualified it.

That flag was introduced for the *coverage* count (Q-13), where
over-approximating doubt is right because the cost is a pessimistic report.
Partitioning reused it for a *safety* decision. The two do not want the same
threshold, and nobody chose to make them share one.

## Decision

### 1. The root's own file no longer disqualifies a unit

It still counts toward the reported doubt. Only the partitioning decision
changes.

The condition also did no safety work that was not already done: compatibility
requires disjoint *file* sets, and a unit's own file is always in its file set,
so two units in one file could never be parallelised regardless. What remained
was the claim that an unbound call somewhere in my file makes my caller set
untrustworthy — true of an unbound call anywhere, and the file boundary is not
evidence for it.

Effect: unit partitionability 2.9% → **80.2%**; two-symbol decomposition
4.0% → **95.0%**; mean groups 1.04 → 1.95.

### 2. The plan is staged: shared work first, then concurrent work

`plan` returns a `Plan { shared, parallel }` rather than a flat list.

The old shape merged everything uncertain into one partition *alongside* the
others, then absorbed every partition that collided with it — so one uncertain
unit could collapse the whole plan. Staging inverts it: contended work runs
alone and first, and the rest are free once it has landed, because what made
them unsafe was running *concurrently* with it, not running at all.

Three rules, in order:

1. A unit the index cannot vouch for goes to `shared` (S19 patch 1, intact).
2. A unit contending for a file `shared` holds goes to `shared` too.
3. The rest group by mutual compatibility, merging transitively.

`shared` is deliberately **not** required to be radius-disjoint from the
parallel partitions — it precedes them — but **is** required to be
file-disjoint, because sequence does not resolve a merge conflict.

## What was measured about the risk, honestly

The narrowing was tested against the threat model rather than asserted safe.
A single unbound call site has one true callee, so one site cannot be a shared
caller of two partitions. The real shape is asymmetric: an unbound site in a
file partition B edits, whose true callee is in A's radius.

`examples/narrowing_safety.rs` counts pairs where that is *possible* — the
site's file imports, directly or through one re-export hop, a file defining a
symbol in the other's radius. Over 2,000 random pairs:

```
compatible under the narrowed rule:      1304
of those, the broad rule refused:        1303
of those, a plausible collision exists:   165   (12.66%)
```

**Not zero.** The narrowing admits pairs the broad rule refused, and 12.66% of
them have a path by which a collision could occur. The examples concentrate in
test files — `tests/conftest.py`, `tests/utils/test_helpers.py` — which is the
pytest-fixture shape already recorded as 51% of first-party misses.

So the disjointness *claim* is measurably weaker. What is unchanged is the
guarantee the milestone actually makes:

> Partition-local passes never substitute for the integrated check.

That still holds, and decision 2 is what makes the residual risk tolerable:
contended and uncertain work is serialized into a stage that runs first and
alone, so the pairs most likely to collide are the ones never run
concurrently.

## Consequences

- The integrated full check across the union is now load-bearing in practice,
  not only in principle. Under the old rule it almost never had anything to
  catch, because almost nothing partitioned.
- Suspected-recall-gap recording (S19 patch 3, v1.1 task 13) rises in value
  for the same reason, and should be built before multi-agent execution ships.
- The 12.66% figure is an upper bound and a starting point for narrowing
  further — excluding test files from partition file sets is the obvious next
  experiment, and is not taken here.

## Alternatives rejected

**Leave it broad.** Defensible, and it makes v1.1 not worth building: 4% of
tasks decomposing means agent supervision code that almost never runs.

**Narrow and keep a flat plan.** The 12.66% would stand with nothing
structural against it. Staging is what makes the narrowing defensible rather
than merely faster.

**Exclude test files from the unbound-call scan.** Attractive — the risk
concentrates there — and rejected for now as a second loosening stacked on the
first without its own measurement. It is the next experiment, not part of this
one.

## Reversibility

High. Both halves are local: one `if` in `add_touched_file_misses`, and the
shape of `plan`'s return. The property tests in
`tests/partition_invariants.rs` encode the invariant either way, and were
rewritten rather than relaxed — they now assert that parallel partitions are
pairwise disjoint, that the shared stage is file-disjoint from all of them,
and that no unit the index cannot vouch for ever reaches a parallel partition.
