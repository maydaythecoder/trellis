# ADR 0041 — Amends ADR 0037: the integrated check after partitions merge

**Status:** Accepted.
**Date:** 2026-09-29
**Decided by:** the owner, via the coordinator, on issue #99 (2026-09-28).
**Amends:** ADR 0037, which left the integrated check over the merged union
to "the caller's step". Leaves ADR 0004, 0005 and 0009 as they are; this is
how a staged plan is held to them (CLAUDE.md rules 1, 5, 6 and 7).

## Context

`Supervisor::run_plan` merged what each partition changed into the root and
returned `Supervised`, which carries no verdict. Nothing then checked the
union, and nothing stopped a caller from treating every partition's local
pass as the run's. ADR 0005 says partition-local passes never substitute for
the integrated check, because a plan is only as good as the graph's recall.
A failure that crosses partitions is the evidence that recall missed an
edge, and nothing recorded it.

## Decision

1. **`Supervisor::run_integrated` is the only public way to run a plan.**
   `run_plan` is `pub(crate)`. `run_integrated` snapshots the root (a clean
   worktree is required, rule 7), runs the plan, and then runs
   `trellis_checks::typecheck` and `trellis_checks::run_tests` over `.` on
   the root: the same executed checks a single-agent run and a process agent
   converge on. No new `Verdict` constructor or path exists; the verdicts
   come from `trellis-checks`' public entry points and nowhere else (rule 1).
2. **Both checks always run**, the tests even after a failed type check,
   whatever the partitions reported. A run that is already not converging
   cannot be misled by the second verdict, and it is more output to
   attribute a collision from.
3. **Only the integrated verdicts can converge the run.** A partition's
   outcome can only stop it: under `AllOrNothing` any failed partition, and
   under `LandClean` no partition landing, means not converged, however the
   checks went. An all-or-nothing merge with a failed partition writes
   nothing, and the untouched root it leaves passes; that pass is about
   nothing the run did. A moved `HEAD`, or one that cannot be read, also
   stops it (ADR 0009). No partition's output is read to decide anything
   about another's (rule 5).
4. **Not converged means rolled back, all together.** `Snapshot::rollback`
   restores the root as it was before the plan ran, which takes out every
   partition's merged work at once; the worktrees are already gone. A merge
   that fails part-way is rolled back the same way before its error is
   returned. A failed rollback is an error, never a quiet outcome. The
   snapshot is held by a guard that rolls back when dropped still armed, so
   a panic, or a caller dropping the future while the integrated check runs
   (an interrupt, a timeout), does not leave the unchecked union in the
   root; only convergence disarms it.
5. **A suspected recall gap is recorded from the check's own output.** For
   each failing integrated verdict, the merged paths its stdout or stderr
   names (as written, with `\` separators, or absolute under the root, and
   only as a whole path) are attributed to the partitions that merged them.
   Two or more partitions make a `SuspectedRecallGap { partitions, files,
   check }`. The trail gains `factual.suspected_recall_gaps`, a list of
   `{partitions, files, check}`, `#[serde(default)]` and omitted when empty,
   so every trail written before it keeps its bytes and digest.

## What this does not do

- **`trellis run` still does not run a plan**, so no trail yet carries a
  gap; the CLI copies `Integrated::suspected_recall_gaps` into the trail
  when it does. The record type is mirrored in `trellis-loop` rather than
  imported from `trellis-trail`, because `trellis-loop` does not depend on
  `trellis-trail` and this change adds no dependency. Both are built only
  from a `Verdict`'s output.
- **A gap is suspected, not established.** A failure that names one
  partition's file, or no file, is not recorded, though the collision may
  still be real; a failure that names two may have two causes. The record
  says what the output named.
- **The integrated checks get the caller's `CheckConfig` as given**,
  including its `deadline` and `timeout_s`; the run's remaining wall-clock
  is the caller's to set there.

## Reversibility

High. One module (`crates/trellis-loop/src/integrate.rs`) with no caller
outside its tests, and one additive, optional trail field.
