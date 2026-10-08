# MISSION

## Mission

TRELLIS returns agent work verified rather than merely written: blast radius
mapped, tests run, every decision recorded in the repo.

## The problem

You hand a coding agent a change. It writes something plausible. You now have
to work out what it touched, whether the callers still work, and why it made
the choices it made — and its reasoning evaporated when the session ended.
Tomorrow you reopen the repo and start by re-explaining your own project to a
machine that was here yesterday.

## The shape

A CLI you run inside a Python repo. It builds a call graph of your first-party
code, drives an agent loop against that graph, verifies every change with your
type checker and your test suite, and commits a decision trail so the next
session starts informed.

It runs on models you host, including models with no native tool-calling
support. Your code stays on your machine. Your API key, if you use one, stays
on your machine.

## Goal state, and which of it is delivered

Eight goals. **Five are delivered, three are not.** Two of the three, goals 3
and 6, are the ones a reader would most reasonably assume from the paragraphs
above; goal 8 was set on 2026-10-06 for v1.x. A mission that lists only its
aims reads as a list of claims.

| | Goal | State |
|---|---|---|
| 1 | **Resumption is free.** Reopen a repo, say what's next, re-explain nothing | **Delivered.** Demonstrated cold on the benchmark repo — `docs/bench/wedge-20260920.md` |
| 2 | **Scope is known before the change.** The harness names the first-party call sites reaching a function, and names honestly the ones it could not resolve | **Delivered, and bounded.** Precision 100% on two hand audits and over the full corpus mechanically; recall 72.0%, short of the 85% gate |
| 3 | **Verification executes.** "The callers still work" means the type checker ran clean and covering tests passed | **Half.** Both run, and a failing suite blocks convergence. *Covering* is the missing word: nothing maps tests to call sites, so `Verified` is always 0 and the trail says so rather than guessing |
| 4 | **Failure is boring.** Non-convergence rolls back completely and hands back a legible reason | **Delivered.** Seven injection points including a real SIGINT and a dead orchestrator — `docs/bench/rollback-20260920.md` |
| 5 | **The trail is durable.** Committed, agent-readable, in the repo | **Delivered**, with provenance and a SHA-256 over the factual half |
| 6 | **It runs on your hardware**, on quantized open weights, without native tool calling | **Not demonstrated.** Four configurations measured; the floor is located only at tier 4, not under constrained decoding (BENCHMARK_AND_EVAL.md §5.7). Under constrained decoding, both measured are below it: 0.5B repaired 0 of 20, Coder-3B 0 of 17; the harness drives them correctly and neither can do the work. At tier 4, `qwen3:8b` Q4_K_M on Ollama repaired 5 of 20, measured while the step history showed it CamelCase tool names (#541; re-measure: #545) |
| 7 | **Cost and wall-clock are bounded and visible** | **Delivered.** Budgets enforce and divide across partitions; iterations, wall clock and context fill are recorded per run |
| 8 | **Two clients of `trellis rpc`, one engine** (v1.x): a Tauri desktop that renders a whole run, and a Go/Bubble Tea TUI that replaces the ratatui TUI at parity | **Not delivered.** Recorded in ADR 0052 (Accepted 2026-10-07, #500); neither client is built, and the ratatui TUI is still the one that ships |

Goal 6 is the one to read twice. Every mechanism it needs exists and is
tested — constrained decoding, the adapter tiers, the loop, the rollback — and
an A/B established that the harness is not what holds those models back. What
has not been found is a local model that can actually do the task. That is a
measurement about available small models, not a claim about this design, and
it is the difference between "supported" and "demonstrated".

## Non-goals for v1

Any language but Python. Repos without a test suite. Cross-repo analysis. Any
surface beyond the CLI. Any networked component. Multi-user anything. Running
while the machine is asleep.

*2026-10-07 (#515), "Any surface beyond the CLI" amended by ADR 0052:* the
Tauri desktop and the Go TUI, both clients of `trellis rpc`, are goal 8 (v1.x)
and no longer non-goals. A web surface and any networked component remain
non-goals.

*2026-10-07 (#557), "Cross-repo analysis" read by the owner's decision of
2026-10-07:* the non-goal does not cover listing recent repositories with
each one's last outcome, which `trellis overview` and the desktop show as a
launcher list (ADR 0021 rows 44–47). Accepted; built 2026-10-07 (#558).
*As built 2026-10-07 (#558):* `trellis overview` prints that list, each
repository with its newest trail's outcome, and compares nothing across
repositories.

## Positioning

### The index is not a moat, and this document will not pretend otherwise

Aider's tree-sitter repo map, a wave of MCP code-graph tools, Sourcegraph, and
GitHub's own code navigation all occupy the code-graph space. Nothing about
TRELLIS's call graph is novel, and a competitor could reproduce it.

It is worth being precise about what "commodity" does and does not mean here,
because the one candidate this project actually evaluated was not usable:
`github/stack-graphs` is **archived**, which is why it was ruled out as an S1
option rather than adopted. Commodity means the capability is widely available,
not that any particular implementation is maintained and droppable-in. Those
are different claims and only the first one is being made.

Building it in-house is justified by fit and simplicity, not by
differentiation:

- The graph must expose a three-valued confidence model
  (`Resolved` / `Candidates` / `Unresolved(Reason)`). Consuming a tool that
  returns a flat edge list means discarding the distinction the whole product
  rests on.
- The graph must be incremental at file granularity, because the loop edits
  files mid-run.
- It must run in-process with no daemon, no Node runtime, and no network.

A general-purpose code-intelligence service satisfies none of those three
cheaply. That is the argument — and it is an argument about cost of
integration, not about capability.

### The wedge is the harness and the trail

What is actually hard to copy is the combination of four properties, each of
which is a constraint a competitor would have to accept:

1. **Correctness claims are unforgeable.** A pass verdict can only originate
   from a process that ran. This is enforced in the type system
   (`trellis-checks::Verdict`), not in a prompt. Most harnesses let the model
   declare success; that is the difference.
2. **Incompleteness is reported, not hidden.** Every blast radius ships with
   its coverage. The tool never emits a bare "complete".
3. **Failure returns the repository to its exact prior state.** Verified by
   inducing a mid-run panic in CI, not asserted in a README.
4. **The decision trail is committed, structured, and split** into what
   happened and what a model thought. Future sessions read it as context, which
   is what makes resumption free.

Property 1 is also what makes small local models viable. A design resting
correctness on model judgment is hardest to run on weak models; a design
resting it on a type checker barely cares which model you use.

### What would make this positioning wrong — the trigger fired

This was written before any measurement existed, deliberately (S16): if
recall on the benchmark repository came in below 60%, goal 2 was not
deliverable and the blast-radius claim died.

**It fired.** First measurement: 47.8%, 95% CI [40.0%, 55.6%]. The owner took
one bounded backstop rather than the pivot (ADR 0011 option C, recorded as a
deliberate deviation in ADR 0012), and receiver typing raised recall to 72.0%
on a *fresh* sample, CI [63.2%, 80.8%]. The interval now sits entirely above
the trigger, so the pivot does not execute.

What that leaves is not a clean pass. **The S1 fallback band — at or above 60%,
below 85% — applies**, which means the blast-radius guarantee survives and a
further backstop is required before v1.0 can claim it. Fixture typing and
return-type propagation were measured and rejected; Pyright over LSP was then
chosen (ADR 0031) and its first slice is built behind `[index] backstop =
"pyright"`; its recall is pending the human audit. The third thing found
instead was a denominator error, corrected, which moved no recall figure at
all.

The positioning above therefore stands, with goal 2 explicitly bounded rather
than claimed outright. Had the pivot executed, what survived would have been
context continuity, governance enforcement, test-anchored checks and the
committed trail — a smaller product addressing the same problem, with the index
demoted from guarantee to hint.

## Status

**v0.1 complete. v0.3 at eight of nine. v0.2 at six met with one descoped by ADR.** The resolver,
the agent loop, the orchestrator, rollback, governance, the trail, resume, a
session and a full-screen interface all exist and are tested — 320 tests, and
`docs/STATUS.md` audits every claim in this repository against the code rather
than against the plan.

Recall on the benchmark repository is **72.0%**, hand-audited, 95% CI
[63.2%, 80.8%]; precision is 100% on both audits and mechanically over the full
corpus. Neither figure appears anywhere without its method beside it.

Three things stand between here and v1.0: the 85% recall backstop (ADR 0031,
code), a human audit (A-34), and a local model that can pass the floor.
