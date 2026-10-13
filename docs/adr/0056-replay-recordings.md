# ADR 0056 — Every run writes a replay recording that `scripted:` plays back

**Status:** Accepted (owner-directed). Where recordings live is the owner's
decision of 2026-10-12 on #620; the rest is the ticket's specification.
G7 applies to the pull request (`surface:public`).
**Date:** 2026-10-12
**Decided by:** the owner, 2026-10-12, on #620: "replay recordings live in
`.trellis/replays/`, with its own self-ignoring `.gitignore` and a
`trust.rs` skip for `replays/*.json` (an ADR 0023 §4 amendment, written in
the same PR)".
**Holds:** rules 1, 2 and 10 (ADR 0004, ADR 0007); ADR 0009 (a clean
worktree before a run).
**Amends:** ADR 0023 §4 (what the `.trellis/` digest skips; the dated
amendment is in that ADR).

## Context

The committed trail keeps `tool`, `target` and a one-line `outcome` per
action on purpose (ADR 0007; `crates/trellis-trail/src/lib.rs`, "a rendering
for a reader, not the observation the model saw"). An `edit`'s `old` and
`new` and a `bash` script are recorded nowhere a program can read, and floor
logs hold steps as Rust `Debug` text. `ScriptedModel` already plays a
`{"steps":[...]}` file through the parser the runtime adapter uses, but no
run wrote one. `docs/BENCHMARK_AND_EVAL.md` §5.9 shows the cost: runs whose
outcome a client bug set had to be run again against a live model, with its
tokens and its variance, to separate the bug from the model.

#620 parked on where the file goes. `.trellis/.gitignore` is written once
from `trellis_index::cache::IGNORE` and never rewritten, so a line added to
it never reaches a repository that already has one: its first recording
would be untracked and the next run would refuse a dirty worktree. And the
trust digest covers everything under `.trellis/` except what trellis itself
writes, so a recording would read as trusted code changing.

## Decision

1. **Where.** Every `trellis run` that reaches its snapshot writes
   `.trellis/replays/<run_id>.json`, `<run_id>` being the trail's id. Written
   after the run has disposed of the tree (left for review, rolled back, or
   interrupted), because a rollback with writer guards removes ignored files
   the run created. Single-agent and multi-agent runs, and runs started by
   `session`, `rpc`, `tui` and `work`, all go through `cmd_run_with`, which
   writes it. A run refused before its snapshot ran nothing and writes none.
   The run prints `replay:   .trellis/replays/<id>.json`.
2. **Ignored by its own file.** The directory gets
   `.trellis/replays/.gitignore` holding `*`, written when absent and never
   overwritten (`trellis_loop::REPLAY_IGNORE`), so it ignores itself and every
   recording, and a repository whose `.trellis/.gitignore` predates this is
   covered. This repository's root `.gitignore` names the directory too.
3. **Refused while tracked.** `trellis run` refuses to start, before the
   snapshot, while `git ls-files .trellis/replays` lists anything, and names
   the files and `git rm -r --cached .trellis/replays`. A tracked recording
   came with the repository and is not a record of a run on this machine;
   rollback would also reset it.
4. **Format.** The script `ScriptedModel` reads: each step the driver handed
   the loop, as the flat reply `ToolSet::parse_value` accepts
   (`registry::step_json`: `call_json` for a call, `{"tool":"done",...}` for
   `done`), in order, including steps a gate or a person refused. A
   single-agent run writes `steps`. A multi-agent run writes one `targets`
   section per agent, keyed by the first of its targets, which is the
   section `ScriptedModel::from_json_for` gives that agent. `recording`,
   `run_id`, `base_commit` and `task` are beside them for a reader; the
   loader reads none of them.
5. **Replay.** `git checkout <base_commit>` (or any clean tree at it), then
   `trellis run --model scripted:.trellis/replays/<id>.json "<task>"`, with
   the same `--target`s for a multi-agent recording. Edits, checks and
   rollback are real; only the model's decisions are read from the file.
6. **Rules hold.**
   - Rules 1 and 2: a recording carries steps, never a verdict. `Step` has
     none, and every check runs again.
   - Rule 10: the file is read back as data, through `ScriptedModel`, parsed
     against the replaying run's own tool set. An unknown tool, a tool the
     run does not offer, or a malformed step is refused at load, before the
     snapshot. Nothing in it reaches a prompt (a scripted run sends none),
     and `task` is not read: the replaying person types the task.
7. **Trust (ADR 0023 §4, amended 2026-10-12).** The digest skips regular
   files named `*.json` directly under `.trellis/replays/`, and
   `.trellis/replays/.gitignore` while its bytes are `REPLAY_IGNORE`. A
   `.gitignore` a person edited there, and any other file there, is hashed.

## Consequences

- A floor run, or any run, can be re-run offline over the same model
  decisions, for no tokens and with no sampling variance. A replay is a
  re-test of the harness against recorded decisions, never a measurement of
  a model: S7 still holds that no result obtained through `scripted:` is a
  model floor.
- Recordings accumulate under `.trellis/replays/`; nothing prunes them.
  They hold every edit's text and every `bash` command a model gave, in the
  clear, on the user's disk, as the worktree already does.
- A run that took no step writes a recording with no steps, which `scripted:`
  refuses to load (`script has no steps`): there is nothing to replay.
- An interrupted multi-agent run records only the agents that finished;
  an agent stopped mid-run leaves no section.
- A recording made with tools a plain `trellis run` does not offer (a queued
  run's `ask_human`, a custom tool the repository no longer has) is refused
  at load, naming the step.

## What is not built

- **Fork at step N onto a live model**: a composite `Model` that plays N
  recorded steps and then hands over, and a spec grammar for it. A ticket of
  its own.
- **Checking the base.** A replay on a different commit is not refused; the
  recording's `base_commit` is for the person.
- **Pruning** old recordings.

## Alternatives rejected

- **`.trellis/work/`** (the parked recommendation): already ignored and
  outside the digest, but `work/` holds partition worktrees that trellis
  creates and removes, and the owner chose a directory of its own.
- **A line in `.trellis/.gitignore`**: never reaches an existing repository
  (Context).
- **Recording in the trail**: the trail is committed and read back as
  context; carrying every edit's text there would grow every repository's
  history and every resume prompt, which ADR 0007 keeps out on purpose.
- **A hook in each adapter's parse path** for the raw reply `Value`: two
  hooks to keep equal, and the scripted driver has none. Recording the
  `Step` and serializing it with `step_json`, held by a round-trip test over
  every tool, covers all three drivers in one place.

## Reversibility

Remove the write in `cmd_run_with`, the refusal in `run_body`, and the two
skips in `trust.rs`. Existing recordings stay ignored by their own
`.gitignore` until deleted. Nothing else reads them.

Tests: `replay_recording_*` in `crates/trellis-cli/tests/replay.rs` (a
recording per run, loadable, ignored, and the refusal while tracked; a
replay reproducing a converged run's `proposed` list and check sequence; a
multi-agent run's sections) and in `crates/trellis-loop/src/registry.rs`
(`step_json` reads back as the same step for every tool);
`a_recording_plays_back_the_steps_it_holds` in `scripted.rs`;
`only_what_trellis_writes_is_left_out_of_the_dot_trellis_digest` in
`trust.rs`.
