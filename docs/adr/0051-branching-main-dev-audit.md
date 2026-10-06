# ADR 0051 — Branches: main, dev, audit, and ticket branches cut from dev

**Status:** Accepted (owner-directed).
**Date:** 2026-10-04
**Decided by:** the owner's request of 2026-10-04 ("a launch main branch for
the current version, finished and ready to launch; dev for the main
development branch; feature and ticket branches from there; and an audit
branch sitting between dev and feature branches where they are brought
together and merge conflicts are resolved"), carried out by the session under
the owner's grant of full authority for the repository's git and CI (#378).
**Amends:** ADR 0036 (the pre-push hook now protects `main` as well as
`v0.1-resolver`).

## Context

Until now `v0.1-resolver` was the default branch, moved only by the owner's
pull requests, and agents integrated tickets on `drain`, which reached
`v0.1-resolver` through a squashed PR. The names said nothing about the roles,
and there was no branch that meant "ready to launch".

## Decision

| Branch | Role | Created from | Moved by |
| --- | --- | --- | --- |
| `main` | launch: finished and ready to ship; the default branch; tagged `v0.1` at creation | `origin/v0.1-resolver` at 747df54, the last state the owner merged | the owner, merging `dev`'s pull request on GitHub |
| `dev` | development trunk; every ticket and feature branch is cut from it | `origin/drain` once #378 is merged | promotion from `audit` when its gates pass |
| `audit` | integration: ticket and feature branches meet, conflicts are resolved, gates and the security audit run | the same commit as `dev` | `git merge --no-ff` of a green ticket or feature branch |
| `t<n>-<slug>`, `feature/<slug>` | one ticket; a feature spanning several tickets | `dev` | their author; deleted after merging |

1. **Flow.** ticket or feature → `audit` → `dev` → `main`. Promotion from
   `audit` into `dev` is a fast-forward where `dev` has not moved, a
   `--no-ff` merge otherwise; conflicts are resolved on `audit`, never on
   `dev`.
2. **Gate base** (`scripts/gates/lib.sh`). A ticket, feature or `audit`
   branch is measured against `origin/dev`; any other branch, `dev` included,
   against `origin/main`. Until these branches exist on the remote the
   script falls back to `origin/drain` and `origin/v0.1-resolver`, so the
   change that adds them can be gated before they are pushed.
3. **Protection.** The pre-push hook refuses every push to `main` and to
   `v0.1-resolver`; `dev`, `audit` and ticket branches go through. GitHub
   branch protection still needs Pro on a private repository (ADR 0036).
   *2026-10-04 (#380):* it also refuses `drain` (frozen, item 5) and every
   `v*` tag (`refs/tags/v*`), whose push starts `release.yml` and is the
   owner's act; other tags go through.
4. **CI** runs on pushes to `main`, `dev`, `audit` and `v0.1-resolver`, and
   on every pull request.
   *2026-10-06 (#477):* CI runs on a push to `main` only, and on a pull
   request into `main` or `dev` (`audit` left the push list in #381, the
   pull request list narrowed to `main` and `dev` in #426, and `dev` leaves
   the push list here). Promotion from `audit` into `dev` goes through an
   `audit → dev` pull request (as `dev`'s history shows, e.g. "merge #471
   (audit -> dev)"), whose CI ran on the tree `dev` receives, and the
   `dev → main` pull request runs CI again on `dev`. A run on the `dev` push
   repeated that work, and when the concurrency group cancelled it GitHub
   showed nine failed checks on the `dev → main` pull request #473. Merging
   `main` back into `dev` after a squash leaves `dev`'s tree as it was, so
   it needs no run either. Reversed by adding `"dev"` back to `push.branches`
   in `ci.yml`.
5. **Legacy.** `v0.1-resolver` (frozen), `drain` (frozen at the commit `dev`
   and `audit` start from), `inherit`, `agents-setup` and
   `orchestrator-state` are kept, not deleted: deleting a branch cannot be
   undone, and their history is cited by issues and ADRs.
   *Amended 2026-10-06 (#474):* the owner had `agents-setup` (tip 040b634),
   `drain` (44ac48e), `orchestrator-state` (2f5e5c3) and `v0.1-resolver`
   (747df54) deleted; a comparison against `dev` found nothing in them still
   wanted. The remote now holds only `main`, `dev` and `audit`, the permanent
   branches. The tips above are what a citation by issue or ADR resolves to.
   The pre-push hook still refuses `drain` and `v0.1-resolver`, so a clone
   fetched before the deletion cannot re-create them by accident.

## Reversal

Point the default branch back at `v0.1-resolver`, restore `lib.sh`'s
`INTEGRATION_BRANCH="drain"`, and the old flow resumes; no branch was
deleted and no history rewritten. *Amended 2026-10-06 (#474):* the legacy
branches are now deleted; reversing means re-creating `v0.1-resolver` at
747df54 and `drain` at 44ac48e first (with `--no-verify`, the owner's act).

## Consequences

- An agent names three branches instead of two, and the agent definitions,
  `docs/BUILD_SYSTEM.md` and the hook's self-test say which.
- ADR 0027's decision log keeps its name; its rows now record merges into
  `audit`.
