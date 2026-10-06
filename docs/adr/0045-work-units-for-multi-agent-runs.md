# ADR 0045 — Work units for `trellis run --agents N` are named targets

**Status:** Accepted. Built by #465, #466 and #482 (see the amendment below).
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

## Amendment 2026-10-06: built (#465, #466, #482)

The decision above is built as written; nothing in it changed.

- **#465: flags and refusals, all before the snapshot.** `trellis run` parses
  `--agents N|auto` and repeatable `--target` (§1, §3). With no target, any
  width but 1 is refused naming `--target` and `trellis partitions` (§2); `0`
  and anything not a number or `auto` are refused naming the accepted values
  (§3); an unknown or ambiguous target is refused as `radius` refuses it
  (§1); `auto` on a backend with no measured capacity is 1 and says why
  (S18). Tests in crates/trellis-cli/tests/partitions.rs:
  `agents_above_one_without_target_is_refused_naming_target_and_partitions`,
  `agents_zero_is_refused_naming_the_accepted_values`,
  `agents_that_is_not_a_number_is_refused_naming_the_accepted_values`,
  `agents_with_an_unknown_target_is_refused_before_the_snapshot`,
  `agents_auto_on_a_scripted_backend_runs_at_width_one_and_says_why`.
- **#466: the plan's partitions run through `Supervisor::run_integrated`.**
  `run_partitioned` (crates/trellis-cli/src/run_cmd.rs) gives each partition
  its own agent, worktree and share of the one budget (rule 9), the run's
  governance (the vault's notes root included, ADR 0049), and checks the
  merged union as a whole (§4, ADR 0042). `run_integrated` alone takes the
  snapshot and the rollback. Refused before the snapshot: approve mode above
  width 1, trusted repository code (its guards watch the root), and
  `--image`. Option (a)'s scripted model gives each target its own steps
  through a `targets` section (crates/trellis-loop/src/scripted.rs, tested by
  `an_agent_plays_its_targets_section_and_falls_back_to_steps`,
  `a_section_for_a_symbol_the_run_was_not_given_is_refused`,
  `two_sections_for_one_agent_are_refused_rather_than_one_dropped` and
  `a_bad_step_in_a_section_is_refused_naming_the_section`). Tests in
  partitions.rs: `three_partition_run_converges_with_every_partitions_edit`,
  `induced_collision_fails_the_integrated_check_rolls_back_and_records_a_gap`,
  `agents_above_one_in_approve_mode_is_refused_before_the_snapshot`,
  `agents_above_one_with_trusted_repository_code_is_refused_before_the_snapshot`,
  `a_partitions_edit_into_the_vault_is_refused_and_named`.
- **#482: width 1 with targets.** Until #482, `--agents 1 --target ...` (and
  `auto` resolving to 1) printed that partitions run one at a time and then
  ran one agent on the whole task, without the targets. It now runs the
  partitions one after another through the same `run_partitioned` path, as
  §2 says, with every refusal above; at width 1 approve mode is allowed, one
  agent asking at a time. A run with no target is the single-agent run,
  unchanged. Test:
  `agents_one_with_targets_runs_each_partition_through_the_partitioned_path`.

## Amendment 2026-10-06: an explicit N asks the backend (#483)

§3 judged only `auto` against the backend, so `--agents 3` on a backend
that serves one request at a time ran three agents at once and said
nothing. An explicit N above 1 now takes the same probe as `auto` and
`trellis doctor` (`doctor_cmd::probe_batching`): measured continuous
batching keeps N, and anything else (single slot, unmeasured, unknown, or a
probe that failed) runs at width 1 through the partitioned path, with the
`agents:` line saying why and what N would need. This is one more ceiling
beside §4's plan, not a change to it. It applies to hosted providers too,
which batch but report no number. The second Consequence above no longer
holds for runs above width 1: a scripted model has no measured capacity, so
`three_partition_run_converges_with_every_partitions_edit` and
`agents_above_one_in_approve_mode_is_refused_before_the_snapshot` are
served by a loopback llama.cpp stub that reports its slots. Test:
`agents_explicit_on_a_non_batching_backend_runs_at_width_one_and_says_why`.
