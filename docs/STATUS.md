# STATUS

Part 8 of the brief is the *project's* definition of done, not any one
session's. This maps each item to its actual state, so nothing is quietly
assumed complete.

Last updated 2026-09-19.

| # | Part 8 item | State | Where |
|---|---|---|---|
| 1 | Every document in Part 6 exists | **Done** | `docs/`, 9 documents + 10 ADRs |
| 2 | `ASSUMPTIONS` block, every default chosen where the brief was silent | **Done** | `docs/ASSUMPTIONS.md`, 23 entries |
| 3 | All soft spots in `RISK_REGISTER.md` with a costed patch each | **Done** | `docs/RISK_REGISTER.md`, all 18 |
| 4 | **S1 resolved**: measured resolver reach, fallback trigger recorded | **Resolver built; repository-wide reach measured; hand-audited recall pending.** See "the measurement" below | `trellis-index/src/resolve.rs`, `BENCHMARK_AND_EVAL.md` §2 |
| 5 | **S14 resolved**: named benchmark repo, threshold with reasoning | **Done** | `BENCHMARK_AND_EVAL.md` §1, §3 |
| 6 | **S16 resolved**: the recall figure that fires the pivot | **Done** — 60% | ADR 0002, `ROADMAP.md` |
| 7 | **S7 resolved**: model floor across seven metrics | **Not done.** Spike designed, not run. Needs inference | `BENCHMARK_AND_EVAL.md` §5 |
| 8 | **S17 resolved**: llguidance embedded, inference path in an ADR, licence verified, matrix published | **Mostly.** Embedded and tested; licence verified MIT 1.8.0; matrix published. **Inference path deliberately open** | ADR 0006, `TECH_SPIKE.md` §3 |
| 9 | Grounding contract structural — `Verdict` unforgeable, `ToolResult` has no pass variant | **Done** | `trellis-checks/src/verdict.rs`, `trellis-loop/src/tools.rs` |
| 10 | One main loop with tools, not a verification fan-out, ADR says why | **ADR done. Loop not built** — v0.2, gated on the recall call | ADR 0005 |
| 11 | Multi-agent parallelizes work only, ADR says why | **Done in design and rules; no concurrent execution yet** | ADR 0005, `trellis-loop/src/partition.rs` |
| 12 | Partitioning treats unresolved/candidate edges as unpartitionable; integrated checks never skipped | **Rule 1 implemented and property-tested over 400 seeds, now against a real resolver rather than synthetic graphs. Rule 2 documented; enforced when integration is built** | `partition::plan`, `tests/partition_invariants.rs` |
| 13 | `trellis doctor` reports batching capability and multi-agent tier | **Types and reporting done; probing not implemented** — reports `Unknown`, and says so | `trellis-model/src/tier.rs` |
| 14 | Rollback verified by induced mid-run failure in CI | **Verified in the test suite. No CI configured yet** | `trellis-vcs/src/lib.rs` tests |
| 15 | Bidirectionality verified by property test | **Done** — 199 random graphs | `trellis-index/tests/bidirectional.rs` |
| 16 | v1.0 gate verbatim in `ROADMAP.md`, all five conditions | **Done** | `ROADMAP.md` |
| 17 | v1.0 non-goals explicit and substantial | **Done** — 10 entries | `ROADMAP.md` |
| 18 | Every major technology choice has a recorded rejected alternative | **Done** | ADRs 0001–0010 |
| 19 | Factual claims cited; unverifiable ones flagged | **Done** | `OPEN_QUESTIONS.md`, final section |
| 20 | No marketing language anywhere | **Done** | — |

## The measurement (resolves S1's reach; fires S16)

The resolver exists and runs on the benchmark corpus (poetry 2.5.0 sdist,
SHA-256 in `BENCHMARK_AND_EVAL.md` §1). 400 call sites were sampled with seed
20260919 and audited blind; 159 had a first-party true callee.

| | |
|---|---|
| **Recall** | **47.8%** (76/159), 95% CI [40.0%, 55.6%] |
| **Precision** | **100.0%** (76/76) |
| `Name` / `Attribute` recall | 84.7% / 17.2% |
| Wrongly excluded as external | 0 of 114 |
| Mechanical soundness check | 4,443 / 4,443 edges clean |

**The S16 pivot trigger (recall < 60%) has fired.** The whole confidence
interval is below it. ADR 0011 records the evidence, the diagnosis and the
options; **the decision is the project owner's and is pending, and no v0.2 work
starts until it is made.**

This is a recall finding only. Nothing the graph says is wrong — precision is
100% and every `Resolved` edge passes the mechanical check. It says too little,
not anything false.

Three findings that stand regardless of which option is chosen:

- The deficit is almost entirely in `Attribute` call sites, which carry 55% of
  the denominator at 17.2% recall. `Name` resolution is at 84.7%, near the gate
  on its own.
- The brief expected `DecoratorIndirection` to dominate the misses and S1's
  recorded patch was built on it. It is 3 sites out of 11,233, against 10,646
  for `MethodDispatchUntyped`. That patch branch is falsified.
- 51% of first-party misses cite pytest fixtures, and the recurring pattern is
  a receiver whose type is locally evident — `self._x = SomeClass()`, or an
  annotated parameter. poetry is mypy-strict. If a backstop is ever built, the
  evidence points at local receiver typing, not the Pyright dependency S1
  reaches for.

## The three that are genuinely open

Items 4, 7 and 10 still cannot be closed by writing documents. Item 4 now needs
only the audit scored. Items 7 and 10 need a working inference path and a
working agent loop, and both are v0.2.

## What was verified rather than assumed

- tree-sitter 0.27.0 pairs correctly with tree-sitter-python 0.25.0 at runtime
  (`tests/grammar_abi.rs`). The brief warns about this and it is now a test.
- llguidance 1.8.0 is MIT and actively maintained (upstream push 2026-09-04).
- `github/stack-graphs` is **archived**. Not an S1 option.
- `Kixiron/lasso` last shipped 2024-08-19. Replaced with a 40-line interner.
- `bincode` 3.0.0 is a protest release that deliberately fails to compile.
- `poetry.core` is a separate distribution, absent from the poetry tree, and
  imported 379 times. Treating it as first-party on the strength of its root
  package put those calls in the recall denominator as misses.

## Where the brief's own text needed correcting

- **llguidance API.** The brief's three-call sketch omits `CommitResult`'s
  `backtrack` and `ff_tokens`. Neither is optional: fast-forward tokens are a
  throughput win, backtracking is correctness. ADR 0006, `TECH_SPIKE.md` §3.
- **`Verdict` serialization.** A derived `Deserialize` would be a public
  constructor from arbitrary JSON, defeating the seal and opening the S10 hole.
  `Verdict` is `Serialize`-only; trails read back as inert `RecordedCheck`.
- **The dominant miss reason.** The brief predicts decorator indirection will
  dominate and it does not, by three orders of magnitude (above).

## Closed since the last update

`RadiusReport::coverage.unresolved` was structurally always zero: a reverse
walk cannot see an unresolved edge, because such an edge names no target and
never enters `reverse`. The radius now takes its unresolved count from the
files it touches — an over-approximation, which is the safe direction. Closes
OPEN_QUESTIONS Q-13.

## Next, in order

1. **Blocked on a decision.** ADR 0011 is Proposed. Option A follows the
   pre-registered rule and pivots; B re-audits with a human first; C builds one
   bounded backstop and re-measures, which deviates from the rule and needs its
   own ADR. Nothing below starts until this is settled.
2. If the pivot is taken: rewrite `MISSION.md` positioning, demote the index to
   a labelled hint throughout, and remove multi-agent partitioning from the
   roadmap — it is built on radius disjointness (S19) and cannot survive.
3. **S6 is now a v0.3 blocker regardless of the decision.** `trellis radius
   Factory` reports 123 resolved call sites beside 3,023 unresolved ones. That
   is honest and unusable, and it is the first screen a user meets.
4. A human re-audit before any v1.0 gate claim (A-34, Q-15). This audit is
   model-read; the protocol asks for human-read and the two are not the same
   evidence.
5. Deferred, not forgotten: incremental `Tree::edit` update (brief 4.3, not
   built — single-file-save latency is currently a full ~120 ms rebuild), and
   Q-14's latent unsound edge on the self-dispatch path.
