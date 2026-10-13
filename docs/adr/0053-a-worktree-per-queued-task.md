# ADR 0053 — Amends ADR 0020 §5: `work` runs each queued task in a worktree of its own

**Status:** Accepted (owner-directed) when the owner approves #624's pull
request (G7: the ticket is `risk:high`). Not built: the build is #709 and
#710, named at the end.
**Date:** 2026-10-12
**Decided by:** ADR 0021 row 59–63, as amended on 2026-10-06 (#332),
Accepted (owner-directed) as the session's choice under the owner's
2026-10-04 delegation. That row fixed the shape: the branch
`trellis/task/<id>`, ADR 0037's worktree mechanics, removal of an
unconverged task's worktree, a converged one kept, the queue left where it
is, a clean worktree per task, no push. This ADR records it and settles the
details the row left to it (#624). The two marked *default* below (a
dirty task worktree, `--max-tasks`) are the conventional choice #624's
dispatch named; the rest (the path, the id and existing-branch refusals,
keeping a branch that holds a trail, trust under the starting root) are the
session's choice from the code as it stands. The owner confirms all of them
at G7, or sends them back.
**Amends:** ADR 0020 §5 (`work` stops at the first converged change) and
§1 (which queue a run in a linked worktree uses); ADR 0021 row 59–63 (the
ADR it asked for now exists).
**Holds:** ADR 0009 (rule 7), ADR 0008 (rule 9), ADR 0023, ADR 0032 and
ADR 0037 unchanged.

## Context

ADR 0020 §5 stops `work` at the first converged run: the change is left
uncommitted in the one worktree `work` runs in, the next task needs a clean
tree, and accepting the change is the person's decision. An unattended
`work` therefore processes a queue only as far as its first success.

Paperclip runs each task in an isolated git worktree by default
(`enableIsolatedWorkspacesByDefault`, `packages/shared/src/validators/instance.ts:47`
at e733bddc; `enableWorktreeRunExecution`, `:93`). ADR 0021 row 59–63
accepted the same for trellis once #98–#101 had landed. They have: #98
gave `trellis_vcs::Worktree` a caller (ADR 0037), and #99–#101 are closed.
What exists to build on:

- `trellis_vcs::Worktree::create(root, run, partition, base)` adds a linked
  worktree at `.trellis/work/<run>/partition-<n>/` on a new branch
  (`crates/trellis-vcs/src/lib.rs`), and `discard` removes the directory and
  deletes the branch.
- `.trellis/work/` is ignored, so a linked worktree there does not dirty the
  tree it was made from (C-010 in `CORRECTNESS_LEDGER.md`), and the index
  does not walk it (A-32 in `docs/ASSUMPTIONS.md`).
- `work` refuses before a claim when `trellis_vcs::pending(root)` is not
  empty (ADR 0020 §5, "Refusing up front"), and `Claim::release(false)` puts
  a claimed task back untouched, as the Ctrl-C-after-claim path does.
- A run that does not converge commits its trail on the branch it ran on
  (`record_trail`), which is how ADR 0020 §5 keeps the tree clean after a
  rollback.
- Trust is recorded per repository root path, outside the repository
  (`TrustStore`, keyed by the canonical root; ADR 0023).

## Decision

1. **Branch and place.** Each claimed task runs in a linked worktree at
   `.trellis/work/task/<id>/` on the branch `trellis/task/<id>`, where
   `<id>` is the task's queue id. It is created with ADR 0037's mechanics:
   `git worktree add -b` through `trellis_vcs`, one at a time (execution is
   still serial, ADR 0020 §4). The base is the starting worktree's `HEAD`
   when the task is claimed, so a change a person has committed between two
   tasks is under every task after it.

2. **Refusals before the run, all after the claim and all the same way.**
   The task is put back with `Claim::release(false)` (not counted as an
   attempt), anything already created for it is removed, and `work` prints
   why and exits 1, as it does today for a dirty tree before a claim:
   - *Default (owner to confirm):* **the task worktree is not clean.**
     After it is created, `work` runs `trellis_vcs::pending` on it, the
     same check it runs before a claim. A fresh worktree should always
     pass; one that does not (a checkout filter, an attribute that rewrites
     on checkout) is refused rather than snapshotted dirty (ADR 0009).
   - **The id is not a branch name** (`git check-ref-format --branch`
     fails). Ids `queue add` makes always pass; a file placed by hand in
     `pending/` might not.
   - **`trellis/task/<id>` already exists, or the directory does.** It
     holds an earlier attempt's trail or a converged change (decision 4),
     and trellis does not delete what a person may still want. The message
     names the branch and `git worktree remove` / `git branch -D` as the
     person's way to clear it.

   Before each claim `work` still checks the starting worktree with
   `trellis_vcs::pending`, unchanged: a task is based on that worktree's
   `HEAD`, and uncommitted work there is something the task would not see.

3. **The run is the run.** The task runs through `cmd_run` exactly as ADR
   0020 §5 says, with the task worktree as the run's root: its snapshot,
   rollback (ADR 0009, ADR 0046), throwaway clone for untrusted writers
   (ADR 0032), budgets (one task's budget, never multiplied, rule 9) and
   every refusal of a moved `HEAD`, changed trusted code or changed `.git/`
   apply to that root. Trust is looked up under the **starting worktree's**
   key, and the task worktree's trusted code must fingerprint-match that
   entry; a mismatch is refused as untrusted, as it would be in place
   (ADR 0023). A task worktree is not a new repository to be trusted.

4. **What each outcome does.** ADR 0020 §5's table changes in one row and
   gains a column:

   | Run outcome | Task moves to | Task worktree | `work` then |
   | --- | --- | --- | --- |
   | Converged | `done/` | **kept**, with the change uncommitted, for a person | **continues** |
   | Rolled back, tree restored | `failed/` | removed | continues |
   | Not restored | `failed/` | kept, for a person to look at | stops, exit 2 |
   | `cmd_run` returned an error | `failed/` | removed | stops, exit 1 |
   | Interrupted, tree restored | `pending/`, `attempts + 1` | removed | stops, exit 130 |
   | Interrupted, not restored | `failed/` | kept | stops, exit 130 |

   "Removed" is `git worktree remove` of the directory. The branch is
   deleted only when its tip is still the base commit: an unconverged run
   commits its trail on the task branch, and deleting that branch would
   delete the trail. A kept worktree is also what a not-restored run leaves,
   since nobody has looked at it and deleting it would destroy the evidence.
   `done/` and `failed/` record the task worktree's path and branch beside
   the run id, so `queue list` can say where a converged change is. The two
   fields are omitted from a task file while unset, as `stale_releases` is
   (ADR 0020 §5, #352), so every earlier file loads and is written back
   unchanged.

5. **The queue stays where `work` started.** The queue is the starting
   worktree's `<git-dir>/trellis/queue/` (ADR 0020 §1). A task worktree has
   no queue of its own, and nothing run inside it reads or writes one: ADR
   0020 §1's "each worktree has its own queue" stays true of worktrees a
   person makes, and a task worktree's git directory is not a queue's home.

6. **`--max-tasks`** — *default (owner to confirm):* keeps ADR 0020 §5's
   meaning. It bounds how many tasks one invocation runs, converged or not,
   counted the same way as now. Each run is still bounded by its own budget
   (ADR 0008). It is not a bound on how many kept worktrees may exist.

7. **trellis never pushes, merges or commits a converged change.** Taking a
   kept change into a branch a person uses is the person's act, as ADR 0020
   rejected committing it automatically. Removing a kept worktree is theirs
   too; trellis never removes one it kept.

## Consequences

- An unattended `work` runs a whole queue instead of stopping at its first
  success, and each converged change waits in its own worktree. ADR 0020's
  Consequences line "A long unattended session stops at its first converged
  change" no longer holds once this is built.
- Kept worktrees and the branches of failed tasks accumulate under
  `.trellis/work/task/` and `trellis/task/`. trellis does not prune them;
  each costs a checkout's disk space. A pruning command is not decided here.
- A task requeued from `failed/`, or put back in `pending/` after an
  interrupt whose trail was committed, is refused at its next claim until
  its branch is cleared (decision 2), because that branch holds the earlier
  attempt's trail. This is louder than reusing or renaming the branch, and
  loses nothing.
- Tasks are based on `HEAD` at their own claim, not at the invocation's
  start, so two tasks in one invocation can have different bases. Each
  converged change is reviewed against its own branch's base.
- Execution stays serial (ADR 0020 §4). Running several task worktrees at
  once is the parallel case ADR 0005 and ADR 0016 govern, and is not
  decided here.

## Alternatives rejected

- **Delete an unconverged task's branch with its worktree**, as
  `Worktree::discard` does for a partition. It deletes the trail commit
  `record_trail` made on that branch, and trails are the record of what a
  run did (ADR 0007).
- **Reuse or rename an existing `trellis/task/<id>` on a second attempt.**
  Reusing it resets a branch that may hold a converged change; renaming
  (`trellis/task/<id>-2`) departs from the row's branch name and hides the
  earlier attempt behind a suffix. Refusing loses nothing.
- **Drop the starting worktree's clean check**, since nothing is written
  there. The base is that worktree's `HEAD`; a task started while a person
  has uncommitted work there runs against a tree they are not looking at,
  and the check costs one `git status`.
- **Make `--max-tasks` count converged tasks only**, or bound kept
  worktrees. Either changes a flag's meaning that scripts already depend
  on; a bound on kept worktrees is a separate decision.
- **Trust each task worktree on its own.** It would ask the person to
  `trellis trust` a directory trellis made, for code they already trusted
  at the same commit.

## Reversibility

High until built: this ADR changes no code. Once built, setting `work` back
to running in place restores ADR 0020 §5 exactly; kept worktrees and task
branches are ordinary git objects a person removes with `git worktree
remove` and `git branch -D`. Reversed if the build finds that a run rooted
in a task worktree cannot keep ADR 0009's snapshot and rollback (ADR 0021
row 59–63's own condition): then the per-task worktree is deferred to v1.1
with no tracking issue, and §5's stop-at-first-converged rule stands.

## The build

Filed from this ADR, each blocked by #624:

- **#709, `trellis-vcs`:** a task variant of `Worktree` (path
  `.trellis/work/task/<id>/`, branch `trellis/task/<id>`, base passed in),
  refusal of an existing branch or directory and of an id that is not a
  branch name, and removal that keeps a branch holding a commit past its
  base (decisions 1, 2, 4).
- **#710, `work.rs` (`trellis-cli`) and `trellis-queue`**, blocked also
  by #709: claim, create, check clean, run rooted in the task worktree with
  trust looked up under the starting root, keep or remove by outcome,
  continue past a converged task, and record the worktree and branch in
  `done/` and `failed/` (decisions 2 to 6).
