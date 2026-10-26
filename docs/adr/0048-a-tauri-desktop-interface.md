# ADR 0048 — A Tauri desktop interface, in `desktop/`, as a `trellis rpc` client

**Status:** Accepted (owner-directed). Built (story S15, #365, every ticket
closed; the page's panes, the last of it, by #513, 2026-10-10).
**Date:** 2026-10-04
**Decided by:** the session, under the owner's 2026-10-04 delegation of the
upstream plan's open questions ("ready for completion without decisions"),
option A of DESK-01's question (#285).
**Holds:** CLAUDE.md rules 2, 3, 4, 8 and 10 unchanged, and the crate
boundaries unchanged: the desktop depends on no trellis crate. It amends ADR
0021's "Owner decisions of 2026-09-28" paragraph on desktop and web
interfaces (#70).
**Reference design:** colibri (`desktop/`, `web/`, Apache-2.0) at ce370e8.
The design is borrowed; no colibri code is used.

## Context

ADR 0021 kept desktop and web interfaces out of this repository under BRIEF
Part 1 ("any surface beyond the CLI"), and named `trellis rpc` (ADR 0025 §5)
as the way an external front end drives trellis (#70). On 2026-10-04 the
owner asked for a desktop interface with Tauri, and delegated the plan's open
questions; this ADR records the result.

colibri's desktop is a Tauri 2 shell around a Vite and React build of its web
UI, with no IPC command and no plugin: the page fetches the inference engine
over loopback HTTP. TRELLIS cannot copy that. A page that reached a model
runtime itself would bypass the snapshot, trust gate, checks, rollback and
trail that `cmd_run_with` owns. The failure modes this ADR has to prevent: a
second path that forges a verdict or skips trust; script injection through
trail or model text (S10); a key exposed through or laundered by the
interface; a killed child that skips rollback; Tauri's dependency tree
entering the root workspace and the `Cargo.lock` the dependency audit
measured; a JavaScript toolchain on every build machine; and a second
divergent interface (Q-19).

The full analysis behind each section is in
`docs/plans/2026-10-04-upstream/desktop-team-analysis.md` and the track's
draft, `desktop-adr-0048-draft.md`, beside it.

## Decision

### 1. Its own workspace

`desktop/` is its own Cargo workspace, as `fuzz/` is: its own `[workspace]`
table and its own committed `Cargo.lock`, with `--locked` on every cargo
command that touches it. Its dependencies are `tauri` 2, `serde` and
`serde_json`, with `tauri-build` 2 as its build dependency, and no
`trellis-*` crate. The root workspace excludes it, so the root workspace and
the root `Cargo.lock` never see Tauri.

### 2. How it reaches trellis

From one file only, it spawns `trellis --root <repo> rpc`,
`trellis --root <repo> --json log --limit <n>` and `trellis --version`. It
never reads trellis's human text (stderr) as data: what it shows comes from
JSON on stdout.
*Amended 2026-10-06 (#500), by ADR 0052, accepted 2026-10-07:* a
`trellis rpc` client may show trellis's stderr to the user in a pane capped
in size, as human text. It still never reads stderr as data.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07):* a fourth spawn, `trellis [--root <repo>] --json overview`
(ADR 0050 section 3, amended), read as one JSON object on stdout, with
`--root` only when the desktop has a root. Accepted; built 2026-10-07 (#558,
#296).
*As built 2026-10-07 (#558, #296):* `trellis overview` is ADR 0050 section
3's, as built. The desktop's `child.rs` spawns it as `--root <repo> --json
overview`, or `--json overview` in the current directory with no root, and
parses stdout as one object; with no root the core emits it once at
startup, and the page's `get_overview` command reads it on demand. Drawing
it is #562's and not built.

### 3. Children, environment and keys

It never kills a trellis child: it sends `quit` and waits (ADR 0025 §5). It
adds nothing to a child's environment and reads no key variable (ADR 0010,
ADR 0024 §3). Only test code sets `TRELLIS_CONFIG_DIR`, so tests never read
the developer's own config.
*Amended 2026-10-07 (#560), owner-approved (G7, 2026-10-07):* `trellis`
starts `trellis-desktop` with its environment unchanged, as it starts
`trellis-tui` (ADR 0052, amendment of 2026-10-07), because the desktop's own
`trellis rpc` child needs the user's key variables for a hosted model the
user chose. That launch is one of the two `INTERFACE_LAUNCHES` held by
`crates/trellis-cli/tests/child_env.rs`: it may sit only in
`crates/trellis-cli/src/launch.rs`, marked `// key-exempt: trellis-desktop`,
once. Every other child trellis starts stays scrubbed. The desktop itself
still adds nothing to a child's environment and reads no key variable; what
its child receives is what the desktop was given. The owner's condition,
recorded on #564: after launch the desktop asks which model to use and keeps
prompt input disabled until one is chosen (section 5, amended). The test's
exemption is built (#560); the launch was not then, and is now (#374,
below).
*As built 2026-10-07 (#374):* the launch is built, for `trellis --dsk`
(ADR 0050 section 1, as built), as that marked spawn in `launch.rs`.

### 4. Hosted model specs

It forwards a hosted spec (`openai-chat:` or `anthropic:`) as `set_model`, or
as a prompt's `model`, only when it is byte-equal to the user's own
configured spec, because `rpc.rs` counts a model the client names as the
user's choice (ADR 0024 §3). A spec the desktop did not get from the user's
own configuration is never sent.

### 5. The page and the shell

The page is static HTML, CSS and ES modules, with no Node, npm or bundler,
and the shell has no Tauri plugin. The desktop's Rust core (`desktop/src/`)
computes every view-model, where `cargo test` holds it, and page JavaScript
only renders what the core emits. Every trellis- or model-supplied string
reaches the DOM through `textContent` or `createElement` (ADR 0007, S10). The
CSP has no `'unsafe-inline'`, no `'unsafe-eval'` and no network origin. The
window's capability is `core:event:allow-listen`, `core:event:allow-unlisten`
and the app's own commands.
*Amended 2026-10-06 (#500), by ADR 0052, accepted 2026-10-07:* the
Go toolchain is an accepted cost, for `tui/` only (the Go TUI, a second
`trellis rpc` client), pinned by a go.dev tarball and its SHA-256. This
narrows the Context's refusal of a toolchain on every build machine to
JavaScript: Node, npm and a bundler are still refused, here and in `tui/`.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07, given with the approval of #560), for sections 2 and 5:* after
launch the desktop asks which model to use, with the user's configured spec
preselected and the local runtimes `first_run` finds (ADR 0050 section 2)
listed, and prompt input (the composer, Send and steering) stays disabled
until a model is chosen in that window. A spec the user saves is written to
the user's own config, never `.trellis/config.toml` (ADR 0024 §3); section
4 on hosted specs is unchanged. Accepted, not built (#564).
*As built 2026-10-10 (#564):* `trellis rpc` refuses `prompt` with "choose a
model first" until `set_model` has given the session a model, and answers
`list_models` with the configured spec, `first_run`'s probed specs and the
user's providers by name and key variable (ADR 0025 §5, added 2026-10-10).
The desktop's model pane (`ui/model.js`), open at launch, lists them with
the user's configured spec preselected, or with none the configured spec
(*corrected 2026-10-10, #564 review:* it had preselected the spec the
configuration resolves to, where the repository's wins over the user's).
The page is served with the composer's
textarea, Send and control `disabled`, and opens them only once the view's
`model`, the core's record of the child's successful `set_model` answer, is
set; `rpc_send` refuses a `prompt` before then. Choosing the configured
spec sends `set_model {configured: true}`, which names no model, so section
4's guard is unchanged; a listed or typed spec is held to it as before.
*As built 2026-10-11, owner-directed (the owner's request of 2026-10-11:
"the current view is nice in the trellis atlas, could you add that to the
current view without removing the other ones"):* the rail switches seven
sections, in this order: overview, run, trails, codebase, atlas, board and
vault. The Atlas is the one added, after codebase; every other section and
what it does are unchanged (`page_rail_buttons_control_sections_that_exist`
in `src/lockdown.rs`). `ui/atlas.js` draws it on a canvas from the view's
`atlas` key, which the core (`src/atlas.rs`) computes from `get_history`,
`get_codebase` (with `edges` and `ref`), `get_symbols` and `get_radius`
answers, the map's layout included. The page uses no library and loads
nothing from a network, writes text through `textContent` only, reads its
colours from `style.css` at draw time, and takes no animated flight under
`prefers-reduced-motion` (the `atlas_` tests in `src/lockdown.rs`).
*Amended 2026-10-26 (#795), owner-directed (the owner's decision of
2026-10-25, about 22:50 New York time, recorded on story #790: replace the
desktop's tabs with the Trail and burrowing levels):* the section rail of
2026-10-11 is replaced by the shell of the owner's brand book
(`docs/design/system/components/Shell.md` and `Trail.md`;
`docs/design/DESIGN.md` §1). The page is one overview a person burrows into,
with no tabs anywhere. Across the top, the Trail: the path from the overview
to where the person is, each earlier segment a way back, the current one
marked, then the depth as `L n/4` and an `ESC out` keycap. On the left, the
sibling rail: the panels of the level the person came from, collapsed, each
still a way in. In the middle, the stage, which holds the level entered.
Along the bottom, the status bar: the keys that work at that level as
keycaps, then the model and the time. `⏎` on a focused panel or row, or a
panel head's `⤢`, goes in; `ESC` goes out one level and keeps the scroll and
selection of the level it leaves. The seven sections (overview, run, trails,
codebase, atlas, board and vault) become panels of the overview, and each
keeps its fetch-on-show: what a section asks the core for when it is shown,
its panel asks when it is entered. One module owns the level keys, and
typing in a field is never taken as navigation. What a level registers (its
DOM listeners and its Tauri subscriptions) ends when the person leaves it.
The fonts, the focus ring and the order of stop-and-send that the shell
needs are rows in `docs/design/DESIGN.md` §2.1. Accepted, not built: #826
builds the shell and rewrites `page_rail_buttons_control_sections_that_exist`
for levels.

### 6. The client contract

The event and trail fields listed in
`crates/trellis-cli/tests/client_contract.json` (DESK-07) are what an rpc or
`log --json` client may rely on, held by tests through the binary. Fields it
does not list stay as unversioned as ADR 0025 §4 leaves them.
*Added 2026-10-12 (#627):* the `runtime identified` event (ADR 0047
decision 13) is listed: `runtime` and `served` always, and `kv_slots`,
`tiers_vram`, `tiers_ram` and `tiers_disk` only when colibri's `/health`
reported them, absent and never 0 otherwise, with the contract saying when
each is left off. It is sent once per run, before the first model request,
only for an endpoint identified as colibri, and stays out of the trail.
Held by `rpc_runtime_identified_carries_every_field_the_contract_lists_once_per_run`
and `rpc_runtime_identified_leaves_unreported_counts_absent_never_0` in
`tests/modes.rs`. The desktop pane that draws it is #628.
*Added 2026-10-26 (#796):* fields trellis already emitted are listed; listing
them changes no output. They are `approval_request`'s `request_id`; the
`level` beside every event's fields; the turn's `duration_ms` and
`summarised`; the `compacted` and `compacted on request` events; the answers
of `steer`, `abort`, `approve`, `compact`, `get_commands`,
`get_session_stats`, `get_available_models`, `get_last_assistant_text`,
`vault_search`, `vault_graph` and `vault_check`; `run_end`'s outcome words
and the turn's check kinds as `outcome_values` and `check_kind_values`;
`prompt`, `follow_up` and `quit` as the commands that answer with no data;
and `log --json`'s `trail.base_commit`, `ended_at`, `factual.radius`,
`context`, `actions` and `steering`, for the summary level. Some were read
before: the TUI reads the `level`, a steer's `after_step` and `abort`'s
ids; the desktop reads `abort`'s text, `get_commands`' prompts, the
`get_session_stats`, `get_available_models` and vault answers, and
`run_end`'s outcome words. The rest (`request_id`, `duration_ms`,
`summarised`, both compaction events, the `approve`, `compact` and
`get_last_assistant_text` answers and the trail paths) no client reads yet;
they are listed for the levels story #790 plans. `get_state`'s answer moves
from `rpc.commands` to `rpc.responses` beside the others. What not every
client reads yet is under `rpc.not_every_client`, and the trail paths under
`log_json.not_every_client`, with `read_by` naming the clients that read
each field, because both clients' contract tests require their parsers to
keep every field of `rpc.events` and `rpc.framing`, and the desktop's
`every_trail_field_in_the_log_contract_is_read` (`src/trails.rs`) every
path of `log_json`'s `always` and `optional`. Held by
`rpc_stream_carries_every_field_the_desktop_reads`,
`log_json_carries_every_trail_field_the_desktop_reads` and
`rpc_steer_abort_compact_answers_carry_what_the_contract_lists` in
`tests/modes.rs`, with the `rpc_vault_` tests there for the vault answers.
*Added 2026-10-26 (#799), owner decision of 2026-10-25 (build additive rpc
fields):* trellis reports the budget's own elapsed time and every ceiling,
so neither client runs a clock of its own against the budget. `model call`
and every `turn`, the governance audit's included, carry `elapsed_ms`: the
loop's monotonic wall-clock spend, with every wait on a person in approve
mode taken out, the figure the wall-clock ceiling is checked against.
`run_end` carries it when the loop ran and returned, and leaves it off,
never sending part of a run as the whole, for an interrupt or abort, an
error before the loop and a partitioned run. `run budget` carries, beside
`max_iterations`, `max_wall_clock_ms`, `max_tokens` (the budget's own name,
in tokens) and `context_bytes` (the driver's prompt ceiling in bytes, left
off for the scripted driver, which has none). Every `turn` carries
`completion_tokens_used`, the counter `max_tokens` is checked against: the
budget counts completion tokens only, so the field says so, and it is never
summed with prompt tokens. No client reads them yet, so they are under
`rpc.not_every_client` with empty `read_by`, and the clients' contract
tests are unchanged. Held by
`rpc_elapsed_excludes_time_waiting_on_an_approval` and
`rpc_run_budget_carries_every_ceiling` in `tests/modes.rs`, and on the audit
turn by `rpc_every_turn_through_the_governance_audit_keeps_the_contract`.
*Added 2026-10-26 (#800):* every `turn`, the governance audit's included,
carries `tool`, `target` and `wrote`: the trail's record of the turn,
computed by `trellis_loop::turn_action`, the function the trail's action
(`action_from`) also uses, from the turn's whole observation. Both clients
had worked these out by splitting `call` at a space and searching the
observation's tail for `applied: true`, logic copied from the retired
ratatui bridge, so the live view and the trail could disagree about what a
turn touched. `wrote` is true only for an edit that applied or a file
`write_file` created; what `bash` or a custom tool changed is the trail's
`changed`, not `wrote`. The governance audit's turn now carries
`duration_ms`, the audit's own time as the trail records it, so
`duration_ms` is on every turn. `call` and `observation` are unchanged: the
clients move to the new fields in their own tickets, and until then the
fields are under `rpc.not_every_client` with empty `read_by`. Held by
`rpc_turn_tool_target_wrote_match_the_trail_actions` in `tests/modes.rs`,
which compares every turn event of a scripted, governed run with the
trail's actions, and by
`rpc_every_turn_through_the_governance_audit_keeps_the_contract`.
*Added 2026-10-26 (#802):* the `turn` of an edit that applied carries
`lines_added`, `lines_removed`, `start_line` and `diff`, in every control
mode. Until now a client learned what an edit changed only from
`approval_request.detail`, which exists in approve mode alone and has no
`@@` header or line numbers. The figures come from the text `apply_edit`
read and wrote, not a second read of the file, through the hunk
`preview_edit` also uses, so the counts on an approval question and on the
turn agree; `EditOutcome` carries them in a field that is not serialized,
so a `post_tool` hook's input is unchanged. `start_line` is the 1-based
line, in the file before the edit, of the first line removed or replaced
(found with `line_of`, which moved into trellis-loop for this). `diff` is a
unified diff of that one hunk with a line of context either side, cut at a
line past 8192 bytes (the cap a command's output is kept to) with a last
line saying how many diff lines were cut; the counts stay the whole edit's.
It is repository text and model output, and a client shows it as text and
never interprets it (CLAUDE.md rule 10). The four fields are absent on
every other turn, bash's included, since nothing diffs what a command
changed. `approval_request.detail` is unchanged, and the trail records no
line counts here; that is its own ticket. The fields are under
`rpc.not_every_client` with empty `read_by`. Held by
`edit_turn_carries_line_counts_and_a_unified_diff` in trellis-loop, and in
`tests/modes.rs` by `rpc_stream_carries_every_field_the_desktop_reads` (auto
mode) and `rpc_elapsed_excludes_time_waiting_on_an_approval` (approve mode).
*Added 2026-10-26 (#809):* `read_file {path}` answers with one file's text
as `{path, text, bytes, cut}`, for the whole-file view no command fed (ADR
0058 §4). It refuses, by the rule each breaks, what `list_paths` refuses as
a prefix, any path `list_paths` would not offer (so an ignored `.env` and
anything in `.git` are never opened), a link that resolves outside the
root or into `.git` (`resolve_repo_input`), anything that is not a file,
and bytes that are not UTF-8. At most 256 KiB is read; a file past that is
cut at the last whole character before it, with `cut` true and `bytes` the
file's size. The answer is listed under `rpc.responses.read_file`, which
neither client's contract test reads, and the desktop's `rpc_send` does not
forward it yet. Held by `rpc_read_file_refuses_git_and_outside_paths` and
`rpc_read_file_cuts_a_file_past_the_cap_and_says_so` in `tests/cli.rs`.
*Added 2026-10-26 (#810):* a `plan` event, for the Plan card (ADR 0058 §5).
It is sent after the turn of a `plan` call the gates let through, in a run
whose repository enables the opt-in tool (`[tools] enable = ["plan"]`), with
`turn`, `steps` (the plan as kept: a JSON array of `{text, state}` in one
string, since an event's fields are flat), `steps_cut` and `texts_cut`. At
most eight steps of at most 200 characters are kept; the counts say what was
cut. The steps are the model's words: a client shows them as text and makes
no count of `done` ones. A person's edit goes back as a `steer`. The trail
records the call as an action like any other, with no new field. The event
is under `rpc.not_every_client` with empty `read_by`, so neither client's
contract test reads it yet. Held by `plan_result_is_output_never_a_pass` and
`plan_event_carries_the_steps_and_cuts_past_eight` in trellis-loop, and by
`rpc_plan_event_keeps_the_contract_and_the_trail_records_the_call_as_an_action`
in `tests/modes.rs`.
*Added 2026-10-26 (#806):* `pause` and `resume`, for the Now strip's Pause
(ADR 0058 §1). Neither takes fields. `pause` is answered `{turn}` once the
loop holds at the top of its next iteration, after a `paused` event with
the same `turn`; `resume` is answered at once with no data, and the loop
sends `resumed` with `turn` and `paused_ms` as it goes on. Held time is not
charged to the wall clock, so `elapsed_ms` leaves it out. Refusals: `no run
in flight`, `already paused`, `not paused`, and, after `run_end`, that the
run ended before it reached the hold. A `resume` before the hold is
reached withdraws the pause; when input ends a held run goes on, since
nothing could resume it. `abort`, `approve`, `steer` and `follow_up` work
while held. The answers are under `rpc.responses`, the events under
`rpc.not_every_client` with empty `read_by`, so neither client's contract
test reads them yet, and the desktop's `rpc_send` does not forward them.
Held by `rpc_pause_holds_before_the_next_model_request`,
`rpc_pause_is_not_charged_to_the_wall_clock`,
`rpc_resume_continues_the_same_run` and
`rpc_abort_while_paused_rolls_back_and_hands_back_the_held_steer` in
`tests/modes.rs`.
*Added 2026-10-26 (#807):* `set_budget`, for the Budget card's mid-run
limits (ADR 0058 §2). It takes any of `max_iterations`,
`max_wall_clock_ms` and `max_tokens`, the names and units `run budget`
sends, at least one, and is answered at once with all three after the
change. It raises and never lowers: a value below the ceiling in force is
refused naming it, a value equal to it changes nothing, and a request with
any refused value changes nothing. Also refused: `no run in flight`, a
value that is not a whole number above 0, `context_bytes` (the driver's,
fixed for the run), any other field, and a raise before the loop's first
budget check or once the loop has returned; one accepted during the run's
last turn reaches no check and changes nothing. The raise takes effect at
the loop's next budget check, where `run budget` is sent again with every ceiling, before
that iteration's `model call`; `run budget`'s entries say so and gain no
field. The answer is under `rpc.responses`, so neither client's contract
test reads it yet, and the desktop's `rpc_send` does not forward it. Held
by `rpc_set_budget_refuses_a_lower_limit` and
`rpc_set_budget_sends_run_budget_again` in `tests/modes.rs`, and by
`budget_shares_divide_a_raised_budget` in trellis-loop (rule 9).
*Added 2026-10-26 (#808):* `withdraw`, for the Queue card's cancel (ADR
0058 §3). The command's own `id` names the steer or follow-up to take back,
by the id it was sent with, as `approve`'s names its question; it is
answered at once with `{id, type}`. A withdrawn steer is in no model request
and its `steer` response is answered `success: false`, so a client reads
two responses with that id, told apart by `command`; a withdrawn follow-up's
run never starts. Either leaves `get_state`'s `pending` and is not in
`abort`'s `undelivered`. Refused: `already delivered` once a request
carries the steer or the follow-up's run has started, `unknown id`, `no run
in flight`, and a missing or non-string id. The loop now marks a steer
delivered before its `steer delivered` event, so `pending` no longer lists
a steer after that event. The answer is under `rpc.responses` and no event
is added, so neither client's contract test reads it yet, and the
desktop's `rpc_send` does not forward it. Held by
`rpc_withdraw_refuses_a_delivered_steer` and
`rpc_withdraw_removes_an_undelivered_follow_up` in `tests/modes.rs`.

### 7. Auditing Tauri's tree

G5 audits `desktop/Cargo.lock` on every run under `desktop/deny.toml`
(DESK-02): a copy of the root policy whose only added lines are the two
supported targets under `[graph]`; per-crate licence exceptions for
`MPL-2.0`, `Unicode-DFS-2016`, `CC0-1.0` and `0BSD`; an `ignore` for each
advisory of kind `unmaintained`; and an `ignore` for RUSTSEC-2024-0429 (`glib`
0.18, kind `unsound`), which Tauri's `gtk` ^0.18 requirement keeps in the
Linux graph. Each entry names its crate, the chain that reaches it, a reason
and what would end it, as the root file's `RUSTSEC-2023-0089` entry does. A
`vulnerability` advisory is never ignored: there is no `audit.toml`, so
`cargo audit` fails on one until a lock update, in its own commit, clears it.
The root `deny.toml` is unchanged.
*Amended 2026-10-07 (#289), owner-directed:* `desktop/deny.toml` may also
hold one per-crate exception for `Apache-2.0 WITH LLVM-exception`, for
`target-lexicon` only: a build-time, Linux-only dependency reached through
`cfg-expr` <- `system-deps` <- the gtk 0.18 `-sys` crates' build scripts,
never linked into the binary. It applies to the desktop workspace only; the
root `deny.toml` is unchanged.

### 8. The product interface and what it may show

The TUI stays the product interface under Q-19 (#16). The desktop shows what
trellis reports and computes no verdict (ADR 0004). It shows radius as three
counts, never a ratio (CLAUDE.md rule 3).
*2026-10-07 (#515), amended by ADR 0052, accepted 2026-10-07:* the product
interface under Q-19 is the Go TUI (`tui/`) once it reaches parity and the
launch handoff ships (T8, #514), with the ratatui TUI as the fallback for one
release. Until then the ratatui TUI stays the product interface. The desktop
rules in this section are unchanged.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07):* the desktop is the default surface of bare `trellis` (ADR
0050 section 1, amended); the TUI stays the product interface under Q-19,
under `--tui` and `trellis tui`, as the line above says. The desktop rules
in this section are unchanged. Accepted; built 2026-10-07 (#561).
*As built 2026-10-07 (#561):* bare `trellis` at a terminal starts the
desktop through `--dsk`'s launcher and prints the overview (ADR 0050
section 1, as built); `--tui` and `trellis tui` start the ratatui TUI, which
stays the product interface until #514's handoff ships.

### 9. A desktop that exits mid-run

A desktop that exits mid-run leaves trellis to finish the run, roll back and
write its trail, held by the test DESK-20 adds.

### 10. Read-only views

The desktop's codebase, radius, queue and vault views read through read-only
rpc commands: `get_codebase` (DESK-13), `get_radius` (DESK-14), `get_queue`
(DESK-15), and `vault_search` and `vault_graph` (DESK-16). Each is added to
ADR 0025 §5 by its own dated amendment, answers from its own thread as
`compact` does, and writes nothing. No `--json` is added to `index`,
`radius`, `queue` or `vault`, so ADR 0025 §4's `--json` commands stay `run`,
`work` and `log`, and section 2's three spawns stay three. No rpc command the
desktop sends writes the vault or the queue; every write it causes goes
through `prompt`, and so through the orchestrator, as #70 requires.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07):* `overview` joins ADR 0025 §4's `--json` commands, so they are
`run`, `work`, `log` and `overview`, and section 2's three spawns are four;
`index`, `radius`, `queue` and `vault` still have no `--json`. The desktop
shows four read-only views, each from these sources:
- graph: `get_codebase` with `path`;
- run: the stream's frames, `get_state` (its `tier`) and the last trail;
- cross-repo: the overview's `recent`;
- board: `get_queue` with `blockers` and `parent`, and PR and CI state read
  live from the user's own `gh` CLI when it is installed and logged in
  (owner, 2026-10-07). trellis still opens no socket itself: `gh` is a child
  process started through `secrets::scrub` like every other, read-only
  (`gh pr list`, `gh run list`), with a bounded wait; when `gh` is absent or
  not logged in, the board says so.

`get_codebase` and `get_radius` refuse a root with no `.git` entry. Nothing
here writes. Accepted; built in part 2026-10-07: `overview` and its
`recent` and `gh` board state (#558) and the desktop's spawn of it (#296);
not built: `path`, `blockers`, `parent` and the refusal (#559), and the
drawn views (#562).
*As built 2026-10-07 (#558):* the board's `gh` is the user's first `gh` on
PATH, started through `secrets::scrub` with no token, read-only (`gh pr
list --state open`, `gh run list -L 10`), each bounded to 10 seconds; when
it is missing, not logged in, times out or fails, the overview's `pr_ci`
says so as `{"available": false, "reason"}` (ADR 0050 section 3, as
built).
*As built 2026-10-11, owner-directed (the owner's request of 2026-10-11:
"the current view is nice in the trellis atlas, could you add that to the
current view without removing the other ones"):* rpc gives the desktop's
Atlas section what it draws, read-only and each on its own thread, each
refusing a root with no `.git` entry: `get_codebase` with `edges: "all"`
for every module edge and `ref` for the graph at a commit (its `ref`,
`commit` and `date`; HEAD's `commit` and `date` without one), `get_symbols`
for definitions with their lines and their `calls_in`,
`calls_in_ambiguous` and `calls_out` counts, `get_radius`'s `reached` and
`sites` lists beside its unchanged counts, and `get_history` for HEAD's
first-parent commits, the tags in its history and per-path churn measured
back from HEAD's committer date (ADR 0025 §5, added 2026-10-11). A
commit's tree is read through `trellis_vcs` into a temporary directory
outside the repository and removed; nothing the repository holds is
written. No `--json` is added to `index`, `radius` or any other command,
and section 2's spawns are unchanged.
*Added 2026-10-12 (#632):* the vault view also reads `vault_check` (ADR
0025 §5, added 2026-10-12, #631), one more read-only type `rpc_send`
forwards: the report line, the issues, the warnings and the run gate's
answer `trellis vault check` prints, as text under the vault section's
Check pane, asked as each child starts and on the pane's button. The gate
answer is vault validation, not a verdict (ADR 0004): it never reaches the
run section's checks pane, held by
`vault_check_is_drawn_in_the_vault_section_and_never_as_a_check_row` in
`src/lockdown.rs`. No `fix` is sent; rpc refuses the field by name, and
every vault write stays a `trellis vault` command (#70). Section 2's
spawns are unchanged.
*Amended 2026-10-12 (#654), owner-directed (the owner's decision of
2026-10-12, on #654):* after `get_codebase` indexes the worktree, it saves
that index to `.trellis/index.bin`, so the overview's codebase pane, which
reads only a current cache (ADR 0050 section 3), describes the graph the
Codebase section just drew instead of saying "not indexed; run trellis
index". This is the one write a read-only view causes, and it is not a new
writer: it goes through `index_cmd::save_index`, the function `trellis
index` saves through, onto `trellis_index::cache::save`, after the same
refusal of a root with no `.git` entry; the self-ignoring
`.trellis/.gitignore` that writer makes keeps the worktree clean (ADR
0009), and the trust digest already leaves the blob out (ADR 0023, amended
2026-09-27). A `get_codebase` with `ref` saves nothing, since the cache
describes the worktree. The answer says which as `index_saved`, with
`index_bytes` when saved and `index_not_saved` (why) when not; a failed
save does not fail the answer. `get_radius`, `get_symbols`, `get_history`,
`get_queue`, `vault_search`, `vault_graph` and `vault_check` still write
nothing, and the vault and the queue are still written only through
`prompt`. The desktop
reads the overview again when the Codebase section's answer says it saved;
the Atlas's worktree answers save too, and the overview shows them at its
next read. Built 2026-10-12 (#654).

## Alternatives rejected

**B, a separate repository.** It leaves the protocol with no first-party
client in this repository's CI, so a change that breaks the client would not
fail the change. **C, no desktop.** It closes the track the owner opened.

## Consequences

- The protocol and its one first-party client change in one commit and one
  CI run, so DESK-07's contract tests fail the change that breaks the client.
- G5 audits Tauri's tree on every run, bounded by section 7.
- Reversed if the owner reaffirms #70's exclusion of a desktop inside this
  repository: this ADR is then superseded, ADR 0021's amendment says so, and
  the track moves to a separate repository as in B.
