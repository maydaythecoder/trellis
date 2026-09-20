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
