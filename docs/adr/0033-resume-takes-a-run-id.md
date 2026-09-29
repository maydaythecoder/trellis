# ADR 0033 — `trellis resume` takes an optional run id

**Status:** Accepted.
**Date:** 2026-09-29
**Decided by:** the owner, via the coordinator, on issue #25 (2026-09-28).
**Does not amend:** ADR 0007. The trail is still read back as untrusted data.

## Context

`trellis resume` read the newest trail and nothing else. `trellis log` has
printed a run id for every trail since trails gained `run_id`, and
`Entry::run_id` falls back to the filename stem for older ones so that every
run has a handle, but no command accepted that handle. Reading an older run
meant opening its JSON by hand.

The CLI surface is fixed by brief 4.12 and grows only when the owner directs
it. The owner directed this one.

## Decision

`trellis resume [<run_id>]`, and `/resume [<run_id>]` in the session.

- With no id, the command behaves as before: the newest trail.
- With an id, it loads the trail whose run id equals it exactly, as
  `trellis log` prints it. A prefix or substring does not match: a partial
  match could pick one of two runs sharing it, and resuming from the wrong
  run is worse than refusing.
- An id no trail carries is an error that lists the ten most recent run ids,
  newest first.

## What does not change

- The named trail is read as `UntrustedTrail` and printed through the same
  control-character filter as `log` (ADR 0007, S10). Run ids in the error
  are trail-derived text and go through that filter too.
- `trellis run --resume` still carries the newest trail and takes no id.
  What reaches a run is still `PriorContext`, fenced as data, and cannot make
  a run converge (`prior_context_can_never_make_a_run_converge`, the
  `resume_not_a_verdict` guard). When the named run is not the newest,
  `resume` says that `run --resume` would carry a different trail rather
  than offering it.

## Limits

- A trail that does not parse is skipped by the listing `resume <run_id>`
  searches, with a warning, so an id naming a corrupt trail is reported as
  unknown rather than as corrupt.

## Reverses if

The owner withdraws the argument. Removing it restores the previous surface
exactly; no on-disk format changed.
