# STATUS

Part 8 of the brief is the *project's* definition of done, not any one
session's. This maps each item to its actual state, so nothing is quietly
assumed complete.

Last updated 2026-09-18.

| # | Part 8 item | State | Where |
|---|---|---|---|
| 1 | Every document in Part 6 exists | **Done** | `docs/`, 9 documents + 10 ADRs |
| 2 | `ASSUMPTIONS` block, every default chosen where the brief was silent | **Done** | `docs/ASSUMPTIONS.md`, 23 entries |
| 3 | All soft spots in `RISK_REGISTER.md` with a costed patch each | **Done** | `docs/RISK_REGISTER.md`, all 18 |
| 4 | **S1 resolved**: measured resolver reach, fallback trigger recorded | **Half.** Fallback and trigger recorded; reach **not measured** — the resolver does not exist | ADR 0001, `BENCHMARK_AND_EVAL.md` §4 |
| 5 | **S14 resolved**: named benchmark repo, threshold with reasoning | **Done** | `BENCHMARK_AND_EVAL.md` §1, §3 |
| 6 | **S16 resolved**: the recall figure that fires the pivot | **Done** — 60% | ADR 0002, `ROADMAP.md` |
| 7 | **S7 resolved**: model floor across seven metrics | **Not done.** Spike designed, not run. Needs inference | `BENCHMARK_AND_EVAL.md` §5 |
| 8 | **S17 resolved**: llguidance embedded, inference path in an ADR, licence verified, matrix published | **Mostly.** Embedded and tested; licence verified MIT 1.8.0; matrix published. **Inference path deliberately open** | ADR 0006, `TECH_SPIKE.md` §3 |
| 9 | Grounding contract structural — `Verdict` unforgeable, `ToolResult` has no pass variant | **Done** | `trellis-checks/src/verdict.rs`, `trellis-loop/src/tools.rs` |
| 10 | One main loop with tools, not a verification fan-out, ADR says why | **ADR done. Loop not built** | ADR 0005 |
| 11 | Multi-agent parallelizes work only, ADR says why | **Done in design and rules; no concurrent execution yet** | ADR 0005, `trellis-loop/src/partition.rs` |
| 12 | Partitioning treats unresolved/candidate edges as unpartitionable; integrated checks never skipped | **Rule 1 implemented and tested. Rule 2 documented; enforced when integration is built** | `partition::plan`, `Index::is_partitionable` |
| 13 | `trellis doctor` reports batching capability and multi-agent tier | **Types and reporting done; probing not implemented** — reports `Unknown`, and says so | `trellis-model/src/tier.rs` |
| 14 | Rollback verified by induced mid-run failure in CI | **Verified in the test suite. No CI configured yet** | `trellis-vcs/src/lib.rs` tests |
| 15 | Bidirectionality verified by property test | **Done** — 199 random graphs | `trellis-index/tests/bidirectional.rs` |
| 16 | v1.0 gate verbatim in `ROADMAP.md`, all five conditions | **Done** | `ROADMAP.md` |
| 17 | v1.0 non-goals explicit and substantial | **Done** — 10 entries | `ROADMAP.md` |
| 18 | Every major technology choice has a recorded rejected alternative | **Done** | ADRs 0001–0010 |
| 19 | Factual claims cited; unverifiable ones flagged | **Done** | `OPEN_QUESTIONS.md`, final section |
| 20 | No marketing language anywhere | **Done** | — |

## The three that are genuinely open

Items 4, 7 and 10 cannot be closed by writing documents. They need,
respectively: a working resolver, a working inference path, and a working agent
loop. Everything that *could* be settled from a standing start has been.

## What was verified rather than assumed

- tree-sitter 0.27.0 pairs correctly with tree-sitter-python 0.25.0 at runtime
  (`tests/grammar_abi.rs`). The brief warns about this and it is now a test.
- llguidance 1.8.0 is MIT and actively maintained (upstream push 2026-09-04).
  The schemars → JSON Schema → llguidance path compiles real derived schemas.
- `github/stack-graphs` is **archived**. The brief listed it as an open S1
  option; it is not one.
- `Kixiron/lasso` last shipped 2024-08-19. Replaced with a 40-line interner.
- `bincode` 3.0.0 is a protest release that deliberately fails to compile.
- All benchmark-repo figures in `BENCHMARK_AND_EVAL.md` §1.

## Where the brief's own text needed correcting

- **llguidance API.** The brief's three-call sketch omits `CommitResult`'s
  `backtrack` and `ff_tokens`. Neither is optional to handle: fast-forward
  tokens are a throughput win, backtracking is correctness. Recorded in ADR
  0006 and `TECH_SPIKE.md` §3.
- **`Verdict` serialization.** The brief specifies the seal but not what happens
  when a verdict is written to a trail and read back. A derived `Deserialize`
  would be a public constructor from arbitrary JSON, defeating the seal and
  opening exactly the S10 hole. `Verdict` is `Serialize`-only; trails read back
  as inert `RecordedCheck`.

## Next, in order

1. Resolver Pass 2 (module map) and the bind step. Model-free, testable with
   `insta`, unblocks the recall measurement that gates everything.
2. The hand-audited recall sample on poetry (400 sites, seed recorded).
3. Evaluate that figure against the thresholds in ADR 0002 **before** starting
   v0.2. If it is below 60%, the pivot fires and v0.2 is a different project.
