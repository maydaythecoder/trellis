# ROADMAP

## v0.1 — Measure (no product ships)

Replace assumptions with numbers.

- Name a benchmark Python repo: mid-size, first-party-import-heavy, real test
  coverage. Propose three with selection criteria.
- Build the index against it. Measure: precision on a hand-audited sample,
  recall against hand-audited ground truth, build time, incremental update
  latency, peak memory, and the `Reason` distribution.
- Probe the model floor (Part 7, S7).

**Exit:** a measured recall figure, a stated threshold with reasoning, a known
model floor, and an ADR on whether a semantic backstop is needed.

Ground truth must be a **hand-audited sample**, not another tool's output.
Comparing your graph to Pyright's measures agreement, not correctness, and both
share blind spots.

**Progress.** Benchmark repos named and measured (`BENCHMARK_AND_EVAL.md` §1);
thresholds stated with reasoning (§3); Pass 1 built and measured. Resolution,
the recall audit, and the model-floor spike remain.

## v0.2 — The loop

Main agent loop, tool set, llguidance adapter across at least two local models
plus one frontier API, orchestrator actor with serialized writes and git
rollback, full `tracing` instrumentation.

**Exit:** one real change on the benchmark repo runs end to end; an induced
mid-run failure rolls back cleanly.

## v0.3 — Memory

Context file, prompt, and governance rule import. Committed trail with the
factual/judgment split. Trail read back on resume.

**Exit:** the wedge, demonstrated once — reopen, resume, re-explain nothing.

## v1.0 — The harness

Hard gate. All five, no substitutions:

1. Recall on the benchmark repo meets the v0.1 threshold.
2. Zero unsound edges — every `Resolved` edge verified real on the benchmark
   corpus.
3. A real change runs to human approval with a complete committed trail and
   grounded checks.
4. Induced mid-run failure returns the repo to its exact pre-run state.
5. Resumption works.

Plus: acceptable operation on a local open-weight model at or above the measured
floor, via constrained tool calling.

### Named gate: the S16 pivot trigger

**If measured recall on the benchmark repository is below 60%, the
blast-radius guarantee dies and TRELLIS pivots.**

This is a gate, not a contingency. It is evaluated at v0.1 exit, before any
v0.2 work begins, and it is written here because the figure was chosen before
the measurement existed (`docs/ADRs/0002-edge-confidence-model.md`).

On a pivot, what survives: context continuity, governance enforcement,
test-anchored checks, and the committed trail — a smaller product that still
solves the problem in `MISSION.md`, with the index demoted from guarantee to
hint. What dies: goal state 2, every blast-radius claim in the product copy,
and multi-agent partitioning, which is built on radius disjointness.

The intermediate band — recall at or above 60% but below 85% — fires the S1
fallback instead: a semantic backstop before v1.0, decorator unwrapping first,
Pyright over LSP if that is insufficient.

### v1.0 non-goals, explicit

Not deferred politely. Out of scope, and the gate does not bend for them:

- **Any language but Python.** No multi-language index, no polyglot repos.
- **Repos without a test suite.** The grounding contract has nothing to ground
  on. `trellis doctor` says so plainly rather than degrading.
- **Cross-repo analysis.** First-party means *this repository*.
- **Any surface beyond the CLI.** No TUI, no editor plugin, no web UI, no MCP
  server.
- **Any networked component.** No telemetry, no sync, no hosted anything.
- **Multi-user anything.** No shared state, no accounts, no collaboration.
- **Running while the machine is asleep.**
- **Multi-agent mode with N>1.** Built but not enabled; see v1.1.
- **Repos above ~300k lines.** Supported range is 100k–300k (S9). A larger repo
  is not refused, but nothing is claimed about it.
- **Automatic remediation of unresolved edges.** TRELLIS reports them; it does
  not guess.

## v1.1 — Multi-agent mode enabled (N>1)

Requires the single-agent numbers first — partition size and agent count cannot
be tuned without knowing what one agent costs and how long it takes.

**Gate:** a change that decomposes into three or more disjoint partitions
completes correctly in parallel, with a deliberately-induced cross-partition
collision caught by the integrated check and recorded as a suspected recall gap
(S19).

## v1.x

Semantic backstop if v0.1 demands it. Frontier escalation policy. Coverage-gap
UX. Budget calibration from real usage.

## v2.0 — each needing its own brief

More languages. Cloud execution. Shared environments. Messaging and scheduling.
Desktop and web surfaces. MCP exposure. Template sharing. Multi-user.
