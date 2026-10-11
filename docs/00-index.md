# TRELLIS — Build Documents

A Rust agent harness for Python repositories. It builds a call graph of your first-party code, drives an agent loop against it, verifies every change with your type checker and test suite, and commits a decision trail so the next session starts informed.

## Reading order

| Doc | Milestone | What it produces |
|---|---|---|
| `00-index.md` | — | This file. Constraints, dependency chain, conventions. |
| `01-v0.1-measure.md` | v0.1 | Numbers. The index, a benchmark, a measured recall figure, a model floor. No product. |
| `02-v0.2-loop.md` | v0.2 | The agent loop, grounded checks, rollback. First end-to-end change. |
| `03-v0.3-memory.md` | v0.3 | Trail, context import, resume. The wedge demonstrated. |
| `04-v1.0-harness.md` | v1.0 | Hardening to the five gate conditions. The version you use daily. |
| `05-v1.1-multi-agent.md` | v1.1 | Parallel agents on radius-disjoint partitions. |

Read `00` and `01` before writing any code. Each version doc assumes the previous one shipped.

## For engineers joining

Start with `MISSION.md`, then `ARCHITECTURE.md` and `GLOSSARY.md`; `adr/README.md` indexes the decisions; `../CONTRIBUTING.md` covers setup, branches and review.

## The two constraints

These are why the design looks the way it does. If you find yourself designing around either, stop.

### Constraint 1 — Complete call-graph recall is impossible in Python

PyCG, the academic state of the art, reports roughly 99.2% precision against about 69.9% recall. Roughly three call edges in ten are statically invisible. This is a property of the language — dynamic dispatch, decorator indirection, `getattr`, conditional imports — not a tooling gap that better engineering closes.

Encouragingly, the prevalence research (Yang et al., MSR '22, 3M+ Python files) found genuinely unanalyzable constructs like `eval` rare, while decorators and context managers are widespread. Your misses will be boring and common, not exotic.

**Consequence:** the graph is high-precision and explicitly incomplete. Every edge carries a confidence label. Every run reports coverage. Never emit a bare "complete."

### Constraint 2 — An LLM cannot verify work without an external signal

Huang et al. (ICLR 2024, DeepMind): without external feedback LLMs struggle to self-correct and often degrade — GPT-4 on GSM8K fell from 95.5% to 91.5% to 89.0% over two rounds of intrinsic self-correction. A companion result found models are specifically bad at *detecting* errors while being able to fix ones they're pointed at. MAST (Cemri et al., 2025) annotated 1,642 multi-agent traces and found 41–86.7% failure rates across seven frameworks. A scaling study measured independent multi-agent topologies amplifying errors 17.2-fold versus a single agent, with centralized coordination containing it to 4.4-fold.

The counterpart matters as much: Chen et al. (ICLR 2024) showed self-debugging works consistently **when anchored to unit-test execution**, with ablations confirming the execution does the work.

**Consequence:** correctness verdicts come only from processes that execute. The model reads their output and decides what to do next; it never issues a pass verdict. Enforced in the type system, not in a prompt.

This is also what makes weak local models viable. A design resting correctness on model judgment would be hardest to run on small models; one resting it on a type checker barely cares.

## Dependency chain

```
v0.1  trellis-core ─┬─ trellis-index ──── measurement harness
                    └─ (throwaway probe rig for the model floor)
                              │
v0.2                          ├─ trellis-checks  (Verdict seal)
                              ├─ trellis-model   (adapter + llguidance)
                              ├─ trellis-vcs     (git snapshot/rollback)
                              └─ trellis-loop    (tools, budgets, orchestrator)
                                        │
v0.3                                    └─ trellis-trail
                                                  │
v1.0                                              └─ hardening to the gate
                                                            │
v1.1                                                        └─ partitioning N>1
```

## Workspace

```
trellis/
├── Cargo.toml                 # workspace
└── crates/
    ├── trellis-core/          # shared types, IDs, errors
    ├── trellis-index/         # tree-sitter → call graph
    ├── trellis-checks/        # typecheck, tests, imports — owns Verdict
    ├── trellis-model/         # adapter tiers, llguidance, inference
    ├── trellis-loop/          # agent loop, tools, budgets, orchestrator
    ├── trellis-trail/         # decision trail
    ├── trellis-vcs/           # git snapshot and rollback
    ├── trellis-vault/         # Apex vault → one note's context; must not depend on trellis-model
    ├── trellis-queue/         # local task queue + heartbeat loop, file-backed in the git dir
    ├── trellis-tickets/       # ticketing label rules + separate binary; depends only on trellis-core (ADR 0040)
    ├── trellis-cli/           # binary
    └── tui-demo/              # what `trellis tui` runs; the one TUI (Q-19)
```

Crate boundaries are load-bearing. `trellis-index` must not depend on `trellis-model` — the graph has to be testable with no model anywhere near it. `trellis-checks` owning `Verdict` is what makes Constraint 2 structural.

## Conventions

- `thiserror` in libraries, `anyhow` at the binary edge.
- `tracing` spans are the single instrumentation path. Budget accounting reads from them. Do not build a second telemetry mechanism.
- `insta` snapshot tests for anything that produces structured output — graph dumps, partition plans, trail entries.
- Every non-obvious choice gets an ADR in `docs/adr/`. Format: Context / Decision / Consequences / Alternatives rejected / Status / Reversibility.
- Verify every crate version and license before committing. Several dependencies here move quickly.

## Non-goals through v1.1

Any language but Python. Repos without a test suite. Cross-repo analysis. Any surface beyond the CLI. Any networked component. Multi-user anything. Running while the machine is asleep.

## Soft spot register

Soft spots are numbered S1–S21 and referenced across all version docs. The full register with severities and patches lives in `docs/RISK_REGISTER.md`, written during v0.1. Each version doc lists the ones it touches.

---

## Where this build actually is

This section is not part of the specification above. It exists so a reader
does not have to reconstruct the state from git history.

`docs/STATUS.md` holds the current state, audited gate line by gate line
against the code; its summary: v0.1 is complete, v0.2 is six met, one
descoped by ADR 0013 and three partial or unmet, and v0.3 is eight of nine.

Branches follow ADR 0051: `main` is the launch branch, moved only when the
owner merges a pull request from `dev`; `dev` is the trunk that ticket and
feature branches are cut from; `audit` is the integration branch they merge
into before it is promoted into `dev`. No other branch is permanent; the
legacy ones were deleted on 2026-10-06 (ADR 0051 item 5).

The one thing a reader should carry into every other document: **recall was
measured at 47.8%, which fired the pre-registered S16 pivot trigger; a bounded
backstop then raised it to 72.0% on a fresh sample (CI [63.2%, 80.8%],
precision 100%), so the pivot does not execute.** ADR 0011 holds the first
result, ADR 0012 the decision to attempt the second and its outcome, and
`01-v0.1-measure.md` holds both with their caveats.

That leaves the project in the **S1 fallback band** — above the 60% pivot
trigger, below the 85% v1.0 gate. The blast-radius guarantee survives and a
further backstop is required before v1.0 can claim it. The next item is not
that backstop: it is the S6 degradation UX, which the measurements promoted
from v1.x to a v0.3 blocker.
