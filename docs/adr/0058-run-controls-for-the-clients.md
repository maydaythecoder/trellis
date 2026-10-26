# ADR 0058 — Run controls for the clients: pause and resume, raising a limit, withdrawing a queued message, `read_file`, and a plan tool

**Status:** Accepted (owner-directed). Sections 4 and 5 built (#809,
#810). Not built: #806, #807 and #808 build sections 1 to 3; #868 records
the controls in the trail (section 6).
**Date:** 2026-10-26
**Issue:** #798 (story S20, #790).
**Decided by:** the owner, 2026-10-25 about 22:50 New York time, in the
session: accept and build all five run controls, keeping CLAUDE.md rules 2
and 9. The shape of each below was written by the session under the owner's
instruction of the same evening to work unattended; each part is reversible
until its ticket is built.
**Holds:** CLAUDE.md rule 2 (`ToolResult` has no pass variant), rule 7 (a
clean worktree before a run) and rule 9 (budgets divide across partitions,
never multiply); the stops for a stale pass, a moved HEAD and changed
trusted code or `.git/` (ADR 0004, 0009, 0023); ADR 0007 (a trail is read
back as untrusted data); ADR 0024 §3 (nothing here sends a key anywhere).
**Amends:** ADR 0008 (a person may raise a ceiling during a run) and ADR
0025 §5 (one queued message may be taken back; `clear_queue` stays
excluded).

## Context

The owner's brand book (`docs/design/system/`) and the run-view wireframe
draw five controls that `trellis rpc` does not answer, and
`docs/design/DESIGN.md` §2.1 marks each `not built`:

- Pause and Resume in the Now strip (NowStrip card; the wireframe's
  `paused`, "held before the next model call").
- Budget limits that extend mid-run (Budget card).
- Queued messages a person takes back (Queue card). rpc delivers every
  steer at the next model request, `abort` hands back what was never
  delivered, and nothing takes back one item: ADR 0025 §5 records "No
  `clear_queue`, by the owner's decision".
- A whole-file view at the code level (the wireframe). `list_paths` lists
  paths; no command reads a file's text for a client.
- A plan checklist (Plan card). The loop has no plan: a step is a tool call
  or `done` (`Step` in `crates/trellis-loop/src/agent.rs`).

A client that drew these without the engine would invent state: a pause the
loop does not honour, a limit the budget does not enforce, a cancel the
model still receives. DESIGN.md §2 forbids that: every control on screen is
one rpc answers, drawn as it works, or marked not built.

What the engine has to build on, on `dev` at e51f277:

- `run_seeded` checks the budget at the top of each iteration, then drains
  the steering channel, then makes the model request. That drain,
  `SteeringLog::drain`, is the one point where a steer is delivered.
- The wall-clock spend already leaves out time spent waiting on a person in
  approve mode (`waited_on_human` in `run_seeded`).
- `Budget::shares(n)` deals iterations and tokens across `n` partitions so
  the shares sum to the ceiling, the remainder one apiece to the first in
  plan order; wall-clock is copied whole, since partitions in a stage run at
  once (ADR 0008).
- A steer or a follow-up enters an unbounded channel as soon as it is sent,
  and the server records it with the id it was sent with (`SteeringPort` in
  `crates/trellis-cli/src/rpc.rs`). The loop's drain delivers a steer and
  sets a follow-up aside; when the run ends, the port hands back what was
  not delivered, and the `Chain` runs each follow-up in the order received.
- `list_paths` answers from `git ls-files --cached --others
  --exclude-standard` (`at_file_paths`), so an ignored `.env` or build
  directory is never offered, and refuses an absolute prefix, a `..`
  component and a control character (`path_prefix`).
- A built-in tool added after the measured five is opt-in, so a repository
  with no `[tools]` section sends the prompt it was measured with
  (`DEFAULT_ON` and `OPT_IN` in `crates/trellis-loop/src/registry.rs`).

## Decision

### 1. Pause and resume

- `pause` and `resume` take no fields. Each is refused with `no run in
  flight` when there is none; `pause` with `already paused`, `resume` with
  `not paused`.
- `pause` is answered once the loop holds: at the top of its next
  iteration, after the model request in flight and the tool call it chose
  have finished, before the budget is checked and the steering channel
  drained. A run that ends before it reaches the hold answers `pause` with
  an error that says so, after its `run_end`.
- Events: `paused` with `turn`, the model call it holds before, numbered as
  `model call` numbers it; `resumed` with `turn` and `paused_ms`.
- Time held is not charged to the wall-clock budget, as time waiting on a
  person in approve mode is not, and the `elapsed_ms` of #799 leaves it out
  too: a run a person paused to read should not end because they read it.
- While paused, `steer` and `follow_up` are accepted and wait as they do
  during a model request, so a steer sent while paused is in the first
  request after `resume`; `set_budget` and `withdraw` are accepted; `abort`,
  `quit` and Ctrl-C take the interrupt path as always, so a paused run rolls
  back and writes its trail.
- Pausing changes no stop rule. An edit made in the tree during a pause
  makes an earlier pass stale (ADR 0004), and a HEAD that moved or trusted
  code or `.git/` that changed during a pause stops the run as it would
  without one (ADR 0009, 0023); the run reads nothing new while held, so
  each is caught where it is caught now.
- The hold is a signal the loop receives with its seed, as the steering
  channel is. rpc's dispatch requests it as it requests an abort, and
  answers from a waiter thread, so `approve` and `abort` are still read at
  once while the loop finishes its turn.
- Held by `rpc_pause_holds_before_the_next_model_request`,
  `rpc_pause_is_not_charged_to_the_wall_clock` and
  `rpc_resume_continues_the_same_run` (#806), and by the stop tests that
  exist for a moved HEAD and changed trusted code.

### 2. Raising a limit: `set_budget`

- `set_budget` takes any of the ceilings `run budget` carries (#799), under
  the same names and units, at least one. It raises a ceiling; it never
  lowers one. A value below the run's current ceiling is refused, naming the
  current ceiling, and nothing changes; a value equal to it changes nothing.
  Refused with `no run in flight` when there is none.
- It is answered with the run's ceilings after the change. The raise takes
  effect at the loop's next budget check, and `run budget` is sent again
  then with every ceiling, so a client redraws its meters from the event it
  already reads.
- Rule 9: a raise is to the run's whole ceiling. A partitioned run deals
  the raised ceiling through `Budget::shares`, as it dealt the first, so the
  shares sum to the raised ceiling and never to N times it. rpc starts no
  partitioned run today, so no command reaches one yet; the rule is held in
  the budget code, so the first command that does cannot multiply.
- A raise that lands after the loop has escalated (its one extra turn,
  granted on exhaustion, ADR 0008) and lifts every exhausted ceiling above
  the spend ends the escalation, and the next request does not repeat the
  final notice.
- ADR 0008's ceilings stay hard. The engine enforces whatever ceiling is
  current; only the person moves one, during the run, and the move is
  recorded (section 6). Neither the engine nor the model raises a ceiling.
- Held by `budget_shares_divide_a_raised_budget`,
  `rpc_set_budget_refuses_a_lower_limit` and
  `rpc_set_budget_sends_run_budget_again` (#807).

### 3. Withdrawing a queued message: `withdraw {id}`

- `withdraw` takes the `id` a `steer` or `follow_up` was sent with; a
  message sent without one cannot be withdrawn. It is answered with
  `{id, type}` once the message is withdrawn, and refused with `already
  delivered` once a steer is in a request or a follow-up's run has started,
  and with `unknown id` when no queued message carries the id.
- One lock decides each message's fate. The loop marks a steer delivered
  under it before putting it in a request, and `withdraw` marks it withdrawn
  under it only while it is still waiting; whichever comes first wins and
  the other is told so. The drain skips a withdrawn steer, so the model
  never receives it.
- A withdrawn follow-up is taken out of the port's record, or out of the
  `Chain`'s queue once the run it was sent during has ended, so its own run
  never starts.
- The list of what is not yet delivered in `get_state` (#801) loses the
  message, and `abort`'s `undelivered` does not name it.
- ADR 0025 §5 is amended: one message a person names can be taken back;
  `clear_queue`, which drops everything queued, stays excluded, and `abort`
  still hands back everything undelivered.
- Held by `rpc_withdraw_refuses_a_delivered_steer` and
  `rpc_withdraw_removes_an_undelivered_follow_up` (#808).

### 4. `read_file {path}`

- Read-only: it changes nothing in the tree. `path` is repository-relative.
- Refused, each by the rule it breaks: an absolute path, a `..` component or
  a control character, as `list_paths` refuses a prefix; a path
  `list_paths` would not offer (tracked, or untracked and not ignored), so
  a file git ignores is never read; anything that resolves outside the root
  or into `.git` (`trellis_core::paths::resolve_repo_input`); and a file
  that is not UTF-8 text.
- Answered with `{path, text, bytes, cut}`: `bytes` is the file's size, and
  a file past the cap is cut at the last whole character before it, with
  `cut` true. The cap starts at 256 KiB, a constant beside
  `LIST_PATHS_LIMIT`, and is a display limit, not a measurement.
- It reads the working tree as it is when asked, a run's edits included, and
  is answered from another thread, as `vault_search` is.
- It is the client's read, not the model's. The model's `read_file` tool is
  unchanged, and nothing this answers enters a prompt.
- Held by `rpc_read_file_refuses_git_and_outside_paths` and
  `rpc_read_file_cuts_a_file_past_the_cap_and_says_so` (#809).

### 5. A plan tool

- A built-in tool, `plan {steps: [{text, state}]}`, with `state` one of
  `todo`, `current` and `done`. It is opt-in, as every built-in after the
  measured five is (`[tools] enable = ["plan"]`), so a repository with no
  `[tools]` section sends the prompt it was measured with.
- At most eight steps, each at most 200 characters. Past either, the plan is
  cut and the tool's result says what was cut: the Plan card's "a plan over
  eight steps is a sign the task should have been split".
- The result is `ToolResult::Output`, naming what was kept, and never
  `Checks` (rule 2). A step's `done` is the model's own account of its
  progress, drawn as the model's words (DESIGN.md §2, rule 1): nothing
  converges on it and no count is made from it.
- A `plan` event carries the steps as kept, with the turn. The trail records
  the call as it records every tool call, as an action; it needs no new
  trail field.
- A plan call is a turn like any other and is charged to the iteration
  ceiling, so a model cannot loop on it for free.
- The person's edits go back as steers. A client sends the edited list, or
  what the person typed, as a `steer`, which the trail's steering record
  already keeps as theirs (ADR 0034). The engine never edits the plan; the
  model may call `plan` again with its own.
- Held by `plan_result_is_output_never_a_pass` and
  `plan_event_carries_the_steps_and_cuts_past_eight` (#810).

### 6. What the trail records

The events above show each control while the run goes on. The trail is the
record a later session reads (ADR 0007), and a pause, a raise and a
withdrawal are the person's decisions: whoever reads a run that went past
the ceiling it started with has to see that a person raised it. They are
recorded in a new list in the trail's `factual` section, `controls`, one
entry per control with the turn it took effect: a pause with how long it
held, a raise with the ceiling and its old and new values, a withdrawal
with the message's kind and its text cut as a steer's text is
(`STEER_TEXT_MAX_CHARS`). The list is omitted when empty, so a trail with no
control keeps its shape and its digest, and a trail written before it reads
back with no list.

That changes the trail format, so it is a ticket of its own, risk:high,
merged only with the owner's G7 approval (#868). Until it lands, #806, #807
and #808 send their events and record nothing in the trail. None of them
writes a control into the steering record, which holds text a person sent
to the model and nothing else.

### 7. The contract

Each command's answer and each event above is listed in
`crates/trellis-cli/tests/client_contract.json` by the ticket that builds
it, one ticket at a time (ADR 0052), with a dated note in ADR 0048 §6, and
both clients' contract tests read every listed field. `get_commands` lists
each new command, and DESIGN.md §2.1's rows for these controls change from
`not built` to the command when its ticket lands.

## Consequences

- `trellis rpc` gains five commands (`pause`, `resume`, `set_budget`,
  `withdraw`, `read_file`) and four events (`paused`, `resumed`, `plan`, and
  `run budget` sent again after a raise). Commands rpc already answers do
  not change.
- One more opt-in tool. A repository with no `[tools]` section sends
  byte-identical prompts.
- The trail format changes once, under G7 (#868).
- A run's budget can now end higher than it started. The ceiling a run was
  started with is in its `run budget` event, and the person's raises are in
  the trail once #868 lands.

## Alternatives rejected

- **A hold inside the model adapter or the rpc reader.** Rejected: the loop
  is where the budget and the stop rules are checked, and a hold anywhere
  else would either be charged to the wall clock or step around them.
- **A pause that is charged to the wall clock.** Rejected: it would end the
  runs a person paused in order to read them.
- **`set_budget` that lowers a limit.** Rejected for now. A limit below the
  spend ends the run at the next check, which `abort` already does with the
  rollback the person asked for, and a lowered ceiling has no defined share
  for partitions already part-way through a stage. Reopen it with a use
  case.
- **A raise granted to one partition.** Rejected by rule 9.
- **`clear_queue`.** Still excluded, as the owner decided (ADR 0025 §5):
  `abort` hands back everything undelivered.
- **Editing a queued message in place.** Rejected: withdraw it and send
  again, so that what the model received and what the person sent stay one
  record.
- **`read_file` without `list_paths`'s rule.** Rejected: a client could then
  read a `.env` that git ignores, which `@` completion was built never to
  offer.
- **A plan the engine keeps and edits.** Rejected: the plan is the model's
  words, and a person's change reaches the model as a steer recorded as
  theirs. The Plan card says the same: the component "never pretends to
  edit the model's mind directly".
- **Recording a control as a steering entry.** Rejected: a steering entry
  says a person's text reached the model or waited for a later run, and a
  pause or a raise is neither.
- **The plan tool on by default.** Rejected for now: it would change every
  measured prompt. Turning it on by default needs a measurement first.

## Status

Accepted (owner-directed), 2026-10-25. Sections 4 and 5 built (#809, #810); the rest not built.

## Reversibility

Each control is additive: a command or a tool, its events and its contract
fields. Until its ticket lands nothing changes. Once one has, removing it
means removing its contract entries, one change at a time (ADR 0052). The
trail's `controls` list, once written, is part of committed trails; because
the field is optional, removing the writer later leaves every trail
readable.
