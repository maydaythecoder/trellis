# ADR 0046 — Rollback re-reads and verifies what the run wrote

**Status:** Accepted. Built.
**Date:** 2026-10-04
**Decided by:** the owner, on #239 (options a and c), via the session.
**Holds:** ADR 0009 and ADR 0004 unchanged, and strengthened: rollback of
trellis's own writes is complete, and the run never reports it complete when
it is not. It loosens none of the ten rules in CLAUDE.md. It adds no
dependency (`std::fs::File::set_modified` is already used in `trellis-vcs`).

## Context

On the self-hosted WSL runner the VM clock stepped back about 110 s between a
fixture's commit and a test's edit (#238's investigation, evidence in #239).
Git trusts a worktree file whose stat (size, mtime, ctime) matches its index
entry when the entry is not racily clean, and it compares mtimes in whole
seconds. A same-size edit inside the original file's second is not read:
`git reset --hard` leaves it in place, and on the runner's git 2.43
`git status --porcelain` was empty afterwards. `Snapshot::is_restored`
reads `status`, so trellis would have printed `Worktree restored: true` over
a file that was not restored. Windows git notices after the reset in the
same reproduction, so the false report is platform-dependent; the leftover
edit is not.

`trellis-vcs` already defends its fingerprint path against the racy-clean
case (`a_same_size_edit_in_the_index_s_own_second_is_not_read_as_unchanged`).
The rollback path did not.

## Decision

1. **Before the reset, every path the run wrote is given an mtime no
   checkout produces** (one second after the epoch), so its stat disagrees
   with its index entry and git reads its contents
   (`Snapshot::rollback_written`). Only the run's own writes, as the
   orchestrator recorded them: the cost scales with them, not with the
   repository (option a; option b, re-reading the whole index on every
   rollback, was rejected for that cost).
2. **After the reset, the same paths are compared by content with the
   snapshot commit** (`Snapshot::written_differ`): a path the commit lacks
   must be gone, a path it has must hash to the committed blob. The run
   reports `restored: true` only when that list is empty and `is_restored`
   agrees, and names any path that still differs (option c, as a check on
   option a rather than instead of it).
3. **Hashing runs with the repository's filter drivers off**, as the
   fingerprint does: a driver is repository code. Built-in end-of-line
   conversion still applies, so a CRLF checkout hashes to the committed
   blob. A repository that depends on a filter driver (LFS) reads as
   differing: a false alarm, never a false "restored".
4. **The panic guard keeps the plain reset.** The written list lives in the
   orchestrator inside the future that panicked; `is_restored` still reports
   what git can see there.

## Not covered

Files written by something other than trellis's write path (a command that
reaches the repository by absolute path) are not in the written list. ADR
0032's position stands: the fix for those is isolation, not another git
check.

## Tests

`rollback_clock_step_*` in `crates/trellis-vcs/src/lib.rs` reproduce the stat
state (the file's mtime put back to the index entry's, the index stamped
later, `core.trustctime=false` standing in for the ctime that the clock step
also put in the same second) and show: a plain rollback leaves the edit and
the content check reports it; `rollback_written` restores the file and the
content check is empty; created and deleted paths read as differing until
rolled back. Run on Windows and on Linux (WSL).
