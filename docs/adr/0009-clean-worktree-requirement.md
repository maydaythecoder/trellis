# ADR 0009 — The clean-worktree requirement

**Status:** Accepted
**Date:** 2026-09-18

## Context

Rollback must return the repository to its exact pre-run state. That is v1.0
gate condition 4 and goal state 4. It is the one path that has to be reliable,
because it is the path taken when everything else has already gone wrong.

## Decision

`trellis run` **requires a clean worktree** and refuses otherwise, naming the
uncommitted changes and saying why.

Before a run: record `HEAD`. During: the orchestrator is the only writer. On
any failure or budget exhaustion: `git reset --hard` to the recorded commit,
then `git clean -fd` to remove untracked files the run created.

Both steps are required. `reset --hard` alone leaves files the run created;
`clean -fd` alone leaves modified tracked files. Residue is what turns "failure
is boring" into "failure is a mystery".

Rollback is idempotent — error paths fire twice more often than anyone expects.

Verified by an induced mid-run panic in the test suite, and by the same at
larger scale in CI against the benchmark repository.

"Any failure" includes a panic in `trellis run` itself. From the snapshot
until the outcome is dispatched, a guard's `Drop` rolls back while the
panic unwinds, reports on stderr and lets the panic continue; no trail is
written, because the turn list died with the loop. After dispatch each path
owns the tree (a converged run leaves it, the others roll back and commit a
trail), and the guard is disarmed so a later panic cannot remove that
commit. A panic before the snapshot has nothing to undo. (Amended after the
second review of PR #1: before it, a panic inside the run's `block_on`
unwound past the rollback. Test:
`a_panic_inside_the_run_still_rolls_the_worktree_back`.)

*Amended after the tenth review of PR #1: the recorded point is a commit on
a branch.* The snapshot records the branch `HEAD` is on (or that it is
detached) beside the commit. `reset --hard` moves whichever branch `HEAD`
is on, so after a writer's `git checkout -b` it reset the writer's branch,
left the user's holding the writer's commits, and "restored" was reported
because the commit matched. The rollback now re-points `HEAD` first
(`symbolic-ref HEAD <branch>`, or `update-ref --no-deref HEAD <commit>` for
a detached snapshot) without touching the worktree, then resets and cleans
as before; when `HEAD` cannot be re-pointed nothing is reset, since the
reset would move a branch the snapshot never recorded. Restored now also
means `HEAD` is on the recorded branch. A branch the writer created, or a
ref other than `HEAD` and its branch that it moved, is not recorded and
not put back. Evidence, in `trellis-vcs`:
`rollback_after_a_branch_switch_puts_the_original_branch_back_at_the_snapshot`,
`is_restored_is_false_on_another_branch_at_the_same_commit` and
`a_detached_snapshot_is_rolled_back_detached_and_moves_no_branch`, all three
failing before the change (HEAD left on the writer's branch, or reported
restored there). Where a moved `HEAD` is refused: ADR 0026 §11 (process
agents) and ADR 0023 §5 (model runs).

*Amended after the eleventh review of PR #1: an unborn branch, and a
HEAD that cannot be put back.* The branch was read with `rev-parse
--symbolic-full-name HEAD`, which fails when `HEAD` names a branch with no
commit, as a writer's `git checkout --orphan x` leaves it. The rollback
returned that error before `reset --hard` and `clean -fd`, so nothing was
restored. The branch is now read with `symbolic-ref -q HEAD`, which names
an unborn branch, and `head_moved` reports one as "`x` (no commit)" rather
than failing. The sentence above, "when `HEAD` cannot be re-pointed nothing
is reset", is replaced: when it cannot, the tree is still restored, with
`read-tree --reset -u <commit>` and `clean -fd`, which write the index and
the worktree and move no ref, and the rollback then reports the failure
with anything else that failed. A reset would still move a branch the
snapshot never recorded, so it is not used on that path. Evidence, in
`trellis-vcs`:
`rollback_after_an_orphan_checkout_puts_the_branch_back_and_restores_the_tree`
(failed before: `rev-parse HEAD` errored on the unborn branch) and
`a_head_that_cannot_be_put_back_still_restores_the_tree_and_reports_it`
(failed before: the tracked edit was left in place).

What this rollback guarantees when trellis is not the only writer (bash,
custom tools, hooks, process agents) is stated in ADR 0023 §5 and ADR 0026
§11: for those writers it is best-effort against git's view of the tree.

## Alternatives considered

**Snapshot a dirty tree and restore it.** Rejected. It means TRELLIS takes
custody of uncommitted work and hands it back, which is a data-loss bug
generator in the exact path that must never lose work. The stash-based version
has the same problem with extra failure modes (stash conflicts on restore).

**Commit the user's changes automatically before starting.** Rejected: it
rewrites the user's history without asking, and "TRELLIS made a commit I did not
write" is a worse surprise than "TRELLIS asked me to commit first".

**Warn and proceed.** Rejected: the guarantee becomes conditional, and the
condition is invisible at the moment it matters.

## Why

This is a real constraint on the user and the right trade. Refusing costs the
user one `git stash`. The alternative costs some user their uncommitted work,
once, unrecoverably — and that is the kind of failure a tool does not come back
from.

The refusal message states the reason, because an unexplained refusal reads as
an arbitrary obstacle and gets worked around.

## Implementation note

`trellis-vcs` shells out to `git` rather than using `git2` for snapshot and
rollback. The brief blesses this as an acceptable shortcut; it is chosen
deliberately here. `reset --hard` and `clean -fd` have exact, well-understood
semantics as commands, and this is a poor place to be learning a binding's edge
cases. `git2` remains for worktree management in multi-agent mode, where the
library genuinely helps.

## What changes our mind

Nothing about the requirement. The *implementation* moves to `git2` if shelling
out proves slow or fragile in CI — measured, not assumed.
