# ADR 0037 — Amends ADR 0016: running a staged plan in partition worktrees

**Status:** Accepted.
**Date:** 2026-09-29
**Decided by:** the owner, via the coordinator, on issue #98 (2026-09-28).
**Amends:** ADR 0016, which said how a plan is *shaped* and not how it is
*run*. Leaves ADR 0005, 0008 and S19's three patches as they are; the rules
they carry (CLAUDE.md rules 5, 6 and 9) are what this shape is held to.

## Context

ADR 0016 made `partition::plan` return `Plan { shared, parallel }`: work the
index cannot vouch for, and work contending for its files, runs first and
alone; the rest may run concurrently once it has landed. Nothing ran a plan.
`trellis_vcs::Worktree` had been written ahead of need (C-002) and had no
caller, and the crate had no `JoinSet`, no semaphore and no drop guard.

## Decision

`trellis_loop::Supervisor::run_plan` runs a plan.

1. **The shared stage runs alone, first.** It is joined to completion before
   any parallel partition is spawned. If it does not land, the parallel stage
   does not start and its partitions are reported as not started, under
   either `FailurePolicy`: ADR 0016's case for running them concurrently is
   that the contended work has already landed.
2. **The parallel stage runs in a tokio `JoinSet` bounded by
   `Semaphore(N)`.** Both come from tokio's `rt` and `sync` features, which
   the workspace already enables. The permit is taken before the partition's
   worktree is created, and worktrees are created one at a time by the
   supervisor, because concurrent `git worktree add` calls contend for
   `.git/worktrees`.
3. **One `trellis_vcs::Worktree` per partition**, branched from the run's
   base commit, owned by a drop guard in `trellis-loop`. The guard lives in
   the partition's task. A panic drops it with the task's future, so the
   directory and branch go and the panic comes back as that task's
   `JoinError`, attributed to its partition by task id and reported as
   `Panicked`. On a normal finish the task returns the guard, and the
   worktree lives until the merge has read it. The guard's `Drop` logs a
   failed removal rather than panicking, because it runs during unwind.
4. **Merge in plan order, into the root's working tree, uncommitted.** Plan
   order is `Plan::in_order`, whatever order the tasks finished in. What a
   partition changed is read from its worktree with `git diff --name-status`
   against the base and `git ls-files --others`, through
   `trellis_vcs::command()`. Every change set is read before anything is
   written, so an all-or-nothing merge stops before its first write. A path
   an earlier partition already changed is not overwritten; the later
   partition is reported `Unmergeable`. So is a path under `.trellis/` or
   `.git/`, or one that is not a regular file.
5. **Budgets are dealt, not split.** `Budget::shares(n)` gives the remainder
   of iterations and tokens to the first partitions in plan order, so the
   shares sum to the run budget exactly. A plan with more partitions than the
   run has iterations or tokens is refused before any worktree exists.
   `Budget::shares` is the only division: `Budget::split`, which floored
   and clamped each share to at least one, granting a wide plan more than
   the ceiling, is removed (amendment below).

## What this does not do

- **It issues no verdict.** `Supervised` carries each partition's own
  outcome and the paths merged, nothing a caller could read as "checked".
  The integrated check over the merged union runs on the root afterwards,
  and it is the caller's step (rule 6). No partition's output is read to
  decide about another's (rule 5).
- **`trellis run` does not call it yet.** Running more than one agent from
  the CLI, and recording suspected recall gaps, are the rest of #24.

## Consequences

- **Wall-clock is not shared across stages.** ADR 0008 copies the whole
  wall-clock ceiling to every share because partitions run concurrently. The
  two stages run one after the other, so the parallel stage is given what the
  shared stage left, and does not start if it left nothing.
- **Parallel partitions branch from the base, not from the shared stage's
  result.** They do not see the shared stage's edits while they work. The
  merge is still conflict-free by construction, because ADR 0016 keeps the
  shared stage file-disjoint from every parallel partition; what they could
  not see is exactly what the integrated check exists to catch.
- **Rollback of the root is the run's snapshot's.** A merge that fails
  part-way through a write leaves the root holding part of it, and says so;
  `Snapshot::rollback` restores it as it restores any other run.
- **`FailurePolicy::LandClean`'s documentation contradicts itself** (failed
  worktrees "preserved" and "deleted"). This ADR takes "deleted": the drop
  guard removes every worktree before `run_plan` returns. The enum's comment
  is left for the ticket that wires `LandClean` into `trellis run`.
- An empty `.trellis/work/<run>/` directory is removed after the run. A
  worktree that could not be removed is left in place with a warning naming
  it.

## Alternatives rejected

**Merge with `git merge` onto the run's branch.** Moves `HEAD`, and a run
whose `HEAD` moved stops counting (ADR 0009). Applying into the working tree
leaves the existing snapshot, rollback and integrated check in charge.

**Branch parallel partitions from the shared stage's result.** The owner's
decision names the run's base commit. Sequencing already keeps the stages
apart in time, and file-disjointness keeps the merge clean.

**Create worktrees inside the tasks.** Faster to start, and races on
`.git/worktrees`.

## Reversibility

High. The supervisor is one module (`crates/trellis-loop/src/supervise.rs`)
with no caller outside its tests, and `Budget::shares` is additive.

**Amended 2026-10-05 (#402, #429):** `Budget::split` is removed, so decision 5 names
`Budget::shares` as the only way a budget is divided.
