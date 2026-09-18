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

## Goal state

1. **Resumption is free.** Reopen a repo, say what's next, re-explain nothing.
2. **Scope is known before the change.** The harness names the first-party call
   sites reaching a function, and names honestly the ones it could not resolve.
3. **Verification executes.** "The callers still work" means the type checker
   ran clean and covering tests passed.
4. **Failure is boring.** Non-convergence rolls back completely and hands back
   a legible reason.
5. **The trail is durable.** Committed, agent-readable, in the repo.
6. **It runs on your hardware**, on quantized open weights, without native tool
   calling.
7. **Cost and wall-clock are bounded and visible.**

## Non-goals for v1

Any language but Python. Repos without a test suite. Cross-repo analysis. Any
surface beyond the CLI. Any networked component. Multi-user anything. Running
while the machine is asleep.

## Positioning

### The index is not a moat, and this document will not pretend otherwise

Aider's tree-sitter repo map, a wave of MCP code-graph tools, Sourcegraph, and
GitHub's own code navigation all occupy the code-graph space. Nothing about
TRELLIS's call graph is novel, and a competitor could reproduce it.

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

### What would make this positioning wrong

If measured recall on the benchmark repository comes in below the pivot
threshold in `docs/ADRs/0002-edge-confidence-model.md`, goal state 2 is not
deliverable and the blast-radius claim dies. What survives is context
continuity, governance enforcement, test-anchored checks, and the committed
trail — a smaller product that still addresses the problem above, with the
index demoted from guarantee to hint. That trigger is written down before the
measurement exists, deliberately (S16).

## Status

Pre-v0.1. The workspace, the grounding contract, the parse pass, the
partitioning rules, and rollback exist and are tested. Resolution, the agent
loop, and model probing do not. No recall figure has been measured, and this
document contains none.
