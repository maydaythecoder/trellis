# ADR 0028 — Pyright over LSP as the recall backstop

**Status:** Accepted. Not yet built; the implementation ticket is filed when
this ADR lands.
**Date:** 2026-09-29
**Decided:** 2026-09-28 by the owner, via the coordinator (issue #125)
**Builds on:** ADR 0002 (thresholds), ADR 0012 (receiver typing), ADR 0014
(radius coverage)

## Context

ADR 0012's receiver typing moved recall to 67.9% (seed 20260919) and 72.0%
(seed 20260920): above the 60% pivot trigger, below the 85% v1.0 gate. That
puts the project in ADR 0002's S1 fallback band, so a further backstop is
required before v1.0. `BENCHMARK_AND_EVAL.md` §4 chose Pyright over LSP as
that fallback on 2026-09-18, after stack-graphs was found archived. §3 later
found the remaining misses sit in attribute chains and cross-file locals,
which pointed at an uncosted alternative: cross-file type propagation built
in-house.

#121 bounded both on the two audited samples, in
[`docs/bench/backstop-ceiling.md`](../bench/backstop-ceiling.md). The samples
were read by a model, not a person (A-34), so every figure below is a
model-read upper bound, not recall, and none may be compared to the §3
thresholds as recall.

§4 also names a cheaper intermediate to try first: measure the `Reason`
distribution, and if `DecoratorIndirection` dominates, unwrap decorators
before adopting Pyright. That measurement has been made, and the condition
did not hold. Before ADR 0012, `DecoratorIndirection` was 3 of 11,233
unresolved sites and `MethodDispatchUntyped` was 10,646 (ADR 0011). After ADR
0012 and ADR 0015, they were 63 and 6,090 of 6,737 (`BENCHMARK_AND_EVAL.md`
§2, measured 2026-09-20). Both are corpus-wide counts of what the resolver left
unresolved, not counts over the audited sample. The decorator intermediate's
precondition is false, so §4's "try first" ordering does not apply.

## Decision

**Pyright over LSP is the recall backstop.** It is queried only for call
sites the resolver leaves `Unresolved` or `Candidates`. It is never queried
for a site the resolver bound `Resolved`, and never for one it classified as
external.

## Why not cross-file type propagation

Its ceiling, the recall a sample would show if it bound every miss of the
shapes it can reach and bound each one correctly, is
**119/159 = 74.8%** (seed 20260919) and **78/100 = 78.0%** (seed 20260920).
Both are model-read upper bounds (A-34), and both are under 85%. Even if it
bound every site it could reach, correctly, neither sample would reach the
gate.

The largest miss bucket on both seeds is a parameter annotated with a
first-party class. 27 of its 29 misses are `Candidates`: the declared type is
already known, and ADR 0012's upper-bound rule keeps every override. Type
propagation does not reach those sites, because their type is not what is
missing.

The same report puts Pyright's trigger at every miss in both samples
(51 of 51, 28 of 28 carry `Unresolved` or `Candidates`). That says where the
trigger fires. It is not a measurement of Pyright, which was not run, and it
is not a recall figure. What recall Pyright actually produces is measured by
the human audit in #3.

## Costs

- **A Node.js runtime dependency at run time.** Pyright is a TypeScript
  program on Node. It is invoked as an external process and is not a crate
  dependency. No Rust crate is added for it.
- **Memory on large repositories: pending.** §4 records Pyright as multi-GB on
  large repositories, but that is an assessment and was not measured here.
  The figure is measured by the implementation ticket, before the v1.0 gate is
  claimed (milestone v1.0, `docs/04-v1.0-harness.md`).
- **No batch call-graph API.** Pyright is driven one query at a time. That is
  why it serves as a backstop and not as a replacement. The trigger bounds the
  number of queries to the sites the resolver could not bind.

## Absence degrades; it does not fail

If Node or Pyright is missing, fails to start, or fails on a query, the site
keeps the verdict the resolver gave it (`Unresolved` stays `Unresolved`,
`Candidates` stays `Candidates`), and the index completes. The index never
fails because the backstop is unavailable, and coverage reports that the
backstop did not run, so a user can tell "Pyright ran and found nothing" apart
from "Pyright was not there".

## Provenance: a Pyright-bound edge is recorded with its source

Pyright's answer is evidence about the runtime type, not proof. ADR 0012
showed that annotations can be wrong, and Pyright reads the same annotations.
So:

- **Every Pyright-bound edge records its source.** Today `Edge` is
  `{ from, target }` and `Coverage` counts `resolved`, `candidates`,
  `unresolved`, `external` and `by_reason`, with no source on either. The
  implementation ticket adds the source. This ADR does not design the field.
- **Coverage reports Pyright-bound edges separately from resolver-bound
  edges**, as a count beside the resolver's counts, never merged into
  `resolved` and never as a fraction (CLAUDE.md rule 3, ADR 0014).
- **ADR 0012's invariants still hold for the resolver's own edges.** A
  declared type is an upper bound. A type fact counts only when it resolves to
  a first-party class. Conflicting statements type nothing. The resolver's
  output is unchanged by this ADR, and so are `examples/check_soundness.rs`
  (zero violations) and 100% precision on the audited sample. Pyright can only
  act on a site after the resolver has given its verdict. It does not feed
  back into the resolver.

## Constraints the implementation inherits

- **The Pyright child process goes through `trellis_core::secrets::scrub`**,
  like every other spawn. `tests/child_env.rs` enforces this.
- **`trellis-index` still never depends on `trellis-model`.** Pyright is a
  deterministic static analyser, not a sampler, but it is still an input the
  graph did not have before. The implementation ticket decides how the
  Pyright version is recorded alongside the index, because the graph's output
  now depends on it. Resolver tests and goldens run with Pyright absent, so
  they keep testing the resolver alone.
- **Partitioning of Pyright-bound edges is not decided here.** Whether such an
  edge counts as certain for S19 partitioning (rule 6, ADR 0005) is left to the
  implementation ticket. Until that ticket decides it with its own reasoning,
  the conservative reading holds: a site the resolver left `Unresolved` or
  `Candidates` stays unpartitionable.
- **Agreement with Pyright is not correctness.** Recall is still measured
  against the hand audit (§2), never against Pyright's own answers.

## What changes our mind

- The human audit (#3) showing that Pyright-bound edges are unsound at a rate
  that the separate count does not make safe to show.
- Pyright's memory on the benchmark corpus, once measured, being too large for
  the machines trellis targets.
- A cheaper in-house rule shown, on a human-audited sample, to clear 85% on
  its own.
