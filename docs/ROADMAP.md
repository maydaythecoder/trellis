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

### Named gate: the S16 pivot trigger — **FIRED 2026-09-19**

**If measured recall on the benchmark repository is below 60%, the
blast-radius guarantee dies and TRELLIS pivots.**

> **Fired at 47.8%, 95% CI [40.0%, 55.6%] — then averted.** The project owner
> chose ADR 0011 option C: one bounded backstop, recorded as a deliberate
> deviation in ADR 0012. Receiver typing raised recall to **72.0%**, CI
> [63.2%, 80.8%], on a **fresh** sample (seed 20260920), confirmed against
> 67.9% on the original. Precision 100% on both.
>
> The interval now sits entirely above the 60% trigger, so the pivot does not
> execute. **The S1 fallback band (60–85%) applies instead**: the blast-radius
> guarantee survives, and a further backstop is required before v1.0 can claim
> it. The pre-registered thresholds are unchanged.
>
> Precision measured 100% and all 4,443 `Resolved` edges pass the mechanical
> soundness check, so this is a recall finding alone. Under a pivot the index
> is demoted to a hint, but it is a hint that is right when it speaks.

This is a gate, not a contingency. It is evaluated at v0.1 exit, before any
v0.2 work begins, and it is written here because the figure was chosen before
the measurement existed (`docs/adr/0002-edge-confidence-model.md`).

On a pivot, what survives: context continuity, governance enforcement,
test-anchored checks, and the committed trail — a smaller product that still
solves the problem in `MISSION.md`, with the index demoted from guarantee to
hint. What dies: goal state 2, every blast-radius claim in the product copy,
and multi-agent partitioning, which is built on radius disjointness.

The intermediate band — recall at or above 60% but below 85% — fires the S1
fallback instead: a semantic backstop before v1.0, planned as decorator
unwrapping first, Pyright over LSP if that is insufficient. The backstop is
Pyright over LSP, decided in
[ADR 0031](adr/0031-pyright-recall-backstop.md). Its first slice is built
behind `[index] backstop = "pyright"` (#168), and #177 ran it on poetry:
1,859 call sites bound by Pyright, counted apart from `resolved` and kept out
of the graph. That count is not recall; the method is in
[`bench/pyright-backstop-poetry-20260929.md`](bench/pyright-backstop-poetry-20260929.md).

**The measurement did not land in that band, and the decorator-first ordering
above is falsified in any case:** `DecoratorIndirection` accounts for 3 of
11,233 unresolved sites, against 10,646 for `MethodDispatchUntyped`. See S1 and
ADR 0011.

### v1.0 non-goals, explicit

Not deferred politely. Out of scope, and the gate does not bend for them:

- **Any language but Python.** No multi-language index, no polyglot repos.
- **Repos without a test suite.** The grounding contract has nothing to ground
  on. `trellis doctor` says so plainly rather than degrading.
- **Cross-repo analysis.** First-party means *this repository*.
- **Any surface beyond the CLI.** No TUI, no editor plugin, no web UI, no MCP
  server.
  - *Built exception:* `trellis tui`, at the owner's direction, and now what
    bare `trellis` opens. Recorded as a deviation in `STATUS.md`, "Not on the
    critical path, but real".
  - *Built exception:* `trellis rpc`, JSONL commands on stdin and events on
    stdout, for a caller that drives trellis as a subprocess (ADR 0025). It
    is a local pipe, not a server.
- **Any networked component.** No telemetry, no sync, no hosted anything.
  - *Built exception:* hosted model providers, reached with the user's own
    key from the user's machine (ADR 0024). `--no-default-features` builds a
    local-only binary with no TLS.
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

Maintainer tooling, not product surface: `trellis-tickets`, a separate binary
for trellis's own GitHub issues (new, triage, ready, sweep, scan, index), so
the `trellis` CLI that brief 4.12 fixes does not grow. Recorded in
[ADR 0040](adr/0040-ticketing-tool.md); built by #76-#81.

## v2.0 — each needing its own brief

More languages. Cloud execution. Shared environments. Messaging and scheduling.
Desktop and web surfaces. MCP exposure. Template sharing. Multi-user.
