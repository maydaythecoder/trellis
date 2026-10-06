# ADR 0026 — Local orchestration: task model, agent profiles, routines, budgets

**Status:** Accepted. Amended in implementation: §1, §4, §5, §7, §8 and
§10. §1 amended again for `ask_human` (#140). §4 and §11 amended again for `pass_env` when ADR 0024 §3's key gap
was closed. §7 amended again for IANA zone names (#47). §1 and §9 amended
for `@profile` mention wakes (#57). §1 and §9 amended again: `queue note` refuses inside a run (#189). §5 amended again for prompt tokens (#108). §1 amended again: up to 8 questions per `ask_human` call, not built (#331). §12 added for
services (#60), with a line in §4. Each amendment is marked in place. §4's `tools` list and skill deny
list were integrated with ADR 0023 and ADR 0019 afterwards; see
"Integrated" at the end.
**Date:** 2026-09-27
**Extends:** ADR 0020 (the local work queue). Every rule there still holds:
the queue lives in the git directory, files are published by hard link and
never renamed, execution is serial, and `work` stops at the first converged
change.
**Reference design:** Paperclip (`paperclipai/paperclip`, MIT), the rows of
ADR 0021 dispositioned W-E. Semantics were read from Paperclip's own docs
(`docs/api/routines.md`, `doc/execution-semantics.md`) where the inventory
was not exact. No Paperclip code or server is used.

## Context

ADR 0020 built the queue as a list of tasks that run oldest first. Paperclip
organizes work with more structure than that: priorities, dependencies,
sub-tasks, named agents with their own instructions and budgets, schedules,
approvals and an audit log. ADR 0021 took each of those as W-E.

Two things found while reading the code decide the shape of what follows.

1. **The trail did not record tokens.** The brief for this wave says the
   budget ledger reads spend from the trail. It could not: `Factual` records
   prompt size in bytes (`context`) and nothing else about spend. So this
   ADR adds a token field to the trail (§8), and the ledger reads that.
2. **Every mutation collides with the no-overwrite rule.** `publish` refuses
   an existing name (ADR 0020 §3), and `set`, `note`, `cancel`, `approve`, a
   routine's last-fired time and a profile's pause all rewrite a file. One
   protocol has to cover all of them (§2).

## Decision

### 1. The task model

A task file gains these fields. Every one has a serde default, so a task file
written under ADR 0020 still loads unchanged.

| Field | Meaning |
|---|---|
| `status` | `backlog`, `todo`, `in_progress`, `in_review`, `blocked`, `done`, `cancelled`, `failed` |
| `blocked` | The typed reason when `status` is `blocked`: `blockers`, `children`, or `manual` with a detail naming what unblocks it |
| `priority` | `critical`, `high`, `medium` (default), `low` |
| `labels` | Free strings |
| `parent` | A task id. Structure only, as in Paperclip: it is not a dependency |
| `assignee` | An agent profile name (§4) |
| `blockers` | Task ids this task waits for |
| `needs_approval`, `approval` | §6 |
| `notes` | `[{at, actor, text, id?, mentions_read?}]`, appended by `queue note` and by the queue. `id` and `mentions_read` are #57's, below; both are omitted when absent |
| `routine` | The routine that created the task, if any (§7) |
| `args` | Arguments for the task's `prompt` template, the one unsplit string `run --args` takes (§3). Omitted from the file when absent |

The goal is the vault stem already stored as `focus` (ADR 0020 §6).
`queue add --goal` is an alias of `--focus`.

**The directory is still the state; `status` refines it.** `State` gains
`cancelled/`. Which statuses may sit in which directory is fixed:

| Directory | Statuses | Default when `status` is absent |
|---|---|---|
| `pending/` | `backlog`, `todo`, `blocked` | `todo` |
| `claimed/` | `in_progress` | `in_progress` |
| `done/` | `in_review`, `done` | `done` |
| `failed/` | `failed` | `failed` |
| `cancelled/` | `cancelled` | `cancelled` |

A task in `done/` written before this ADR reads as `done`, not `in_review`.
Those tasks converged under ADR 0020 and have almost certainly been reviewed
since, and reading them as `in_review` would leave every task that names one
as a blocker waiting forever.

**A converged run now ends `in_review`, not `done`.** Its change is in the
worktree waiting for a person, which is what `in_review` means in Paperclip.
`trellis queue set <id> --status done` records that the person accepted it.

**What is claimable.** A pending task is claimable when all of these hold:
- its status is `todo`
- every blocker is `done`
- it does not need an approval it has not received
- its assignee, if any, passes the gate in §4

The heartbeat claims the highest priority first and, within a priority, the
oldest. It reads every pending file to sort them. That costs one read per
pending task per tick, and the queue is a person's backlog, not a message
bus. A file that is not valid task JSON still goes to `failed/` when a claim
reaches it, as in ADR 0020.

**Blockers and parents wake tasks.** These rules are taken from Paperclip
`execution-semantics.md` §6:
- A blocker is resolved only when it is `done`. A `cancelled` blocker does
  not satisfy the dependency. The edge stays unresolved until someone
  removes it (`queue set <id> --unblock <blocker>`).
- A task that is `blocked` for reason `blockers` becomes `todo` once every
  blocker is `done`.
- A task that is `blocked` for reason `children` becomes `todo` once every
  direct child is terminal. Terminal is `done`, `cancelled` or `failed`.
  Paperclip has no `failed` status. Trellis adds it, and counts it as
  terminal because a failed child will not move again without a person.
  `in_review` is not terminal.
- A parent waits on its children only if it was set to wait with
  `--status blocked --reason children`. Adding a child does not change the
  parent, because Paperclip says structure is not dependency.

These checks run in `Queue::reconcile`. `work` calls it before every claim,
and every CLI command that ends a task calls it too. Each wake is written to
the activity log (§9).

Adding blockers that are not yet `done` sets the task to `blocked` for reason
`blockers`. A task set to `todo` while it still has unresolved blockers stays
unclaimable, and `queue list` says why.

*Amended in implementation: a heartbeat that finds tasks but none it may
claim now says so.* `work` without `--every` used to return "queue empty"
whenever nothing was claimed. With gated tasks, that line is false. The loop
prints each waiting task and its reason once, and it stops with
`Stop::Waiting(n)`: "nothing claimable; n pending task(s) waiting". This
exits 0, like an empty queue, because nothing is wrong: the tasks are waiting
as designed. "Queue empty" is printed only when nothing is pending.

*Amended for #140 (owner decision, 2026-09-28): a run may end by asking a
person a question.* Paperclip tasks can put a question or a one-to-eight
option decision to a person and wait (ADR 0021, Paperclip 30 and 34). Here:

- **The tool.** `ask_human {question, options}` is a model tool with at
  most 8 options. The question is 1 to 2,000 characters and each option 1 to
  200; a call outside that is refused with the reason and the model may ask
  again. It is offered only to a run of a queued task (`work`), because
  only there does a question have a task to wait on; `trellis run`,
  `session`, `tui` and `rpc` do not offer it. `[tools] disable` can turn it
  off, `[tools] enable` naming it is refused, and a profile's `tools` list
  keeps it only by naming it (it is added before the profile narrows the
  set). It counts as one turn, and `[control] approve` may list it: in
  approve mode the person sees the question quoted and a "no" is a refusal
  like any other, after which the run goes on.
- **It never converges.** Allowed through the gates, the question ends the
  run as `NotConverged::AskedHuman`, after its turn is recorded, whatever
  passed before it: only an executed check converges a run (ADR 0004). The
  run is rolled back like every other unconverged one. If the rollback
  fails, the run is reported as not restored and the question is left to
  the trail, since the tree is the more urgent fact.
- **The task waits.** `work` puts the task back in `pending/` with status
  `blocked` and a new reason, `question`, which carries the question and
  its options: `{"reason": "question", "question": "…", "options": […]}`.
  The run counts as an attempt; the task gets no `outcome`, because it has
  not ended. `reconcile` never wakes it, since only a person can answer.
  The heartbeat then goes on to the next task, as after any restored run.
- **The question is a note, by a new actor.** It is recorded as a note
  whose `actor` is `model`, so model output is never read as a person's
  note, and the activity log gets a `task.question` event with the run id.
  The note text, the event detail, the heartbeat's output and `queue
  list`'s waiting line all carry the question Debug-quoted, so model output
  cannot start a line of its own or pass for a sentence of ours.
- **The answer uses what exists.** `trellis queue note <id> "<answer>"`,
  then `trellis queue set <id> --status todo`. There is no new command,
  flag or `--reason` value. (The ticket said `--status pending`; `pending`
  is a directory, and `todo` is the status that lives in it.)
- **The next run gets the answer.** When the claimed task has a `model`
  note, `work` appends one fenced block after the task text: the last such
  note, labelled as the previous run's model output and still quoted, then
  every note by a person after it, each attributed and indented so that no
  line of it can close the fence. Notes by the heartbeat or a routine are
  left out. A task that never asked is given the text it always was. The
  answer is input to the run, like the task; it is never a verdict.

Both additions are additive on disk: `blocked.reason` gains a value and
`notes[].actor` gains one, and a task file written before them parses
unchanged. The reverse does not hold: a build from before this amendment
cannot parse a task that uses them, so its `queue list` shows the file as
unreadable and its `work` moves it to `failed/`, as it does any file that is
not valid task JSON.

*Amended 2026-10-05 (#331), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**One `ask_human` call may carry up to 8 questions.** A model with three
questions now asks them over three runs and three rollbacks; Paperclip's
`ask_user_questions` takes 1 to 64 questions in one interaction. Not built:
this records the rule a later ticket builds to.

- **The tool.** `ask_human` takes 1 to 8 entries of `{question, options}`.
  Each question is 1 to 2,000 characters with at most 8 options of 1 to 200
  characters, as for one question above. A call outside those bounds is
  refused with the reason, after which the model may ask again.
- **One note, one reason.** The questions are stored as one `model` note
  and one `blocked` reason, `question`, holding the list. The note, the
  `task.question` event, the heartbeat's output and `queue list`'s waiting
  line Debug-quote each question, as above for one. A task file holding
  today's single `question` and `options` still loads, as a list of one.
- **One answer.** One person's `queue note`, then `queue set --status
  todo`, answers every question in the list. The next run is given the
  questions quoted as model output, followed by every person's note after
  them, as input and never a verdict (ADR 0004).
- **Older binaries.** A binary from before the change cannot parse a task
  blocked on a list of questions, so its `queue list` shows the file as
  unreadable and its `work` moves it to `failed/`, as recorded above for the
  `question` reason itself.
- **Not taken.** A multi-select form like Paperclip's
  `request_checkbox_confirmation` (up to 200 options to tick) is not built:
  no coding task here needs it, and it would be a second interaction kind
  to quote safely.

Reversed if the build finds that a list of questions cannot be quoted and
fenced as safely as one question is today: then one question per call
stays, and this amendment says so.

*Amended 2026-09-29 (#57), by the owner's decision of 2026-09-28:* **a
person's `@profile` mention wakes that profile's task.** A note added with
`trellis queue note` that contains `@<name>`, where `<name>` is an existing
profile that is not paused, puts the task back for that profile: `work`,
at the top of each tick and before it claims, sets the task's `assignee` to
`<name>`, its status to `todo` with no `blocked` reason and no `outcome`,
and moves it to `pending/`. The task is then claimable exactly as any `todo`
task is: unresolved blockers, a missing approval, and the profile's gate
(§4) still hold it. Each wake is logged as `mention_wake` (§9) with the note
id, and the heartbeat adds its own note saying so.

- **A token** is a whitespace-separated word that starts with `@`, after
  any opening bracket or quote, whose remainder less trailing punctuation
  is a valid profile name. `@reviewer,` is a mention; `ops@example.com` is
  not.
- **Unknown and paused names** are logged as `mention_ignored`, with why,
  and nothing moves. A pause in the profile file or the overlay both count;
  a mention is not a way round one. A note that names several runnable
  profiles wakes the task for the first and logs the rest as ignored.
- **Once per note.** Each note carries `mentions_read`, set in the same
  edit that wakes the task, and set too when every name in it was ignored,
  so nothing is logged or woken twice whether one heartbeat or several read
  it. A task in `claimed/` is skipped with its notes unread and read on a
  later tick.
- **Only a person's note counts.** `queue note` is the only writer of a
  note `id` (`n` and the note's 1-based position in the task's list, so
  `n1`, `n3` when the queue wrote the second), and a note wakes only if its
  actor is `human` *and* it has one. The actor alone is not enough: the
  queue also writes `human` notes on a person's behalf (a cancel reason, an
  approval, a manual routine fire), and those are records, not messages to
  an agent. Notes by the heartbeat, a routine, or a model never wake
  anything, so one agent's output cannot start another agent (the intent of
  ADR 0005). A note written before #57 has no id and wakes nothing.
- **A run cannot write a person's note.** *Amended 2026-09-30 (#189),
  replacing the "known limit" this bullet stated:* a run with `bash`, a
  hook, a custom tool or a process agent enabled can spawn `trellis queue
  note` itself, and without a check the queue would record that as a
  person's note, so one agent could wake another. `queue note` now refuses
  when `TRELLIS_RUN_ID` (set for `bash`, custom tools and hooks) or
  `TRELLIS_TASK_ID` (set for a process agent, which gets no run id) is
  present in its environment, an empty value included. It writes nothing
  to the task, exits non-zero with a message saying a run cannot write a
  person's note, and logs `task.note_refused` (§9).
  **Its limit, stated:** this stops the accidental path, a child that
  inherits the run's environment and calls `trellis queue note`. A child
  that removes those variables on purpose, or starts a process that does
  not inherit them, is not caught by it; stopping that writer is the
  sandbox's job (ADR 0032, "Limits" and "What is not built"), not another
  environment check.

*Amended 2026-10-06 (#341; Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation; PC-114):*
**A person may block a task until a set time.** Paperclip lets an issue's
assignee or the board set a one-shot re-check (ADR 0021, PC-114). Here the
`blocked` reasons are `blockers`, `children`, `manual` and `question`,
`reconcile` wakes only the first two, and nothing wakes a task at a time;
routines and `work --every` (§7) create new tasks rather than re-run a
waiting one. ~~Not built: this amendment is the ADR for the flag #342 builds,
and it builds nothing itself.~~ Built for #342.

- **The reason and the flags.** A new `blocked` reason, `until`, holds an
  RFC 3339 instant in UTC. A person sets it with
  `trellis queue set <id> --status blocked --reason until --at <instant>`.
  No model tool sets the wait.
- **The wake.** The first `Queue::reconcile` after the instant sets the
  task to `todo`, once, whether that reconcile comes from a `work`
  heartbeat or from a `queue` command that calls it (`queue set`, `queue
  cancel`, `queue reject` and `queue import` do), so the task can be set to
  `todo` while no `work` is running. The wake is written to the activity
  log (§9) as the other wakes are. The task runs only when `work` claims
  it, and is then claimable exactly as any `todo` task is.
- **Refusals.** `--reason until` without `--at`, `--at` without `--reason
  until`, and an `--at` that is not RFC 3339 are each refused with the task
  file unchanged.
- **Listing.** `queue list` prints `blocked until <instant>` on the task's
  waiting line.
- **On disk.** `blocked.reason` gains the value `until`, so a task file
  written before the change parses unchanged. A binary from before the
  change cannot parse a task blocked `until` (`Blocked` is tagged on
  `reason` with no fallback variant), so its `queue list` shows the file as
  unreadable and its `work` moves it to `failed/`, as recorded above for
  the `question` reason.

Reversed if routines and `work --every` are found to cover every wait
people set in practice: then PC-114 stays excluded and #342 is closed
unbuilt.

*Amended 2026-10-06 (#345; Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation; PC-138):*
**A queued task may name its own model.** Paperclip's task composer picks
the model per message and keeps it as a per-issue override (ADR 0021,
PC-138). Here the model is chosen per run (`run --model`, the session's
`/model`), per profile (`model`, §4) or for a whole heartbeat (`work
--model`, else `[model] spec`), and `Task` has no model field. This
amendment is the ADR for `queue add --model` and built nothing itself.
*Built 2026-10-06 (#346):* `Task::model` in `trellis-queue`, the flag and
the precedence in `crates/trellis-cli/src/work.rs`, held by
`queue_add_model_refuses_a_spec_that_does_not_parse` and
`a_task_that_names_a_model_runs_on_it_over_its_profile_and_work_model`
(`tests/orchestration.rs`) and
`a_task_that_names_a_model_gets_no_key_for_an_endpoint_the_user_did_not_name`
(`tests/hosted_keys.rs`).

- **The field and the flag.** A task gains an optional `model` field, set
  by `trellis queue add --model <spec>` and parsed as `run --model` parses
  it. `work` runs such a task on that model, over its assignee profile's
  model and over `work --model`, and the run's trail records the model in
  its provenance, as it records the model that drove any run.
- **The key rule.** A task file can arrive through `trellis queue import`,
  so the task's model is held to the rule ADR 0024 §3 applies to a
  profile's model: it counts as the repository's choice, and a hosted
  endpoint it names is sent a key only when the user's own config names
  that endpoint.
- **What it does not add.** A routine's `[task]` cannot set the field.
  Thinking stays in `[model] thinking`; there is no thinking level per task.
  A task has no generated title: its text is its only name.
- **A process agent's task.** *Amended 2026-10-06 (#458; the owner's
  choice (a)):* a process agent (§11) runs its own command and no model is
  chosen for it, so a task's model would be stored and never used. `queue
  add --model` with an `--assignee` that is a process agent is refused,
  naming the profile and why, and no task is written. A task that reaches
  one with a model later (`queue set --assignee`, `queue import`) still
  runs, and `work` prints `model: ... unused` naming the agent before the
  command starts. Held by `process_agent_task_model_*`
  (`tests/orchestration.rs`).

Reversed if the key rule cannot be held for task files that arrive through
`queue import`: then a profile per model, and #346 is closed unbuilt.

*Amended 2026-10-06 (#347), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**#57's mention wakes stay, and every run of a queued task is given the
notes a person added since its last run.** Today a run sees a person's notes
only when the task's last run ended on an `ask_human` question:
`Task::answers_since_question` (crates/trellis-queue/src/lib.rs) anchors on
the last `model` note and returns `None` without one, and `text()` in
crates/trellis-cli/src/work.rs then gives the run the task's text alone. So
`trellis queue note <id> "@reviewer check the error paths"` wakes the task
for `reviewer` and the woken run never reads the sentence, and a person who
notes a request on an `in_review` task and sets it back to `todo` gets a run
that does not see it. Paperclip keeps comments as context for the assignee
and, since #14577 ("Mentions in task comments are kept as context for the
assignee; only explicit assignment and review requests start work"), no
longer starts a run from a mention. Trellis keeps the wake and adds the
context. ~~Not built: this records the rule a later ticket (CLIP-03) builds
to; `notes_seen` does not exist in the code today.~~ Built for #351.

- **What a run is given.** After the task's text, one fenced block holding
  every note a person wrote with `trellis queue note` (actor `human`, with
  a note id) after a marker, each attributed and indented as above. When
  the last run asked, the block holds that question quoted as model output
  with every `human` note after it, as this section already says. Notes by
  the heartbeat, a routine or the model are left out, and the notes are
  input to the run, never a verdict (ADR 0004).
- **The marker.** A new task field, `notes_seen`: the number of notes the
  task held when its last run was claimed. It is set by `Claim::finish` and
  `Claim::ask` and by nothing else, so a run cut short and put back by
  `Claim::release` leaves it, and the next run is shown the same notes. A
  task file without the field (written before the change, or rewritten by
  an older binary, which drops it) is read as if no run had been shown any
  note.
- **#189's limit reaches the prompt.** A child that removes
  `TRELLIS_RUN_ID` and `TRELLIS_TASK_ID` on purpose can still write a note
  as a person (above). Before this change such a note could wake a task;
  after it, the note also reaches the next run's prompt.
- **Mentions.** A trellis mention counts only in a person's `queue note`,
  which refuses inside a run (#189), so the agent-to-agent dispatch
  Paperclip withdrew in #14577 does not arise here. ADR 0021 rows 25 and 41
  do not change.

Reversed if a run is found writing a `human` note with an id by any route
other than the deliberate one #189 names: then a person's note reaches a
run only after a question, until the sandbox closes that route.

### 2. Changing a file that already exists

Every rewrite goes through one of three paths. None of them renames a file.

- **A pending task is edited by claiming it.** `claim(id)` gives this process
  the task under the same atomic checkout `work` uses. The edit is applied,
  and the changed task is published back to `pending/` (or to its new
  directory) before the claim is removed. While the
  edit is in progress, `work` does not see the task. If `work` holds the
  task, the edit fails with "claimed by pid N". The window ADR 0020 §3
  already accepts also applies here. A claimer that read the old file a few
  microseconds before the edit could run the old content.
- **A claimed task is not edited.** The running process publishes its own
  copy when it finishes, so an edit would be silently lost. `queue set` and
  `queue note` on a claimed task refuse and say why.
- **A task in `done/`, `failed/` or `cancelled/`, and the queue's other
  state files, take an exclusive lock.** The lock is
  `<git-dir>/trellis/locks/<name>.lock`, published by hard link, and the hard
  link fails if a lock already exists. Profile overlays and routine state
  use the same path.

  *Amended after the third review of PR #1.* The holder first removed the
  old file and then published the new one, so a failure or a crash between
  the two lost the file: a finished task vanished from every directory. Now
  the holder publishes the new content at the fixed side name
  `.<name>.next`, removes the old file, links the side copy into place and
  removes the side copy. A complete copy exists at every instant. Readers
  that find the name missing read the side copy, `queue list` shows it, and
  the next holder of the lock finishes the swap. If the side copy exists
  while the old file is still there, the edit never took effect and the
  side copy is discarded. If linking the side copy into place fails, the
  caller gets an error, but the edit has already taken effect. The error
  says so and names the side copy. The lock is taken by every mutation of a finished
  task: `queue set`, `queue note`, `queue requeue`, and the removal done by
  `import --on-conflict replace`. Before this, only `edit` took the lock, so a requeue
  that ran during an edit moved the task to `pending/`, and the edit then
  wrote it back to `failed/`. That left one task in two states.
  `a_requeue_during_an_edit_of_a_failed_task_leaves_it_in_one_state` and
  `an_edit_of_a_finished_task_that_fails_midway_never_loses_it` hold both. Approval requests are never rewritten in
  place: they only move between `pending/`, `approved/` and `rejected/` by
  publish-then-remove, like a task, so a second decision on the same request
  fails at the publish. A crash while the lock is held leaves
  the lock behind, and the next writer gets an error that names the lock
  file. Removing it by hand is the recovery. This is the same kind of
  visible, stuck-but-never-lost state as the two-copies window in ADR 0020
  §3.

  *Amended after the fourth review of PR #1: every reader goes through one
  rule, and the settle has one owner.* A task left only as its side copy was
  listed, because the directory listing reports a side copy whose name is
  missing. But `queue export` copied the missing name and aborted, and
  `queue list` showed no age for the task. Every read of a record now goes
  through `files::read_settled`: its bytes, its age and an export's copy.
  That function tries the name, then the side copy, then the name again,
  and takes no lock. Evidence:
  `export_and_list_read_a_task_whose_only_copy_is_its_side_file`, which
  failed before the change, first on the missing age and then, with that
  assertion set aside, on export's NotFound.
  The unfinished swap was settled in three places around one edit: by the
  task lock, by the locked-replace helper and by `replace`. `replace` now
  owns the settle for an edit in place. The helper's copy is gone, because
  the read before it already reads a side copy that is the record.
  The task lock keeps its settle. The review assumed that settle was
  redundant, and it is not. `queue requeue`, the removal on import, and an
  edit that moves a task to another state all act on the name and never
  call `replace`. A side copy they left behind would bring the task back
  in its old state. Guards:
  `a_task_left_only_as_its_side_file_is_moved_or_removed_whole` and
  `an_overlay_change_cut_short_at_the_swap_is_read_and_finished_by_the_next`.

**The activity log is appended, not published.** `activity.jsonl` is opened
with `O_APPEND`, and each record is one `write_all` of one line. It is never
renamed, replaced or truncated. The hard-link rule exists so that a file is
never seen half-written and never overwritten. An append-only log meets the
second part by construction. It meets the first part as long as each line
goes out in one write, and a reader skips a last line that does not parse.

### 3. The queue subcommands

- `queue add` gains `--priority`, `--label` (repeatable), `--goal`,
  `--parent`, `--assignee`, `--blocked-by` (repeatable), `--needs-approval`
  and `--backlog`. The parent, the blockers and the assignee must exist when
  the task is added. This follows ADR 0020's rule that anything checkable is
  checked at add time.
- *Amended by the owner, 2026-09-28 (#112):* `queue add --prompt <name>
  --args "<string>"`, which ADR 0025 left to this ADR. The string is stored
  unsplit as the task's `args` and, when the task runs, expanded by the same
  function `trellis run` uses (`templates::task_text`: one splitter, one
  substitution, one join), so a queued template expands byte for byte as
  the equivalent `run --prompt --args` would. `--args` requires `--prompt`,
  as on `run`. The string is split once at add time as well, so an unclosed
  quote is refused then rather than when the heartbeat reaches the task.
  The field has a serde default and is not written when absent, so older
  task files load and rewrite unchanged; a binary older than this reads a
  file that has it by ignoring the key, and runs the template without its
  arguments. Routines do not take `args`. Evidence:
  `queue_add_args_expand_byte_for_byte_as_run_args` and
  `queue_add_args_an_older_task_file_reads_back_unchanged` in
  `crates/trellis-cli/src/work.rs`.
- `queue list` shows priority, status, labels, assignee, blockers, parent
  and the reason a pending task is not claimable. It filters with
  `--status`, `--label`, `--assignee` and `--priority`.
- `queue set <id>` changes priority, labels, assignee, parent, blockers,
  approval requirement, or status (`--status blocked` requires `--reason`).
- `queue note <id> "<text>"` appends a note.
- `queue cancel <id> [--reason]` moves the task to `cancelled/`.
- `queue approve`, `queue reject`: §6.
- `queue log [--limit N]`: §9.
- `queue export`, `queue import`: §10.

*Amended 2026-10-06 (#336), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**`queue cancel <id> --children [--reason <r>]`** cancels the named task
as `queue cancel <id>` does, then every descendant by `parent`, at any
depth, that is in `pending/` (backlog, todo or blocked). The subtree is
read from the index `queue set` uses, so trellis-queue does not change.

- Descendants in `done/`, `failed/` or `cancelled/`, `in_review` ones
  among them, are left as they are and named in the output.
- If the named task or any descendant is in `claimed/` when the command
  starts, nothing is written and the refusal names each claimed id.
- **Best-effort against a claim made while it runs.** The queue cannot
  hold every task at once, so a `work` tick may claim a descendant between
  the check and its move. That descendant is skipped, the rest are
  cancelled, the output names each id cancelled and each not, and the
  command exits non-zero. A running descendant is reported, not stopped.
- Each cancelled task gets its own `task.cancel` line with the reason, and
  each descendant's line names the id the command was given.
- **Not taken.** A subtree pause like Paperclip's tree holds would be a
  second state beside `status`, kept for a later restore. Cancel is
  terminal and needs none. Reversed if a person needs to stop a subtree and
  resume it later: that pause is its own ticket and ADR, and this flag
  stands.

*Amended 2026-10-06 (#338), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**`queue list --text <s>`** lists only tasks whose id, task text, labels
or note text contain `<s>`, compared after lowercasing both. It is one
more predicate beside `--status`, `--label`, `--assignee` and
`--priority`, and narrows whatever they list. A task file `queue list`
cannot read is still listed as unreadable, whatever `--text` says, since
it has no text to match and is the one entry a person must repair. An
empty or all-whitespace `--text` is refused, since it would match every
task. A backlog of a few hundred tasks needs no ranking; reversed if
backlogs grow to where a substring match lists too much to read, then
BM25 over task text (as `vault search` ranks notes, ADR 0022) is its own
ticket, and this flag stays.

### 4. Agent profiles

A profile is `.trellis/agents/<name>.toml`, committed with the repository
like the rest of `.trellis/`:

```toml
role = "reviewer"
instructions = "agents/reviewer.md"   # under .trellis/, prepended to the task
model = "llamacpp:http://127.0.0.1:8080"
# command = ["python", "tools/agent.py"]   # a process agent (§11); excludes model
# timeout_s = 1800
# pass_env = ["ANTHROPIC_API_KEY"]         # process agents only; default none (§11)
reports_to = "lead"
paused = false
tools = ["read_file", "edit"]           # carried for W-B's registry; see below
allow_write = ["docs/", "*.md"]         # optional; deny_write's syntax; only narrows (#182)

[control]
mode = "auto"          # or "approve"
approve = ["edit"]

[skills]
allow = ["refactor"]   # absent means all
deny = []

[budget]
monthly_tokens = 2000000
lifetime_tokens = 50000000
warn_at = 0.8

# [[services]]                          # started beside each task (§12, #60)
# name = "web"
# command = ["python", "-m", "http.server", "8000"]
```

**Validation** happens whenever profiles are loaded, which is every command
that reads them:
- A name is lowercase ASCII letters, digits, `-` and `_`.
- `model` and `command` are mutually exclusive. With neither, the task uses
  the `work` model.
- `reports_to` must name an existing profile, and the chain must not form a
  cycle (Paperclip item 4).
- `[control]` uses the same vocabulary as the repository's `[control]`
  (ADR 0018).
- `warn_at` is in (0, 1].
- A process profile may not set a budget, because trellis cannot see the
  process's tokens (§8). A budget it could never enforce would read as a
  limit that exists.
- *Amended (wave K):* `pass_env` is refused on a profile without `command`.
  A model profile starts no process to pass a variable to, and accepting
  the field would read as a pass-through that happened. An entry that is
  blank or contains `=` is refused as not a variable name.
- *Amended (#182):* an `allow_write` entry that is blank is refused: it
  parses as the exact path "", which admits nothing, and would read as a
  scope that is not one.
- *Amended (#60):* each `[[services]]` entry has exactly `name` and
  `command`. The name uses a profile name's alphabet (it becomes part of a
  file name), is unique within the profile, and `command` must name a
  program. Either kind of profile may declare services (§12).

**What a profile changes about a run of its tasks:**
- **`instructions`**: the file's text goes ahead of the task, the same way
  `--prompt` does.
- **`model`**: overrides `work --model` and `[model] spec` for this
  profile's tasks.
- **`[control] mode`**: overrides the dial. `work` still has nobody to ask,
  so an `approve` profile's tasks can be claimed only when `work` runs
  attached to a terminal. Otherwise they are unclaimable and give that
  reason, instead of being claimed and refused by `NEEDS_A_TERMINAL`.
- **`[control] approve`**: ~~replaces the repository's approve list for the
  run.~~ *Amended after the tenth review of PR #1:* adds to the approve
  list the repository's and the user's files layer to (ADR 0025 §7), for
  the run. A profile is repository content, and replacing the list let
  `approve = ["read_file"]` drop the user's own entries and the default
  `edit`. A profile can only add restrictions. Evidence:
  `a_profile_s_approve_list_adds_to_the_user_s_and_drops_nothing` in
  `crates/trellis-cli/src/user_config.rs`, which failed before with
  `["read_file", "edit"]` where `["edit", "run_checks", "read_file"]` was
  due. The other fields were checked for the same direction and already
  only restrict: `tools` and `[skills]` narrow, `[budget]` is a further
  cap, and `[control] mode` can only raise `work`'s hands-off default to
  approve (a profile without it runs auto under `work`, as every task
  does, ADR 0020 §5). `pass_env` gives a variable back to a process
  agent, and is a committed choice `trellis trust` shows (§11).
- **`[skills]`**: filters the catalog disclosed to the model (ADR 0019).
- *Amended (#182, owner's decision of 2026-09-29):* **`allow_write`**: the
  only paths this profile's runs may write, in `deny_write`'s rule syntax
  (`vendor/` a prefix, `*.lock` a suffix, `setup.py` exact; folded for
  separators, `.` segments and, on Windows and macOS, case). **It only
  narrows.** Absent, the run is judged by the repository's rules alone,
  exactly as before the field existed, and a profile file without it
  parses and serializes unchanged. Present, a write must be admitted by it
  *and* not denied by the repository: `deny_write` is consulted first
  everywhere, so a path the repository denies stays denied whatever
  `allow_write` names, and the refusal names the repository's rule.
  `allow_write = []` is not the same as absent: it admits no write (the
  field is an `Option`, so the two cannot collapse). A write is admitted
  only when both the path as spelt and the path it really lands on through
  any link are inside; one whose landing place cannot be worked out is
  outside. Enforced where writes are judged:
  - a model run's `edit` and `write_file`, at the governance gate, before
    the file is read, refused as a `deny_write` hit is, naming the list and
    `.trellis/agents/<name>.toml`;
  - a `bash` or custom-tool command's changes, at the throwaway clone's
    carry-back (ADR 0032), refused and named in the tool's output;
  - a process agent (§11), at its throwaway clone's carry-back (ADR
    0032, "Process agents", #175), over every file it edited, created or
    deleted there, `deny_write` first, with the rules read before the
    command ran: each out-of-scope file is named as `governance:
    refused:`, is never written into the repository, and the run does not
    converge and is rolled back. The post-command judge still applies it
    to what git shows changed in the repository, for a command that wrote
    there by absolute path, which the clone does not stop.
  Ignored files are outside what `allow_write` judges: the checks and the
  tests write caches there on every run, and a scope that counted them
  would refuse every run of a scoped agent. They are not part of the change
  a run leaves for review, and `deny_write` still binds them. The scope is
  also kept out of the set that decides which changed ignored files count
  as unrestorable, for the same reason. The model is told the scope in its
  rules, and `agents show` prints it when set.
- **`tools`**: stored and validated only as a list of strings. W-B is
  building the tool registry at the same time (ADR 0023), and this field is
  what it will read. Until then, **it has no effect on the run**, and
  `agents show` says so. *Integrated: it now narrows the run's tool set;
  see "Integrated".*

The first four settings reach `cmd_run` through one new field on
`RunSeeding`. It is an optional reference to the profile, and every caller
except `work` passes `None`.

*Amended in implementation:*
- **The skill filter narrows the list the model is shown, and nothing
  more.** The `read_skill` tool reads any skill by exact name through
  `skills::read_body` in the orchestrator, and that path is owned by W-B. So
  a denied skill is left off the list, but a model that already knows the
  name can still read it. `agents show` says so. Enforcing the filter at the
  tool belongs with the tool registry (ADR 0023). *Integrated: enforced at
  `read_skill`; see "Integrated".*
- **`work --model` is optional.** A queue whose tasks all name a profile
  with its own model, or a process command, needs no default model. A task
  that does need one, when none is configured, is left pending with that
  reason instead of failing.
- **The profile's approve list travels on `RunSeeding`, not `RunControl`.**
  `RunControl` is built with a struct literal at six call sites across the
  modules that W-D is editing. `RunSeeding` has two such sites, `run` and
  `work`; every other caller uses `default()`. The approve list is applied
  to the dial just after `cfg.control(flag)`.

**Pause state is not written into the profile file.** The TOML is tracked,
and writing to it would dirty the worktree that every run needs clean
(ADR 0009). `agents pause`, the budget hard stop and budget raises write an
overlay at `<git-dir>/trellis/agents/<name>.json`, using the lock protocol of
§2. A profile is paused if its file says `paused = true` or its overlay says
so. `agents resume` on a profile paused in its file says to edit the file.

**Subcommands:** `trellis agents list`, `show <name>`,
`pause <name> [--reason]`, `resume <name>`.

*Amended 2026-10-06 (#337), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**A profile may name a vault note as its memory.** Paperclip's
persistent agent files (ADR 0021, PC-117) are a directory each agent keeps
across tasks and sessions and writes itself. Here a profile's `instructions` file lives
under `.trellis/`, and model writes never land in `.trellis/` (ADR 0023), so
agent-writable files there are not an option; what one run learns reaches
the next through the trail (ADR 0007) and vault notes (ADR 0017). The
answer:

- **The field.** A profile may set `memory = "<vault stem>"`. The note is
  compiled through trellis-vault as standing context for every run of the
  profile's tasks, as `--focus` is (ADR 0017), and a run edits it as a note
  through the normal write path, so every change to it is in the diff a
  person reviews.
- **Not built.** This amendment builds no profile field; today a profile
  has no `memory`. A separate ticket, filed after #337 closes, builds it
  under the rules here.
- **Where it is read from.** Only from the vault directory `[context]
  vault` names, which is resolved through
  `trellis_core::paths::resolve_repo_input` (crates/trellis-cli/src/config.rs)
  and walked without following links, as CLAUDE.md requires of every
  repository-supplied prompt input (ADR 0025).
- **A vault under `.trellis/`.** If `[context] vault` is configured under
  `.trellis/`, the note is readable and unwritable by a run, since model
  writes never land there (ADR 0023).

Reversed if a standing note cannot be added beside `--focus` without
changing the compiled bytes for runs whose profile sets no `memory`: then a
plain repository file read through `resolve_repo_input`.

### 5. Budgets in tokens

For each profile, the ledger sums the tokens recorded in trails (§8) whose
`agent` is that profile. It keeps two sums:
- **This calendar month, UTC.** The window resets at 00:00 UTC on the 1st.
  A run belongs to the month its `started_at` falls in.
- **Lifetime.**

There are no dollars. COST_MODEL.md does not price tokens, and ADR 0021
excludes dollars until it does.

*Amended 2026-09-29 (#67), by the owner's decision of 2026-09-28:* dollars
are deferred to v1.x, not excluded. Runs are token-only through v1.0. In
v1.x dollars arrive only beside tokens, never replacing them, and only once
COST_MODEL.md carries a per-provider, per-model price table with its source
and retrieval date and prompt tokens are counted, since hosted providers
bill both and a price without its method is a number CLAUDE.md does not
allow.

**What is counted, stated plainly.** The count is **completion tokens as the
runtime adapter reported them**. It is `Model::tokens_used()`, which the
runtime adapter sums from each completion's reported count. *Amended
2026-09-30 (#108):* the ledger now carries prompt tokens as a second number,
field `prompt_tokens`: the trail's own `prompt_tokens`, which the adapter
sums from each reply's reported prompt figure (`Model::prompt_tokens()`),
kept per month and lifetime beside the completion sums and never added to
them. `agents list` and `agents show` print it as its own clause, `prompt
tokens <month>: <n>, lifetime <n>`. A run with any reply that reported no
prompt figure records none; the clause prints `unknown` when every counted
run lacks one, and otherwise the known sum followed by `(<k> runs
unknown)`, never 0 for an unknown. Budgets still limit completion tokens
only, so a budget here limits generated tokens, not billed tokens. A
scripted run records no token count. A process
agent's tokens are unknown to trellis.

**A hosted reply with no usage figure** (amended after the second review of
PR #1; before it such a reply was recorded as 0, so spend read as free and
no budget tripped). The count is unknown, never zero:
- The trail records `tokens: None` for the run once any reply went
  unreported, because the sum of the others would be a lower bound
  presented as a count.
- The in-run `max_tokens` ceiling charges each unreported reply at the
  `max_tokens` it was requested with, the most it can have cost, so it
  trips early rather than never.
- A run executed as a profile with a `[budget]` is refused at the first
  unreported reply: the run ends not converged and rolls back, with a
  reason naming the two fixes (a provider that reports usage, or no
  `[budget]`). The ledger itself is unchanged, and still counts only
  measured spend. The limit, stated: that one reply's tokens, at most its
  `max_tokens`, are spent and counted nowhere, and `agents show` reports
  the run among those without a count.

**The ledger covers the trails in this checkout.** Trails are committed, so a
branch that does not contain a run's trail does not count its spend. Trails
are read as untrusted data (ADR 0007). A trail whose factual digest does not
match is still counted, because it is the only record available, and
`agents show` reports how many such trails there are.

**Thresholds.** *Amended in implementation: every profile with a budget is
checked at the top of every `work` tick, before anything is claimed.* The
next tick after a run is the "after each run" check. The gate then reads the
pause from the overlay.
- **At `warn_at` of either window:** `work` prints a warning naming the
  window, the spend and the limit.
- **At 100% (hard stop):** the profile is paused by the budget in its
  overlay, and `work` skips its tasks. A `budget_override` approval request
  is created, unless one is already pending for this profile.
  - `trellis queue approve <request> --raise <tokens>` raises the window
    that tripped and resumes the profile. A monthly raise applies only to
    the month it was granted in.
  - `trellis queue reject <request>` keeps the profile paused.
- *Amended in implementation:* **one request per pause, not per tick.** A
  request is opened only on the transition into a budget pause. A profile
  that stays over its limit keeps its pause and opens nothing more, so a
  rejection stands until something changes. The first version reopened a
  request on the next tick after every rejection; a unit test now covers
  it.
- **A budget pause lifts when the profile is under every limit again.** That
  happens when the month rolls over, or when someone raises `[budget]` in
  the profile file, which is the way out after a rejection. The resume is
  logged. A pause by a person is never lifted automatically, and a profile
  a person paused opens no request while it is over its limit.

The budget is a cost guard, not a correctness claim. Nothing about it
touches `Verdict`.

### 6. Approvals

**Task approval** (Paperclip item 32, the review stage run before a task
starts):
- `queue add --needs-approval`, or `queue set --needs-approval true`, makes a
  task unclaimable until it is approved.
- `trellis queue approve <task-id>` records `{decision: approved, at}` on the
  task.
- `trellis queue reject <task-id> [--reason]` records the rejection and
  moves the task to `cancelled/`.

**Budget override requests** (§5) are files under
`<git-dir>/trellis/approvals/{pending,approved,rejected}/<id>.json`. Their
ids begin with `budget-`, so `queue approve <id>` can tell them from task
ids.

*Amended after the seventh review of PR #1:*
- **A request belongs to one pause.** When `work` lifts a budget pause (a
  rollover, or `[budget]` raised in the file), every pending request for
  that profile is closed: moved to `rejected/` with the reason
  `superseded: the pause was lifted: <why>`, and logged as
  `approval.supersede`. A later hard stop reuses a pending request only
  when it names the same window, the same month for a monthly window, and
  the same limit; any other pending request for the profile is closed as
  superseded and a new one opened. Before this, a request stayed pending
  after its pause lifted and the next pause reused it, so approving it
  raised September's limit in October, on September's figures.
- **A decision holds the request's lock** (`approval-<id>`) from its check
  to its end, and `approve` claims the request (moves it out of
  `pending/`) before it applies the raise. Two approvals of one request in
  two terminals used to both pass the check and both raise the limit; a
  test with two threads saw the doubled raise. A raise that fails to write
  puts the request back in `pending/`. A crash between the claim and the
  raise leaves the request approved with no raise and the profile still
  paused, which `queue list` shows and a `[budget]` raise in the file
  resolves; the old order could leave the raise applied and the request
  pending, to be approved again.

Every request and every decision is logged (§9).

### 7. Routines

A routine is `.trellis/routines/<name>.toml`:

```toml
schedule = "0 9 * * 1-5"    # minute hour day-of-month month day-of-week
timezone = "UTC"            # or "+02:00", "-05:30", or an IANA name: "Europe/Amsterdam"
concurrency = "coalesce"    # coalesce | skip | always
catch_up = "skip"           # skip | enqueue
catch_up_max = 5            # with enqueue: at most this many missed fires
paused = false

[task]
text = "Triage new failures for {{date}} ({{routine}}), area {{area}}"
priority = "high"
labels = ["triage"]
assignee = "reviewer"
# focus, prompt, needs_approval, parent

[vars]
area = "parser"
```

**Cron.** The parser is written by hand. It supports:
- `*`, lists, ranges and steps (`*/15`, `1-5/2`, `10/5`). *Amended after
  the fifth review of PR #1:* a step of 0, or one larger than the field's
  maximum (60 for minutes, 24 for hours, 32 for day-of-month), is refused.
  `1/4294967295 * * * *` overflowed the step loop, a panic in a debug build
  and a wrapped value in release; the loop's addition is now checked as
  well. Evidence: `a_step_larger_than_the_field_is_refused_not_overflowed`
  in `crates/trellis-queue/src/cron.rs`, which panicked with "attempt to
  add with overflow" before the change.
- month names `jan`–`dec` and day names `sun`–`sat`
- day-of-week 0–7, where both 0 and 7 are Sunday
- the usual rule that when day-of-month and day-of-week are both restricted,
  a day matches if either one does. *Amended 2026-09-27:* "restricted" is
  Vixie cron's test, a field whose first character is not `*`. A field
  starting with `*` (including `*/2`) makes the two AND, so
  `0 9 */2 * mon` is odd-numbered Mondays; it had been read as restricted
  and ORed, firing every Monday and every odd day.

It does not support `L`, `W`, `#`, `?` or `@daily`-style names.

**Timezones: UTC and fixed offsets only, not IANA names.** IANA names would
need a timezone database. That means a new dependency, and the small crates
that bundle one pin the tz data to whenever they were built. A fixed offset
does not follow daylight saving time, so a routine that should fire at 09:00
Amsterdam time is written `+01:00` in winter and `+02:00` in summer, or it is
written in UTC. `Europe/Amsterdam` is refused with that explanation. It is
not treated as UTC. An offset is exactly `+HH:MM` or `-HH:MM`, two digits in
each field, checked on the text rather than on the parsed number: `-5:00`,
`+05:5` and `+005:00` are refused rather than read as something close.

*Amended 2026-09-29 (#47; owner decision 2026-09-28):* **IANA names are
accepted.** `UTC` and fixed offsets parse exactly as before; anything else is
looked up in `jiff::tz::db()`. Dependency reason, per ADR 0021's rule: `jiff`
is already in the build through `trellis-cli` (vault dates), so
`trellis-queue` gains a crate edge and `Cargo.lock` gains no package. The
queue enables only `std`, `tzdb-zoneinfo` and `tzdb-bundle-platform`, a
subset of what `trellis-cli` enables; `tz-system` is not needed, since a
routine names its zone and never reads the machine's.
- **Source, and how stale data shows.** On Unix the OS zoneinfo directory
  (`TZDIR`, else `/usr/share/zoneinfo`), which the OS package manager keeps
  current. On Windows, which has none, the data bundled into the binary by
  `jiff-tzdb` (0.1.8 in `Cargo.lock`), as old as the build; it moves with
  `cargo update`. `trellis doctor` prints which one is in use (`tz data:`),
  and an unknown name's error names it too, since a name missing from stale
  data reads the same as a misspelling.
- **A nonexistent local time** (inside a spring-forward gap, e.g. 02:30 in
  Amsterdam on 29 March 2026) fires at the first instant after the gap
  (03:00 local), not at jiff's `compatible` 03:30. Every gap minute a
  schedule names maps to that one instant, so it fires once.
- **A repeated local time** (inside a fall-back fold) fires once, at the
  earlier instant. The second pass through the hour does not fire, and is
  not a missed fire for catch-up.
- **An unknown name is refused**, including `Etc/Unknown`, which jiff answers
  with a zone that behaves as UTC. A routine is still never read as UTC
  because its zone was not understood.

Evidence: `iana_zone_fires_at_local_nine_on_both_sides_of_each_transition`,
`iana_zone_spring_gap_fires_at_the_first_instant_after_it`,
`iana_zone_autumn_overlap_fires_once_at_the_earlier_instant` and
`iana_zone_unknown_name_is_refused_not_read_as_utc` in
`crates/trellis-queue/src/cron.rs`. They need the tz data present: on a Unix
machine without zoneinfo they fail rather than pass on UTC.

**Firing.** `work` evaluates routines on every tick before claiming. The
state of each routine is stored at `<git-dir>/trellis/routines/<name>.json`,
using the lock protocol of §2. It records `armed_at`, `last_fired` (the
scheduled instant) and the id of the last task the routine created.
- A routine seen for the first time is armed at that moment and does not
  fire for times before it.
- The due instants are the schedule's instants after `last_fired` (or
  `armed_at`) and at or before now.
- **`catch_up = "skip"`** (Paperclip `skip_missed`): only the most recent
  due instant fires. The earlier ones count as missed, and the log records
  how many were dropped.
- **`catch_up = "enqueue"`** (Paperclip `enqueue_missed_with_cap`): the most
  recent `catch_up_max` instants fire, oldest first.
- **Concurrency** applies to each fire. A routine is active while a task it
  created is in `pending/` or `claimed/`.
  - `coalesce` (the default) creates no task. It appends a note to the
    active task saying the routine fired again. This matches Paperclip's
    `coalesce_if_active`, which links the new fire to the active run.
  - `skip` creates no task and logs the skipped fire.
  - `always` creates a task every time.
- The template is filled per fire:
  - `{{date}}` and `{{time}}`: the scheduled instant in the routine's
    timezone
  - `{{routine}}`: the routine's name
  - any key in `[vars]`
  An unknown `{{name}}` is an error when the routine is loaded, not when it
  fires, and a var may not shadow a built-in.

*Amended in implementation:*
- **A `paused = true` routine is not evaluated at all**, so its state does
  not move. When it is unpaused, the fires it missed while paused are
  handled by its catch-up policy, as any other missed fires are.
- **The look-back is clamped to 400 days** so that a routine left
  unevaluated for years cannot stall a tick. The enumeration is one
  `next_after` per due instant, and a year of every-minute fires is about
  526,000 of them.
- **A template's assignee must name an existing profile.** This is checked
  when routines are loaded, like the template variables.
- **A routine is loaded, and fails, as a set.** `work` refuses to start while
  any routine or profile is invalid. That applies the ADR 0020 rule "refuse
  before claiming" to definitions as well as to tasks. Once `work` is
  running, a definition that breaks mid-session is reported once, and the
  last good set is kept.
- *Amended after the seventh review of PR #1:* **each scheduled instant is
  enqueued at most once.** The state was saved once, after every due fire
  of a tick, so a failure on the second fire left the first fire's task
  created with `last_fired` unsaved, and every later tick enqueued it
  again. Now the state is saved after each fire, under the routine's lock
  held for the whole tick; and a scheduled fire's task records the instant
  as `routine_at`, so a failure between a task and its save is found on
  the next tick (a task of this routine for this instant, in any state, is
  that fire) instead of being enqueued twice. The check runs before the
  concurrency policy, so `coalesce` does not note the instant as a second
  fire. A test fails each write of a three-fire tick in turn and checks
  each instant is queued exactly once. `routines run` is not a scheduled
  instant: its task has no `routine_at`, and two in one second are two
  tasks. The check reads every task file, as `add_task`'s index already
  does, so a fire costs two reads of the queue rather than one.
  *Amended after the eighth review:* that read was made once per due
  instant, so a catch-up of `n` instants read every task file `n` times
  under the routine's lock. It is now made once per tick of a routine
  with anything due, into a map of this routine's `routine_at` instants,
  still under the lock and after the state is settled, so the
  idempotence above holds (its test passes unchanged). Not one read
  across all routines: each routine's read must be under its own lock,
  or a concurrent `work` could fire an instant between the read and the
  lock. Evidence: `a_tick_scans_the_queue_once_however_many_instants_are_due`,
  which counted five scans for five instants before the change.
  *Amended after the ninth review:* `coalesce` and `skip` still read every
  pending and claimed task once per due instant to find the open one. The
  same one read per tick now also names it (the first claimed task, else
  the first pending one, as before), and a fire that enqueues makes its
  task the open one for the rest of the tick. What that gives up, stated:
  a task that finishes while the tick is firing is still taken as open
  for the rest of that tick, so a later fire of the same tick may coalesce
  into it (the note it cannot take is logged) where a fresh read would
  have enqueued; the next tick reads again. Evidence:
  `a_coalescing_tick_scans_the_queue_once_however_many_instants_are_due`,
  which counted six reads for five instants before the change.

`work --every S` now idles until the sooner of S seconds and the next due
routine.

*Amended 2026-09-29 (#68), by the owner's decision of 2026-09-28:* the
long-lived watcher stays excluded, and ADR 0020's no-daemon decision
stands. The OS scheduler (cron, systemd timers, Task Scheduler) can start
`trellis work --once` on a schedule with no trellis daemon: each start is
one tick, which evaluates routines and runs at most one task, and the fires
missed between starts go to the routine's catch-up policy as above. Two
limits follow from `work` itself. A tick on a dirty worktree stops before
routines are evaluated, so while a converged change waits for the human
nothing fires and the fires accumulate as missed. And a scheduled start
usually has no terminal, and without one a task assigned to a profile in
approve mode is not claimed. A
trellis-owned daemon was not taken because it would be a process with
repository write access and no terminal attached, which is where ADR 0023's
limits on unsandboxed writers matter most.

**Subcommands:**
- `trellis routines list` shows the schedule, timezone, policies, next due
  instant and last fire.
- `trellis routines run <name>` fires the routine now. Concurrency still
  applies, as in Paperclip's manual run.

*Amended 2026-10-06 (#348), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**A routine may skip a scheduled fire when no commit has landed.** A
routine fires through its concurrency and catch-up policies alone
(`routine::evaluate`), so a nightly "triage new failures" routine queues a
task on a night when nothing changed, and the run spends tokens to find
nothing. Paperclip's `require_external_activity` gate suppresses a
scheduled fire when its activity log has nothing since the routine's last
dispatched run apart from the routine's own records. In a repository the
activity such a routine exists for is commits, which the activity log (§9)
does not see. ~~Not built: this records the rule a later ticket (CLIP-05)
builds to; neither the key nor the state field exists in the code today.~~
Built for #349.

- **The key and the state.** A routine may set `activity_gate = "commits"`.
  Its state gains a field `last_head`, written only when a scheduled fire
  enqueues a task.
- **A quiet fire.** A scheduled fire is quiet, and skipped, when HEAD
  equals `last_head`, or when every path changed between them is under
  `.trellis/runs/`. Trellis commits a trail there after every run (ADR
  0007, `trellis_vcs::commit_paths`), so those commits do not count, or
  each run of the routine's own task would count as activity.
- **Failing open.** A `last_head` that is not a full hex object id is
  treated as absent and never passed to git, where every spawn goes through
  `trellis_vcs`. Any git error fires rather than skips.
- **What is not gated.** `trellis routines run <name>` is not gated. A
  routine without the key fires on its schedule alone, as today. The gate
  does not read the queue's activity log.
- **Older binaries.** `RoutineFile` has `deny_unknown_fields`, so a binary
  from before the change refuses a routine that sets the key, and `work`
  refuses to start while any routine is invalid (a running `work` keeps its
  last good set of routines and says so).

Reversed if routines that set the gate are found skipping fires that had
new work, because that work arrives without a commit (an outside service's
state, for example): those routines leave the key unset, and the gate is
withdrawn only if no routine has a use for it.

### 8. What a trail now records

`Factual` gains two optional fields. Each is omitted when it is `None`, the
same shape `control` uses, so an older trail keeps its shape and its digest:
- `tokens: Option<u64>`: completion tokens, as §5 defines them. `None` for a
  scripted run and a process run.
- `agent: Option<String>`: the profile the run was executed as.

`UntrustedFactual` reads both back. This is the minimal additive change to
`trellis-trail`. The CLI changes that carry the fields are:
- `TrailFacts` gains two fields.
- `cmd_run` reads `tokens_used()` from its driver after the loop.
- `RunSeeding` gains the profile reference (§4).

These are edits to `main.rs` outside the "mod lines, variants and dispatch"
rule for this wave, and they are listed as such.

*As implemented, the complete list of `main.rs` edits outside mod lines,
variants and dispatch:*
- the `RunSeeding` field, and `profile: None` in the `run` arm's literal
- the destructure in `cmd_run`
- the approve-list override after `cfg.control`
- the skill `retain`
- the `tokens` read
- the two `TrailFacts` fields, and their use in `record_trail` and in one
  test helper
- the `Queue` variant's doc comment, which now names ADR 0026

### 9. The activity log

`<git-dir>/trellis/activity.jsonl` gets one line per mutation:

```json
{"at":"…","actor":"human|work|routine","action":"task.add","entity":"task:<id>","run_id":null,"detail":"…"}
```

**What is logged:**
- task add, set, note, cancel, claim, finish, requeue, wake
- `mention_wake` and `mention_ignored`, each naming the note (§1, #57)
- `task.note_refused`: a `queue note` refused because it ran inside a run
  (§1, #189). Its actor is `human`, the identity the command claims; its
  `run_id` is `TRELLIS_RUN_ID`'s value when that was the variable found,
  and its detail names the variable and the note's text
- approval request, approve, reject
- agent pause and resume, including budget pauses and resumes
- routine fire, coalesce, skip and missed-drop
- import

`trellis queue log [--limit N]` prints the newest N lines (default 20), with
control characters escaped. The log lives in the git directory for the same
reason as the queue.

### 10. Export and import

`trellis queue export <dir>` writes:
- `agents/*.toml`, and each profile's `instructions` file under
  `agents/instructions/`
- `routines/*.toml`
- `tasks/<state>/<id>.json`

It does not export overlays, routine state, approval requests or the
activity log. Those are this checkout's runtime state, not definitions.

**Nothing secret exists to scrub.** Profiles name a model spec, and a key
never goes in a file trellis reads (ADR 0010). Paperclip scrubs secrets and
paths on export. Trellis has no secrets to scrub. The paths it exports are
relative to `.trellis/`.

`trellis queue import <dir> [--dry-run] [--on-conflict skip|replace|rename]`
reads the same layout:
- Profiles and routines are written into `.trellis/`, which leaves the
  worktree dirty. They are definitions a person commits, like any edit to
  `.trellis/`.
- Tasks are published into the queue.
- On a conflict, the default is `skip`. `replace` overwrites. `rename` gives
  the incoming item a new name or id and rewrites references to it within
  the imported set: `assignee`, `reports_to`, `parent`, `blockers` and a
  routine's `task.assignee`.
- `--dry-run` prints the plan and writes nothing.

The profiles are validated after the import is planned, including cycles
across old and new profiles, and before anything is written. An import that
would create an invalid set is refused as a whole.

*Amended in implementation:*
- **Claimed tasks are not exported**, and the export says how many it left
  out. A task in `claimed/` belongs to a run in progress. A `claimed/`
  folder in an import is refused.
- **Export refuses a folder that is not empty.** An export mixed into
  another folder's contents would bring those contents along when it is
  imported.
- **An instructions file is shared by path, not owned by one profile.** An
  import that creates or renames a profile whose instructions path already
  exists locally with different content is refused as a whole. Only
  `--on-conflict replace` overwrites it.
- **Task ids from an import folder must be lowercase ASCII letters, digits
  and dashes.** Each id becomes a file name in the git directory.
- **`--on-conflict replace` against a task claimed here is refused while
  planning**, before anything is written. It used to be found only when
  the apply reached that task, after the profiles and routines ahead of it
  were already in `.trellis/`. A claim made between the plan and the apply
  is still refused at that task, with the earlier items written; the
  window is the length of the import.

### 11. Process agents

This is Paperclip's `process` adapter. A profile with
`command = ["prog", "arg", …]` instead of `model` runs its tasks like this:

0. **Trust** (*amended 2026-09-27*, ADR 0023 §4). The command is
   repository code, and before this amendment `work` ran it in a cloned
   repository with no trust decision, where a hook or a custom tool would
   not have loaded. Each process agent's argv, and the files it names, is
   now an item in the trust fingerprint (`process agent <name>`, shown by
   `trellis trust`). A task assigned to an untrusted process agent is not
   claimed: it stays pending with the reason `agent <name> runs a
   repository command; not trusted: run trellis trust`, and the runner
   checks again before it spawns. *Amended after the fifth review of PR
   #1:* the claim gate decides trust once per heartbeat tick, at the first
   pending process task, and reuses that answer for the others in the same
   tick; it had walked and hashed `.trellis/` once per pending process task
   per tick. The answer is dropped at the next tick, so a `trellis trust`
   run while `work --every` waits takes effect there, and the check right
   before the spawn is unchanged. Evidence:
   `trust_is_decided_once_per_tick_and_again_at_the_next` in
   `crates/trellis-cli/src/work.rs`, which counted three checks for three
   tasks before the change.
1. **Snapshot** (ADR 0009). The worktree must be clean, exactly as for a
   model run.
2. **Run** the command in the worktree root. *Amended (#175):* in a
   throwaway clone of the repository, and what it changed there is carried
   back under the refusals every writer meets; ADR 0032, "Process agents",
   is where that is recorded, with what it closes and what it does not.
   Its stdin is the task text,
   with the instructions file prepended, and its environment has
   `TRELLIS_TASK`, `TRELLIS_TASK_ID` and `TRELLIS_AGENT` set. Its stdout and
   stderr go to `work`'s terminal. *Amended 2026-10-05 (#317):* and
   `AI_AGENT=trellis`, as for `bash`, custom tools and hooks (ADR 0023 §3),
   a fixed variable beside `TRELLIS_TASK_ID`; services are unchanged.
   - It runs under `timeout_s` (default 1800). On timeout the process is
     killed.
   - `Child::kill` terminates the direct child only. On Windows, and on Unix
     for a process that ignores its group, a grandchild can outlive it. That
     is recorded here, not solved.
     *Amended after the eighth review of PR #1:* on Unix the command leads
     its own process group, and however it ends — exit, timeout or Ctrl-C
     — `kill -KILL -- -<pgid>` ends every member before anything is
     judged or rolled back, so a backgrounded writer cannot land after
     either. On Windows `taskkill /T /F` walks the tree on a timeout or an
     interrupt, while the command is alive; after a normal exit it is gone
     and the tree cannot be walked from it, so what it started keeps
     running. A process that left its group (`setsid`) is not reached on
     Unix. Unix-only test, which this Windows development host cannot run:
     `exec::tests::what_a_command_backgrounded_cannot_write_after_the_call_returns`
     covers the same group kill for `bash`.
     *Amended after the ninth review of PR #1: reverted for process
     agents.* In a group of its own the command was a background job on
     the user's terminal, and the kernel stops a background job that
     writes to the terminal (with `tostop`) or reads from it, with
     SIGTTOU or SIGTTIN: an interactive agent hung. A process agent is the
     user's chosen command on their terminal, so it runs in trellis's own
     process group, the foreground one, again. The cost, stated: on Unix
     there is no group of its own to signal, so on a timeout or an
     interrupt only the command itself is killed, and after a normal exit
     nothing is; a grandchild it started (a backgrounded writer included)
     can outlive it and write after the comparison and the rollback.
     Ctrl-C at the terminal reaches the whole foreground group, the
     command's children included. Windows is unchanged (`taskkill /T`
     while the command is alive). Guarded, Unix-only and never run on this
     Windows host: `a_process_agent_runs_in_trellis_s_own_process_group`
     in `tests/orchestration.rs`, which compares the agent's process group
     with the test's; a stopped background job needs a terminal to stage.
     *Amended by ADR 0035:* on Windows the command runs in a Job Object
     with kill-on-close and no breakaway, and the job is terminated however
     the command ends (exit, timeout or interrupt), before `.git/config`
     is restored and the tree compared, so a backgrounded writer cannot
     land after either. `taskkill /T` is no longer used here. Guarded by
     `job_object_kills_a_process_agent_s_tree` in
     `tests/orchestration.rs` (Windows only). Unix is unchanged.
   - Ctrl-C kills the child and rolls back.

   *Amended (wave K): the environment has no keys unless the profile
   passes them.* The command's environment is trellis's own less every
   hosted key variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and every
   `[[providers]] key_env`), removed as for every other child (ADR 0024
   §3). An external agent may legitimately need a key of its own, so the
   profile field `pass_env = ["NAME", …]` puts back exactly the named
   variables, each only if it is set in trellis's environment. The default
   is none.

   Passing a key is the profile owner's explicit choice, and it is made
   knowing this: **whatever a process agent prints is recorded.** Its
   stdout and stderr are not captured or scrubbed by trellis; they go
   straight to `work`'s terminal, and from there into whatever keeps that
   output (scrollback, a CI log, a file `work` was redirected to). What it
   writes into the worktree is left for review if the run converges. A
   process agent given a key and told to print its environment will print
   the key. `agents list` and `agents show` name the passed variables,
   never their values. *Amended after the third review of PR #1:* so does
   `trellis trust`. The process agent's trust item carries `pass_env`, and
   the item prints `receives: ANTHROPIC_API_KEY` on its own line. Before
   this, the key was in the digest only because the profile file was, and
   nobody trusting the repository was told a key would be handed to it
   (`trust_names_the_keys_a_process_agent_receives`). Evidence:
   `a_process_agent_inherits_no_key_unless_its_profile_passes_that_one` in
   `crates/trellis-cli/tests/child_env.rs`.
3. **Check** with the grounded checks, the same pair `trellis check` runs:
   typecheck and tests over `.`.

   *Amended after the second review of PR #1: governance binds a command
   as it binds a model.* Before it, a process agent was judged on the two
   checks alone, and a `[governance] deny_write` path or a
   `.trellis/governance/` rule that refuses a model's edit did not refuse
   a command's. Now, over every path `git status` names after the command
   (untracked files one by one):
   - each path is matched against `[governance] deny_write` and every
     `deny-write` document; a match is a breach, named in the output;
   - when the repository has rule documents, the same `governance_audit`
     the loop runs at convergence produces a `Verdict` over those files,
     recorded with the other two.

   *Amended after the third review of PR #1: the judge is read before the
   command.* The `[checks]` commands, `deny_write` and the rule documents
   were read from the worktree after the command ran. A command could
   therefore rewrite what verifies it, for example by setting `typecheck`
   to a command that always passes or by deleting a `deny_write` line, and
   then converge. All three are now loaded before the command is spawned,
   as the `.git/` bytes below are, and a profile whose rules do not load is
   refused before anything runs. Any changed path under `.trellis/` is a
   breach, whatever the rules say, because that directory holds the checks,
   the rules and the trust inputs that judge the run. The run does not
   converge and is rolled back
   (`a_process_agent_that_rewrites_its_own_checks_does_not_converge`). What
   is not covered, stated: the repository's own test and typecheck
   configuration outside `.trellis/` (`pyproject.toml`, `conftest.py`, the
   tests themselves). A command can change those, just as a model run can,
   and the diff that is left for review is where that shows.

   **`.git/` is outside the worktree the rollback restores**, and `git
   status` does not see it, so a command could plant a hook or set
   `core.hooksPath` and survive any rollback. The bytes of `.git/config`,
   of every file under the hooks directory and (*amended after the sixth
   review*) of `info/exclude`, each as `git rev-parse --git-path` names
   it, are read before the command and compared after. `info/exclude`
   decides what `git status` and the rollback's `git clean` can see: a `*`
   written there hid every later run's new files from both
   (`a_process_agent_that_writes_git_info_exclude_has_it_restored`). A
   difference is put back (the old bytes rewritten, a new file removed),
   named in the output, and makes the run not converged. What is not
   covered, stated: the rest of `.git/` (refs, objects, `packed-refs`, the
   index), a change the command makes and reverts before it exits, and a
   file that cannot be written back, which is reported as not put back. A
   process that outlives the command (see step 2) can still write after
   the comparison.
   *Amended after the eighth review of PR #1:* the guard moved to
   `trellis_vcs::git_guard`, and a model run with `bash`, custom tools or
   hooks uses the same one (ADR 0023 §5). It now also covers
   `config.worktree` (created or changed), every file a config
   `include.path` or `includeIf.*.path` names that lives under the git
   directory (followed through includes of includes, to git's depth of
   10), and `info/attributes`, which can bind paths to a filter the config
   defines. `--git-path hooks` already follows `core.hooksPath`, and a
   config edit that re-points it is a change to `config`. Not covered,
   stated: an include target outside the git directory (a tracked one is
   the rollback's; one elsewhere on disk is not the repository's), and a
   submodule's own git directory under `.git/modules/`. A hook written back
   gets the permissions it had at capture: written with the default mode it
   was not executable on Unix, so a deleted hook was "put back" and no
   longer ran (`a_deleted_hook_comes_back_with_its_permissions`, which
   failed before on the read-only bit; the Unix mode bits it also checks
   are not exercised on Windows). A hook whose mode alone changed is not
   detected: the comparison is by bytes.
   *Amended after the ninth review of PR #1: links.* The walk of the hooks
   directory followed links and restore removed every file absent at
   capture, so a planted `.git/hooks/lib -> $HOME` had restore delete the
   home directory's files, and a hook replaced by a link to `~/.bashrc` had
   the hook's bytes written over it. Now nothing is read, written or
   removed through a link: every path is looked at with `symlink_metadata`,
   a link is recorded by its text and never entered, a planted link is
   removed as a link (never its target), a changed link is made again as a
   link, a file is written back with `create_new` (which does not follow a
   link) after whatever is at its path is removed, and a directory on the
   way to it that has become a link is replaced by a directory. Nothing
   outside the git directories as they resolved at capture is written or
   removed, so a hooks directory outside them (`core.hooksPath` into the
   worktree, or elsewhere) is no longer guarded; re-pointing
   `core.hooksPath` is a change to `config`, which is. A hook the user keeps
   as a link is compared as the link, so an edit to its target is not a
   change here (the target is the worktree's, or not the repository's). The
   comparison now includes a file's permissions, so a mode change alone
   (`chmod +x` enabling a hook) is detected and put back. Evidence, in
   `trellis_vcs::git_guard`:
   `a_planted_directory_link_in_the_hooks_is_removed_and_its_target_is_untouched`
   (a junction on this Windows host; it failed before with the target's
   file listed as a new hook), `a_hooks_subdirectory_replaced_by_a_link_is_not_written_through`,
   `a_hook_replaced_by_a_file_link_comes_back_as_a_file_and_the_target_is_untouched`
   (it failed before with the target overwritten) and
   `a_hook_that_was_a_link_at_capture_is_compared_as_a_link_and_never_written_through`,
   all four failing before the change; and
   `a_hooks_directory_outside_the_git_directory_is_not_touched`. What is
   not closed, stated: each path is checked and then acted on, so a process
   still running while restore runs (a grandchild that outlived the
   command, step 2) could swap a directory for a link in between; closing
   that needs `openat`-relative calls std does not offer. On Windows a link
   is made again only where symbolic links may be created (developer mode
   or the privilege), and a junction comes back as a directory link;
   otherwise it is reported as not put back.

   *Amended after the sixth review of PR #1: files git ignores.* `git
   status` does not list them, so a command could write
   `.venv/.../sitecustomize.py`, an ignored path under `.trellis/` or an
   ignored path `deny_write` names, and the breach test, governance and the
   rollback (`git clean -fd` leaves ignored files) all missed it. Now
   `git ls-files --others --ignored --exclude-standard` is listed just
   before the command is spawned and again just after it ends (after the
   `.git/` bytes above are put back, before the checks, which write caches
   of their own), each file with its `symlink_metadata` length and
   modification time, and on Unix its inode and change time. The
   difference is three lists: created, modified, deleted.
   - All three are named in the output and recorded in the trail as a
     second `process` action whose target is `files git ignores`. They are
     not `proposed`: that list is what a commit would carry.
   - All three are subject to the `.trellis/` breach and `deny_write`,
     whether or not anything git shows changed
     (`a_process_agent_s_ignored_write_under_trellis_or_a_denied_path_is_a_breach`).
     The `forbid` audit is not run over them: it reads contents, and a
     `pip install` is thousands of files.
   - On a run that does not converge, every ignored file the command
     created is removed (a link is removed as a link), and then any
     directory that removal left empty
     (`a_process_agent_s_ignored_writes_are_removed_on_rollback`). A
     created file that cannot be removed makes the tree not restored.
   - **What cannot be put back, stated.** An ignored file that existed
     before and was changed or deleted: trellis keeps no copy of ignored
     files (a `.venv` can be gigabytes), so it is named as not put back.
     A directory that was empty before the command and held only what it
     created is removed with it.
     *Amended after the eighth review of PR #1:* such a rollback is not a
     restored tree, and says so: the run ends `NotRestored`, `work` stops
     with exit 2 and names the files
     (`a_process_agent_that_changed_a_pre_existing_ignored_file_leaves_the_tree_not_restored`,
     which failed before with the run reported rolled back). Before, the
     restored status was git's alone and `work` went on over a file it
     had just said it could not put back. The cost, stated: a process
     agent that runs the repository's tests itself rewrites their caches
     (`.pytest_cache`, `__pycache__`) when they already exist, so a failed
     run of such an agent stops `work` as not restored. Unlike a model
     run (ADR 0023 §5), the command and the tools it runs are one process
     tree and cannot be told apart. A `.git/` file that could not be put
     back makes the rollback not restored too; before, it was only
     printed.
     *Amended after the ninth review of PR #1:* that cost was the common
     case, so a changed pre-existing ignored file now counts only where it
     was already a breach: under `.trellis/` or named by `deny_write`. Any
     other is named ("left as it is") and the run ends rolled back, since
     the tracked tree is restored, which is what ADR 0009 promises. A
     deleted pre-existing one still makes the tree not restored: that is
     a loss, not a rewritten cache. Evidence:
     `a_process_agent_that_changed_a_pre_existing_ignored_file_is_reported_and_work_goes_on`
     (renamed from `…_leaves_the_tree_not_restored`; it failed before with
     "left the tree NOT restored") and
     `a_changed_or_deleted_ignored_file_the_rules_protect_leaves_the_tree_not_restored`.
     The same rule holds for a model run (ADR 0023 §5).
   - **What a converged run keeps.** Ignored files the command created,
     such as a `.pytest_cache`, stay. That includes a `sitecustomize.py` or
     a `.pth` file written into an ignored `.venv`, which Python runs at
     start-up, inside the very checks that judged the run. A repository
     that wants that refused says `deny_write = [".venv/"]`.
   - **What the comparison misses, stated.** It is by stat, not by
     content. On Windows, a rewrite that keeps a file's length and sets its
     modification time back is not seen; on Unix the change time, which a
     process cannot set, catches it. A listing that fails after the
     command makes the run not converged and removes nothing.
   - *Amended after the seventh review of PR #1:* the listing, the diff
     and the removal moved to `trellis_vcs::ignored`, and a model run with
     `bash`, a custom tool or hooks uses them too (ADR 0023, "after a
     pass").
   *Amended after the tenth review of PR #1: HEAD may not move.* The
   changed paths were what `git status` named against the current HEAD,
   so a command that committed part of its change (a `.trellis/` write,
   say) hid that part from the `.trellis/` breach, `deny_write` and the
   audit, converged on the part it left uncommitted, and the commit was
   never rolled back. Now HEAD is compared with the snapshot right after
   the `.git/` bytes are put back: a different commit or a different
   branch (a commit, a reset, a checkout, a branch switch) is a breach.
   The checks are not run, the run does not converge, and the output
   names the move. The changed paths are now taken against the snapshot's
   commit (`git diff --name-only --no-renames <snapshot>` plus the
   untracked files), so what was committed is judged and named with the
   rest. The rollback puts HEAD back first (ADR 0009, amended): on the
   recorded branch with `symbolic-ref`, or detached at the recorded commit,
   then `reset --hard` to it, so the user's branch no longer holds the
   agent's commit. Evidence, in `tests/orchestration.rs`:
   `a_process_agent_that_commits_part_of_its_change_is_refused_and_the_commit_undone`
   (it failed before with "CONVERGED (converged: both checks passed)") and
   `a_process_agent_that_switches_branch_is_refused_and_put_back_on_the_original`
   (it failed before with "changed nothing", HEAD left on the agent's
   branch). A model run with `bash`, a custom tool or hooks is held to the
   same rule (ADR 0023 §5). Not covered, stated: a branch the command
   created is left in place, and a ref other than HEAD and the snapshot's
   branch (another branch moved with `git branch -f`, a tag) is not
   compared.
4. **Decide.** The run converged if and only if the command changed at least
   one file, both checks pass, the governance audit (when there is one)
   passes, no changed path (ignored ones included) is denied or under
   `.trellis/`, nothing under `.git/config`, the hooks or `info/exclude`
   changed, and HEAD is the commit and branch the snapshot recorded.
   - A command that changed nothing has nothing to verify, and ends `failed`
     with that reason.
   - On anything other than convergence, the tree is rolled back exactly as
     after a failed model run.
   - The command's exit code is recorded. It is not evidence of success.
5. **Record** a trail, built in the CLI from `trellis-trail` types. Its
   fields:
   - `proposed`: the changed paths
   - `check_outcomes`: read from each `Verdict`'s `passed()` and kind
   - one `process` action, carrying the argv, the exit status and the
     duration
   - `agent`: the profile
   - `tokens`: none

   As with a model run, the trail is committed and nothing else is.

**The external agent never produces a verdict (ADR 0004).** Its exit code,
its output and anything it writes are data. Convergence comes only from the
two `Verdict`s that `trellis-checks` constructs from processes it ran itself.

#### What rollback guarantees when trellis is not the only writer

*Added after the eleventh review of PR #1.* ADR 0009's rollback rests on
one premise: during a run the orchestrator is the only writer (ADR 0005).
With `edit` and `write_file` as the only writers that premise holds, and
the rollback's guarantee is exact. When `bash`, a custom tool, a hook or a
process agent was enabled, it did not: each was an unsandboxed process
running in the repository with the user's authority, with write access to
the whole worktree and to `.git/`, and for them the rollback was
best-effort against git's view of the tree.

*Amended 2026-09-30, #54:* that caveat no longer holds for these four
writers. Each runs in a throwaway clone (ADR 0032), and no configuration
runs it in the repository instead; what it changes there reaches the
repository only through a carry-back under the refusals every writer
meets. For what such a writer does where it runs, `.git/` included, the
rollback's guarantee is exact, as for `edit` and `write_file`. What a
clone does not stop is stated once, in ADR 0032, "Limits": no network
isolation, reads outside the clone, writes that reach the repository by
absolute path, and processes that outlive the call. The list below is
kept as the record of what the review rounds found; it now holds only
for a writer that reaches the repository by absolute path. Evidence: the
four `isolated_writer_*` tests in `crates/trellis-loop/src/isolate.rs`,
one per writer, each showing a planted file, `core.hooksPath`, a branch,
a tag and a `skip-worktree` flag written from inside it leave every byte
under the repository's `.git/` as it was.

This is a limit, not a defect awaiting another guard. Eleven review rounds
have closed one route after another with a post-hoc git check, and each
round has found another, because the checks read state the writer can
also write. The known ways around it, from rounds 5 to 11, stated so they
are not assumed closed:

- **Backgrounded processes on Windows.** After a normal exit
  `taskkill /T` has no live process to walk the tree from, so what the
  command started keeps running and can write after the comparison and
  the rollback (ADR 0023 §3; ADR 0026 §11 step 2). On Unix a process that leaves
  its group (`setsid`) is not reached, and a process agent runs in
  trellis's own group, so nothing is killed after its normal exit.
  *Amended by ADR 0035:* on Windows, commands and process agents now run
  in a Job Object that is terminated however they end, so this route is
  closed there except for a process started outside the job. The Unix
  half is unchanged.
- **`.git/` writes outside the guarded files.** The git guard restores
  `config`, `config.worktree`, include targets under the git directory,
  the hooks, `info/exclude` and `info/attributes`. The rest of `.git/`
  (refs, `packed-refs`, objects, the index, `.git/modules/`) is not
  compared or restored, nor is a change made and reverted before the
  command exits.
- **Ignored-path tricks, `.gitignore` edits included.** `git clean -fd`
  never removes an ignored file, and what is ignored is decided by files
  the writer can edit: a tracked `.gitignore`, or a new one in a
  directory it creates. The before/after listing of ignored paths
  (`trellis_vcs::ignored`) narrows this, but it reads the same view of the
  tree the writer shaped.
- **`skip-worktree` and `assume-unchanged`.** `git update-index
  --skip-worktree` or `--assume-unchanged` on a tracked file hides an edit
  to it from `git status` and `git diff`, and so from every judgment made
  from the changed paths, and from what the rollback's reset is relied on
  to restore.
- **Refs other than `HEAD`.** Only `HEAD` and the branch it was on are
  recorded (ADR 0009). A branch the writer creates, another branch it
  moves (`git branch -f`), a tag, a stash entry or a note is left as it
  is.
- **Pre-existing ignored files changed.** An ignored file that existed
  before the run (`.env`, a virtualenv, build output, a cache) and that a
  writer modifies is neither restored by the reset nor removed by the
  clean. The ignored-file listing removes new ignored paths and notices a
  changed one by its stat (a rewrite that keeps length and modification
  time passes on Windows), but holds no copy to restore it from.
- **TOCTOU in restore.** Each comparison and each restore is a separate
  step. A process still running (the first item) can write between the
  check and the restore, or after both.
- **Code reached by import outside `.trellis/`.** Trust covers every byte
  under `.trellis/` and refuses hook and tool code named outside it (ADR
  0023 §4, amended after the eleventh review), but code a hook or tool imports from
  elsewhere in the repository is an ordinary file a run may edit.

**What holds.** For trellis's own edits (`edit`, `write_file`), and for
the edits a `bash` or custom-tool command or a hook makes inside its
clone, and the edits, new files and deletions a process agent makes
inside its own, that are carried back (ADR 0032; hooks and process
agents *amended 2026-09-29*, #174 and #175), the rollback's guarantee
is exact, as ADR 0009 states. A run with none of
these writers enabled (the default: `bash` off, and an untrusted
repository loads no hook, custom tool or process agent) keeps it.

**The real fix is isolation**, and the first step is built (ADR 0032,
*amended 2026-09-29*): `bash` and custom tools run in a throwaway `git
clone` of the repository (copy-in/diff-out), and only their edits to files
trellis tracks come back, through the orchestrator's write path, where the
rollback covers them exactly. Against a `bash` or custom-tool command that
writes where it runs, every item in the list above is closed: its
`.gitignore` edit, index flags, `.git/` writes, refs and ignored files stay
in the clone, and what it started writes into a directory that is removed.
*Amended 2026-09-29, #174 and #175:* hooks and process agents run in
clones too (ADR 0032, "Hooks" and "Process agents"). A hook's edits
come back through the same write path; a process agent's edits, new
files and deletions come back through its own carry-back, under the
same refusals, where the rollback covers them exactly. The list still
holds, in full, for any writer that reaches the repository by
absolute path, which a clone does not stop (ADR 0032, "Limits": no network
isolation, reads and writes outside the clone still possible). The guards
above are kept for those writers. Containers or an OS sandbox, for network
and home-directory isolation, remain deferred (ADR 0021, 1.6 and 13.3).
`trellis trust` prints one line stating the limit when it records trust for
any of these writers.

*Amended 2026-10-06 (#350), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.*
**A process profile may drive an agent over the Agent Client Protocol.**
Paperclip now drives Claude Code, Codex and Kimi through the Agent Client
Protocol (ACP), one JSON-RPC protocol over stdio that many agents speak, so
one client reaches them all without a wrapper per agent; ADR 0021 rows
10–11 keep the per-agent adapters excluded. Paperclip's own runner protocol
(PRP) belongs to its control plane and stays out (ADR 0021, PC-109). Not
built: this note records the bounds the build is held to, and the build,
with a new ADR that maps the protocol's messages inside these bounds, is a
separate ticket.

- **The profile.** A process profile may set `protocol = "acp"`. Trellis is
  the ACP client.
- **Where it runs.** The agent runs in the throwaway clone (ADR 0032) with
  the scrubbed environment and only the variables its `pass_env` names (ADR
  0024 §3), as every process profile does.
- **No client capabilities.** Trellis advertises no `fs` or `terminal`
  client capability. A request for one (`fs/read_text_file`,
  `fs/write_text_file`, `terminal/create`) is refused and recorded in the
  trail.
- **Permission requests.** A `session/request_permission` goes to the
  profile's `[control]` gate.
- **The record.** The agent's messages and tool-call reports are recorded
  in the trail as untrusted data (ADR 0007).
- **Writes and convergence.** What the agent writes comes back through the
  clone's carry-back under trellis's refusals (ADR 0032), and only trellis's
  executed checks converge the run (ADR 0004).

Asking permission is the agent's choice (Paperclip's own driver runs with
`approve-all` unless told otherwise), so a tool call the agent does not ask
about is held as a process profile's is today, by the clone and the
carry-back. The protocol adds a record and a partial gate, not a new
boundary.

Reversed if the build finds an ACP agent that will not run without an `fs`
or `terminal` client capability, or that ACP's permission flow cannot be
held to the `[control]` gate: that agent stays on argv and stdin, and if no
agent fits the bounds, no process profile speaks ACP.

### 12. Services

*Added for #60 (owner's decision of 2026-09-28, recorded on the issue).*
Paperclip lets a workspace run runtime services, such as a dev server. A
profile declares them as `[[services]]` (§4), and every task that profile
runs gets them:

1. **Trust.** A service's argv is repository code, as a process agent's is.
   Each one is an item in the trust fingerprint, labelled `service
   <agent>/<name>` and shown by `trellis trust`, and the profile file is
   under `.trellis/`, so it is in the digest as every byte there is. A task
   whose profile declares a service is not claimed until the repository is
   trusted (the same gate and reason as §11 step 0), and `work` checks
   again right before it spawns. Like a process agent's, a service's argv
   may name code outside `.trellis/` (`python manage.py runserver`): the
   refusal of ADR 0023 §4 applies to hooks and custom tools only.
2. **Where and when.** Each service starts in the worktree the task runs
   in, after the task's text has been built and before the run itself (the
   model loop or the process agent), in the order declared. There is no
   readiness wait: a task that needs the port open must wait for it
   itself. Until #24 gives each task a worktree of its own, "the task's
   worktree" is the one `work` runs in. *Corrected 2026-10-04: #24 was
   split into #98–#101. #98 gives each partition of a multi-agent run its
   own worktree (`trellis-loop/src/supervise.rs`); none of the four gives
   a queue task one, so the task's worktree is still the one `work` runs
   in.*
3. **Environment.** The scrubbed environment every child gets (ADR 0024
   §3), plus `TRELLIS_TASK_ID`, `TRELLIS_AGENT` and `TRELLIS_SERVICE`.
   Nothing is put back: a profile's `pass_env` is for its process agent,
   and its services never receive those variables. Evidence:
   `service_gets_the_scrubbed_environment_and_not_its_profile_s_pass_env`
   in `tests/child_env.rs`.
4. **Output.** stdin is empty. stdout and stderr go to
   `<git-dir>/trellis/services/<agent>.<name>.log`, truncated at each
   start, never to the terminal: under `work --json` stdout carries JSONL
   only, and on Unix a service in a process group of its own writing to the
   terminal would be stopped by SIGTTOU. Nothing reads the log. It is not
   bounded in size while the task runs.
5. **Stopping, on every way out of the task.** The services are held by a
   guard in `work`'s runner, and dropping it kills each one with its whole
   process tree, in reverse order: after a converged or failed run, an
   error, Ctrl-C (the run returns as interrupted and the guard drops after
   it) and a panic that unwinds through the runner. If one service cannot
   start, those already started are killed before the task ends as an
   error.
   - **Windows:** each service is in a Job Object of its own, with
     kill-on-close and no breakaway (ADR 0035); the job is terminated and
     the service reaped. If trellis itself is killed, closing the job
     handle ends the tree.
   - **Unix:** each service leads a process group of its own and is not
     reaped until it is stopped, so the group id stays its own and `kill
     -KILL -- -<pgid>` (`trellis_core::process::kill_tree`) cannot reach a
     stranger; then the service is killed and reaped. A process that left
     the group (`setsid`) is not reached. If trellis is killed with
     SIGKILL, nothing stops the group.
   Evidence: `service_is_killed_with_its_tree_when_the_task_ends` in
   `tests/orchestration.rs`: a service backgrounds a heartbeat, and a second
   task in the same `work` measures it and finds it stopped. The second
   task is what makes this a test of the task's end rather than of
   trellis's exit, which on Windows ends the job anyway. With the guard's
   stop removed, it failed. Run on Windows here; the Unix branch of the
   same test has not been run on this host.
6. **Records, and `doctor`.** Each started service is recorded at
   `<git-dir>/trellis/services/<pid>.json` (pid, the start time the
   liveness probe of ADR 0020 §5 reports, agent, service, task, argv). The
   record is removed once the service has been killed and reaped within two
   seconds, and left otherwise. `trellis doctor` lists every recorded
   service whose pid is alive with the recorded start time (or without one,
   when either time is unknown). One whose task is not claimed is listed as
   having outlived its task; one whose task is still claimed is listed
   apart, as running for that task, which is expected while `work` runs it
   (a claim a dead `work` left behind also reads as claimed, until it is
   released as ADR 0020 §5 describes). Only the service's own pid is
   recorded: a process it started that escaped the kill is not listed.
7. **The trail.** Each service is one action, tool `service`, target
   `<name>: <argv>`, first in the list under turn 1. Its outcome says it
   was started and that its output is a log, not a check. `Action` has no
   pass field, and nothing a service prints or returns reaches
   `check_outcomes` (ADR 0004). Evidence:
   `service_starts_in_the_task_s_worktree_and_the_trail_records_it_never_as_a_check`.

**What rollback guarantees with a service, stated.** A service is an
unsandboxed writer with the user's authority, of the same kind as a hook
or a process agent, and it runs while the run is judged and while it is
rolled back. Everything §11's "What rollback guarantees when trellis is not
the only writer" lists applies to it. A service that writes into the
worktree before the snapshot makes the run refuse to start (ADR 0009); one
that writes during the run has its writes judged, and rolled back, as the
run's own. No post-hoc check is added for it; the fix is isolation (ADR
0032, "What is not built").

## What is not built

- ~~**IANA timezone names** (§7).~~ Built for #47; see §7's amendment.
- ~~**The per-profile `tools` list takes no effect** until W-B's registry
  reads it (§4).~~ Integrated; see below.
- ~~**A profile's skill deny list does not stop `read_skill` by exact name**
  (§4). It only narrows the list the model is shown.~~ Integrated; see
  below.
- ~~**`@profile` mentions in notes do not wake anything.** ADR 0021 defers
  them.~~ Built for #57; see §1's amendment.
- ~~No prompt-token accounting~~ (built for #108, §5). **No dollar
  accounting** (§5; deferred to v1.x by #67). *Corrected 2026-10-04: dollar
  accounting alone is not built.*
- **Routines fire only while `work` is running.** There is no daemon
  (ADR 0020). A schedule with no `work` running accumulates missed fires,
  which the catch-up policy then handles.
- **Webhook and API triggers.** ADR 0021 excludes them.
- **On Unix, grandchildren of a process agent are not reaped** (§11; it
  runs in trellis's own process group). On Windows its Job Object ends them
  (ADR 0035).
- **Services (§12): no readiness wait, no per-task worktree until #24, no
  declaration on a single task** (only on a profile), and no services for
  `trellis run`, which runs no profile. On Unix a SIGKILL of trellis leaves
  a service's group running; `doctor` is what reports it. *Corrected
  2026-10-04: #24 was split into #98–#101, and none of them gives a queue
  task a worktree of its own (§12 item 2).*

## Alternatives rejected

- **Rewriting the profile TOML to pause a profile.** Rejected: it dirties
  the worktree (§4).
- **Treating `failed` as not terminal for parent wakes.** Rejected: nothing
  moves a failed child again without a person, so the parent would wait
  forever.
- **A token budget for process agents, enforced by reading the process's own
  report.** Rejected: that is a number the external agent chooses, and the
  budget would then be the external agent's own opinion of its spend.
- **Joining tasks to trails to attribute spend, instead of recording the
  profile in the trail.** Rejected: a trail outlives the queue, which is per
  worktree and is lost when a linked worktree is removed (ADR 0020 §1). The
  trail is the durable record, so it says who ran.

## Consequences

- `trellis agents` and `trellis routines` are new subcommands, and `queue`
  grows. This was owner-directed, like ADR 0020.
- A trail may now carry `tokens` and `agent`. Readers that predate this ADR
  ignore them.
- `work` does more on each tick: it evaluates routines, runs
  `reconcile`, checks budgets and reads every pending file. Each is a
  directory listing and small file reads.

## Integrated

The two §4 items this wave left inert, closed after ADR 0023's registry
landed. Tested unless stated.

- **`tools` narrows the run's tool set.** `ToolSet::narrowed` intersects the
  set `[tools]` built, after trust, with the profile's list. It is called in
  `cmd_run` right after `tooling::prepare` and before the driver is built,
  so the prompt, the schema, the script parser and the orchestrator all get
  the narrowed set, and bash joins the approve list only if it survived.
  - **Only narrows.** A listed tool the repository did not enable, or did
    not trust (bash, a custom tool), is not added. The run's `tools:` line
    names it: `narrowed by agent profile `x`; not added, because this
    repository does not offer them: grep`
    (`a_profile_cannot_widen_the_repository_s_tools_and_the_line_says_so`,
    `a_profile_tool_list_can_never_widen_the_repositorys_set`).
  - **`run_checks` is always kept,** listed or not, for the reason ADR 0023
    §1 refuses to disable it. §4's own example, `tools = ["read_file",
    "edit"]`, omits it, and refusing that list would have made the example
    invalid; the `tools:` line says it was kept.
  - **An unknown name is an error**, not a no-op, as in `[tools]`: checked
    against every built-in and every custom tool the repository declares,
    trusted or not, when profiles are loaded (`agents list`, `work`,
    `doctor`) and again at run start.
  - **The orchestrator refuses the rest** with the same "not a tool in this
    run" as any call outside the set
    (`a_profile_tool_list_narrows_the_run_and_the_orchestrator_refuses_the_rest`).
    A script that calls a removed tool fails before the snapshot
    (`a_profile_without_edit_cannot_edit_and_the_run_costs_nothing`).
  - **A process profile may not have a `tools` list.** Refused at
    validation, for the budget's reason: it does not call trellis's tools,
    so the list could never be enforced, and an unenforceable restriction
    reads as one that holds.
- **The skill filter is enforced at `read_skill`.** The profile's
  `[skills]` becomes a `SkillPolicy` in `trellis-loop`, used both to filter
  the listing and by the orchestrator, which serves `read_skill`; one
  predicate, so the two cannot drift. A denied name is an error observation
  naming the profile and the list: `skill `refactor` is denied to agent
  profile `reviewer`: its [skills] deny = ["refactor"]`, and a name outside
  an allow list says `allow = [...]` instead
  (`read_skill_by_exact_name_is_refused_when_the_profile_denies_it`,
  `a_profile_s_denied_skill_is_refused_at_read_skill_in_a_real_run`).
  `--skill` forcing is not filtered: it is a person at the terminal, not
  the profile, and `work` never forces a skill.
- `agents show` no longer says either field is inert.
