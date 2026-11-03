# ADR 0061 — A draft pull request when work starts

**Status:** Accepted (owner-directed). Built (#1051).
**Date:** 2026-11-03
**Issue:** #1051.
**Decided by:** the owner, 2026-11-03, as a standing rule for every agent
and contributor.
**Holds:** ADR 0051's branch flow unchanged: ticket and feature branches are
still cut from `origin/dev` and merged into `audit` with `--no-ff` once
their gates exit 0. Every rule in CLAUDE.md "Rules that need an ADR to
change"; none is loosened.

## Context

Work on a ticket was visible only once its branch was merged into `audit`.
Until then nothing on GitHub said a ticket was taken, which branch held it,
or how far it had got, and two runners could start the same ticket.

## Decision

The moment work starts on a ticket, its draft pull request is opened:

1. Cut `t<n>-<slug>` (or `feature/<slug>`) from `origin/dev` and push it at
   once; an empty commit is enough to give it something to push.
2. `gh pr create --draft --base audit` with a body that links the ticket.
3. `gh pr ready` only once the ticket's Verify lines and the gates pass.
   Then the branch is merged into `audit` with `git merge --no-ff`, as
   ADR 0051 says, and pushed; that merge is what closes the pull request.

The draft pull request gets no CI: `ci.yml` runs on pull requests into
`dev` and `main` and on a push to `main`, and nothing runs on a pull request
into `audit` (docs/BUILD_SYSTEM.md, "CI runners"). The evidence for a ticket
is still its gates and the CI of the `audit → dev` pull request that carries
it. No gate checks that the draft exists: a gate that asked GitHub would
fail offline, and a gate that cannot run fails (docs/BUILD_SYSTEM.md,
"Gates").

## Consequences

- The open draft pull requests are the list of tickets in progress, each
  with its branch.
- `docs/BUILD_SYSTEM.md` states the rule once; CLAUDE.md and the
  queue-runner, orchestrator, git-ci-steward and implementer definitions
  point at it.

## Reversal

Drop the rule from those places and amend ADR 0051's line; nothing else
depends on the drafts.
