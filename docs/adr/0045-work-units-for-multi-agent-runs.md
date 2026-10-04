# ADR 0045 — Work units for `trellis run --agents N` are named targets

**Status:** Accepted. Not built (#101 builds it).
**Date:** 2026-10-04
**Decided by:** the owner, on #211 (option a), via the session.
**Holds:** CLAUDE.md rules 5, 6 and 9 unchanged: no agent judges another,
unresolved and candidate edges stay unpartitionable and integrated checks
are never skipped, and budgets divide across partitions. It grows the
`trellis run` surface by one flag, recorded here as brief 4.12 requires.

## Context

`Supervisor::run_integrated` needs a `Plan`, and `partition::plan(index,
units)` builds one from work units, each a target `SymbolId`. `trellis run`
has only the task sentence. The one caller today, `trellis partitions`,
takes symbols on the command line, and its own doc comment says turning
prose into work units "needs a model able to decompose a task. None is
available above the floor." BRIEF 4.13 assumes "a task that decomposes into
N units" without saying how. #101 stopped there (SPEC-WRONG, 2026-09-30).

Nothing measured so far can decompose a task: on poetry, qwen3:8b could not
emit a valid tool call in eight runs out of eight
(`docs/bench/cost-by-radius.md`).

## Decision

1. **The work units of a multi-agent run are named by the user.** `trellis
   run` takes `--target <symbol>`, repeatable. Each target resolves through
   the same symbol resolution `trellis radius` and `trellis partitions` use;
   an unknown or ambiguous symbol is refused before anything runs, with the
   candidates listed, exactly as `radius` refuses it.
2. **`--agents` above 1 with no `--target` is refused**, with a message
   naming `--target` and `trellis partitions` (which shows the plan the
   targets would produce, without running it). A run with targets and
   `--agents 1` runs the plan's partitions one after another.
3. **`--agents` accepts a number of at least 1, or `auto`.** `auto` is
   `BatchingCapability::auto_agents()` from the doctor probe, which is 1 for
   an unmeasured or unknown backend. `0` and anything else are refused,
   naming the accepted values. The width is never more than the plan's
   number of parallel partitions.
4. **The plan decides concurrency, not the flag.** Targets reached through an
   unresolved or candidate edge share one conservative partition (S19,
   rule 6); `--agents 8` over a plan with two parallel partitions runs two.
   The integrated check runs after the merge whatever the partition-local
   results were (ADR 0042).
5. **Model decomposition is not part of this.** A model proposing targets
   from the task sentence is a later, separate ticket, gated on a measured
   configuration that can do it; its proposals would go through the same
   resolution and refusal as `--target`.

## Alternatives rejected

**Model decomposition now (option b).** It needs a new wire shape and a
model above a floor that no measured configuration reaches. Shipping it
would make multi-agent runs depend on the least reliable part of the system.

**Both at once (option c).** Option (a) is the first half of (c) and the
second half needs its own measurement, so (c) reduces to (a) plus a ticket.

## Consequences

- #101's Decision is this ADR, and its Done when adds `--target` handling
  and the refusal in point 2.
- The work units are fully determined by the command line, so every
  multi-agent test drives `trellis run` with scripted models and no runtime.
- A user must know which symbols a task touches. `trellis partitions` and
  `trellis radius` are how they find out, and both already exist.
