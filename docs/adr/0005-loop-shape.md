# ADR 0005 — Loop shape, and why judgment is never parallelized

**Status:** Accepted
**Date:** 2026-09-18

## Context

Two tempting architectures for an agent harness: a pipeline of specialized
stages, or a committee of agents that review each other. Both are common. Both
are wrong here, for different reasons.

A scaling study measured independent multi-agent topologies amplifying errors
**17.2-fold** against a single agent, with centralized coordination containing
it to **4.4-fold**.

## Decision

**One main agent loop.** One model, a flat tool list, iterating. Blast radius
and checks are *tools the model calls*, not stages in a pipeline written around
it. The harness supplies tools, context management, and permission boundaries,
then gets out of the way.

**Many agents may run at once; none of them judge each other.** TRELLIS
parallelizes *work* — several agents on disjoint regions of a change, each in
its own git worktree. It never parallelizes *correctness*: no agent reviews
another's output for a pass verdict, no voting, no consensus.

**Partition by blast radius.** Two partitions may run concurrently only when
their radius sets **and** their file sets are both disjoint. Files matter
separately: two symbols in one file cannot be edited by two agents without a
merge conflict, however disjoint their call graphs.

**Optimistic partitioning, pessimistic integration.** Per-partition checks are
a filter, not a guarantee. When partitions return, the orchestrator applies
patches to the master worktree in a deterministic order and runs the **full**
check suite across the union. Partition-local passes never substitute for the
integrated check, because partitioning is only as trustworthy as the graph's
recall, and recall is partial (S19).

**Single-owner orchestrator.** It owns the `Index` outright — no
`Arc<RwLock<Index>>`. Everything else sends messages over a channel. Write and
check serialization comes free from the single owner, with no lock anyone can
forget to take.

**Default failure policy is `AllOrNothing`.** `LandClean` exists but is valid
only when the integrated check passes on the landed subset. A failed
partition's work never lands partially; its worktree is deleted.

## Alternatives considered

**A verification fan-out** — one agent writes, N agents review. Rejected: it is
precisely the topology the 17.2× figure describes, and it rests correctness on
model judgment, which ADR 0004 forbids. It also *feels* more rigorous, which is
what makes it dangerous.

**A rigid pipeline** (plan → radius → edit → check). Rejected: it substitutes a
hand-crafted decision tree for the model's judgment, and every real task
eventually needs a step the tree does not have.

**`Arc<RwLock<Index>>`.** Rejected: a lock anyone can forget, in exchange for
concurrency the design does not want anyway, since writes must serialize.

**Granting each partition the full budget.** Rejected: N agents on a runaway
loop is an N-times faster way to spend the user's money. Budgets divide.

## Why

Parallel work multiplies throughput. Parallel judgment multiplies error. Keep
the coordination centralized and the judgment mechanical.

Sub-agents for context isolation on separable subtasks are fine — that is
delegation, not review.

## What changes our mind

The single-owner orchestrator serializing a wide change is a real ceiling
(S5), not one to assume away. If the measured decomposition ratio shows real
tasks rarely partition into more than one group, multi-agent mode is not worth
its complexity and v1.1 should be dropped rather than shipped.
