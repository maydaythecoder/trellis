# Contract records to marks

What each record `trellis rpc` sends makes on screen in each face: every
framing record, every event and every command answer that
`crates/trellis-cli/tests/client_contract.json` lists, those under
`rpc.not_every_client` included, one row each. `system/events.md` is the
design system's intent and uses names trellis does not emit
(`session.start`, `tool.call`); this page uses only the contract's.

How it was read: from the contract, the desktop
(`desktop/src/frames.rs`, `desktop/src/state.rs`'s `apply` and `turn`, the
page modules under `desktop/ui/`) and the TUI (`tui/internal/frames/frames.go`
`Parse`, `tui/internal/model/model.go` `frame` and `response`, the handlers
it calls) as they stand at the commit that last changed this file. Each mark
names the function or file that makes it. Words used:

- **not read**: the face's parser drops the record or the field; nothing
  on screen comes from it.
- **parsed, not shown**: the parser keeps it, as the contract tests
  require, and nothing draws it.
- **not asked**: the face never sends the command, so it never reads the
  answer.
- **not reported**: trellis sends nothing a mark could come from.

A refused answer (`success: false`) is said as a refusal in both faces
unless the row says otherwise: the desktop as `<command> refused: <error>`
in the transcript and the alert (`state.rs` `apply`, unless the command is
one of `VIEW_READS`, whose view says it), the TUI as
`! <command>: <first line of error>` (`model.go` `response`).

`scripts/checks/design-events.py` fails when a name the contract lists has
no row here, or a row names something it does not list. It does not check
that a mark is right; the code it cites is the authority, and a change to a
mark changes its row.

## Framing records

| kind | name | desktop | TUI |
|---|---|---|---|
| frame | `run_start` | Starts a run (`apply`): `task` becomes the transcript's Task row, `model` and `root` go to Where it is, `id` settles a follow-up queue item as started, and every run pane is cleared. `version` and `trellis` parsed, not shown. | Starts a run (`model.go` `frame`): `id` marks the composer's prompt or follow-up as started (`composeRunStarted`); a run trellis started itself (a follow-up) clears the panes. `task`, `model`, `root`, `version` and `trellis` parsed, not shown (the TUI echoes the task it sent; model and root come from `get_state`). |
| frame | `run_end` | The Now strip and the Outcome pane show `outcome` as a Status and `exit` as a Stat; `elapsed_ms` is the wall-clock figure, absent shown as unreported; `error` goes to the alert; `run_id` names the trail the summary reads; `restored` is shown in the outcome (`apply`, `now.js` `drawNow`). A converged run asks `get_last_assistant_text`. | The transcript border reads `<outcome> · exit <n>`, then `worktree restored: <restored>` or the error's first line (`OutcomeLine`); the strip keeps `outcome`, `exit` and `elapsed_ms` (this view's own clock, marked, when it is absent); an `error` outcome writes each error line as `! …` (`runEnd`). A converged run asks `get_last_assistant_text`. |
| frame | `response` | Routed by `command` and `id` to the answer rows below (`apply`). `data` is read only for the commands that row names. | Routed by `id` to what asked it, else by `command` (`response`), to the answer rows below. |
| frame | `approval_request` | A question dialog with `question` and `detail` (the diff) and Approve and Reject (`dialogs.js` `drawApprovals`); a `question` naming an `ask_human` call also writes a `model asks you` transcript entry with the model's question (`state.rs` `update`); a repeated `id` is ignored. `request_id` not read. | A modal with `question` and the `detail` diff, scrollable, answered y or n, or set aside for the code level (`approvalRequested`, `view.Asking`); the Now strip names the `question` on screen beside the keys, cut to fit (#950); a `question` naming an `ask_human` call also writes a `? model asks you` row with the model's question; more than `ApprovalsCap` waiting are declined unseen and said. `request_id` not read. |

## Events

| kind | name | desktop | TUI |
|---|---|---|---|
| event | `turn` | A transcript turn entry, once a turn, `turn <turn> · model <model_ms> ms · <completion_tokens> tok out · <prompt_bytes> B prompt` from the turn's first event, a figure not carried null and drawn unreported; a transcript tool row with the tool sentence (`wrote` kept for quiet mode), the raw `call` and up to the first lines of `observation` in a `<details>`; a check row `<check_kind> passed/failed · trellis-checks verdict` from `verdict` `Some(true)`/`Some(false)`; Actions, Edits (only when `wrote`, with `lines_added`, `lines_removed`, `diff`), Checks (with `duration_ms`), Radius (`resolved`, `candidates`, `unresolved` as three counts), Budget (`turn`, `elapsed_ms`, `completion_tokens_used`, `prompt_bytes`, `completion_tokens`, `model_ms`, `queue_wait_ms`) and the phase in Where it is (`state.rs` `turn`). `start_line` is sent to the page and not drawn; `summarised` not read. | A divider `─ turn <turn> · model <model_ms> ms · <completion_tokens> tok out · <prompt_bytes> B prompt`, once a turn, a figure not carried drawn `—`; a transcript row `· <ToolSentence>` with the first six lines of `observation` under it (`wrote` kept for quiet mode, and the raw `call` and whole `observation` for ^O, #948); then, from the verdict alone, a check row `✓`/`✗ <check_kind> passed/failed · trellis-checks verdict` (`turn`); the Actions, Edits (only when `wrote`, with the counts and `diff`), Checks (with `duration_ms`) and Radius panes, the budget rows (`completion_tokens_used`, `elapsed_ms`, `prompt_bytes`, #951) and the strip's phase, `running <tool> → <target>` (`panesTurn`, #950). `start_line` parsed, not shown; `summarised` not read. |
| event | `model call` | Where it is and the Now strip say `calling the model, turn <turn>`; `elapsed_ms` moves the wall clock (`apply`). | The strip's phase becomes the model call for `turn`; `elapsed_ms` moves the budget's time (`frame`). |
| event | `run budget` | The Budget pane's ceilings, replaced whole: `max_iterations`, `max_wall_clock_ms`, `max_completion_tokens`, `max_context_bytes` (absent shown as unreported) (`apply`, `render.js` `budget`). | The budget box's four rows' ceilings, `max_iterations`, `max_wall_clock_ms`, `max_completion_tokens` and `max_context_bytes` (absent drawn `—` with no Meter), replaced whole, what was counted kept (`runBudget`, `budgetBox`, #951). |
| event | `steer delivered` | The oldest pending steer row reads `delivered after turn <after_step>` (`apply`). | The nth steer sent is marked delivered after turn `after_step` (`steerDeliveredEvent`). |
| event | `runtime identified` | The Runtime pane: `runtime` as a Status, `served` as code, and `kv_slots`, `tiers_vram`, `tiers_ram`, `tiers_disk` as four Stats, each unreported when absent (`render.js` `runtime`). | Where it is names `<runtime> · <served>` (`runtimeText`). `kv_slots`, `tiers_vram`, `tiers_ram` and `tiers_disk` parsed, not shown. |
| event | `plan` | The Plan pane, replaced whole: `steps` as the model wrote them, or why they cannot be read, with `turn`, and a note when `steps_cut` or `texts_cut` is above 0; Edit sends a steer (`apply`, `plan.js` `drawPlan`). | The plan pane, replaced whole, with `turn`, the cut notes and why the steps cannot be read when they cannot (`planEvent`, `view/plan.go`); ^E edits it into a steer. |
| event | `paused` | The Now strip and Where it is say `paused` (`apply`); `turn` parsed, not shown. | `· paused before the model call for turn <turn>` in the transcript; the strip says paused (`pausedEvent`). |
| event | `resumed` | The Now strip and Where it is leave `paused` (`apply`). `turn` and `paused_ms` parsed, not shown. | `· resumed after a hold of <paused_ms> s` in the transcript (`resumedEvent`). `turn` parsed, not shown. |
| event | `compacted` | Not read (no `compacted` in `frames.rs`). | Not read (`Parse` returns Ignored; a WARN or ERROR line would show its message as a note). |
| event | `compacted on request` | Not read; the page never sends `compact`. | Not read; the TUI never sends `compact`. |

## The event line

| kind | name | desktop | TUI |
|---|---|---|---|
| line | `event_line` | `level` not read: an event the desktop has no record for is dropped. | `level` WARN or ERROR on an event the TUI has no record for shows its message as a note (`Parse`, `frame`). |

## Command answers

| kind | name | desktop | TUI |
|---|---|---|---|
| answer | `prompt` | No data. A refusal marks the queued prompt not run, with why (`apply`). | No data. A refusal hands input back and marks the prompt not run (`response`). |
| answer | `follow_up` | No data. Its run's `run_start` settles the queue item. | No data. Its run's `run_start` settles the queue item. |
| answer | `quit` | Not read: the core sends `quit` and waits for the child to exit (`commands.rs`). | Not read: `rpc.Quit` sends it and waits for the child to exit. |
| answer | `classify` | Not asked. | `intent` `question` shows `answer` as text; `task` sends the line as a `prompt`; anything else says so (`classified`). |
| answer | `list_paths` | Not asked. | `paths` complete the `@` fragment or are offered below the task line (`pathsAnswered`). |
| answer | `pause` | Ends the wait for the hold (`apply`); `turn` not read. The hold is shown from the `paused` event. | Ends the wait for the hold (`response`); `turn` not read. |
| answer | `resume` | No data; success changes nothing (the `resumed` event does). | No data; success changes nothing (the `resumed` event does). |
| answer | `set_budget` | Success not read: the row's ceiling is redrawn from the next `run budget` (`render.js` `budget`). | Success not read: the row draws the next `run budget` (`sendRaise`). |
| answer | `withdraw` | `id` and `type` put the message back in the composer and mark it withdrawn (`withdraw_answered`). | `id` and `type` mark the item withdrawn and put its text back on the task line with its verb (`takenBack`). |
| answer | `read_file` | The code level's whole-file view: `text`, with `path`, `bytes` and `cut` said (`code.js`, `WholeFile::answered`). | Not asked. |
| answer | `list_models` | The model picker's list: `local`, `probed` with each address and error, `providers` by name and key variable (`model.js`). | Not asked. |
| answer | `set_model` | The Model pane names `model`, `configured` and `saved_to` (`Chosen::from_data`). | Not read beyond success, which asks `get_state` again (`response`). |
| answer | `get_codebase` | The Codebase level and the Atlas (`codebase.js`, `atlas.js`). | `/codebase`: the module count, the three call-site counts and each module's calls (`codebaseAnswered`). |
| answer | `get_radius` | The radius view and the Atlas: reach and doubt as separate counts, reached symbols and sites (`radius.js`, `atlas.js`). | `/radius <symbol>`: reach and doubt as counts, never a ratio (`radiusAnswered`). |
| answer | `get_symbols` | The Atlas (`atlas.js`). | Not asked. |
| answer | `get_history` | The Atlas (`atlas.js`). | Not asked. |
| answer | `get_queue` | The Board (`queue.js`). | Not asked. |
| answer | `get_state` | The Tier pane from `tier`; Where it is from `model` and `root`; the composer's queue from `pending` (`apply`, `pending_listed`). `running`, `classifying`, `run_request_id`, `last_run` and `pending_approvals` not read. | The status bar's model and tier and Where it is's root; the composer's queue from `pending`, and the Now strip's `N queued` from what that queue still holds as queued (`stateAnswered`, `pendingListed`, #950). `running`, `classifying`, `run_request_id`, `last_run` and `pending_approvals` not read. |
| answer | `steer` | The steer row reads `delivered after turn <after_step>`, or not delivered with the error (`apply`). | The steer is marked delivered after `after_step`, or not delivered with why (`steerAnswered`). |
| answer | `abort` | The composer's undelivered list from `undelivered`, and those queue items marked not delivered (`apply`). | Each `undelivered` steer and follow-up marked not delivered (`abortAnswered`). |
| answer | `approve` | Removes the question with that `id` from the dialog (`apply`); `approved` not read. | Not read: the modal closes when the answer is sent (`answer`). |
| answer | `compact` | Not asked: forwarded by the core, sent by no page module. | Not asked. |
| answer | `get_commands` | The composer's starters (`apply`, `composer.js`). | Not asked. |
| answer | `get_session_stats` | The session tokens pane: each model's prompt and completion tokens apart, `runs_not_counted` said (`SessionTokens::from_data`). | Not asked. |
| answer | `get_available_models` | The settings catalogue and the hosted-spec guard's `configured` (`apply`, `settings.js`). | Not asked. |
| answer | `get_last_assistant_text` | The summary's closing words, fenced as model output, only for the run that converged (`apply`, `summary.js`). | The summary's closing words, cut to `ClosingCap`, only for the run that converged (`closingAnswered`). |
| answer | `vault_search` | The Vault level (`state/vault.rs`, `vault.js`). | Not asked. |
| answer | `vault_graph` | The Vault level (`state/vault.rs`, `vault.js`). | Not asked. |
| answer | `vault_check` | The Vault level (`state/vault.rs`, `vault.js`). | Not asked. |
