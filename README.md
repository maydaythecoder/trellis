# TRELLIS

A Rust agent harness for Python repositories. It builds a call graph of
first-party code, drives an agent loop against that graph, verifies changes with
the repository's own type checker and test suite, and commits a decision trail.

**Status: pre-v0.1.** The foundation exists and is tested. The resolver, the
agent loop, and model inference do not. No recall figure has been measured, and
nothing in this repository reports one.

## What works today

```
cargo run -p trellis-cli -- index     # Pass 1 across the repo, with counts
cargo run -p trellis-cli -- check     # run the configured type checker and tests
cargo run -p trellis-cli -- doctor    # report adapter tier, batching, tooling
```

`radius`, `run`, `resume` and `partitions` are defined in the CLI surface and
exit with "not implemented yet". They need the resolver and the agent loop
(v0.2).

## Built and tested

| Component | State |
|---|---|
| Workspace, 8 crates | Builds clean |
| Pass 1 parser — defs, imports, call sites, decorators, relative imports | Tested; runs on real repos |
| Graph types — three-valued `Target`, bidirectional adjacency | Tested by property test over random graphs |
| Coverage reporting | Tested |
| **The grounding contract** — sealed `Verdict`, `ToolResult` with no pass variant | Tested |
| Partitioning, including the S19 conservative partition | Tested |
| Budgets, including divide-not-multiply across partitions | Tested |
| Trail — factual/judgment split, untrusted read-back | Tested |
| Rollback — clean-worktree requirement, reset + clean, induced panic | Tested |
| Adapter tiers, schemars → JSON Schema → llguidance | Tested against the real llguidance API |

## Not built

Resolver Pass 2 and the bind step. The agent loop. The orchestrator actor.
Model probing and in-process inference. Incremental index update. Index
persistence. Multi-agent execution (the partitioning rules exist; nothing runs
them concurrently yet).

## Two constraints that shape everything

**Complete call-graph recall is impossible in Python.** Roughly three call edges
in ten are statically invisible. So the graph is high-precision and *explicitly
incomplete*: every edge carries a confidence label, every run reports coverage,
and nothing ever emits a bare "complete".

**An LLM cannot verify its own work without an external signal.** So correctness
verdicts come only from processes that execute — type checker, test runner,
import check. The model reads their output and decides what to do next; it never
issues a pass verdict. This is enforced in the type system, not in a prompt:
`Verdict` is constructible only inside `trellis-checks`, and `ToolResult` has no
pass variant.

## Documents

| | |
|---|---|
| [`docs/BRIEF.md`](docs/BRIEF.md) | The specification. Everything else refers to it |
| [`docs/MISSION.md`](docs/MISSION.md) | Mission, goal state, positioning |
| [`docs/TECH_SPIKE.md`](docs/TECH_SPIKE.md) | Resolver algorithm, edge schema, tool contracts, adapter tiers, orchestrator |
| [`docs/RISK_REGISTER.md`](docs/RISK_REGISTER.md) | Every soft spot, with severity, failure scenario, and costed patch |
| [`docs/BENCHMARK_AND_EVAL.md`](docs/BENCHMARK_AND_EVAL.md) | v0.1 in full: benchmark repos, recall method, thresholds, model-floor spike |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Milestones, the v1.0 gate verbatim, the pivot trigger as a named gate |
| [`docs/COST_MODEL.md`](docs/COST_MODEL.md) | Cost and wall-clock per change, per configuration |
| [`docs/ASSUMPTIONS.md`](docs/ASSUMPTIONS.md) | Every default chosen where the brief was silent |
| [`docs/OPEN_QUESTIONS.md`](docs/OPEN_QUESTIONS.md) | Grouped by who answers |
| [`docs/adr/`](docs/adr/) | Ten decision records |
| [`CLAUDE.md`](CLAUDE.md) | Repository conventions and what needs an ADR to change |

## On numbers in this repository

Every figure in the documents is either a measurement with its method stated
beside it, or is marked pending with the milestone that will produce it. There
are no estimates presented as measurements and no placeholder figures presented
as findings. The recall figure the whole v1.0 gate depends on has not been
measured; the thresholds that will judge it were chosen first, deliberately, and
are recorded in `docs/adr/0002-edge-confidence-model.md`.

## Licence

Apache-2.0.
