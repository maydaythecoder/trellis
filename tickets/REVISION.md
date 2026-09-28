# Revision — drain run 2026-09-28

The tickets are GitHub issues on `maydaythecoder/trellis` (project #2), not
files; this is the summary of the revision pass over them. Decisions made
during it are in `docs/adr/0027-drain-run-decision-log.md`. Each parked
ticket carries its question in a `## Parked` section on the issue.

```
Revised:   31 tickets   (Done when + Verify written, pre-check recorded)
Created:   33           (#121–#153, from splits and extracted decisions)
Merged:    0 duplicates
Unblocked: 1            (#101 no longer waits on #100, which was already done and is closed)
Executable now: 25      ← the queue for Phase 2
  + 20 more once their blockers close (blocked only on open issues)
Parked:    76           (label `parked`, each with a question and a recommendation)
```

Method: four architect agents, one per group of stories, each rewrote its
tickets under one brief and ran every new Verify once on `inherit`. The counts
are from `scripts/tickets/next.sh --why` and `gh issue list --label parked` at
the end of the pass, after `scripts/tickets/sync-ready.py` mirrored the
verdicts onto the labels.

Before the pass, no open ticket had a `## Done when` section, so G0 accepted
none (the old `ready` label was on 14, set from a hard-coded table).

## Fixpoint

One pass converged: after it, no open ticket lacked Done when / Verify unless
it was parked, split into children, or a story. The second pass was the audit
that established this; it changed nothing.

## Pre-check

Every new Verify was run once on `inherit` before any work. All exited
non-zero (1, or 127 where the script the ticket creates does not exist yet),
except #100's, which exited 0: its work was already on the branch. It was
verified through G6 and closed. Measurement tickets whose runs take longer
than ten minutes (#144, #147, and the mutation run in #126) were not run in
the pre-check; their Verify lines, which check the report exists, exit 1.

## Found while revising

- The gates and the queue disagreed: `next.sh` read the `ready` label, G0
  read the body. Both now read the body; `sync-ready.py` keeps the label in
  line for the board.
- `cargo test -p x <filter>` exits 0 when nothing matches, so a Verify naming
  tests not yet written would pass. `scripts/tickets/verify-tests.sh` fails
  on zero.
- `triage:blocked` never cleared once blockers closed; now it does.
- `inherit`'s history was rewritten mid-pass to drop key-shaped test literals
  (GitGuardian); the tip tree is byte-identical, base commits in tickets
  were moved to `74855c9`. `scripts/checks/measured-at.sh` (#132) needs its
  base to be an ancestor of HEAD, so another rewrite would break #6, #15,
  #17, #97 and #132's Verify lines.
