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
value.

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
