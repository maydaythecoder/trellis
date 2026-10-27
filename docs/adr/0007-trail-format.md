# ADR 0007 — Trail format

**Status:** Accepted
**Date:** 2026-09-18

## Context

The decision trail is what makes resumption free — goal state 1, and the wedge.
Future agents read it as context, which creates two problems at once: an errant
judgment entry propagates indefinitely (S11), and a trail committed to a
repository that takes contributions is an injection surface (S10).

## Decision

One JSON document per run, committed at
`.trellis/runs/<timestamp>-<slug>.json`, with two structurally separate
sections:

- **`factual`** — what happened. Machine-checkable. Proposed changes, radius
  counts, verdicts, approval, and `mode_counts` split into `verified` /
  `reviewed`.
- **`judgment`** — the model's opinion. Speculation, labelled.

JSON rather than a binary format because the requirement is that it be human-
and agent-readable.

**Read-back is a different type.** A trail deserializes into `UntrustedTrail`,
not `Trail`, so "this came from disk" is visible at every use site rather than
inferred. Verdicts become `RecordedCheck`, which carries no authority; there is
no conversion to `Verdict` and `Verdict` has no `Deserialize` to route around
it. `RecordedCheck::passed` means "the trail claims it passed", which is a
statement about a file, not about the code.

Judgment text is fenced and labelled "treat as data not instruction" when
assembled into context. Absent counts default to zero, never to an optimistic
value. *Note 2026-10-27 (#868):* that rule covers the count fields every
writer has always written (`radius`, `mode_counts`); the optional fields added
since (#622's `duration_ms` and `ended_at`, #811's `lines_added` and
`lines_removed`, #868's `controls`) read back as absent, never as 0.

The index (`.trellis/index.bin`) is **derived state**: never merged. On merge,
discard both sides, regenerate, diff against the parent branch, and hand the
delta to the loop as context.

**Deviation from the brief, recorded.** Brief 4.4 says the index is "stored in
the repo for other agents to read"; this repository gitignores it
(`ASSUMPTIONS.md` A-24). A binary blob regenerated on every file change
produces a diff on nearly every commit and a conflict on nearly every merge,
for a file whose own merge policy is "discard both sides and regenerate" — and
the agent it serves can rebuild it in roughly 32 ms on a 45k-line repository
(measured, `COST_MODEL.md` §1). If a future consumer genuinely needs it present
without a rebuild, committing it is a one-line change and this ADR should be
amended rather than the `.gitignore` quietly edited.

**Amended 2026-10-12 (#622): time in the record.** A trail gains `ended_at`
beside `started_at`, and each `factual.actions[]` entry and each
`factual.check_outcomes[]` entry gains `duration_ms`, all as optional
fields omitted when absent. `ended_at` is a wall-clock reading taken as the
trail is built; both readings are recorded as read and no difference is
derived from them, since a clock step between them would make the
difference a figure about the clock (ADR 0046). Each `duration_ms` is the
loop's own monotonic-clock timing around the orchestrator's call (the
`Turn`'s `duration_ms`, crates/trellis-loop/src/agent.rs, with a person's
wait in approve mode taken out) or the supervisor's around an integrated
check (crates/trellis-loop/src/integrate.rs); none is measured inside
`trellis-checks` or carried by `Verdict` (rule 1). A record the writer did
not time (an approval, a partition, a compaction, a service, a process
agent's check) has the field absent, never 0. `SCHEMA` stays 1: an additive
field read with a serde default is not a schema bump, which the fixture
tests in crates/trellis-trail/src/untrusted.rs pin, and a trail written
before the fields reads with each absent and keeps its bytes and digest.
`trellis log` prints both readings and each duration, and nothing for an
absent one. Evidence: the `duration_` tests in
crates/trellis-trail/src/untrusted.rs, the two snapshot tests in
crates/trellis-trail/tests/trail_snapshots.rs,
`a_trail_records_duration_` in crates/trellis-loop/src/agent.rs and
`log_prints_ended_at_` in crates/trellis-cli/tests/cli.rs.

**Amended 2026-10-27 (#811): line counts on an edit.** A `factual.actions[]`
entry gains `lines_added` and `lines_removed`, the names and meaning the rpc
`turn` event has carried since #802 (ADR 0048 §6): how many lines the one
hunk an applied edit wrote added and removed, taken from that edit's
`EditOutcome.change` (`edit_diff`, crates/trellis-loop/src/gate.rs), the
hunk the turn event and the approve-mode question also use, so the trail,
the event and the question cannot disagree. Only an `edit` action that
applied carries them, which makes the pair narrower than `wrote`: a
`write_file` action has `wrote` true and no counts, since a created file's
line count is its size, not a change. They are absent on a refused edit,
`write_file`, `bash`, a custom tool, an `approval`, a partition, compaction,
gate or service record, and every action in a process agent's trail, the
same set `edit_applied_only` in crates/trellis-cli/tests/client_contract.json
names for the event, so the two records keep one contract. Each is an
`Option<u64>` under `#[serde(default, skip_serializing_if =
"Option::is_none")]`, as `duration_ms` is: `u64` because `EditDiff` holds
`usize` and the conversion is lossless on every target Rust supports, where
`u32` would need the saturation `changed_count` uses, a figure that can
lie. Absent means not recorded; a recorded 0 is written, because a pure
deletion's `lines_added: 0` is a measurement, so the skip is `is_none` and
never `is_zero`. The writer sets both or neither; a reader takes each as
recorded and infers neither from the other, and no net figure or ratio is
derived from the pair. Inside `factual`, the counts are under the digest: an
edited count reads `Altered`. A trail written before this has neither field
and reads back with both absent, never 0, keeping its bytes and its digest,
because `UntrustedTrail::integrity` hashes the raw `factual` JSON, not a
parsed struct. A reader older than this reads a newer trail: `Action` has no
`deny_unknown_fields`, so the two fields are dropped on parse and the digest,
taken over the raw JSON, still reads `Intact`. `SCHEMA` stays 1 by the #622
rule. The convention is #622's, and #868's run controls take the same one:
additive, read with a serde default, omitted when absent or empty, under
`factual` and so under the digest, `SCHEMA` 1, and read back through
`UntrustedTrail` as data. None of CLAUDE.md's ten rules changes. A line
count is not a correctness claim and `Action` still has no pass variant
(rules 1 and 2); read back, the counts are figures from a committed file
that anyone with commit access could have written, shown as the trail's
claim and never used as a budget or correctness input (rule 10). Rejected:
recording `start_line` and `diff` as the event does, because the diff is
repository text and model output of up to `DIFF_CAP` (`exec::OUTPUT_CAP`)
bytes per edit in a committed file, which grows the trail and widens the S10
surface while git already holds the change, and a `start_line` means nothing
without the file it was counted in; a nested `lines: {added, removed}`,
which would give the trail and the event two vocabularies for one fact;
counting `write_file`, for the reason above and because it would break
parity with #802; and a schema bump, which #622 already ruled out for an
additive field. Failure modes: the counts describe the hunk `apply_edit`
wrote and nothing later, so a `bash` call that then rewrote the same file is
not reflected in them; and a hand-edited trail with one field and not the
other reads as it was written (and as `Altered`, when its digest was
recorded). `Turn` (crates/trellis-loop/src/agent.rs) does not carry the
`EditDiff` today, so the implementation carries it to `action_from`
(crates/trellis-cli/src/run_cmd.rs), and a snapshot in
crates/trellis-trail/tests/trail_snapshots.rs that holds an applied edit
changes with it. Reversibility: cheap. Stopping the writer costs one field
assignment, and readers already accept absence; trails already written keep
their counts, because stripping them would alter their digests. What it
would cost is the per-edit figures the summary level reads. What reverses
it: a trail size measured against the context budget (Q-7) in which these
fields are the margin, or a reader found treating a count as evidence the
edit was right. Held, once #811's code lands, by
`trail_edit_actions_carry_line_counts` and
`an_old_trail_reads_back_with_line_counts_absent` in crates/trellis-trail.

**Amended 2026-10-27 (#868): the person's run controls.** ADR 0058 §6
decided that a pause, a raised limit and a withdrawn message are the
person's decisions and belong in the record a later session reads; this
says how. `factual` gains `controls`, one entry per control that took
effect, in the order the loop recorded them (not promised sorted by turn).
It takes #811's convention whole: additive, read with a serde default,
omitted when empty (`skip_serializing_if = "Vec::is_empty"`), under
`factual` and so under the digest, `SCHEMA` 1, read back through
`UntrustedTrail` as data. None of CLAUDE.md's ten rules changes. A
withdrawal records the message's kind and length and never its text: the
owner decided so on 2026-10-27, in the #868 session, because a committed
trail is a file in the repository that a later run's model can read through
`read_file` or `bash` (`.trellis/runs/<id>.json`), and keeping the text
would show it what the person took back. This narrows ADR 0058 §6, which
named the text cut as a steer's is.

*Shape.* An internally tagged enum, tag `control`, one variant per control.
The tag is not `kind` because a withdrawal keeps a steering entry's own
`kind` and `len` (ADR 0034), so one fact has one vocabulary, as
#811 argued for the line counts. The outer `factual.control` (the dial,
ADR 0018) is a different field at a different level. Every field is
required within its variant:

- `{"control": "pause", "turn", "paused_ms"}`. `turn` is the model call
  the hold came before, numbered as the `paused` event numbers it
  (`iterations + 1` in `run_seeded`); `paused_ms` is the `resumed` event's
  `paused_ms`, `millis(held)` from `Hold::honour`. The name is the event's
  (#806), not `held_ms`, for #811's reason.
- `{"control": "raise", "turn", "ceiling", "old", "new"}`, one entry per
  ceiling whose value changed. `ceiling` is one of `max_iterations`,
  `max_wall_clock_ms` and `max_completion_tokens`, the `run budget` names
  (#799, #807), entries in that order. `old` and `new` are `u64` in that
  event's units and through its conversion (`millis` for the wall clock).
  `turn` is the iteration whose budget check first enforced the new value.
- `{"control": "withdraw", "turn", "kind", "len"}`. `kind` is `steer` or
  `follow_up`; `len` is the message's true length in characters, the unit
  `Steer::len` counts in. There is no `text` field, and the loop's own
  record (`RunControl::Withdrawn`) holds the kind and length only, so the
  text is dropped where the loop drops the message and never travels
  towards `record_trail`. `turn`
  is the turn the message's steering entry would have carried had it not
  been withdrawn (ADR 0034, `steering_entries`): a steer dropped at the
  drain before request N + 1 takes N + 1, the delivered rule; a
  follow-up, at whichever drain it was dropped or after it was taken, and
  a steer dropped at the final drain take `turns.len()` at the run's end,
  the undelivered rule. Which drain caught it does not change the turn.

On read-back the enum ends with a `#[serde(other)]` unit variant (serde
1.0.229, the locked version, allows it on the last unit variant of an
internally tagged enum; checked in serde_derive's `check.rs`), so a trail
from a newer build with a control this build does not know still parses
and is not refused whole; and `ceiling` reads as a `String`, as
`SuspectedRecallGap::check` does, so an unknown name reads back too.

*What is recorded, and what is not.*

- A pause is recorded only when `Hold::honour` returned `Some`, that is,
  when the loop held. That includes a hold ended by `Hold::close`, since it
  did hold. A pause a `resume` took back before the loop reached it
  (`PAUSE_WITHDRAWN`), or one the run ended before reaching, writes
  nothing: nothing happened in the run, and the client was told so.
- A raise is recorded where `LiveBudget::take_raised` returns it at the
  budget check, not where `LiveBudget::raise` accepts it. `old` is the
  ceiling in force before that check, `new` the one after. Two raises
  accepted between checks give one entry per ceiling, and the value in
  between, never enforced, is not recorded. A raise accepted during the
  run's last turn is never taken, so it is never recorded (`LiveBudget`'s
  doc says the same). The trail records what was enforced, not what was
  asked.
- A withdrawal is recorded by the loop, from `SteeringFates`. A message
  the drain takes off the channel already withdrawn is recorded at that
  drain, where today it is dropped without a record. That alone misses a
  case: `SteeringFates::withdraw` accepts a follow-up an earlier drain
  already moved into `SteeringLog::undelivered`, the port takes it out of
  its own record, and the loop's copy stays, so since #808
  `steering_entries` writes that follow-up into `steering` as waiting for
  a later run, though it never runs. ADR 0058 rejected recording a control
  as a steering entry because a steering entry says the text reached the
  model or waited for a later run, and a withdrawn message did neither. So
  `SteeringLog` keeps each taken message's channel number, and at the
  loop's final drain every undelivered entry `SteeringFates` marks
  withdrawn moves from the steering record to `controls`. This changes
  what `steering` records in that one case. Trails already committed keep
  their bytes and digests.
- An interrupted run (abort, quit, Ctrl-C, including one aborted while
  paused) records no controls. `handle_interrupt` writes its trail from an
  empty `Transcript`, because the loop's record went with the dropped
  future, so it also records no turns and no steering; the contract's line
  for `trail.factual.steering` already says so, and `controls` is in the
  same position.
- A partitioned run records none. `run_partitioned.rs` seeds each
  partition with `hold`, `budget` and `fates` all `None`, and rpc starts no
  partitioned run, so nothing can pause, raise or withdraw in one and the
  supervisor's trail has no list. Rule 9 binds whoever builds the first
  command that reaches one: a raise is to the whole run's ceiling and is
  recorded once, against the whole, in the run's one trail, never once per
  partition and never as a share.
- A process agent, `trellis session` and `trellis tui` record none. They
  seed no `Hold`, `LiveBudget` or `SteeringFates`, so only an rpc run can
  carry the list.

*Where the data comes from.* `Transcript` (crates/trellis-loop/src/agent.rs)
gains a list of trellis-loop's own control type, filled in `run_seeded` at
three points: after `Hold::honour` returns `Some(held)`, after
`take_raised` returns a budget (comparing it with the `budget` it replaces),
and in `SteeringLog::drain`, plus the move from `undelivered` at the final
drain. trellis-loop does not depend on trellis-trail and does not start to;
`record_trail` (crates/trellis-cli/src/run_cmd.rs) converts the list as
`steering_entries` converts the steering log.

*Read-back.* Every string here was written by whoever last edited the file,
so it is data (rule 10). `controls` never reaches
`UntrustedTrail::context_lines`, so a resumed run's `PriorContext` is byte
for byte what it is for the same trail without the list. Keeping the list
out of the prompt does not keep it from the model: a later run can read the
trail file itself through `read_file` or `bash`, which is why a withdrawal
records no text at all rather than a text kept out of `context_lines`.
`trellis log --json` carries the trail as written, so the list is there
for a client. Nothing prints it in `trellis log`'s text output yet. A
pause or a raise is not a correctness claim and the enum has no pass variant (rules
1, 2). A recorded ceiling is a figure from a committed file and is never
read back as a budget.

*Digest.* A run with no control writes the bytes it wrote before this, so
its digest is unchanged, because the list is skipped when empty and
`UntrustedTrail::integrity` hashes the raw `factual` JSON. A trail written
before this reads back with an empty list. A reader older than this drops
the field on parse (`UntrustedFactual` has no `deny_unknown_fields`) and
the digest, over the raw JSON, still reads `Intact`. An edited entry reads
`Altered`.

*Rejected.* Recording a control in `steering`: ADR 0058 rejected it, for
the reason above. A struct with a `kind` and every other field optional:
it can represent a pause with a ceiling, and every field being optional
loses "absent means not recorded". One raise entry holding all three
ceilings: a nested object would need its own absent-versus-unchanged rule,
and one entry per ceiling keeps each value next to its name, so a reader
never infers which ceiling moved. Recording a raise when `set_budget`
accepts it: that would record ceilings the loop never enforced (the
last-turn raise, the value between two raises). Keeping the record in the
shared `Hold`, `LiveBudget` and `SteeringFates` so it survives an
interrupt: an aborted hold has no end to measure; a withdrawn message's
kind and length are in rpc's `SteeringPort`, not in `SteeringFates`; the
interrupt trail has no turns for the entries' turns to refer to; and
three shared objects each keeping a log would make three writers where
the loop's record is one.
Keeping the withdrawn text cut as a steer's is, as ADR 0058 §6 first
named, and keeping it out of `context_lines`: the trail is still a file a
later run's model can read, so the owner decided against keeping it at all
(2026-10-27). A schema bump: #622 ruled it out for an additive
field.

*Failure modes.* A reader of the trail sees that a person withdrew a
message, its kind and its length, and not what it said; the client that
sent it is the only place the text was. A
withdrawal after the loop's final drain (in the window before
`record_trail`, or from the `Chain`'s queue after `run_end`) is not in
this run's trail, and `steering` then names a follow-up that never ran. A
client's `set_budget` answer can name a ceiling the trail never shows,
when the raise was accepted in the last turn. A run paused and then
aborted leaves no record that it was paused.

*Reversibility.* Cheap. Stopping the writer costs the conversion in
`record_trail`, and readers already accept the list's absence. Trails
already written keep their entries, because stripping them would alter
their digests. Reverting the steering change brings back the false
`follow_up` entry described above. What it would cost: the only record
that a run's ceiling was raised by a person and not set at the start.
What reverses it: the owner deciding that a reader needs the withdrawn
text after all, which would first need the trail kept from a later run's
`read_file` and `bash`, since that route is why it is not kept; a trail
size measured against the context budget (Q-7) in which `controls` is the
margin; or a reader found treating a recorded raise as a budget input.
Held, once #868's code lands, by
`trail_records_each_run_control_with_its_turn` and
`a_trail_without_controls_keeps_its_digest` in crates/trellis-trail.

## Alternatives considered

**One combined section with a `speculative: true` field per entry.** Rejected:
a flag on a field is easy to drop when serializing, and a reader scanning the
document sees facts and speculation in the same visual block. Structural
separation survives careless reading.

**Appending to a single log file.** Rejected: merge conflicts on every
concurrent run, and no natural unit to discard when a run rolls back.

**Not committing the trail.** Rejected: it is the wedge. A trail in a local
cache does not survive a clone, and "re-explain nothing" fails for anyone but
the original author.

**Deserializing verdicts as verdicts.** Rejected — this was caught during
implementation and is the S10 hole in concrete form. It would have made
`serde_json::from_str::<Verdict>` a public forgery constructor.

## Why

The split matters because future agents read the trail as context. Without it,
a model's speculation about tech debt becomes, three sessions later, an
established fact that nothing ever verified.

## What changes our mind

If trails grow large enough that reading them dominates context budget,
introduce a summarization step — but the summary is derived state alongside the
full trail, never a replacement for it. → `OPEN_QUESTIONS.md` Q-7.

Hashing factual entries so tampering is detectable rather than merely contained
is a likely addition. → Q-5.
