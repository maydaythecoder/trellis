# Upstream plan, 2026-10-04

colibri (engine and UI), pi, Paperclip and apex-notes, inventoried against TRELLIS
at drain `fcbc1b4`,
with a Tauri desktop interface. Produced by a multi-agent workflow (inventory, a
four-role design pass, adversarial QA with a fix round, and a cross-track completeness
critic); the documents below are its output, committed as written. Every ticket was
then filed through `trellis-tickets new`, so G0's rules gate each one.

| track | story | inventory | team analysis | ADR |
|---|---|---|---|---|
| colibri-runtime | S14 | [inventory-colibri.md](../../inventory/inventory-colibri.md) | [analysis](colibri-runtime-team-analysis.md) | [ADR 0047 draft](colibri-runtime-adr-0047-draft.md); the ADR itself is written by its ticket |
| desktop | S15 | [inventory-colibri-ui.md](../../inventory/inventory-colibri-ui.md) | [analysis](desktop-team-analysis.md) | [ADR 0048](../../adr/0048-a-tauri-desktop-interface.md) (accepted; the [draft](desktop-adr-0048-draft.md) is superseded and numbers its sections differently) |
| pi | S16 | [inventory-pi-20261004.md](../../inventory/inventory-pi-20261004.md) | [analysis](pi-team-analysis.md) | [ADR 0021 amendment draft](pi-adr-0021-amendment.md) |
| paperclip | S17 | [inventory-paperclip-20261004.md](../../inventory/inventory-paperclip-20261004.md) | [analysis](paperclip-team-analysis.md) | [ADR 0021 amendment draft](paperclip-adr-0021-amendment.md) |
| apex | S18 | [inventory-apex-20261004.md](../../inventory/inventory-apex-20261004.md) | [analysis](apex-team-analysis.md) | [ADR 0021 amendment draft](apex-adr-0021-amendment.md) |

## Tickets

### colibri-runtime (S14, #364)
- #279 COL-14: Point trellis model at colibri's own weight tooling, not the catalogue
- #261 COL-02: Add a colibri reader for /v1/models and /health to trellis-model
- #262 COL-04a: Add a one-request tools probe for colibri to trellis-model
- #263 COL-05: Add a colibri request profile to HostedClient: temperature 0, explicit thinking
- #264 COL-06: Record [model] timeout_s as ADR 0047 decision 9's per-request timeout
- #265 COL-06a: Read [model] timeout_s from config, layered as [checks] timeout_s is
- #266 COL-06b: Apply [model] timeout_s to the run and ask model clients
- #274 COL-09: Write docs/runtimes/colibri.md, the serve command and specs runs use
- #281 COL-13a: Turn on colibri streaming in trellis run and show its tokens in the TUI
- #275 COL-16a: Add a chat mode, a key variable and per-input timing to qa/run-classify.py
- #276 COL-16: Measure trellis ask's classifier on colibri before choosing /v1/brio
- #251 COL-01a: Commit ADR 0047 as Proposed with the colibri v1.12.1 inventory
- #252 COL-01: Accept ADR 0047: drive colibri as a tier-1 runtime over openai-chat
- #267 COL-03: Report colibri's served model, slots and memory tiers in trellis doctor
- #253 COL-04c: Answer GET /v1/models with 404 in the trellis-cli loopback test stubs
- #268 COL-04: Identify and probe a loopback colibri endpoint before a run's snapshot
- #269 COL-04b: Report the probed colibri tier in trellis doctor and the TUI status bar
- #270 COL-05a: Turn the colibri profile on in trellis run and trellis ask once identified
- #254 COL-07: Name an openai-chat reply cut at its length limit, as Anthropic's is
- #271 COL-08: Budget an identified colibri endpoint to leave room for the reply
- #255 COL-10a: Read colibri's x-colibri-queue-wait-ms header onto the hosted Reply
- #256 COL-10: Carry output tokens, model time and colibri queue wait on the turn event
- #257 COL-11: Show each turn's tokens per second and queue wait in the TUI budget line
- #258 COL-12a: Record model-call time in run_floor.py and bytes per token without a runtime log
- #277 COL-12: Measure Qwen3.8-Flash-Next on colibri against the model floor at tier 1
- #280 COL-13: Stream replies from a loopback colibri endpoint within the run's wall-clock budget
- #272 COL-15: Log colibri's self-report per run and keep it out of the trail for now
- #259 COL-17: Cite ADR 0013, not ADR 0045, for keeping colibri's segment and edge C ABIs out
- #278 COL-18: Record whether #151 lifts for loopback colibri from COL-12's model-call time
- #260 COL-19: Record why TRELLIS sends colibri no cache_slot and what a slot collision costs
- #273 COL-12b: Give run_floor.py a model timeout flag and a harness wait that covers it

### desktop (S15, #365)
- #297 DESK-19: Build, lint and test the desktop workspace in CI on Windows and Linux
- #285 DESK-01: Accept ADR 0048 placing a Tauri desktop in desktop/ as a trellis rpc client
- #286 DESK-01a: Let verify-tests.sh run trellis-desktop's tests and fail on a dirty desktop/ tree
- #287 DESK-01b: Create the desktop/ workspace with a child module that reads one get_state reply
- #288 DESK-01c: Add the desktop's static page and window icons under desktop/ui and desktop/icons
- #289 DESK-01d: Wrap the desktop core in a Tauri 2 window that shows the get_state reply
- #290 DESK-02: Audit desktop/Cargo.lock in G5 and guard ADR 0048's lines with desktop-guards.sh
- #282 DESK-04: Map root CLAUDE.md, crates/tui-demo and crates/trellis-tickets to areas in trellis-tickets
- #298 DESK-06: Test the desktop's capability, CSP, navigation and new-window limits
- #291 DESK-07: Hold the rpc and log --json fields the desktop reads with tests through the binary
- #299 DESK-08: Add a pure reducer that turns trellis rpc frames into the desktop's run panes
- #300 DESK-09: Build the desktop's view rail and run panes as plain HTML, CSS and ES modules
- #301 DESK-09a: Add the desktop composer: prompt when idle, steer and Stop while a run is in flight
- #302 DESK-09b: Add the desktop's approval dialog, alert banner and trellis messages pane
- #303 DESK-10: Draw the run's action graph as SVG from a layout the desktop core computes
- #304 DESK-11: Add a trails view that shows trellis log --json rows as untrusted text
- #305 DESK-12: Guard model choice in the desktop core so the repository's spec is never the user's
- #306 DESK-12a: Add the desktop's model picker, repository switch and status bar
- #292 DESK-13: Add a read-only get_codebase rpc command returning the module graph as data
- #293 DESK-14: Add a read-only get_radius rpc command with reach and doubt as separate counts
- #307 DESK-17: Add a codebase view over get_codebase with drill-down, pan, zoom and a details panel
- #308 DESK-17a: Add a symbol radius view over get_radius with reach and doubt as two blocks
- #309 DESK-18a: Add a read-only vault notes view to the desktop over vault_search and vault_graph
- #283 DESK-20: Test that an rpc run whose client vanishes still rolls back and writes its trail
- #294 DESK-15: Add a read-only get_queue rpc command listing the queued tasks
- #295 DESK-16: Add read-only vault_search and vault_graph rpc commands
- #310 DESK-03: Add desktop/CLAUDE.md with the workspace's rules and point the root CLAUDE.md at it
- #296 DESK-05: Give the desktop one long-lived trellis rpc child and a fixed three-command IPC surface
- #311 DESK-18: Add a read-only queue view to the desktop with one count per queue status
- #284 DESK-21: Refuse two ADRs with one number, and hold 0046 to 0049 for the four ADRs the plan writes
- #312 DESK-22: Show each turn's output tokens, tokens per second and queue wait in the desktop's run panes
- #313 DESK-23: Show the session's prompt and completion tokens per model in the desktop from get_session_stats
- #314 DESK-24: Record in desktop/README.md why the desktop shows no live token count and no time to first token
- #315 DESK-25: Amend ADR 0021's Apex 15-19 and 38-42 and Paperclip 35-36, 68 and 99-100 rows for the desktop ADR

### pi (S16, #366)
- #316 PI-01: Correct ADR 0021 and ADR 0034 where merged work still reads as landing
- #317 PI-08: Set AI_AGENT=trellis for bash, custom tools, hooks and process agents
- #322 PI-02: Record pi 1.0.2's new capability rows and two row amendments in ADR 0021
- #318 PI-09: Show the session's prompt and completion tokens per model in /status
- #319 PI-10: Add get_session_stats and get_last_assistant_text to trellis rpc
- #323 PI-03: Record pi's built-in MCP client as excluded under #73 in ADR 0021
- #324 PI-04: Record codemode, nested tool calls and tool search as excluded under row 1.7 in ADR 0021
- #325 PI-05: Record classifier, image generation and virtual models as excluded in ADR 0021
- #326 PI-06: Record Anthropic workload identity federation as deferred with cloud signing in ADR 0021
- #327 PI-07: Record user-set sampling parameters as deferred in ADR 0021
- #328 PI-11: Give hooks a read_only fact for each built-in tool call
- #320 PI-02a: Rewrite inventory-pi.md from pi 1.0.2 at upstream commit 200387122ca4
- #329 PI-02b: Record the 39 pi 1.0.2 changes that keep their ADR 0021 disposition
- #321 PI-12: Record pi's unchanged items 4.20 and 8.4 in the ADR 0021 rows that cover them

### paperclip (S17, #367)
- #330 CLIP-01: Update the ADR 0021 and ADR 0026 rows that trellis has outgrown
- #331 CLIP-08: Record in ADR 0026 §1 that one ask_human call may carry up to 8 questions
- #332 CLIP-10: Record in ADR 0021 rows 59–63 that each queued task gets its own worktree in v1.1
- #333 CLIP-11: Name the runtime state queue export leaves behind, paused profiles first
- #347 CLIP-03a: Record in ADR 0026 §1 that a re-run reads a person's notes since its last run
- #348 CLIP-05a: Record in ADR 0026 §7 that a routine may skip a fire when no commit has landed
- #349 CLIP-05: Gate a routine's scheduled fire on commits since its last fire
- #350 CLIP-07: Record in ADR 0021 and ADR 0026 §11 that a process profile may drive an agent over ACP
- #334 CLIP-02a: Add docs/inventory/inventory-paperclip-20261004.md listing PC-101 to PC-144
- #335 CLIP-02: Add the 2026-10-04 Paperclip rows PC-101 to PC-144 to ADR 0021
- #351 CLIP-03: Give a re-run task the notes a person added since its last run
- #352 CLIP-04: Block a task after its third stale-claim release instead of releasing it again
- #336 CLIP-06: Add queue cancel --children to cancel a task and its pending descendants
- #337 CLIP-09: Record in ADR 0026 §4 that a profile may name a vault note as its memory
- #338 CLIP-12: Add queue list --text to filter tasks by a substring of their text or notes
- #339 CLIP-13: Record in ADR 0021 that trellis keeps no persistent conversation per agent profile
- #340 CLIP-14: Amend ADR 0021 rows PC-110 and PC-133 for a premature done and a hosted harness's key
- #341 CLIP-15a: Record in ADR 0026 §1 that a person may block a task until a set time
- #342 CLIP-15: Wake a task blocked until a set time at the first heartbeat after it
- #343 CLIP-16: Record in ADR 0021 that PC-127 folders are a desktop view of routines by label
- #344 CLIP-17: Record a comparability key in floor reports and refuse to compare mismatched ones
- #345 CLIP-18a: Record in ADR 0026 §1 that a queued task may name its own model
- #346 CLIP-18: Add queue add --model to run one task on a model other than its profile's

### apex (S18, #368)
- #353 APEX-01: Refuse symlinked notes/, manifest.json and layout.json in trellis-vault as Apex does
- #359 APEX-03: Record the 2026-10-04 Apex re-inventory in ADR 0021, ADR 0022 and inventory-apex.md
- #360 APEX-07: Hold Apex's SKILL.md loading as a trellis skill and amend ADR 0021 row 35
- #361 APEX-08b: Refuse model writes into the configured vault's notes root during runs
- #354 APEX-02: Keep a vault edit successful when dates.json cannot follow it
- #355 APEX-04: Port Apex's search-result ranking layer into trellis-vault search
- #362 APEX-05: Rank `trellis vault search` with the ported Apex search-result layer
- #356 APEX-08: Record in a new ADR that model writes stay out of the configured vault
- #357 APEX-08a: Add a governance refusal for the vault's notes root that names [context] vault
- #363 APEX-08c: Record the enforced vault refusal in ADR 0022 and the CLAUDE.md invariants
- #358 APEX-08d: Test that a bash command's carry-back cannot write into the vault's notes root

## Decisions taken under the owner's 2026-10-04 delegation

The owner delegated every open decision in this plan ("ready for completion without
decisions"). Each was the option its ticket recommended, recorded in the ticket's
`## Decision` section as the session's choice under the owner's 2026-10-04 delegation, with
what would reverse it. Any one can be reversed by editing that ticket.

- #252 (colibri-runtime): (a) Reach colibri through the existing openai-chat:<base>/v1#<id> or provider: specs, identified by GET /v1/models owned_by "colibri"; no colibri: scheme. ADR 0047 is accepted with status 'Accepted (owner-directed), under the owner's delegation of 2026-10-04.' ADR 0024 §2/§4/§5 and ADR 0028 are amended for identified colibri only. The accepted ADR names the ticket recording each delegated answer and drops the 'asks the owner' wording. Reverses if: A reported run in which a server that is not colibri answered owned_by "colibri" and the profile it then received (temperature 0, explicit thinking, tools probe, colibri byte budget) changed the run's outcome; then (b), a user-written colibri: scheme, replaces identification.
- #264 (colibri-runtime): (a) [model] timeout_s: one key for every runtime, default 300 s. The lower of the repository's and the user's value wins, as [checks] timeout_s does, and 0 is refused. Recorded as '**Delegated answer (timeout):** (a), 2026-10-04,' in ADR 0047 decision 9; COL-06a and COL-06b build it as written. Reverses if: COL-18 records (a) and the stream's idle timeout is shown to end a hung request on every runtime the key serves, leaving a fixed per-request value with nothing to do; or a repository-set value is shown to hold a run past the wall-clock budget the user set.
- #271 (colibri-runtime): (b) 25,088 bytes ((8,192 - 1,024) x 3.5) for an identified colibri endpoint with no [model] context_bytes, keeping a 1,024-token reply reserve. COL-09's guide asks for --ctx 8192 or more and doctor says so; an explicit value still wins. Recorded as '**Delegated answer (byte budget):** (b), 2026-10-04,' in ADR 0047 decision 6. Reverses if: COL-12's bytes_per_token.py --no-runtime-log figure for colibri comes in below 3.5 bytes per token (the constant is then recomputed from the measured ratio), or colibri reports its context window over HTTP at a commit after ce370e8.
- #277 (colibri-runtime): (a) Qwen3.8-Flash-Next on colibri 1.12.1: native tools, one KV slot, 8,192-token default window. It runs on a machine with at least 24 GB RAM and 200 GB free fast disk. The ticket keeps !blocked-ext until such a machine is available; the build machine has 15.2 GB RAM. Done when and Verify name Qwen3.8-Flash-Next and record the weights' Qwen Community License. Reverses if: colibri publishes, after ce370e8, a native-tools family with a memory floor below 24 GB or a measured decode rate above Qwen3.8-Flash-Next's on the same class of machine; or a run on a machine that meets the floor ends every task budget-exhausted at its wall-clock budget.
- #280 (colibri-runtime): Follows COL-18's rule: (a) when COL-12's model_ms_p90 >= 300,000 ms, (b) below. On (a) it is built as written: loopback colibri and openai-chat only, an idle timeout reset by keepalives, the 32 MiB MAX_RESPONSE cap kept on the stream, and at least 8 colibri_stream_ tests. On (b) it and COL-13a are closed unbuilt. (c) is rejected. COL-18 is added to blocked_by, and the first Verify line requires the recorded (a). Reverses if: A later colibri floor run whose model_ms_p90 lands on the other side of 300,000 ms.
- #279 (colibri-runtime): (a) trellis model pull does not fetch colibri containers. The catalogue stays one pinned GGUF file per entry (ADR 0044, Apache-2.0/MIT only). trellis model prints one line pointing at coli convert / coli mirror verify and docs/runtimes/colibri.md; ADR 0047 decision 11 records it. Reverses if: colibri publishes, after ce370e8, pre-converted containers as single files at a pinned revision with one SHA-256 each under Apache-2.0 or MIT; or ADR 0044 is amended to allow multi-file entries.
- #272 (colibri-runtime): (b) Log colibri's self-report as one INFO 'runtime identified' tracing event (served id, kv_slots and tier counts when /health gave them). The trail keeps the spec string and ADR 0007's format is unchanged; ADR 0047 decision 13 records it. Reverses if: colibri reports its resource policy (load-time precision, routing, KV modes) over HTTP at a commit after ce370e8; then (a), an optional self-reported runtime object in the trail's factual section with ADR 0007 amended.
- #276 (colibri-runtime): (c) Measure first, with the pick fixed now. The report reads 'Classifier on colibri: generation' (b) when colibri's --chat score is at or above the local baseline's at the same commit, and 'Classifier on colibri: /v1/brio' (a) when below. Time per input is recorded but does not switch it, because ask classifies and answers in one call. A /v1/brio outcome gets a follow-up ticket; its entropy threshold is the lowest entropy among brio's wrong classifications on the same cases. Reverses if: A change to ask.rs that separates classifying from answering, so time per input would count and the rule would gain a time term; or a later re-run whose colibri score crosses the baseline's in the other direction.
- #278 (colibri-runtime): Rule fixed now: (a) lift #151 for loopback colibri over openai-chat only when COL-12's model_ms_p90 >= 300,000 ms (the [model] timeout_s default); (b) #151 stands for colibri when below. (c) is rejected. Recorded as '**Delegated answer (streaming):** (a|b), <date>' with the p90 and threshold in ADR 0047 decision 10, plus a dated ADR 0024 'Not built' amendment ('#151 is lifted for loopback colibri only' or '#151 stands for colibri'). Reverses if: A later colibri floor run whose model_ms_p90 lands on the other side of 300,000 ms, or a change to the 300 s default of [model] timeout_s, which moves the threshold.
- #285 (desktop): Option A: the Tauri desktop lives in this repository as desktop/, under ADR 0048 (numbered by DESK-21, since 0046 is #239's and 0047 colibri's), Accepted (owner-directed). The ADR also records DESK-02's deny policy (§7), the view-model computed in Rust (§5), and the read-only rpc commands of DESK-13 to DESK-16 (§10). DESK-01 now waits on DESK-21, and its Verify runs scripts/checks/adr-numbers.sh. Reverses if: The owner reaffirms #70's exclusion of a desktop inside this repository: ADR 0048 is then superseded, ADR 0021's amendment says so, and the track moves to a separate repository as in B.
- #290 (desktop): Option A, with RUSTSEC-2024-0429 named. desktop/deny.toml may hold per-crate licence exceptions for MPL-2.0, Unicode-DFS-2016, CC0-1.0 and 0BSD, an ignore for each unmaintained advisory, and an ignore for RUSTSEC-2024-0429 (glib 0.18, unsound), which Tauri's gtk ^0.18 pins on Linux. No vulnerability is ever ignored: there is no audit.toml, and the known quick-xml and time vulnerabilities are cleared by a lock resolved now (plist 1.10.1, time 0.3.55). Any other finding gets no entry: the ticket stays open, !blocked-ext where an upstream release would clear it, and the finding is filed. The root deny.toml is untouched. Reverses if: Ignoring a vulnerability in Tauri's tree becomes the only way G5 can pass, or a second unsound advisory that no lock update clears is reported; the owner is asked then.
- #282 (desktop): Option A for the three paths still unmapped: crates/tui-demo to area:tui, crates/trellis-tickets to area:build, root CLAUDE.md to area:docs. A's desktop half (desktop to area:tui) already landed in #249, so the ticket shrinks to three paths (AREA_PATHS 22 to 25) and no longer waits on DESK-01. Reverses if: The owner adds an area value for tooling or for interfaces, or moves the desktop off area:tui; crates/tui-demo then follows the desktop.
- #292 (desktop): Option A: a read-only get_codebase rpc command, recorded as owner-directed in ADR 0048 §10 and in a dated ADR 0025 §5 amendment. No --json is added to trellis index --graph, so ADR 0025 §4's --json list and ADR 0048 §2's three spawns stay as they are. Reverses if: The owner opens ADR 0025 §4's --json list to one-shot read commands; the graph would then be served as trellis index --graph --json, and get_codebase dropped from ADR 0048 §10.
- #293 (desktop): Option A: a read-only get_radius rpc command with reach and doubt as separate objects, recorded as owner-directed in ADR 0048 §10 and in a dated ADR 0025 §5 amendment. CLAUDE.md rule 3 is held by trellis's own test through the binary. Reverses if: The owner opens ADR 0025 §4's --json list to one-shot read commands; the radius would then be served as trellis radius <symbol> --json with the same test, and get_radius dropped from ADR 0048 §10.
- #294 (desktop): Option A: a read-only get_queue rpc command, recorded as owner-directed in ADR 0048 §10 and in a dated ADR 0025 §5 amendment. The queue's writes stay with trellis queue and trellis work (ADR 0020 §5). Reverses if: The owner opens ADR 0025 §4's --json list to one-shot read commands; the queue would then be served as trellis queue list --json, and get_queue dropped from ADR 0048 §10.
- #295 (desktop): Option A: read-only vault_search and vault_graph rpc commands, recorded as owner-directed in ADR 0048 §10 and in a dated ADR 0025 §5 amendment. vault_search takes its hits from the one function trellis vault search prints from, so APEX-05's ranking applies to both whichever lands first; no cross-track blocker on APEX-05 is added. Reverses if: The owner opens ADR 0025 §4's --json list to one-shot read commands; the vault reads would then be served as --json on trellis vault search and trellis vault graph, and these commands dropped from ADR 0048 §10.
- #322 (pi): Option A: record all 16 new rows (1.16, 2.20, 2.21, 4.21, 4.22, 8.9, 9.7, 10.3, 11.9-11.12, 12.3, 14.4, 15.5, 15.6), the dispositions for 4.11 and 4.15, and the dated amendments to rows 2.1-2.4 and 14 as written, in a section whose opening marks them Accepted (owner-directed) under the 2026-10-04 delegation. Reverses if: The owner overrides a row (ADR 0021 Context), which then gets its own ticket and a dated amendment; or PI-09 is reversed, and row 14.4 then reads deferred.
- #318 (pi): Option A: amend ADR 0025 §6 (Accepted, owner-directed) and print per-model prompt and completion tokens in trellis session /status from one session_extra.rs totals function, never summed, unknown figures shown as unknown, model names printed through plain, no dollars. Reverses if: The owner prefers per-run figures in trellis log (option B) or deferral (option C); either rewrites this ticket, turns PI-02's row 14.4 to deferred, and rewrites or drops PI-10's get_session_stats.
- #319 (pi): Option A: add the read-only rpc commands get_session_stats and get_last_assistant_text, record the other six of pi's unanswered commands as not built or deferred with reasons in ADR 0025 §5's 33-command table, and amend ADR 0021 row 9.4-9.5, both marked Accepted (owner-directed). Reverses if: PI-09 is reversed (get_session_stats loses its totals function and is dropped or rewritten), or the owner wants set_thinking_level over rpc (option B), a new run input.
- #323 (pi): Option A: keep MCP excluded as #73 answered and record row 16.1-16.7 MCP client in ADR 0021, citing #73, ADR 0010/#64 for OAuth credentials, brief Part 1 for HTTP servers, and a [[tools.custom]] command as the supported route to one stdio server. Reverses if: The owner reopens #73: stdio-only MCP under trust (option B, a v2.0 brief line plus an ADR amending ADR 0021 Paperclip 72-75 and ADR 0023) or with streamable HTTP too (option C, superseding brief Part 1 for MCP).
- #324 (pi): Option A: keep codemode, nested tool calls, structured tool results, exposure modes and tool search excluded under row 1.7 (#63) and the fixed tool set; amend row 1.7 and add rows 4.23, 4.24 and 17.1-17.6, each Accepted (owner-directed). Reverses if: The owner reverses #63 (option C: an ADR superseding ADR 0005's one-call-per-turn shape with per-call ids and serialized writes, then codemode), or PI-03 is reversed and a repository's tool list outgrows the prompt (option B: deferred exposure with tool_search for custom tools, amending ADR 0023).
- #325 (pi): Option A: keep classifier models, image generation models and virtual models excluded; note in row 2.14 that they moved to rows 2.17 and 2.18, and add rows 2.17, 2.18 (citing #66) and 2.19 (citing ADR 0043), each Accepted (owner-directed). Reverses if: The owner wants fixed-rule routing between configured models recorded in the trail (option B, amending ADR 0043), or reverses #66 so a classifier picks the route (option C, also amending row 2.14).
- #326 (pi): Option A: defer Anthropic workload identity federation with cloud signing on #45's terms (credentials from the environment or the platform's own chain, the exchange tested against recorded vectors and a loopback stub, the token held in memory only) and record row 2.15 as Accepted (owner-directed). Reverses if: A hosted run reaches a real provider (#8) and the owner wants federation built (option B, an ADR 0024 §3 amendment with tests in hosted_keys.rs and child_env.rs), or the owner treats the token file as a credential file trellis reads (option C, excluded).
- #327 (pi): Option A: keep fixed decoding (temperature 0.0 to local runtimes, none to hosted models) and record row 2.16 sampling parameters as deferred, citing ADR 0028's metric-5 waiver, Accepted (owner-directed). Reverses if: A task measured at ADR 0028's bar of five repetitions shows a gain at another temperature or top_p; then option B (per-model temperature and top_p under the user's [[providers]], recorded in the trail, an ADR 0024 amendment and an ADR 0007 trail field).
- #328 (pi): Option A: one read-only table in gate.rs covering every built-in, and a read_only key added in hooks.rs to the pre_tool and post_tool payloads for built-ins only; custom tools and registry::call_json are unchanged. Recorded in an ADR 0023 §5 amendment and ADR 0021 row 13.5, both Accepted (owner-directed). Reverses if: The owner wants custom tools to declare read_only = true under trust (option B, a config schema change with ADR 0032 carry-back discard), or no payload key at all (option C, row 13.5 built in effect through [control] approve).
- #335 (paperclip): Option (a): PC-101 to PC-144 go into ADR 0021 as written. The section's status line reads 'Accepted (owner-directed), 2026-10-04: the session's choice under the owner's 2026-10-04 delegation'. A new Verify line checks for that exact status-line form. The seven 'S17 question:' sentences stay for CLIP-03a/04/05a/06/07/09/12 to replace. Rows PC-101, PC-114, PC-127 and PC-138 are amended after this lands by CLIP-13, CLIP-15a, CLIP-16 and CLIP-18a. Reverses if: The owner names a row to change before CLIP-02 lands (option b). That row is then rewritten in CLIP-02 first.
- #331 (paperclip): Option (b): one ask_human call carries up to 8 questions, each with up to 8 options. They are stored as one model note and one 'question' blocked reason that holds the list. One person's note answers them all. A task file in today's single-question form still loads, as a list of one. The 200-option checkbox form is not built. This ticket only writes the ADR. FOLLOW-UP NEEDED: a build ticket for the multi-question ask_human (the tool and the queue reason), filed after CLIP-08 closes. Reverses if: The build finds that a list of questions cannot be quoted and fenced as safely as one question is today. Then (a), one question per call.
- #332 (paperclip): Option (b): a worktree per queued task is v1.1 scope. ADR 0021 rows 59–63 record the rules the build follows: branch trellis/task/<id>, made the way ADR 0037 makes a partition's worktree; an unconverged task's worktree is removed and a converged one stays for the person; work goes on past a converged task; the person merges and trellis never pushes; the queue stays in the git dir of the worktree work started from; each worktree is clean before its run. FOLLOW-UP NEEDED: a build ticket blocked by #98–#101, whose own ADR replaces ADR 0020 §5's 'stop at the first converged change' for task worktrees. Reverses if: #98–#101 do not land, or the build's ADR finds that a run in a task worktree cannot keep ADR 0009's snapshot and rollback. Then (a), deferred with no tracking issue.
- #347 (paperclip): Option (a): keep #57's mention wakes, and give every run the notes a person added since its last run, using a notes_seen marker set only by Claim::finish and Claim::ask. The amendment states how far #189's limit now reaches. Rows 25 and 41 are unchanged. CLIP-03, already written for (a), builds it. Reverses if: A run is found writing a human note with an id by any route other than the deliberate one #189 names. Then (c): a person's notes reach a run only after a question.
- #348 (paperclip): Option (b): activity_gate = "commits", with last_head in the routine state. A fire is quiet when no commit outside .trellis/runs/ has landed since the last enqueue. A last_head that is not a full hex id is treated as absent. Any git error fires rather than skips. 'routines run' is not gated. CLIP-05, already written for (b), builds it. Reverses if: Gated routines are found skipping fires whose new work arrives without a commit. Those routines leave the key unset; (a) returns only if no routine uses the gate.
- #350 (paperclip): Option (b): a process profile may speak the Agent Client Protocol, with trellis as the client. The bounds are recorded now as Accepted in ADR 0026 §11: protocol = "acp"; the agent runs in the throwaway clone with a scrubbed environment plus pass_env only; trellis offers no fs or terminal client capability, and a request for one is refused and recorded; permission requests go to [control]; messages and tool calls go into the trail as untrusted data; writes come back through carry-back; only checks converge a run. The per-agent adapters stay excluded. FOLLOW-UP NEEDED: a build ticket plus a new ADR that maps ACP messages inside these bounds. Take the next free number; 0046 to 0049 are held (rollback clock, colibri, desktop, vault writes; scripts/checks/adr-numbers.sh). Reverses if: The build finds an ACP agent that needs fs or terminal client capabilities, or the permission flow cannot be held to [control]. That agent then stays on argv and stdin; if no agent fits the bounds, (a).
- #352 (paperclip): Option (b): after 3 stale-claim releases (MAX_STALE_RELEASES), a dead holder blocks the task with reason manual, logged as stale_claim_blocked. failed/ is not touched. The ADR 0020 §5 amendment is opened as Accepted (owner-directed). The body was already written for (b); the sentences about the other options were removed. Reverses if: Tasks are often blocked this way when their holders died for reasons outside the task, such as host restarts. Raise the limit by an ADR 0020 §5 amendment; (a) returns only if no limit fits.
- #336 (paperclip): Option (b): queue cancel --children, best-effort against claims made while it runs, and no subtree pause. The ADR 0026 §3 amendment is opened as Accepted (owner-directed). The sentences about the other options were removed. Reverses if: A person needs to stop a subtree and resume it later. Then (c)'s pause becomes its own ticket with its own ADR; CLIP-06 stands.
- #337 (paperclip): Option (c): a profile may set memory = "<vault stem>", a vault note compiled as standing context for every run of the profile, as --focus is, and edited through the normal write path. The vault dir is already checked through resolve_repo_input. This ticket only writes the ADR. FOLLOW-UP NEEDED: a build ticket for the profile's memory field. Reverses if: A standing note cannot be added beside --focus without changing the compiled bytes for runs that set no memory. Then (b), a plain repository file read through resolve_repo_input.
- #338 (paperclip): Option (b): queue list --text, a case-insensitive substring match over a task's id, text, labels and notes. The ADR 0026 §3 amendment is opened as Accepted (owner-directed). The sentences about the other options were removed. Reverses if: Backlogs grow until a substring match lists too much to read. Then (c), BM25 over task text, as its own ticket; --text stays.
- #339 (paperclip): Option A: no persistent conversation per agent profile. The per-profile conversation in PC-101 is recorded as excluded, in place of the deferral CLIP-02 writes, with the fresh-run rule and #141 as reasons. The session stays the conversation. Verify now checks for the row's 'Amended ... excluded' text. Reverses if: pi's session tree (rows 8.1–8.4) is built as v2 memory work. Then B: a per-profile transcript, still with no task filing.
- #341 (paperclip): Option A: a person sets an `until` wait with queue set --reason until --at <RFC 3339>, and Queue::reconcile wakes the task once, at the first heartbeat after that time. No model tool sets it. This amendment is the ADR for the --at flag. ORDERING CHANGE: the ticket now also amends ADR 0021 row PC-114 (docs/adr/0021 added to Files), so blocked_by_keys is now [CLIP-02]. The opener and Verify say 'owner-directed', not 'owner decision'. Reverses if: Routines and work --every turn out to cover every wait people set. Then C: PC-114 stays excluded and CLIP-15 is closed unbuilt.
- #342 (paperclip): Builds option A as CLIP-15a records it. docs/adr/0026 was removed from Files, with its §3 bullet and Verify line, because CLIP-15a's §1 amendment is the flag's ADR. Without the change the ticket spanned three areas (agents, cli, docs), which makes triage route it to needs-human; it now spans two. Reverses if: CLIP-15a is reversed to C. Then CLIP-15 is closed unbuilt.
- #343 (paperclip): Option B: PC-127 is recorded desktop-only. The desktop groups routines by their [task] labels, and trellis's routine files and CLI do not change. The amendment says plainly that the view needs a read-only routines listing over rpc (a protocol addition with its own ADR) and a desktop ticket, and that neither exists yet. FOLLOW-UP NEEDED IN THE DESKTOP TRACK: add a ticket for a read-only routines rpc listing (with its ADR, like get_queue) and a routines-by-label view that shows routine text as text, never HTML. Reverses if: The desktop's ADR (ADR 0048, DESK-01) is not accepted, or the desktop plan declines a routines view. Then A: excluded [web-ui].
- #345 (paperclip): Option A: a model per task via queue add --model, used over the assignee profile's model and work --model. The task's model is held to ADR 0024 §3's key rule as the repository's choice. No per-task thinking level and no generated titles. This amendment is the ADR for the flag. ORDERING CHANGE: the ticket now also amends ADR 0021 row PC-138 (docs/adr/0021 added to Files), so blocked_by_keys is now [CLIP-02]. The opener and Verify say 'owner-directed'. Reverses if: The key rule cannot be held for task files that arrive through queue import. Then C, a profile per model, and CLIP-18 is closed unbuilt.
- #346 (paperclip): Builds option A as CLIP-18a records it. docs/adr/0026 was removed from Files, with its §3 bullet and Verify line, because CLIP-18a's §1 amendment is the flag's ADR. The ticket now spans two areas (agents, cli) instead of three. Reverses if: CLIP-18a is reversed to C. Then CLIP-18 is closed unbuilt.
- #360 (apex): Option A, built in effect: Apex's own SKILL.md loads unchanged when copied into .trellis/skills/ or installed as a package (ADR 0039 §10), and trellis carries no skill in its binary. ADR 0021 row 35-37's re-inventory amendment gains 'The writing skill is built in effect (owner-directed, 2026-10-04)', naming the two tests that hold it. ADR 0019 and ADR 0022 §6 stay intact. This is the session's choice under the owner's 2026-10-04 delegation. Reverses if: The owner asks for the skill to ship inside the binary (then through an ADR 0019 amendment), or Apex's SKILL.md at a later ref no longer loads through SkillCatalog, in which case row 35 is amended to not built with ADR 0022 §6's reason.
- #361 (apex): Build it: runs and process agents add the configured vault's notes root to their write refusals through the governance ticket's refusal, a run whose vault's notes root is the repository root stops before the snapshot, and the notes.rs module comment cites the new ADR. This is the session's choice under the owner's 2026-10-04 delegation, following APEX-08's option A. Reverses if: APEX-08's choice is reversed, to the opt-in deny_write form or to withdrawing #30's 'vault notes stay human-authored'. This ticket then closes unbuilt and runs keep their write refusals as they are.
- #355 (apex): Option B: port searchResults.ts into trellis_vault::search beside Index::search, which stays unchanged, and rank only `trellis vault search` with it. search_notes keeps the index ranking of ADR 0024 §6, so the model's 'no note matched' is not turned into weak subsequence hits. No new dependency, and trellis-vault stays free of trellis-model. This is the session's choice under the owner's 2026-10-04 delegation. Reverses if: The owner wants search_notes ranked by the layer too (then a further ticket switches it and amends ADR 0024 §6), or rules the layer UI-only, in which case ADR 0021 row 52 is re-dispositioned excluded [ui] and APEX-04 and APEX-05 close unbuilt.
- #362 (apex): Build it: `trellis vault search` prints the ported layer's results with its arguments, default limit of 12 and reduced-mode lines unchanged. ADR 0022 §4 and ADR 0021 row 52 are amended to built and record that the porting question was answered under the owner's 2026-10-04 delegation. search_notes is untouched, held by a_search_names_matching_notes_and_says_what_they_are passing unmodified. This is the session's choice under that delegation. Reverses if: APEX-04's choice is reversed to leaving search as built. This ticket then closes unbuilt and row 52 is re-dispositioned excluded [ui]. A reversal to also ranking search_notes leaves this ticket unchanged.
- #356 (apex): Option A, structurally: a new ADR (0049, held for it by scripts/checks/adr-numbers.sh) records that, when [context] vault is set, every writer trellis judges (edit, write_file, bash, custom tools, hooks and process agents, the last four through ADR 0032's carry-back) is refused at the vault's notes root with a reason naming [context] vault, and that a vault whose notes root is the repository root stops the run before the snapshot. Status: 'Accepted (owner-directed). Not built: the enforcement is landing'. This extends, and does not loosen, the CLAUDE.md invariant 'Model writes never land in .trellis/ or .git/'. This is the session's choice under the owner's 2026-10-04 delegation. Reverses if: The owner withdraws #30's 'vault notes stay human-authored' or prefers the opt-in deny_write form. The new ADR is then superseded, ADR 0022 §3 and the notes.rs comment are corrected instead, and APEX-08a/b/c/d close unbuilt.
- #357 (apex): Build it: Governance::with_vault and a Refusal enum (Rule or Vault) returned by denies_write_at, so every place that words a write refusal names [context] vault for the vault and words a deny_write rule byte for byte as today. This is the session's choice under the owner's 2026-10-04 delegation, following APEX-08's option A. Reverses if: APEX-08's choice is reversed, to opt-in deny_write or to withdrawing #30's sentence. This ticket then closes unbuilt and nothing adds the refusal.
- #363 (apex): Build it once APEX-08b and APEX-08d land: ADR 0022 §3 gets a note that the refusal is 'enforced for every writer the run judges', the CLAUDE.md invariant 'Model writes never land in .trellis/ or .git/' also names the configured vault's notes root and its vault_notes_root_ tests (an extension, not a loosening), and the new ADR's status moves to 'Accepted (owner-directed). Built'. This is the session's choice under the owner's 2026-10-04 delegation. Reverses if: APEX-08's choice is reversed, to opt-in deny_write or to withdrawing #30's sentence. This ticket then closes unbuilt, and ADR 0022 §3 is amended by the change that reversal files instead.
- #358 (apex): Build it: an isolate.rs test, modelled on allow_write_is_enforced_where_a_command_s_clone_is_carried_back, in which a bash command overwriting vault/a.md in its clone is refused with output naming [context] vault = "vault", leaving the file byte-identical and touched empty. This is the session's choice under the owner's 2026-10-04 delegation, following APEX-08's option A. Reverses if: APEX-08a closes unbuilt because APEX-08's choice was reversed. This ticket then closes unbuilt too.

Questions kept for the owner because answering would loosen a CLAUDE.md rule: 0.


## Not covered by a ticket, with the reason

### colibri-runtime
- COL-ENG-02 (launcher dispatch): colibri resolves the family and engine binary itself; the TRELLIS spec differs between families only in the model id.
- COL-ENG-04 (coli run): one-shot raw generation; `trellis run` is the agent loop (BRIEF 4.12), and TRELLIS reaches colibri only through its HTTP server.
- COL-ENG-06 (coli web): desktop and web surfaces are excluded by brief Part 1 (#70); if the desktop track's ADR 0048 is accepted, the dashboard belongs to that track's tickets.
- COL-ENG-07 (coli stop): TRELLIS supervises no runtime process for any dialect (crates/trellis-model/src/runtime.rs only checks reachability); colibri's pidfile and `coli stop` manage the server.
- COL-ENG-08 (coli info): model geometry is printed by colibri's CLI and is not exposed over HTTP; trellis doctor reports what `/health` exposes (COL-03).
- COL-ENG-09 (coli build, setup.sh, make targets): TRELLIS takes on no C toolchain dependency (ADR 0013); colibri is built by its operator.
- COL-ENG-10 (coli plan, resource planner): a colibri-side placement planner; TRELLIS sees the resulting tiers through `/health` (COL-03) and does not configure placement.
- COL-ENG-12 (coli tune): a colibri-side execution-profile measurement that never changes output; TRELLIS sees its effect only as wall-clock time.
- COL-ENG-15 (coli bench): model-quality benchmarks for GLM; TRELLIS measures its own floor on repository tasks (COL-12).
- COL-ENG-16 (local cluster mode): a networked colibri-side deployment (BRIEF Part 1 excludes networked components from TRELLIS); to TRELLIS a clustered colibri is the same HTTP endpoint.
- COL-ENG-17 (openai_server.py standalone flags): the same gateway `coli serve` starts; COL-09 documents `coli serve`.
- COL-ENG-25 (stop sequences): TRELLIS sends none and needs none; GLM's implicit role stops are applied on the server.
- COL-ENG-26 (assistant-turn continuation): TRELLIS transcripts never end on an assistant message, so continuation is never triggered.
- COL-ENG-35 (GET /metrics): its server-wide Prometheus counters cannot be attributed to one run on a shared server, and TRELLIS exposes no endpoint; TRELLIS reads per-request usage and queue wait instead (COL-10), and operators scrape /metrics directly.
- COL-ENG-36 (GET /experts): routing telemetry for colibri's Brain view; desktop surface excluded by #70 (belongs to the desktop track's ADR 0048 if that is accepted).
- COL-ENG-38 (static dashboard hosting): desktop and web surface excluded by #70.
- COL-ENG-41 (engine-gateway mux protocol): colibri's internal stdin/stdout pipe; TRELLIS speaks HTTP only (ADR 0013), and ECHO's top-k logprobs are not a token-mask hook, so it cannot carry tier 3.
- COL-ENG-42 (legacy SERVE protocol): private to `coli chat`; TRELLIS speaks HTTP only.
- COL-ENG-43 (gateway engine lifecycle): happens inside colibri's server process; TRELLIS supervises no runtime.
- COL-ENG-59 (entropy-coded expert tiers): a lossless storage format inside the engine; TRELLIS neither sees nor configures it.
- COL-ENG-60 (GGUF reader, OLMoE converter): colibri-side conversion; OLMoE's 4,096-token maximum context and missing tools make it unusable for TRELLIS, and COL-04 refuses it.
- COL-ENG-62 (learning cache and pinning): engine-internal expert placement that affects only wall-clock time.
- COL-ENG-63 (live re-pin): engine-internal expert placement that affects only wall-clock time.
- COL-ENG-64 (router-lookahead prefetch): engine-internal I/O scheduling that affects only wall-clock time.
- COL-ENG-65 (expert I/O pipeline): engine-internal I/O that affects only wall-clock time.
- COL-ENG-66 (multi-disk mirror and split): the operator's placement and I/O choice; it affects only wall-clock time.
- COL-ENG-67 (NUMA and OpenMP tuning): engine-internal threading that affects only wall-clock time.
- COL-ENG-78 (correctness oracles): colibri's own engine tests; TRELLIS verdicts come only from the user's repository checks (ADR 0004).
- COL-ENG-79 (MTP drafts): drafts are verified by the target model, so greedy output does not change; it is a speed setting, recorded as part of the serve command in COL-12.
- COL-ENG-80 (n-gram and corpus drafts): verified drafts, so a speed setting only, recorded as part of the serve command in COL-12.
- COL-ENG-81 (DSpark drafter): verified drafts, so a speed setting only, recorded as part of the serve command in COL-12.
- COL-ENG-83 (audio input): TRELLIS has no audio input, and Inkling, the only family with audio, has no tools, so tier 1 is unavailable.
- COL-ENG-85 (expert atlas): data for colibri's Brain view; desktop surface excluded by #70.
- COL-ENG-86 (routing and placement research tools): colibri-side research tooling with no TRELLIS role.
- COL-ENG-87 (dashboard pages): desktop and web surface excluded by #70 (belongs to the desktop track's ADR 0048 if that is accepted).
- COL-ENG-91 (HTTP serving benchmark): a colibri-side serving benchmark; TRELLIS takes its batching figure from `/health` kv_slots (COL-03).
- COL-ENG-92 (micro and system benchmarks): colibri-side kernel and I/O benchmarks with no TRELLIS role.
- COL-ENG-93 (published benchmark tables): colibri's own figures; COL-12 cites the datapoints it compares against, never as TRELLIS measurements.
- COL-ENG-94 (CUDA backend): a colibri build and runtime choice; TRELLIS needs no GPU toolkit (ADR 0013), and COL-12 records the backend used.
- COL-ENG-95 (Windows CUDA DLL, DeepSeek V4 CUDA tier): a colibri build and runtime choice; TRELLIS needs no GPU toolkit (ADR 0013).
- COL-ENG-96 (HIP/ROCm backend): a colibri build and runtime choice; TRELLIS needs no GPU toolkit (ADR 0013).
- COL-ENG-97 (Metal backend): a colibri build and runtime choice; TRELLIS needs no GPU toolkit (ADR 0013).
- COL-ENG-98 (Vulkan backend): a colibri build and runtime choice; TRELLIS needs no GPU toolkit (ADR 0013).
- COL-ENG-99 (layer-segment runtime ABI): relevant only if tier 3 were reopened; ADR 0013 keeps tier 3 unbuilt with candle preferred, and linking a C ABI would add a C toolchain (rejected in ADR 0047).
- COL-ENG-100 (model-edge runtime ABI): version 1 exposes greedy selection only, with no logit-mask hook, and linking it would add a C toolchain (rejected in ADR 0047).
- COL-ENG-101 (GitHub release archives): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-102 (Docker images and compose): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-103 (Nix flake): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-104 (pip package colibri-engine): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-105 (make install layout): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-106 (Windows-native distribution): colibri ships itself; TRELLIS ships only itself.
- COL-ENG-107 (CI matrix): colibri's CI tests colibri; TRELLIS CI covers only TRELLIS.

### desktop
- COL-UI-02 Rail brand button: a brand mark only; cosmetic, kept out as ADR 0021 #74 keeps cosmetics out. The window title names TRELLIS (DESK-01).
- COL-UI-05 Navigation dock: a hover and touch affordance for phone-width web pages. The desktop has a fixed rail (DESK-09) and no touch target, so the dock's behaviour is not ported.
- COL-UI-10 API key field: excluded by ADR 0010 and ADR 0024 §3 (keys come from the environment only) and by ADR 0048 §8. The desktop has no key field and never handles a key.
- COL-UI-11 Probe server: rpc has no probe command and the desktop never calls a runtime. Probing stays with `trellis doctor` and session /model; fetching /v1/models belongs to the colibri-runtime track.
- COL-UI-13 Auto-connect when served by the engine: does not apply. The runtime does not serve TRELLIS, and the desktop spawns trellis locally.
- COL-UI-15 KV session (slot) selector: drive-via-runtime (cache_slot on requests to colibri). This belongs to the colibri-runtime track, not to a desktop view.
- COL-UI-16 Slot indicators: depend on COL-UI-15, which is not built.
- COL-UI-17 Temperature slider: owner-decision row. TRELLIS fixes temperature in code (runtime.rs, hosted.rs), rpc has no setting for it, and the desktop adds none.
- COL-UI-18 Max output tokens: the per-reply cap is automatic and rpc carries no per-reply setting. A port would land in config or the CLI first.
- COL-UI-19 Reasoning toggle: `[model] thinking` is config-level, and ADR 0025 §5 leaves rpc's thinking commands unbuilt. Sending enable_thinking is drive-via-runtime.
- COL-UI-21 Hardware panel: engine-reported hwinfo; drive-via-runtime and colibri-specific.
- COL-UI-23 Expert tier bar: engine-specific weight residency. TRELLIS loads no weights itself (ADR 0013).
- COL-UI-25 Health polling: the desktop polls no runtime; rpc pushes frames as they happen.
- COL-UI-26 Empty-state wordmark: cosmetic brand.
- COL-UI-29 Reasoning stream and block: trellis does not parse reasoning_content, and it is not in the rpc stream.
- COL-UI-30 Copy response: page text can be selected and copied natively, and session /copy already covers the last summary. A copy button adds nothing a TRELLIS view needs.
- COL-UI-31 Regenerate: TRELLIS has no retry-of-a-turn concept. A failed run rolls back, and a new prompt is a new run.
- COL-UI-32 Continue reply: drive-via-runtime (prefill continuation); not in trellis.
- COL-UI-34 Truncation warning: no per-reply finish signal reaches the rpc stream. A run's max_tokens error arrives in run_end.error and shows in the error banner (DESK-09).
- COL-UI-35 Export conversation: already built as `trellis log --json`. A save dialog would need tauri-plugin-dialog, which ADR 0048 does not add.
- COL-UI-39 Image attachments: `--image` belongs to `trellis run` alone (ADR 0041; rpc.rs passes no images). rpc growth for images is not proposed here.
- COL-UI-42 Decode speed: no token rate exists, because replies are read whole (ADR 0024: streaming not built).
- COL-UI-43 Time to first token: needs streaming, which is not built (ADR 0024).
- COL-UI-45 Queue-wait badge and request id: engine response headers; drive-via-runtime.
- COL-UI-46 Streaming chat client: the desktop never calls a runtime. trellis's openai-chat: spec is the client (drive-via-runtime).
- COL-UI-47 Generic-backend degradation: already built in trellis (probed tiers, ADR 0003), and the desktop inherits it through rpc.
- COL-UI-48 Local settings persistence: already built as .trellis/config.toml and the user config (ADR 0025 §7). The desktop stores no settings of its own.
- COL-UI-49 Memory-only conversations: already built the other way round. Trails are committed to the repository (ADR 0007).
- COL-UI-50 Brio scoring capability: owner-decision. It is an engine capability whose probabilities can never be a Verdict (ADR 0004), and there is no TRELLIS data to show.
- COL-UI-51 Brio document card: depends on COL-UI-50.
- COL-UI-52 Brio question rows: depend on COL-UI-50.
- COL-UI-53 Brio run and stop: depends on COL-UI-50.
- COL-UI-54 Brio results: depend on COL-UI-50.
- COL-UI-55 Brain mode tabs: switch between colibri engine views that have no TRELLIS counterpart. The codebase view borrows the interactions instead (DESK-17).
- COL-UI-56 Expert atlas loading: measured data specific to GLM-5.2; nothing in TRELLIS corresponds.
- COL-UI-57 Cortex renderer: a software-3D canvas for the atlas. The codebase graph is 2D SVG (DESK-17).
- COL-UI-61 Guided journey: a cosmetic automatic tour.
- COL-UI-65 Live expert grid: engine routing telemetry (/experts); no TRELLIS data.
- COL-UI-66 Routing pulses and residue: engine routing telemetry; no TRELLIS data.
- COL-UI-67 Live legend and states: depend on the live routing view, which is not built.
- COL-UI-68 Expert hover tooltip: engine and atlas data specific to GLM-5.2.
- COL-UI-69 Profiling data and per-turn table: engine PROF snapshots; drive-via-runtime. TRELLIS's own per-turn data shows in the actions log and budget (DESK-08, DESK-09).
- COL-UI-70 Profiling summary tiles: depend on COL-UI-69.
- COL-UI-71 Phase share bars: depend on COL-UI-69.
- COL-UI-72 Per-turn charts: depend on COL-UI-69.
- COL-UI-73 Profiling chrome: depends on COL-UI-69.
- COL-UI-74 Theme switching: ADR 0021 #74, 'Themes: excluded, cosmetic'. The stylesheet follows the OS colour scheme (DESK-09) and has no toggle.
- COL-UI-76 Localisation: owner-decision. All TRELLIS output is English, and no ADR covers localisation.
- COL-UI-81 Brand mark and font: cosmetic brand assets.
- COL-UI-82 UI primitives: replaced by native HTML controls. ADR 0048's static frontend has no component library and no Tailwind.
- COL-UI-83 One-process dashboard serving: BRIEF 'any networked component'. The desktop opens no port and serves nothing.
- COL-UI-84 /health contract: an engine HTTP contract; colibri-runtime track.
- COL-UI-85 /experts contract: an engine HTTP contract whose only consumer is the live routing view.
- COL-UI-86 /profile contract: an engine HTTP contract; colibri-runtime track.
- COL-UI-87 CORS and Host guard for UI origins: the desktop makes no HTTP request (its CSP connect-src is IPC only, DESK-06), so neither applies.
- COL-UI-90 Browser end-to-end scripts: there is no Node or Playwright. The desktop's logic is tested in Rust (DESK-05, DESK-08, DESK-11, DESK-12) and the page by the guard script; window-level WebDriver tests are not proposed.
- COL-UI-91 Release packaging of the dashboard: ADR 0021 Apex rows 38–42 keep packaging excluded. ADR 0048 builds from source only, and release.yml is unchanged.
- COL-UI-99 Bundling: installers and signing are excluded (Apex rows 38–42), and bundle.active is false (DESK-01).
- COL-UI-100 Updater: excluded (Apex rows 38–42), and there is no network component.
- COL-UI-101 Engine sidecar: the desktop starts only trellis, never a model runtime, and it does not bundle trellis either. The owner-decision row stands.
- COL-UI-102 Mobile scaffolding: no mobile target. TRELLIS supports x86_64 Windows and Linux.
- COL-UI-103 Landing page: owner-decision; TRELLIS has no website.
- COL-UI-105 Site theme: a cosmetic feature of colibri's website.

### pi
- PI-1.1: UNCHANGED, built: crates/trellis-loop/src/agent.rs (ADR 0005). The upstream note (thinkingLevel on each assistant message) has no analogue: a run uses one [model] thinking throughout.
- PI-1.3: UNCHANGED, built: ToolCallWire variants ReadFile, Edit, WriteFile, Grep, Find, Ls, Bash in crates/trellis-loop/src/wire.rs; differences are deliberate (ADR 0023 §2-3).
- PI-1.4: CHANGED, built in effect: [tools] enable and disable (ToolsSection in crates/trellis-cli/src/config.rs) are the + and - lists, layered field-wise (ADR 0025 §7); configuration is read per run, so nothing needs /reload.
- PI-1.5: CHANGED, model-facing cap built (ADR 0019, 2,000-character tail); the new 1 MiB structured result serves codemode callers, excluded under row 1.7 (PI-04).
- PI-1.6: UNCHANGED, deferred by ADR 0021 row 1.6 (needs the sandbox decision); upstream unchanged.
- PI-1.8: UNCHANGED, built in effect: Done at crates/trellis-loop/src/wire.rs:91 and :180; convergence is decided by checks.
- PI-1.9: CHANGED (fixes only), built: retry_delay at crates/trellis-model/src/hosted.rs:505; an HTTP-date Retry-After stays not built (ADR 0024).
- PI-1.10: UNCHANGED, built: is_context_overflow (crates/trellis-model/src/lib.rs:31) and elision in crates/trellis-loop/src/adapter.rs (ADR 0024 §5, ADR 0033).
- PI-1.11: UNCHANGED, built: ! and !! at crates/trellis-cli/src/session.rs:283-284.
- PI-1.12: UNCHANGED, built: TRELLIS_RUN_ID and TRELLIS_MODEL at crates/trellis-cli/src/tooling.rs:226-229.
- PI-1.13: UNCHANGED, built: [tools] shell (ToolsSection.shell); no command prefix by design.
- PI-1.14: UNCHANGED, partial by ADR 0041 (hosted tier 1 only), which declines the remainder.
- PI-1.15: UNCHANGED, deferred to v1.x (#67): its trigger is a dollar estimate.
- PI-2.2: CHANGED, excluded: login and stored credentials (ADR 0010; owner kept excluded 2026-09-28, #64). The new login flows do not touch that reason.
- PI-2.3: UNCHANGED, keys come from the environment only (ADR 0010, ADR 0024 §3, #64).
- PI-2.4: CHANGED (fixes only), deferred: cloud signing kept deferred (#45).
- PI-2.5: UNCHANGED, built: environment only, crates/trellis-cli/src/user_config.rs:7 and :381 (ADR 0024 §3).
- PI-2.6: CHANGED, partial: [[providers]] input_bytes built (#162); typed classifier and image entries follow rows 2.17 and 2.18 (PI-05); cache lifetimes and cost tables deferred (#67).
- PI-2.7: CHANGED, built: crates/trellis-model/src/catalog.rs and trellis model; the type filter serves classifier and image models (PI-05).
- PI-2.8: CHANGED (small), built for the four levels the two spoken protocols carry (crates/trellis-model/src/hosted.rs:66-92); a run uses one [model] thinking throughout, so there is no per-message level to record.
- PI-2.9: UNCHANGED, built: session /model and /models (crates/trellis-cli/src/session.rs:318-330, :379).
- PI-2.10: CHANGED, partial: trellis model pull built (ADR 0044); router management kept deferred (#45); classifier listing follows row 2.17 (PI-05).
- PI-2.11: CHANGED, excluded: Radius is a networked service (ADR 0010; #65 kept excluded); its reach into login, MCP and sharing falls under the same clauses and #73.
- PI-2.12: CHANGED, partial: [[providers]] built (ADR 0024 §2); code-defined providers need an in-process runtime trellis does not have (ADR 0023).
- PI-3.1: UNCHANGED, built: crates/trellis-cli/src/context_files.rs:25 NAMES (AGENTS.override.md, AGENTS.md, CLAUDE.md).
- PI-3.3: UNCHANGED, built as files (.trellis/system.md, .trellis/append_system.md; context_files.rs:412-413); no flags (brief 4.12).
- PI-3.4: CHANGED, built: fenced sections in crates/trellis-loop/src/adapter.rs; the new mcp_servers section follows MCP (PI-03).
- PI-4.1: CHANGED, partial by ADR 0021 row 4: processes exchanging JSON instead of an in-process runtime; the new built-in extensions are MCP, llama.cpp, codemode and tool-search (PI-03, PI-04, #45).
- PI-4.2: UNCHANGED, partial: trust store built (crates/trellis-cli/src/trust_cmd.rs, ADR 0023 §4).
- PI-4.3: UNCHANGED, partial: before_run and after_run; switch, fork and tree events wait on the session tree (#50).
- PI-4.4: UNCHANGED, excluded: ADR 0021 row '4 remaining events' (a provider-request hook could read the key).
- PI-4.5: UNCHANGED, built: before_run and after_run in crates/trellis-loop/src/hooks.rs.
- PI-4.6: UNCHANGED, excluded (UI).
- PI-4.7: UNCHANGED, built: post_tool (crates/trellis-loop/src/hooks.rs:285) and turn_end (hooks.rs:370, #137).
- PI-4.8: CHANGED, built: pre_tool gates and post_tool observes (ADR 0023 §5); the change (nested calls with parentToolCallId, MCP calls) follows PI-03 and PI-04.
- PI-4.9: UNCHANGED, partial: input hook (crates/trellis-loop/src/hooks.rs:340, #137).
- PI-4.10: UNCHANGED, partial: [[tools.custom]], templates and [[providers]]; renderers and shortcuts are in the itemised exclusions.
- PI-4.12: CHANGED (inventory gap: getSessionName), built: /name records Factual::label (crates/trellis-cli/src/session.rs:349-358, #109); the ADR row 7 correction is PI-01.
- PI-4.13: CHANGED, partial: the tool set is fixed at run start (crates/trellis-loop/src/registry.rs:1-11); exposure reporting follows PI-04.
- PI-4.14: UNCHANGED, partial: session /model and [model] thinking; not callable from hooks.
- PI-4.16: CHANGED, partial: hook stdin JSON; executeTool and tools follow nested calls (PI-04).
- PI-4.17: UNCHANGED, deferred with the session tree (#50).
- PI-4.18: CHANGED (inventory gaps), excluded: cosmetic per ADR 0021's itemised list; desktop and web surfaces stay excluded (#70).
- PI-4.19: CHANGED, partial: the new fields are rows 4.24, 13.5, 17.5 and 17.6, covered by PI-04 and PI-11.
- PI-4.20: CHANGED (documentation only), built: hooks run in every mode; approvals over rpc (crates/trellis-cli/src/rpc.rs:671).
- PI-5.1: UNCHANGED, built: crates/trellis-loop/src/skills.rs (ADR 0019).
- PI-5.2: UNCHANGED, built: ReadSkill in crates/trellis-loop/src/wire.rs (ADR 0019).
- PI-5.3: UNCHANGED, built: session /skill (crates/trellis-cli/src/session.rs:384) and trellis run --skill.
- PI-5.4: UNCHANGED, built: user-level and installed skill directories (crates/trellis-cli/src/user_config.rs:58-83; ADR 0039 §10).
- PI-6.1: CHANGED (inventory gap), built: every substitution form in crates/trellis-cli/src/templates.rs:353-427; trellis run --prompt <name> (crates/trellis-cli/src/main.rs:167-172) covers --prompt-template.
- PI-7.1: UNCHANGED, partial per ADR 0021 row 7: /model and /models built; /login and /logout excluded (ADR 0010).
- PI-7.4: UNCHANGED, partial: trellis trust, session /quit, /help and /doctor; configuration is read per run.
- PI-8.1: CHANGED (small), deferred: the session tree kept deferred (#50).
- PI-8.2: UNCHANGED, partial: trail factual fields (ADR 0007), steering (ADR 0034), label (#109).
- PI-8.3: UNCHANGED, deferred (#50).
- PI-8.4: CHANGED (inventory gap: -n/--name), partial: trellis resume <run_id> (ADR 0038) and session /name (#109) cover naming and finding a run; a --name flag on run is CLI growth (brief 4.12) with no consumer beyond /name.
- PI-8.5: CHANGED, built: elision keyed to context_bytes (ADR 0024 §5, ADR 0033); per-model limits are [[providers]] input_bytes (#162); routed-model windows follow PI-05.
- PI-8.6: UNCHANGED, model-written summaries rejected (ADR 0024 §5; owner kept the rejection, #153).
- PI-8.7: UNCHANGED, rejected with summaries (ADR 0024 §5, #153).
- PI-8.8: CHANGED (small), built: trellis log --json (crates/trellis-cli/src/log_cmd.rs); HTML excluded (#143); share excluded (#65).
- PI-9.1: CHANGED (inventory gap), built: trellis run; --export renders a session as HTML, excluded (#143).
- PI-9.2: UNCHANGED, built: run_start and run_end framing in crates/trellis-cli/src/output.rs (ADR 0025 §4).
- PI-9.3: CHANGED (inventory gaps), partial by ADR 0025 §4: --json carries trellis's own tracing events, not pi's vocabulary.
- PI-9.5: UNCHANGED, partial: approval questions over rpc (crates/trellis-cli/src/rpc.rs:671; ADR 0018).
- PI-9.6: CHANGED, built in effect (the crates are libraries); the new extension factories are for MCP, codemode and tool search (PI-03, PI-04).
- PI-10.1: CHANGED, partial: trellis install copies skills and templates (ADR 0039), which declines code-carrying packages.
- PI-10.2: CHANGED, partial: only skills/<name>/SKILL.md and prompts/<name>.md are copied (crates/trellis-cli/src/install.rs:11-14; ADR 0039).
- PI-11.1: CHANGED, excluded: fullscreen mode, cosmetic per the itemised list; trellis tui is its own full-screen surface.
- PI-11.2: UNCHANGED, built: at_file_paths at crates/trellis-cli/src/tui_run.rs:394 (#52).
- PI-11.3: UNCHANGED, excluded: external editor, cosmetic per the itemised list.
- PI-11.4: CHANGED, excluded: themes, cosmetic per the itemised list.
- PI-11.5: UNCHANGED, excluded: keybindings, cosmetic per the itemised list.
- PI-11.6: UNCHANGED, excluded: Mermaid rendering, cosmetic per the itemised list.
- PI-11.7: CHANGED, excluded: inline images, cosmetic per the itemised list.
- PI-11.8: UNCHANGED, excluded: games, cosmetic per the itemised list.
- PI-12.1: CHANGED, built: user and repository layering in crates/trellis-cli/src/user_config.rs (ADR 0025 §7); the newly user-only settings name MCP, trust and cache warming, covered elsewhere.
- PI-12.2: CHANGED (inventory gaps), partial: TRELLIS_CONFIG_DIR (crates/trellis-cli/src/user_config.rs:36-47); the pi-only variables name features excluded or absent.
- PI-13.1: CHANGED (documentation), built and stricter: control dial (ADR 0018), bash off by default (ADR 0023 §3), throwaway clone (ADR 0032).
- PI-13.2: CHANGED, built: crates/trellis-cli/src/trust_cmd.rs and crates/trellis-loop/src/trust.rs (ADR 0023 §4); mcp.json gating follows MCP (PI-03).
- PI-13.3: UNCHANGED, partial: throwaway clone (ADR 0032); containers deferred (ADR 0021 row 13.3).
- PI-13.4: UNCHANGED, built in effect (ADR 0021 row 13.4: ADR 0018, deny_write at crates/trellis-cli/src/config.rs:168, ADR 0009, snapshot and rollback, read-only profile #141); the subagent example is excluded (#130).
- PI-14.1: CHANGED, built: tracing is the one telemetry path; no network telemetry (brief Part 1).
- PI-14.2: CHANGED, built: crates/trellis-loop/src/orchestrator.rs and gate.rs; upstream's removal of the agent-core harness changes nothing here.
- PI-15.1: CHANGED, excluded: networked or multi-attach (ADR 0021 row 15; #65 kept).
- PI-15.2: CHANGED, excluded (ADR 0021 row 15; #65).
- PI-15.3: CHANGED, excluded (ADR 0021 row 15; #65).
- PI-15.4: UNCHANGED, partial: trellis's own eval tooling (docs/BENCHMARK_AND_EVAL.md, qa/run-classify.py).

### paperclip
- PC-01: excluded and unchanged: ADR 0021 Paperclip rows 1–3 [db][multi-user], one repository is one trellis company; a plugin-supplied organization switcher (PC-80) changes nothing here.
- PC-02: excluded [db][multi-user] (rows 1–3); the one change, the UI leaving an archived company, is a web-UI detail.
- PC-03: excluded [db][multi-user] (rows 1–3); the new resolver-policy cap and default responsible user govern several people, and in trellis only a person answers a question (#140, #189).
- PC-04: built, unchanged: reports_to with existence and cycle checks (crates/trellis-queue/src/profile.rs, test reports_to_may_not_form_a_cycle); SVG/PNG export is a view.
- PC-05: excluded [multi-user] (rows 5–6), unchanged.
- PC-06: excluded [multi-user] (rows 5–6); board claim, bootstrap claim and Owner Instance Admin administer an instance for many users.
- PC-12: excluded [server] (row 12), unchanged.
- PC-15: built in effect, unchanged: profiles are files in git (row 15).
- PC-16: deferred (row 16); the plugin driver, lease policies and kubernetes mode are remote execution targets, and the owner set the order on #20: the throwaway clone first (ADR 0032), containers later. No upstream change bears on that order.
- PC-17: deferred with the sandbox (rows 12, 17–18); CreateOS and the Daytona hardening are cloud services (brief Part 1, networked), and the step after the clone stays the one #20 left for later.
- PC-18: follows PC-17's deferral (rows 17–18), unchanged.
- PC-19: built for trellis's task model (ADR 0026 §1); each new column maps to its own row: review_policy to PC-32, responsible_user_id to PC-141, source_trust to PC-58, monitor_* to PC-114, conversation_* to PC-101, harness_kind to PC-31 and PC-68, title_needs_generation to PC-138; unblock_descriptor is the detail a manual blocked reason already carries, and status_version is optimistic locking, which the hard-link claim (ADR 0020) does instead.
- PC-20: built, unchanged (ADR 0026 §1, plus failed).
- PC-21: built (ADR 0020 hard-link claim); the upstream change, ownership claimed with queued runs, is a database-transaction detail.
- PC-22: built (ADR 0020, #59); the controller lease is a server concern, and a converged change awaiting review is protected by the clean-worktree gate: the next tick refuses while it sits in the worktree (ADR 0009).
- PC-23: built, unchanged (ADR 0026 §1, Queue::reconcile).
- PC-24: built, unchanged (blocked reason children).
- PC-26: excluded (rows 26–28, #56), unchanged.
- PC-27: excluded (rows 26–28, #56); the text-attachment tabs are web UI.
- PC-28: excluded (rows 26–28, #56): the trail and the diff are the work product; artifact cards and the gallery are web UI.
- PC-29: built, unchanged (queue add --label, queue set --label/--unlabel).
- PC-31: excluded (row 31, #141); skill_test is Skill Studio's harness (PC-68), and the command-line form of a skill test is trellis run --skill <name> --prompt <template> --args (ADR 0025).
- PC-32: built (needs_approval; a converged run waits in_review until a person sets done); a reviewer-agent stage stays out by rule 5 (ADR 0005), so review_policy's human_only is how trellis already works.
- PC-35: desktop-ui: excluded [web-ui][multi-user] (rows 35–36); belongs to the desktop track and its proposed Tauri ADR; the waiting reasons queue list prints are the CLI analogue.
- PC-36: excluded [multi-user] (rows 35–36), unchanged.
- PC-37: excluded (row 37, #58); summary slots, now written by the Summarizer built-in agent, would be a second record beside the trail.
- PC-38: excluded (row 38, #142), unchanged.
- PC-39: excluded (rows 39–40): one repository; GitHub repository selections are a multi-repository concern.
- PC-40: built (vault lineage, queue add --goal); the change hides a sidebar link.
- PC-42: built, unchanged (ADR 0020 file queue; routine coalescing, ADR 0026 §7).
- PC-44: built (one trail per run, trellis log, resume <run_id>); an interrupted run returns to pending/ with attempts counted (crates/trellis-cli/src/work.rs); scheduled retries are PC-112, which the stale-claim limit ticket of this track covers in part.
- PC-46: built locally, unchanged (ADR 0020): the loop owns claim, run and record.
- PC-48: built, unchanged (ADR 0026 §7, crates/trellis-queue/src/routine.rs).
- PC-49: built ({{var}} templates; revisions through git); typed variables matter where a person supplies values at a manual run, and routines run takes none, so a type would have nothing to check; routine documents fall under rows 26–28 (#56).
- PC-51: partial, unchanged: per-profile token budgets (ADR 0026 §5); company and project scopes have no trellis entity beyond the repository.
- PC-52: built, unchanged (warn_at, monthly and lifetime windows).
- PC-53: built, unchanged (the hard stop pauses the profile; queue approve --raise resumes it).
- PC-54: partial, unchanged: no finance ledger while dollars are deferred to v1.x (#67).
- PC-55: partial, unchanged: hire and strategy approvals excluded [multi-user] (rows 55–57).
- PC-57: built, unchanged (agents pause/resume, queue set, queue cancel, queue reject).
- PC-58: not ported as a named preset, and its parts exist: allow_write bounds scope (#182), the throwaway clone isolates writers (ADR 0032), and a converged run waits in_review until a person sets done; the rest is the OS sandbox #20 left for later. Source trust on issues needs tasks from untrusted sources, and every trellis task comes from a person or a committed routine.
- PC-62: excluded [server] (rows 59–63), unchanged.
- PC-64: built (ADR 0019; trellis install, ADR 0039); stars and comments are multi-user, and a pinned ref is the version.
- PC-65: partial, not ported: the catalog's skills sit at packages/skills-catalog/catalog/{bundled,optional}/<category>/<skill>/SKILL.md; the paperclip-operations ones (issue-triage, reflection-coach, status-card-query, summarize-status, task-planning) drive Paperclip's own API, github-pr-workflow pushes (trellis never pushes), and a generic one (doc-maintenance, qa-acceptance) can be copied into .trellis/skills/ or into a package as skills/<name>/SKILL.md (ADR 0039).
- PC-66: built, unchanged (read_skill progressive disclosure, run --skill).
- PC-67: built, unchanged (per-profile [skills] allow/deny enforced at read_skill).
- PC-68: desktop-ui: excluded [web-ui] (row 68); belongs to the desktop track; the command-line form of a skill test is trellis run --skill <name> --prompt <template> --args.
- PC-69: excluded (ADR 0010); the gcp_secret_manager and vault stubs change nothing.
- PC-70: excluded (ADR 0010), unchanged.
- PC-71: excluded (ADR 0010); trellis's only delivery is a process agent's pass_env from the user's environment (ADR 0026 §11).
- PC-72: excluded: the owner kept MCP and connectors excluded on 2026-09-28 (#73) for a long-lived connection, remote tool schemas the model sees unreviewed, and credentials per server; Composio's retirement, default-on aggregators and 82 apps touch none of those. A trusted custom command tool (ADR 0023) remains the way to give the model an external tool.
- PC-73: excluded with PC-72 (#73); trellis's own tools use the approve gate (ADR 0018).
- PC-74: excluded with PC-72 (#73); the new application types widen the gateway, and #73's reasons apply to each.
- PC-75: excluded with PC-72 (#73) and brief v2.0 (MCP exposure).
- PC-76: excluded [server][web-ui] (rows 76–82), unchanged; executable hooks are the local extension point (ADR 0023 §5).
- PC-77: excluded (rows 76–82); the new approvals.respond capability lets plugin code answer approvals, further still from trellis's rule that only a person answers (#189).
- PC-78: excluded (rows 76–82), unchanged; trellis hooks are JSON-over-stdio processes (crates/trellis-loop/src/hooks.rs).
- PC-79: excluded (rows 76–82), unchanged; custom command tools are the local analogue (ADR 0023).
- PC-80: desktop-ui: excluded [web-ui] (rows 76–82); the new slot types are web UI.
- PC-81: excluded (rows 76–82); distribution image catalogs and plugin-managed agents are server features.
- PC-82: excluded (rows 76–82); LLM Wiki overlaps the vault trellis already has (ADR 0017, ADR 0022).
- PC-83: built, unchanged (JSONL activity log, crates/trellis-queue/src/activity.rs).
- PC-84: built, unchanged (trellis queue log); permission gating is [multi-user].
- PC-85: built, unchanged (own folder layout, crates/trellis-queue/src/portable.rs).
- PC-88: excluded [server][db][multi-user] (rows 88–93); bind presets are a server concern.
- PC-89: excluded (rows 88–93); the preferences removal and CLI-auth fixes are server changes.
- PC-90: excluded (rows 88–93), unchanged.
- PC-91: excluded [db] (rows 88–93); trellis's queue is files in the git dir (ADR 0020).
- PC-92: excluded (rows 88–93), unchanged.
- PC-93: excluded [server] (rows 88–93); release channels and Sentry attribution are deployment concerns.
- PC-94: built in effect, unchanged (the local subcommands and trellis doctor).
- PC-95: built (queue, agents, routines, log, resume); the new client subcommands talk to the REST API (PC-98).
- PC-96: built, unchanged (--json, ADR 0025); there is no API base or key without a server.
- PC-97: excluded [server] (rows 97–98), unchanged.
- PC-98: excluded [server] (rows 97–98); trellis rpc is the supported front-end interface (ADR 0025 §5, #70).
- PC-99: desktop-ui: excluded [web-ui] (rows 99–100, #70); belongs to the desktop track.
- PC-100: desktop-ui: excluded [web-ui] (rows 99–100); each flag gating a capability is dispositioned under that capability's row.

### apex
- APX-01: already built and UNCHANGED (crates/trellis-vault/src/reader.rs:108-170, 212-258; ADR 0022 §1 records the CRLF/BOM and JSON-unquote divergences)
- APX-02: already built and UNCHANGED (reader.rs:172-240; title_falls_back_to_heading_then_basename)
- APX-03: already built and UNCHANGED (reader.rs:164-170, graph.rs key_for, every_apex_null_form_is_a_null_parent_at_the_gate); the path-instead-of-basename write is recorded in ADR 0022 §1
- APX-04: already built and UNCHANGED (edit.rs compose_raw; fix_strips_an_extra_key_the_way_compose_raw_does_and_the_gate_then_accepts, fix_on_a_correct_vault_writes_nothing)
- APX-05: already built and UNCHANGED (refs.rs, graph.rs; a_repeated_link_adds_weight_and_a_parent_link_adds_no_edge)
- APX-06: already built and UNCHANGED (graph.rs:30-118, refs.rs; graph_matches_apex_vectors, a_path_shortcut_wins_over_a_shared_basename)
- APX-08: already built and UNCHANGED (validate.rs manifest_state, edit.rs sync_manifest; fix_rewrites_a_wrong_manifest_in_apex_s_format); the never-read gap is Apex's own
- APX-10: already built and UNCHANGED (dates.rs:21-130, 384-620; ADR 0022 §5); the Unix-only directory sync is recorded there
- APX-11: already built and UNCHANGED (dates.rs:182-382, 674-800; vault_cmd.rs dates --seed; ADR 0022 §5)
- APX-13: already built and UNCHANGED (graph.rs:188-356; graph_has_apex_s_roots_depth_refs_and_issues)
- APX-14: already built and UNCHANGED (graph.rs issues; ADR 0022 §2 records duplicate-alias and trellis's own disconnected)
- APX-15: desktop-ui, excluded by ADR 0021 row 15–19 [ui] and the itemised exclusions; UNCHANGED, so no ticket (the desktop track would need an ADR amending ADR 0021)
- APX-16: desktop-ui, excluded by ADR 0021 itemised 'Large-graph mode: excluded, [ui]'; UNCHANGED
- APX-17: desktop-ui, excluded by ADR 0021 itemised 'Label tiers: excluded, [ui]'; UNCHANGED
- APX-18: desktop-ui, excluded by ADR 0021 row 15–19 and itemised 'Layered-layout canvas'; UNCHANGED
- APX-19: desktop-ui, excluded by ADR 0021 itemised 'Drawing freehand annotations on the canvas: excluded, [ui]'; UNCHANGED
- APX-20: already built and UNCHANGED (edit.rs create; new_slugifies_deduplicates_and_keeps_the_manifest_correct); the dialog and canvas-point flows are UI
- APX-21: desktop-ui, excluded by ADR 0021 row 21, 27–29 [ui]; UNCHANGED
- APX-22: excluded by the ADR 0021 row 20, 22–26 amendment of 2026-09-29 (#61) and ADR 0022 §7; the CHANGED status is already recorded there, and `trellis vault new` is the explicit form
- APX-25: already built and UNCHANGED (edit.rs link; link_appends_once)
- APX-26: already built and UNCHANGED (edit.rs remove; rm_turns_links_to_text_orphans_children_and_prunes_sidecars; ADR 0021 row 20 records 'git is the undo')
- APX-27: desktop-ui, excluded by ADR 0021 row 21, 27–29 [ui]; UNCHANGED
- APX-28: desktop-ui, excluded by ADR 0021 row 21, 27–29 [ui]; UNCHANGED. A command-line import would be a new `trellis vault` action, which is new CLI surface the exclusion does not open
- APX-29: desktop-ui, excluded by ADR 0021 row 21, 27–29 [ui]; git is the undo for `trellis vault` edits; UNCHANGED
- APX-32: already built and UNCHANGED (edit.rs check, vault_cmd.rs check; ADR 0022 §3 check row)
- APX-33: excluded; the owner kept the long-lived watcher excluded on #68 (2026-09-28, 'Keep all excluded'), and `trellis vault dates --seed` is the explicit reconcile
- APX-34: already built and UNCHANGED (edit.rs size warnings at Apex's >= thresholds; vault_cmd.rs search mode line)
- APX-38: desktop-ui, excluded by ADR 0021 row 38–42 [ui/desktop]; UNCHANGED
- APX-39: desktop-ui, excluded by ADR 0021 row 38–42 [ui/desktop]; UNCHANGED
- APX-40: desktop-ui, excluded by ADR 0021 row 38–42 [ui/desktop]; trellis's own CI is not an inherited capability; UNCHANGED
- APX-41: desktop-ui, excluded by ADR 0021 row 38–42 [ui/desktop]; UNCHANGED
- APX-42: desktop-ui, excluded by ADR 0021 row 38–42 [ui/desktop]; UNCHANGED

## Completeness critic

Round 1: 9 gaps, 1 duplicates, 9 ordering edges.
- duplicate: PI-10 / DESK-08: both produce session token totals. PI-10 adds a get_session_stats rpc command; DESK-08's reducer would sum rpc frames to show COL-UI-24 a second way. Keep PI-10 as the one source, and have DESK-08 read get_session_stats instead of deriving the totals itself.
- ordering: DESK-01 blocked_by none, but renumber it first: ADR 0046 already belongs to #239 (branch t239-rollback-clock, Accepted/Built), so the desktop ADR takes the next free number.
- ordering: COL-01a blocked_by DESK-01: ADR 0047 is the number the renumbered desktop ADR needs, so colibri's ADR must be numbered after the desktop takes its number, and colibri's reasons citing "ADR 0046" must be updated to match. (Superseded by #284, which numbered colibri's ADR 0047 and the desktop's 0048; scripts/checks/adr-numbers.sh holds both.)
- ordering: APEX-08 blocked_by COL-01a: APEX-08's "new ADR" has no number and must take the next free one after the desktop and colibri ADRs.
- ordering: DESK-08 blocked_by COL-10: per-turn output tokens, model time and queue wait reach rpc frames only after COL-10 adds them to the turn event, which today carries prompt_bytes only (agent.rs:1017). Without them DESK-08 cannot back COL-UI-41 or COL-UI-44.
- ordering: DESK-08 blocked_by PI-10: session token totals (COL-UI-24) should come from get_session_stats, not from a second summation inside the desktop reducer.
- ordering: COL-16 blocked_by PI-05: pi's classifier models are the System One/Jev classify API (pi packages/ai/README.md:893; CHANGELOG "Models.classify() ... System One API"). COL-ENG-40 is colibri's /v1/systemone (Jev compatibility) and COL-16 measures /v1/brio closed-set scoring. PI-05 asks the owner whether classifier models stay outside trellis, and that answer decides whether COL-16's choice exists at all.
- ordering: COL-05 blocked_by PI-07: hosted.rs:263 deliberately sends no temperature today. COL-05 is the first ticket to put a sampling parameter (temperature 0) on HostedClient, and PI-07 asks the owner what sampling parameters trellis sends.
- ordering: APEX-08 blocked_by CLIP-09: notes.rs says "a run can edit" the vault. APEX-08/08a/08b would record and enforce that model writes stay out of the configured vault's notes root, while CLIP-09 asks the owner whether a profile's runs may read and edit a memory note. If that note lives in the vault, APEX-08 decides CLIP-09's question before the owner is asked, so ask first and write the ADR with or without the exception.
- ordering: DESK-16 blocked_by APEX-05: vault_search over rpc must use the same ranked search path that `trellis vault search` gets from APEX-04/05. Built earlier on the bare Index::search, the desktop and CLI would return different orders, and any desktop field tests would pin the unranked order.

## QA findings still open after the fix rounds

- colibri-runtime COL-01: Files names paths that do not exist and are not marked new: neither docs/adr/0047-colibri-as-a-model-runtime.md nor docs/inventory/inventory-colibri.md exists on any branch (`git log --all` finds nothing for either), yet the body says both are 'committed with this plan' → commit both (renumbered) before filing, or mark them (new) and have this ticket write them, including the 107-row inventory.; Wrong ADR claim: the supplied draft is headed `# ADR 0045 —`. ADR 0045 is the accepted work-units ADR on this branch (docs/adr/0045-work-units-for-multi-agent-runs.md), and COL-03 and COL-10 still cite 'ADR 0045 decision N'. So the claim that 'every ticket in the story cites it as ADR 0047' is false → renumber the draft, and fix COL-03 and COL-10.; Done when misses a needed edit: the owner is asked to accept decision 6 as drafted (LOCAL_CONTEXT_BYTES, 14,336 bytes). COL-08 argues that figure is wrong and puts it to the owner as a/b/c, yet the ADR 0024 §5 amendment line 'by ADR 0047 decision 6' would point at the 14,336 text → require decision 6, and the Consequences line 'holds its prompt to the local budget', to be rewritten as pending the byte-budget answer before acceptance.
- colibri-runtime COL-03: Wrong ADR claim: the body cites 'ADR 0045 decision 3'. ADR 0045 is the accepted work-units ADR → cite ADR 0047 decision 3.; False claim about TRELLIS: 'For a vllm: spec it probes the schema and llama.cpp's /props'. In fact `probe_slots` returns Ok(None) for VllmOpenAi without sending any request (crates/trellis-model/src/runtime.rs:324); only llamacpp: reads /props → correct the sentence.; (advisory) Security test gap: no Done when line says which key doctor's /v1/models and /health reads carry. Doctor falls back to the repository's configured spec (main.rs:557-563), so a repository-chosen loopback endpoint is a real input → add a test: with a repository-chosen loopback spec and the variable unset, no Authorization or x-api-key header reaches the stub, and the key never appears in doctor's output.; (advisory) Only the served id is tested for ESC escaping, but doctor also prints server free text: hwinfo `cpu`/`gpu` and `owned_by` → add an ESC case for hwinfo `cpu`.
- colibri-runtime COL-04: Files line incomplete, and with the missing files the ticket is 6+ (split it): the loopback stubs in crates/trellis-cli/tests/hosted_keys.rs:92, modes.rs:339, prompt_tokens.rs:84 and interrupt.rs:128 answer requests in order, one canned reply per connection. A GET /v1/models before every loopback hosted run uses up a reply and shifts every later one, so the ticket's own Verify line `cargo test -p trellis-cli --test hosted_keys` fails unless those files change → first file a ticket that makes those stubs answer GET /v1/models with 404, and add it to blocked_by.; Done when needs judgment: it covers a refused connection, a timeout, 401/403, 404 and not-colibri, but not `identify` returning RuntimeError (a 5xx, a 405, a 200 that is not JSON). Non-colibri loopback servers commonly answer that way, so whether such a run refuses or continues is left to the implementer, and refusing would be a regression → state that the run continues as today.; Done when needs judgment: 'a probe that outlasts the client's request timeout' does not say which timeout. COL-04a sets no probe timeout, so the test could wait the 300 s model default → name the probe's bound, for example 10 s.
- colibri-runtime COL-04b: Security / CLAUDE.md invariant ('only the user decides where a key is sent'): the TUI label comes from `tier_from_probe(spec)` (tui_run.rs:93, called at :524), which receives only the spec. The TUI's spec can come from `[model] spec` (`model_from_user` is false then). The Done when does not say that the TUI's new identification and probe go through `hosted::resolve(spec, cfg, from_user)`, and nothing tests the repository-chosen case → require it, and add a test that a repository-chosen loopback spec with the variable set sends nothing.; Track conflict: ADR decision 4 names only `trellis run` and `trellis doctor` as probe sites. A TUI run would probe from the status bar and then again in cmd_run (COL-04), which is two tool probes per run → reuse one identification and probe result for the label, or name the TUI in the ADR.
- colibri-runtime COL-05a: Files line incomplete, so the ticket's own Verify fails: `ask_hosted_classify_a_question_takes_no_snapshot` (crates/trellis-cli/tests/hosted_keys.rs, around :497-509) asserts `requests.len() == 1`, and its stub serves one reply in order. Ask's new identification read uses that reply up, so `cargo test -p trellis-cli --test hosted_keys` fails, and hosted_keys.rs is not in Files → add the file, or block on the stub-prep ticket.; (advisory) The Done when does not say that ask's identification read carries the key `ask_hosted`'s `hosted::resolve(spec, cfg, from_user)` gives (ask.rs:248-254) → state it, and assert no header for a repository-chosen loopback spec.
- colibri-runtime COL-07: Cited upstream evidence does not show the claim: c/openai_server.py:3152-3160 clamps `max_tokens` to the server's --max-tokens cap (`limit`), not to the room the prompt leaves. The room clamp and the length flag are in c/colibri.c:9387-9413 → cite those lines.; Done when misses a case the body names: the body says a reply can be cut 'before or inside the tool call', but only no-call replies are covered. A `finish_reason: "length"` reply whose tool-call arguments are cut still produces 'the arguments of X are not JSON', which points at formatting → add that case and a test.
- colibri-runtime COL-08: Wrong upstream evidence, contradicting the ADR's own table: 'colibri's own default reply budget is 1,024 tokens on every family (default_max_output)' is the one-shot `coli run` figure. `coli serve` passes `interactive_max_output` (c/coli:388-393, :2120; 16,384 on most families, 8,192 on Qwen). TRELLIS sends `max_completion_tokens` 16,000, which colibri reads first (c/openai_server.py:3131) → justify the 1,024-token reserve as TRELLIS's own choice, not colibri's default.; Wrong figure: the floor runs' `first_prompt_bytes` range from 9,754 to 9,769. No 9,808 appears anywhere under docs/bench/floor-* → correct it.
- colibri-runtime COL-10: Wrong ADR claim: the body cites 'ADR 0045 decision 8'. ADR 0045 is the accepted work-units ADR → cite ADR 0047 decision 8.; (advisory) Citation: c/openai_server.py:110-270 is the admission scheduler. The `x-colibri-queue-wait-ms` header is set at :4892 and :5340 and sent at :4939 and :5032 → cite those lines.; (advisory) If model_ms and queue wait reach agent.rs through a new required `Model` trait method, the implementors in run_cmd.rs:2582, session_extra.rs:480 and scripted.rs:88 must change too → use a default method, or list those files.
- colibri-runtime COL-11: Files line incomplete: crates/tui-demo/src/tui/mock.rs:33 constructs `Message::BudgetUpdated` and stops compiling once the variant gains fields → add it (5 files).; Done when needs judgment, and the figure can mislead: `<r> tok/s` has no stated formula, and `model_ms` includes retries and colibri's queue wait, which the same line shows separately → state the formula, for example completion_tokens / (model_ms - queue_wait_ms), and assert it in a `budget_line_` test.; Triage: crates/tui-demo/** is under no path in the area table (crates/trellis-tickets/src/rules.rs:32-54), so `area:` abstains and the ticket routes to needs-human. Unlike COL-16a, the body does not say so → say so.
- colibri-runtime COL-12: Done when cannot be met within Files: docs/bench/bytes_per_token.py always reads `<run dir>/ollama-requests.txt` (line 105) and validates against Ollama's server log (its docstring, and the 34-token probe split). A colibri run has no such file, so 'the output of bytes_per_token.py over the run's directory' fails, and that script is not in Files → add bytes_per_token.py and a mode for runs without a runtime request log, or drop the bullet.; Files incomplete: the run directory docs/bench/floor-colibri/ (trails and the new .jsonl logs, as the other floor-* runs commit them) is not listed → list it.; Should be split: the run_floor.py --log-json and model_ms tooling has no external dependency, but it is held behind a measurement marked !blocked-ext → make the tooling its own ticket (blocked only on COL-10), and make the measurement depend on it.
- colibri-runtime COL-13: Done when needs judgment, with protocol consequences: the level and target of the `model token` event are not given. At the default `trellis=info` filter (main.rs:413-414) each delta becomes one stderr line, and one line on `--json` stdout through ADR 0025's event layer (main.rs:457-466) → require a level or target outside `trellis=info` (for example debug), with a test that `--json` stdout carries no `model token` line.
- colibri-runtime COL-15: Done when depends on behaviour no blocker provides: `kv_slots` and the tier counts come from /health, but COL-04 reads only /v1/models in the run path, and no ticket adds a /health read to `trellis run`. This ticket also does not say which key that read carries → add the /health read here, using the key `hosted::resolve` gives, with a header test.; (advisory) An info-level `runtime identified` event also lands on `--json` stdout (ADR 0025 event layer) → say so, or emit it under a target outside `trellis=info`.
- desktop DESK-15: blocked_by_keys is missing DESK-01. DESK-13 and DESK-14 wait on DESK-01 because "under its answer B or C there is no desktop to read it". get_queue exists only for the desktop's queue view, as this ticket's own Question says, so the same reasoning applies here.; The Done when line "It writes nothing and is answered while a run is in flight" is checked by no named test. rpc_get_queue_lists_what_queue_list_prints only compares the answer with `queue list`, so Verify passes even if get_queue blocks behind a run or writes something. DESK-13 and DESK-14 put both properties into their test: `git status --porcelain` prints nothing afterwards, and the command is answered with success true while an approve-mode run waits on its approval_request, before that run's run_end. Do the same here. ADR 0048 §10 also requires the answer to come from its own thread, as compact's does, and no Done when line says so.; Question option A says writes stay with `trellis queue` "as #70 requires". #70 (ADR 0021, line 292) names only "the orchestrator or a `trellis vault` command", so the citation is wrong.
- desktop DESK-16: blocked_by_keys is missing DESK-01, for the reason DESK-13 and DESK-14 give: under DESK-01's answer B or C there is no desktop vault view to serve.; The Done when line "Neither writes, and both are answered while a run is in flight" is checked by no named test. rpc_vault_search_returns_what_vault_search_prints and rpc_vault_graph_returns_what_vault_graph_prints only compare with the CLI's output. Add what DESK-13 and DESK-14 assert: an empty `git status --porcelain`, and an answer with success true while an approve-mode run waits on approval_request. ADR 0048 §10's own-thread rule is also missing from Done when.; Non-blocking: the cited upstream (colibri Brio.tsx:68-77,99-115, a document-upload card) exists but is not a search or graph view. The evidence is real but only loosely matches.
- desktop DESK-03: blocked_by_keys is missing DESK-06. Done when requires each rule to name the file or script that already holds it. The rule "no Tauri plugin, capability permission, CSP source or npm package is added without amending ADR 0046" is held by DESK-06's desktop/src/lockdown.rs, and no chain from DESK-02, DESK-04, DESK-05, DESK-12 or DESK-19 reaches DESK-06.; It edits the root CLAUDE.md's "Crate boundaries are load-bearing" list, which is an invariant list, with needs_owner false and no ADR in that Done when bullet. Add "as ADR 0048 §1 states" to the root CLAUDE.md bullet, so the new invariant traces to the accepted ADR rather than to this ticket.
- desktop DESK-05: Security ordering: from this ticket until DESK-12 lands, rpc_send forwards `set_model` and a `prompt`'s `model` with no guard. rpc.rs:809 sets model_from_user for any spec the client names, and hosted::resolve_with skips the repository-endpoint check (`allowed`) when from_user is true. For that window, this IPC path can send the user's key to a hosted endpoint the repository chose, which breaks CLAUDE.md's "Only the user decides where a key is sent" (ADR 0024 §3). Fix: have rpc_send refuse `set_model` and any `prompt` carrying `model` here, held by a named test (for example rpc_send_refuses_set_model_and_a_prompt_model), and let DESK-12 replace that refusal with its guard. Alternatively, move DESK-12's guard into this ticket.; Non-blocking: Done when says get_state_reply_is_one_json_response still passes, but Verify does not run it. CREATE_NO_WINDOW on Windows has no test.
- desktop DESK-18: The per-status counts (eight cells, 0 when a status has no tasks, no cell summing two statuses) are computed in queue.js, and no test checks them. ADR 0048 §5 keeps the view-model in Rust and has page JS only render. The Verify line `git grep -q -F "in_review"` passes with wrong counts or with a summed cell, so the Done when line on counts is not proven. Fix: have DESK-15's get_queue answer `data.counts` keyed by Status::as_str and check it in its binary test, or type the answer in desktop/src/queue.rs with a test such as queue_counts_one_cell_per_status_and_no_sum, asserted through verify-tests.sh.; "#70 keeps every write on the orchestrator or a `trellis queue` command" misquotes ADR 0021:292, which names the orchestrator or a `trellis vault` command.
- pi PI-12: Claimed upstream evidence does not support the change the ticket records. Every cited line exists and is exact (extensions.md:251, cli.md:100, args.ts:125, CHANGELOG.md:1674/1681). But the premise 'The pi 1.0.2 inventory marks items 4.20 and 8.4 CHANGED' is contradicted by the prerequisite PI-02a. Its current Done when and Verify pin '**4.20 Mode behaviour.** UNCHANGED', '**8.4 Session flags.** UNCHANGED' and '136 capabilities: 54 unchanged, 47 changed, 35 new', and its rule is that a CHANGED tag needs a CHANGELOG entry after 0.87.1. The coding-agent CHANGELOG from 1.0.2 back to 0.87.1 (lines 5-242) has no entry on mode behaviour, ctx.mode, hasUI or --name. --name arrived in 0.78.0 (:1674, :1681) and ctx.mode in 0.78.1 (:1648), both before 0.87.1 (:242). The ticket itself concedes 8.4 is 'not new in 1.0.2'. For 4.20, 'line 251 now tells' is unsupported: the upstream clone is shallow (one commit), and the ticket offers nothing dating that sentence after 0.87.1.; Done when cannot be met without making the ADR contradict its own inventory. It requires recording 4.20 and 8.4 inside PI-02b's section headed 'pi 1.0.2: changed upstream, disposition unchanged', but inventory-pi.md (PI-02a) tags both UNCHANGED. The count bullet would also take that record to 41, against PI-02b's derivation of 47 changed minus 8 = 39.; False cross-reference: 'PI-4.12's change belongs to PI-01' is wrong. PI-01's feature_ids are PI-1.2, PI-2.13, PI-3.2, PI-7.2 and PI-7.3, and its body never mentions 4.12. PI-02a tags 4.12 UNCHANGED as well, so there is no 4.12 change for any ticket to own.; Secondary: 'the item stays built under the row "4 extensions" (W-B)' asserts a disposition ADR 0021 never stated. That row lists specific events and methods (tool_call, tool_result, before_agent_start, ...), and mode behaviour is not among them. In the 0.87.1 inventory it is an unnumbered paragraph (inventory-pi.md:73), the same position as 4.11 and 4.15, which PI-02 treats as items that never had a disposition. If 4.20 needs a disposition, it belongs in PI-02's never-dispositioned list, not in PI-02b's 'changed, disposition unchanged' record.; Non-blocking: the Done when names the table '`## pi (inventory-pi.md)`', but the actual heading is '## pi (`inventory-pi.md`)' (docs/adr/0021-capability-matrix.md:102).; Non-blocking: the Verify block asserts only 3 of the 6 Done when bullets. Nothing checks that 'landing' is absent, that the disposition cells are unchanged, that the count includes the two items, or that '#109'/'deferred' appear. 'That line says the change is documentation only...' needs a reader's judgment rather than a grep. The Verify lines themselves are allowed by ticket-verify.sh and exit 1 on both 612115d and HEAD 6275034.
- paperclip CLIP-04: Files line: it omits docs/adr/0020-local-work-queue.md, yet Done when amends ADR 0020 §5 and Verify line 3 greps that file.; Verify/Done when mismatch: Verify needs 3 tests matching `stale_claim_block_`, but no Done when line names that prefix. The test for "a task file without the field loads with stale_releases 0 and is released as before" could reasonably be named `stale_claim_release_*`. That leaves 2 block tests, and Verify fails without any Done when line being broken. Add "the tests above are named with the prefix stale_claim_block_".; Missing prerequisite/coverage: CLIP-02 adds row PC-112 with "S17 question: blocking a task after repeated stale-claim releases (row 33)". CLIP-03a, 05a, 06 and 07 replace their row's S17 sentence; this ticket does not, and it is not blocked by CLIP-02. Add a Done when line that replaces PC-112's S17 sentence, a Verify line `git grep -q -e "^. PC-112 .*stale_claim_blocked"`, and blocked_by CLIP-02.; The ticket does not say what happens under answers (a) or (c). Other tickets say "closed unbuilt" or "rewritten before work starts". Option (c) needs a different build.; Interaction not named: CLIP-03 adds `notes_seen` and lists "the stale-claim release" among the paths that leave it alone. The new block path here must also leave it alone. Whichever of the two lands second needs a line covering the other.
- paperclip CLIP-09: Missing prerequisite/coverage (the same defect failed in CLIP-04 and CLIP-12): CLIP-02 adds row PC-117 with "S17 question: a profile memory note outside `.trellis/`". This ticket amends only ADR 0026 §4, leaves that sentence in place, and is not blocked by CLIP-02. CLIP-03a, 05a, 06 and 07 all replace theirs. Add a Done when line that replaces PC-117's S17 sentence with the answer, a Verify line `git grep -q -e "^. PC-117 .*persistent agent files"` (or similar), and blocked_by CLIP-02.; (non-blocking) Options (b) and (c) put a repository path or vault stem into every run's prompt. That is a repository-supplied prompt input, so CLAUDE.md requires it to be read through `trellis_core::paths::resolve_repo_input`. The options should say so, so that the ADR the answer leads to does not leave it out.; (non-blocking) No follow-on build or ADR ticket exists for the recommended (c).
- paperclip CLIP-12: Files line: it omits docs/adr/0026-local-orchestration.md, yet Done when amends ADR 0026 §3 and Verify line 2 greps that file.; Verify/Done when mismatch: Verify needs 2 tests matching `queue_list_text_`, but no Done when line names that prefix. Add "the tests above are named with the prefix queue_list_text_ in crates/trellis-cli/tests/queue.rs".; Missing prerequisite/coverage: CLIP-02 adds row PC-124 with "S17 question: filtering `queue list` by a substring of task text". This ticket does not replace that sentence and is not blocked by CLIP-02, unlike CLIP-03a, 05a, 06 and 07. Add the Done when line, a Verify line for `^. PC-124 .*--text`, and blocked_by CLIP-02.; The ticket does not say what happens under (a) or (c). (c) uses BM25 through trellis-vault, which is a different build and possibly a new crate edge for the queue listing.
- paperclip CLIP-02: A Done when line needs judgment: docs/inventory/inventory-paperclip-20261004.md must "list PC-01 to PC-144 each with the upstream paths it rests on", but that re-inventory exists nowhere an implementer can copy it from. It is not in the tree (docs/inventory/ holds only the apex, paperclip and pi files). It is not in the job directory either: no file outside upstream/ contains PC-1xx or the file name. The body gives no upstream paths for PC-01..PC-100 and only one-line clauses for PC-101..PC-144. The claim "every row is written out below, so nothing here rests on a planning document" is true of the ADR rows and false of the inventory file. Building it means redoing the re-inventory, which is research, not a checkable step.; The Verify block cannot catch this: a file holding only the commit hash and the string "PC-144" passes both inventory greps. Either commit the re-inventory first, as its own sourced ticket or an attachment the ticket cites, and gate on it, or narrow this Done when to PC-101..PC-144 with their paths written into the ticket.; (non-blocking) The 44 ADR rows and the "claims not found" list check out against upstream: register_deliverable, skill_test, review_policy, app_webhook, fireflies_hmac, approvals.respond, routines.md:129 (four modes) against constants.ts:651 (six), PLUGIN_SPEC.md §15.2, and SPEC-implementation.md §11.6 `intervalSec` minimum 30. PC-119 and PC-120 here correctly follow ADR 0039 "Not built" and ADR 0022 §6. The ADR draft does not (see track_problems).
- paperclip CLIP-03: A new test is not asserted through verify-tests.sh: the (b)-only Done when line requires rewritten `mention_wake_` tests and `a_person_note_wakes_its_profile_through_the_real_heartbeat`, plus a test that "the note still reaches the next run". No Verify line covers any of them (the three Verify lines are input_since_last_run_, rerun_input_ and the existing ask test). Either add a verify-tests line for the (b) branch, or say that under (b) the ticket is rewritten before work starts, as CLIP-05 does for its (c).; Missing cross-ticket line: CLIP-04 adds a new path out of claimed/ (a stale-claim block to pending/ as blocked). It must leave `notes_seen` alone just as "the stale-claim release" does. Neither ticket mentions the other, and both add Task fields in lib.rs and tests.rs. Add the block path to the list of paths that leave the marker, or set a blocked_by order.; Checked: the cited lines work.rs:457, mention.rs:165, lib.rs:1178 and lib.rs:1257 each clear `outcome` as stated. Task has no deny_unknown_fields. The existing ask test adds a human note with no id after a model note, so it still passes under the new rule.
- paperclip CLIP-06: Contradiction between the body and the Question: option (c) is "(b) plus a subtree pause", but the body says "under (a) or (c) this ticket is rewritten to record that answer ... and builds nothing". Under (c) the --children build in (b) is part of the answer, so the implementer cannot tell whether to build it. Say instead that under (c) the (b) lines stand and the subtree pause becomes a separate ticket with its own ADR.; (non-blocking) Verify line 4 (`^. PC-113 .*--children`) depends on CLIP-02's row text, which has no "--children". The ADR draft's PC-113 row does contain "`queue cancel --children`". If CLIP-02 copies the draft, that line passes before this ticket does anything.; Checked: QueueAction::Cancel and its args are in work.rs (lines 130 and 519). Queue::index().children is used by `queue set` (work.rs:371-380). Queue::lock_task is private (lib.rs:787). A flag together with an ADR 0026 §3 amendment and needs_owner true is correct.
- apex APEX-02: Done when line not observable: "The `?` in vault_cmd.rs therefore cannot fire on a date failure" is a conclusion that no test checks. Test 2 only reaches the validate step inside `File::save`. An implementer can keep `file.save()?` for an `atomic_write` failure, all three tests still pass, and `trellis vault new` still exits non-zero after its note lands. Fix: add a fourth `after_edit_*` test, and raise the verify-tests minimum to 4. In that test, on Unix, make the notes root read-only before calling `after_edit`, so creating the temporary file fails. `after_edit` must return `Ok(Some(m))` with `m` containing `dates.json left unchanged`, and the bytes of dates.json must be unchanged. The test returns early with a `skipped:` line when run as root or on Windows.
- apex APEX-04: Done when needs judgment: the line that generates the vectors names web/searchResults.ts and web/searchIndex.ts, but not what note objects go into buildSearchResults. normalizeNotes (searchResults.ts:114) falls back to `title path body` when `note.searchText` is absent, while noteParser.ts:54 sets searchText to `title path raw`, which is what the app's notes carry and what trellis's Index::build uses. Vectors built from plain objects would hold trellis to a ranking Apex never produces. Fix: say the notes go through web/notes/noteParser.ts `parseNote` first, as the comment on `ranking_matches_apex_vectors` already does.
- apex APEX-05: Done when contradicts the ticket's own Question: the ticket says it applies when the owner answers A or B. Its ADR 0022 §4 amendment must still contain `search_notes keeps the index ranking of ADR 0024`, and both the Done when and the Verify grep require that sentence. Under answer A the sentence is false. Fix: either limit the ticket to answer B, or make the §4 sentence and its Verify line depend on the answer (under A: `search_notes switches in a later ticket`).
- apex APEX-08: blocked_by is missing an obvious prerequisite, or the work is ordered wrongly: two Done when lines claim enforcement that only APEX-08b delivers. The first is the CLAUDE.md invariant edit adding `the configured vault's notes root`. CLAUDE.md says of that list "Each one is held by a test that names it", and no test holds this one until APEX-08b lands. The second is the ADR 0022 §3 note containing `enforced for every writer the run judges`, which is false until APEX-08b lands. Fix: move those two bullets and their two Verify greps into APEX-08b. APEX-08 keeps only the new ADR, with its status saying the enforcement is landing.; Done when needs judgment: the five 'It states' bullets for the new ADR can only be checked by reading the ADR, and Verify greps only its title. Add plain-text phrases a grep can match, e.g. `is the repository root` and `names [context] vault`, and grep them.
- apex APEX-08a: Files and Verify do not cover what the change forces: `Governance::denies_write_at` returns `Option<&Rule>`, and the Done when wants the refusal's wording (`[context] vault = "vault"`) from that result. One way is a new `Rule` variant: then crates/trellis-cli/src/process_agent.rs:159-160 words the refusal as ``deny_write `...` `` until APEX-08b, the very misreport this ticket's text says to avoid. The other way changes the return type: then process_agent.rs and run_cmd.rs:2010 stop compiling, and neither file is in Files. The ticket must say which approach it takes. Verify must add `cargo build -p trellis-cli`: as written, a break in another crate passes Verify, because Verify only runs trellis-loop.
