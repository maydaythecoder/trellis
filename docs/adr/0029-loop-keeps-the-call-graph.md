# ADR 0029 — `trellis-loop` keeps a hard dependency on trellis-index

**Status:** Accepted.
**Date:** 2026-09-28
**Decided by:** the owner, via the coordinator, on issue #7.
**Does not amend:** ADR 0002, 0005, 0011, 0014 or 0016. It records why the
crate graph stays as those ADRs already assume.

## Context

The direction brief (`history/TRELLIS-direction.md`, "graph as an optional
tool crate") made the index optional, so that the harness would survive a
resolver whose recall disappointed. The crate graph never followed:
`trellis-loop` names `trellis-index` as an ordinary dependency in its
`Cargo.toml`, with no cargo feature around it, and no ADR recorded the choice.
Issue #7 asked for one of two outcomes: an ADR accepting the dependency, or
`blast_radius` behind a feature with CI building `trellis-loop` without it.

What `trellis-loop` uses from `trellis-index` today:

- `orchestrator.rs` holds a `trellis_index::Session` as a field of
  `Orchestrator`, opens it in `Orchestrator::new`, reopens it when a write
  creates a `.py` file, and serves the `blast_radius` tool from it
  (`trellis_index::radius::blast_radius`).
- `partition.rs` builds every `WorkUnit` from a blast radius
  (`unit_for`) and plans over an `Index` (`plan`, which also asks
  `Index::is_partitionable`).
- `tools.rs` carries `RadiusReport` in `ToolResult::Radius`.
- `trust.rs` compares `.trellis/.gitignore` against
  `trellis_index::cache::IGNORE` to recognise the index cache as trellis's own
  write.

## Decision

`trellis-loop` keeps a **hard dependency on trellis-index**. There is no cargo
feature gating `blast_radius`, and no CI job builds `trellis-loop` without the
index.

## Why

1. **The orchestrator owns the index session.** `Orchestrator` is the only
   write path to the working tree, and it keeps the `Session` beside that path
   so every edit updates the graph before the next `blast_radius` call reads
   it. The reindex on edit, and the full reopen when a write creates a
   Python file that `Session::update` could not see, exist to stop the tree
   and the graph diverging. Making the session optional would put a second shape of
   `Orchestrator` behind a feature, with a write path that has no graph to keep
   in step, for a build no user asked for.

2. **Partitioning needs the edges to prove units disjoint.** ADR 0005 allows
   two units to run concurrently only when their radius sets and their file
   sets are both disjoint, and ADR 0016 stages the plan on the same proof.
   Rule 6 (Unresolved and candidate edges are unpartitionable; integrated
   checks are never skipped on partition-local passes) is enforced by reading
   the edges: `unit_for` takes `partitionable` from the radius report, and
   `plan` sends any unit the index cannot vouch for to `shared`. Without the
   index there is nothing to prove disjointness with, and the only sound plan
   is one partition. A feature that removed the index would remove
   partitioning with it, not make it optional.

3. **Recall doubt is already handled, and not by a flag.** The direction
   brief's worry was a resolver that under-reports. That is answered where the
   doubt lives:
   - ADR 0002 and ADR 0014: every blast radius ships with its coverage, as two
     facts and never a ratio, so a caller always sees what the radius could
     not bind. A radius that is short says so.
   - ADR 0011: the S16 trigger (recall below 60% on the hand-audited sample)
     is the pre-registered point at which the index is demoted from guarantee
     to hint. It fired at 47.8%, and ADR 0012's receiver typing cleared it on
     a fresh sample (72.0%, CI [63.2%, 80.8%], `docs/STATUS.md`), so the
     guarantee stands in the S1 fallback band.
   A cargo feature would not change what a radius reports or how sure it is;
   it would only change whether the crate compiles without it. Demotion to a
   hint, if it comes, is a change to how the loop treats a radius, decided
   under ADR 0011, not a change to the crate graph.

## Consequences

- `trellis-loop` cannot be built or tested without `trellis-index`. This is
  the direction the boundaries in `CLAUDE.md` already allow: the rule is that
  `trellis-index` must never depend on `trellis-model`, not that the loop must
  be buildable without the index.
- The graph stays testable with no model, because the dependency runs from
  the loop to the index and never back.
- The direction brief's "optional tool crate" is not implemented. This ADR is
  the record that it was declined, and on what grounds.

## What would reverse this

**S16 firing again on the human audit (#3).** That audit is the first
human-read, blind measurement (A-34); the figures above are model-read. If it
lands below 60%, ADR 0011 option A executes: the index is demoted to a hint.
That is the point at which making the graph an optional tool crate, with
`blast_radius` behind a feature and a CI job building `trellis-loop` without
it, should be reconsidered in a new ADR. Nothing short of that reopens this:
not build time, not a wish for a smaller binary.
