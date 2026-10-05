# colibri: web dashboard, desktop shell and site, UI inventory

- **Upstream:** JustVugg/colibri, commit `ce370e87d7b623d7759b52ec2007d75fc5b0e87e` (2026-09-25, "Merge pull request #1663 from JustVugg/dev"), from `git log -1` in the clone.
- **Licence:** Apache-2.0 (`LICENSE`). `NOTICE` reads "Copyright 2026 Vincenzo Fornaro".
  - The Bytesized pixel font in `web/public/fonts/` is SIL OFL-1.1 (`OFL.txt`).
  - `THIRD_PARTY_NOTICES.md` covers only engine code (ds4/ggml MIT, DeepGEMM headers).
  - pi, Apex and Paperclip were MIT (ADR 0021). Porting colibri *code* is different: it carries Apache-2.0 §4 duties (keep the NOTICE, mark changed files).
- **Scope:**
  - The directories asked for: `web/`, `desktop/`, `site/` and `docs/media/`.
  - The README's dashboard sections.
  - The gateway and CLI code that serves the dashboard (`c/openai_server.py`, `c/coli`) and the CI/release steps that build it. Without them the UI cannot be described.
- **Ids:** COL-UI-01 to COL-UI-106. Paths without a directory prefix are under `web/src/`.

## Method

**What was read.** Nothing was run, built or modified.

- **Read in full:**
  - Every file in `web/src` except the non-English i18n bodies.
  - `web/` config (`package.json`, `vite.config.ts`, `components.json`, `tsconfig*`, `index.html`, `README.md`).
  - `web/tests/ui_migration.py` and `web/tests/brio_ui.py`.
  - Every file in `desktop/` except the binary icons.
- **Grepped for packages:** `desktop/src-tauri/Cargo.lock`, for every `tauri*`, `wry` and `tao` package.
- **Read in part:**
  - `site/index.html`: head, header, hero, Brio and "yours" sections, and both scripts.
  - The README sections "See it running", "3. Run it" and the layout tree.
  - `docs/api.md`: "Web dashboard" and CORS.
  - `docs/brio.md`: "In the browser" and "Limits".
  - `c/openai_server.py`: CORS table, `serve_static`, `/health`, `/experts`, `/profile` and endpoint dispatch.
  - `c/coli`: `cmd_web` and its parser.
  - CI: `ci.yml` web job, `release.yml` web steps, `site.yml`.
- **Screenshots opened:**
  - `docs/media/colibri-dashboard.png`
  - `docs/media/colibri-mobile.png`
  - `docs/media/colibri-brain-region.png`
  - `docs/media/colibri-profiling.png`
- **TRELLIS docs read:** `docs/MISSION.md`, `docs/BRIEF.md` Part 1 and Part 5, `docs/ROADMAP.md` non-goals, ADR 0021 in full, the three inventories, ADR 0010, ADR 0004, and ADR 0024 "Not built".
- **TRELLIS code read:** `crates/trellis-model` (`spec.rs`, `runtime.rs`, `hosted.rs`, `grammar.rs`), `crates/tui-demo` (`app.rs`, `message.rs`, `view.rs`, `terminal.rs`, `events.rs`), and in `crates/trellis-cli`: `tui_bridge.rs`, `tui_run.rs`, `session.rs`, `session_extra.rs`, `rpc.rs`, `config.rs`, `main.rs`, `ask.rs`, `models_cmd.rs`.

**One row per distinct control or behaviour.** Where a cluster is one feature (a Brio question card, the runtime grid), every sub-control is named in the row.

**TRELLIS status values:**

- **built:** TRELLIS has the capability, possibly on another surface (the notes say which).
- **partial:** some of it exists.
- **missing:** absent, and no ADR excludes it.
- **excluded:** an ADR or brief clause rules it out (the clause is quoted).

**Dispositions:**

- **already-built:** nothing to do. TRELLIS has it.
- **drive-via-runtime:** the capability lives in colibri's engine or gateway. TRELLIS reaches it in one of two ways, and needs no GUI either way:
  - pointing a model spec at the server (`openai-chat:http://127.0.0.1:8000#<model-id>` works without a key on plain http, per `hosted.rs` `with_key_and_env`);
  - reading one of its JSON endpoints.
- **port:** reimplement natively in TRELLIS's own surfaces (CLI, `session`, `tui`, `rpc`).
- **desktop-ui:** only meaningful in a graphical shell. Every such row is excluded today (X-70). The disposition applies only if an ADR amends ADR 0021's decision #70. The orchestrator's planning list mentions "Tauri desktop tickets", but that is not owner approval and changes no status here.
- **owner-decision:** conflicts with an ADR or brief clause, or needs a product call, before anything is built.

**Exclusion clause codes** (the full clause also appears in each row's `trellis_evidence`):

- **X-70:** ADR 0021 "Owner decisions of 2026-09-28" (#70): "Desktop and web interfaces stay excluded under brief Part 1 ('any surface beyond the CLI')". BRIEF Part 1 Non-goals: "Any surface beyond the CLI."
- **X-THEME:** ADR 0021 "Cosmetic and [ui] exclusions, itemised" (#74): "Themes: excluded, cosmetic." Plus X-70.
- **X-PKG:** ADR 0021 Apex row 38–42 "build, packaging, native, self-update", excluded [ui/desktop]. Plus X-70.
- **X-NET:** BRIEF Part 1 Non-goals: "Any networked component." (ROADMAP v1.0: "No telemetry, no sync, no hosted anything.") Plus X-70.

## The desktop app

`desktop/` is a Tauri v2 shell around the same React UI. There is no second frontend.

**Versions and wiring:**

- Crate `colibri-desktop` 0.1.0, edition 2024, rust-version 1.85.
- `tauri` 2.11.5 and `tauri-build` 2.6.3. The lock adds runtime 2.11.3, runtime-wry 2.11.4, wry 0.55.1, tao 0.35.3 and utils 2.9.3.
- Dev mode runs Vite on `http://localhost:5173`. Release builds package `../../web/dist`.

**What it does not have:**

- **No IPC commands.** It calls `Builder::default()` with no `invoke_handler`, and the web side has no `@tauri-apps/api` dependency.
- **No plugins.** The lock has no `tauri-plugin-*` package.
- **One capability:** `core:default` on window `main`.
- **A CSP that allows only loopback connections:**
  - `connect-src 'self' ipc: http://ipc.localhost http://127.0.0.1:* http://localhost:*`
  - `img-src 'self' asset: http://asset.localhost`
- **No updater.**
- **No sidecar.** The README defers bundling or managing the engine on purpose ("the model is hundreds of gigabytes and must remain an external, user-selected resource").
- **No signing, and no CI or release job builds it.**

**What it does have:**

- Bundle targets `all`, category `DeveloperTool`, icons.
- One 1280×820 window (minimum 860×600).
- Mobile scaffolding (`mobile_entry_point`, `TAURI_DEV_HOST`) but no mobile project.

The UI talks to the engine purely over HTTP, exactly as the browser build does. The task text named `desktop/src/`, which does not exist. The Rust sources are in `desktop/src-tauri/src/`.

## How the UI reaches the engine

| Endpoint | Used by | Cadence | Auth on the server |
|---|---|---|---|
| `GET /v1/models` | Probe server | on demand | `require_auth` |
| `GET /health` | Probe server, runtime panel, slot and Continue gating | 5 s while connected and visible | status is public; details only when authed (#SEC-8) |
| `POST /v1/chat/completions` (SSE) | Chat | per turn | `require_auth` |
| `POST /v1/brio` | Brio | per question | `require_auth` |
| `GET /experts` | Brain › Live routing | 1.5 s | data only when authed |
| `GET /profile` | Profiling | 2 s while visible | data only when authed |
| `GET /experts.json` (static) | Brain › Explore, tooltip | once | none (static file) |

The gateway also serves endpoints the UI never calls: `/v1/completions`, `/v1/messages`, `/v1/systemone` and `/metrics` (Prometheus).

## Features

### A. Workspace shell and navigation

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-01 | Workspace shell | Left icon rail, a workspace that swaps chat/brio/brain/profiling/settings, and a nav dock; Brain goes full-bleed | App.tsx:410-500; chat-design.css (.rail, .workspace) | excluded | X-70; TRELLIS surfaces are the ratatui TUI (tui-demo view.rs:18) and `session` | desktop-ui |
| COL-UI-02 | Rail brand button | Pixel hummingbird; click returns to Chat | App.tsx:412; components/Brand.tsx | excluded | X-70 | desktop-ui |
| COL-UI-03 | New chat | Archives the current slot's conversation in memory, clears messages, metrics and draft, focuses the composer; disabled while streaming | App.tsx:321-326, 413 | missing | ADR 0021 "Where this stands": W-D `/new` not built; no `/new` arm in session.rs:291-392 | port |
| COL-UI-04 | Conversations dialog | `<dialog>` from both Search and History buttons; substring search over live slots and archives; restore an archive into its slot; Esc or backdrop closes; session-only note | App.tsx:85-96, 334-340, 414-415, 491-499 | partial | `trellis log` (--json), `/resume <run-id>` (ADR 0038); no search; branching deferred (ADR 0021 pi 8.1–8.4) | port |
| COL-UI-05 | Navigation dock | Top-edge handle revealing Chat/Brio/Brain/Profiling: hover with 850 ms hide, tap, pull-down swipe, Enter focuses first item, Esc/outside closes, busy dot on Chat | components/NavigationDock.tsx:1-45; tests/ui_migration.py | excluded | X-70 | desktop-ui |
| COL-UI-06 | Workspace topbar | Model button (opens Model tab), connection pill (opens Connection tab), Export and Clear on Chat; hidden on Brain | App.tsx:422-428 | partial | TUI status bar shows model, tier, clean/dirty (view.rs:103); no reachability | port |
| COL-UI-07 | Pages kept mounted | Chat and Brio are hidden rather than unmounted, so a Brio run and a draft survive navigation | App.tsx:443, 485-488; tests/brio_ui.py:58-66 | excluded | X-70 | desktop-ui |

### B. Settings, connection, model and runtime panel

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-08 | Settings page | Heading, four tabs (General, Model, Connection, System), error banner; General holds theme cards, language select and the input hint | App.tsx:84, 320, 429-440 | excluded | X-70; settings are files (ADR 0025 §7, config.rs, user_config.rs) | desktop-ui |
| COL-UI-09 | API endpoint field | Base URL; default `http://127.0.0.1:8000/v1` or `<origin>/v1` when served by the engine; persisted | App.tsx:56-62, 344; lib/storage.ts | built | base URL is part of the spec (spec.rs:20-49); --model, [model] spec, /model, rpc set_model | already-built |
| COL-UI-10 | API key field | Password field kept in memory only, sent as Bearer on every request incl. /health, /profile, /experts; legacy stored key deleted | App.tsx:63, 345; lib/api.ts:108-113; Brain.tsx:62; lib/storage.ts | partial | keys from environment only (hosted.rs:49-53, 660-680; ADR 0010, ADR 0024); no key needed on plain-http loopback | owner-decision |
| COL-UI-11 | Probe server | Aborts prior probe, GET /models, picks the first model if needed, marks connected, then GET /health; spinner | App.tsx:160-187, 342-351; lib/api.ts:125-136 | partial | /model then probe_model (session.rs:318-345, 463); `doctor`; probe_slots on /props (runtime.rs:317-335); no /v1/models fetch | port |
| COL-UI-12 | Connection-state indicator | aria-live dot and label; resets when endpoint or key changes | App.tsx:118-124, 278, 350, 425 | partial | status bar model and tier, TierKnown; errors as `!` lines (tui_run.rs:140-152) | port |
| COL-UI-13 | Auto-connect when served | If not on port 5173 and over http(s), default the endpoint from the page origin and probe once | App.tsx:56-62, 98, 189-192 | partial | TUI probes the configured runtime (TierKnown) | port |
| COL-UI-14 | Model selector | Select of /v1/models ids; default glm-5.2-colibri; persisted | App.tsx:65, 169, 354 | built | --model, /model, /models (session_extra.rs:123), rpc set_model, `#<model>` in spec | already-built |
| COL-UI-15 | KV session (slot) selector | Shown when kv_slots > 1; one conversation per slot; colibri-only `cache_slot` sent only when advertised | App.tsx:69-70, 102-112, 150-155, 242, 355-357; lib/runtime.ts; lib/api.ts:189; lib/api.test.ts | missing | probe_slots reads capacity (S18), does not select a slot (runtime.rs:317-335) | drive-via-runtime |
| COL-UI-16 | Slot indicators | Composer "slot N" chip (opens Model tab) and metrics badge | App.tsx:407, 477 | excluded | X-70 | desktop-ui |
| COL-UI-17 | Temperature slider | Range 0–2, step 0.1, default 0.7 | App.tsx:66, 358 | missing | fixed 0.0 to runtimes (runtime.rs:186-213); omitted for hosted (hosted.rs:263-264) | owner-decision |
| COL-UI-18 | Max output tokens | 1–32768, default 4096, sent as max_completion_tokens | App.tsx:67, 359; lib/api.ts:187 | partial | automatic cap 4096 rising to 16384 (adapter.rs:509, 516); hosted 16000; run budget [budget] max_tokens | port |
| COL-UI-19 | Reasoning toggle | Model-tab toggle and composer chip; sends `enable_thinking` | App.tsx:68, 241, 360-362, 476; lib/api.ts:188 | partial | [model] thinking off/low/medium/high (config.rs:1180-1188), sent as reasoning_effort or Anthropic adaptive thinking (hosted.rs:273, 335); config-level only | drive-via-runtime |
| COL-UI-20 | Controls locked while streaming | Fieldsets, New chat, Clear, history and Regenerate disabled during a generation | App.tsx:323, 342, 352, 413, 426, 455, 495 | built | AppState.running refuses a second task; typed lines become steers (app.rs:37-52) | already-built |
| COL-UI-21 | Hardware panel | CPU, GPU count with VRAM, RAM total and free, cores (from /health hwinfo) | App.tsx:366-371; lib/api.ts:49-57; openai_server.py:4328-4329 | missing | nothing reads runtime hardware | drive-via-runtime |
| COL-UI-22 | Scheduler panel | Active/capacity, Queued/max, Completed, Failures; "Scheduler online · N KV"; unavailable fallback | App.tsx:103-106, 372-378, 398-399; lib/runtime.ts | missing | only llama.cpp total_slots is read | drive-via-runtime |
| COL-UI-23 | Expert tier bar | Stacked VRAM/RAM/disk residency bar with counts and GB | App.tsx:379-394; index.css:92-103 | missing | no residency concept; TRELLIS loads no weights itself | drive-via-runtime |
| COL-UI-24 | Session token totals | Running prompt + completion totals from usage | App.tsx:80, 272-275, 395-397 | built | Budget tokens per run (app.rs:118-122; tui_bridge.rs:257); token ledger (ADR 0026) | already-built |
| COL-UI-25 | Health polling | /health every 5 s while connected, skipped when hidden | App.tsx:132-147 | missing | TUI ticks but polls nothing | port |

### C. Chat

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-26 | Empty-state wordmark | Pixel mark + Bytesized "colibri" above the composer | App.tsx:444; tests/ui_migration.py:94-104 | excluded | X-70 | desktop-ui |
| COL-UI-27 | Suggestion chips | Three buttons that prefill starter prompts | App.tsx:481; i18n/en.ts:20-22, 177-179 | built | prompt templates, `/prompts` (ADR 0025 §3, §6) | already-built |
| COL-UI-28 | Message list | role=log; user bubbles; assistant with avatar and Markdown; typing dots; image thumbnails; auto-scroll | App.tsx:158, 444-457 | built | transcript pane, 2000-line cap, ↑↓ scroll (app.rs:10-14, 53-62; view.rs:144) | already-built |
| COL-UI-29 | Reasoning stream and block | reasoning_content kept apart, shown in a collapsible block, never resent | lib/api.ts:7-10, 180-185, 206-213; App.tsx:244-254, 449-453 | partial | Anthropic thinking replayed (hosted.rs:118, 164-188); not displayed; OpenAI reasoning_content not parsed | port |
| COL-UI-30 | Copy response | Copies a reply; "Copied", or an error on failure | App.tsx:331-333, 455 | built | `/copy` to the clipboard program (session.rs:360; session_extra.rs:177-187) | already-built |
| COL-UI-31 | Regenerate | Resends the last user turn with its images; keeps the draft | App.tsx:294-307, 455; lib/chat.ts; lib/chat.test.ts | missing | no retry of a finished turn | port |
| COL-UI-32 | Continue reply | Offered only when /health continue_assistant is true and the last turn was cut off; streams into the same bubble | App.tsx:309-318, 455; lib/transcript.ts:15-40; lib/runtime.ts:10-13 | missing | empty reply raises the cap and retries (adapter.rs:509-516) | drive-via-runtime |
| COL-UI-33 | Finish-state tracking | Per-message finish: server reason, aborted, error, incomplete; empty bubble dropped | lib/transcript.ts:8-14; App.tsx:277-287 | partial | hosted finish_reason checks (hosted.rs:416-430); trail end reason | port |
| COL-UI-34 | Truncation warning | "Truncated" badge and a note when finish_reason is length | App.tsx:405, 482 | partial | "stopped at max_tokens" error (hosted.rs:422-423) | port |
| COL-UI-35 | Export conversation | Downloads {model, messages} as colibri-chat.json | App.tsx:327-330, 426 | built | `trellis log --json` (ADR 0021 pi 8.8) | already-built |
| COL-UI-36 | Clear conversation | Empties the slot and resets badges and totals | App.tsx:321, 426 | missing | no clearable conversation state | port |
| COL-UI-37 | Composer | Textarea; Enter sends, Shift+Enter newline, IME-safe; send gated on text/image and model | App.tsx:213, 460-480 | built | TUI input submits on Enter, @ completion (app.rs:31-35, 82, 309; view.rs:421) | already-built |
| COL-UI-38 | Stop generation | Stop aborts the fetch and the server cancels; partial text kept | App.tsx:99, 231-233, 478; tests/ui_migration.py | built | Ctrl-C (events.rs:130-136); rpc abort rolls back (rpc.rs:40) | already-built |
| COL-UI-39 | Image attachments | Picker, paste and drop; data-URI thumbnails with remove; resent as image_url parts | App.tsx:91-93, 194-211, 447-448, 460-475; lib/api.ts:177-185 | partial | `run --image`, hosted tier 1 only, ≤5 MiB (main.rs:200-208; ADR 0041) | drive-via-runtime |
| COL-UI-40 | Error banner | role=alert with the server's error.message | App.tsx:434, 459; lib/api.ts:115-123 | built | errors as `!` transcript lines (tui_run.rs:140-152, 182) | already-built |

### D. Live metrics

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-41 | Live token counter | "N tokens" badge while streaming | App.tsx:249-258, 401 | partial | BudgetUpdated tokens (tui_bridge.rs:257) | port |
| COL-UI-42 | Decode speed | tok/s from the first token, prefill excluded | App.tsx:251-271, 402 | missing | tokens and elapsed recorded, no rate | port |
| COL-UI-43 | Time to first token | TTFT badge, to the first reasoning or content token | App.tsx:79, 248, 256, 403 | missing | replies read whole (ADR 0024 "Not built: Streaming") | port |
| COL-UI-44 | Per-turn usage badge | prompt→completion from the final usage chunk | App.tsx:404; lib/api.ts:191, 215 | partial | completion tokens counted; prompt-token accounting not built (ADR 0021 W-E) | port |
| COL-UI-45 | Queue-wait badge | From the `x-colibri-queue-wait-ms` header; `x-request-id` captured | lib/api.ts:227-234; App.tsx:406 | missing | headers not read | drive-via-runtime |

### E. API client and persistence

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-46 | Streaming chat client | POST /chat/completions with stream:true and include_usage; SSE framing tolerant of partial frames and CRLF | lib/api.ts:144-235; lib/api.test.ts; openai_server.py:4394 | partial | openai-chat: spec, no key needed on http (spec.rs:39-46; hosted.rs:193-275); streaming stays out (ADR 0024 #151) | drive-via-runtime |
| COL-UI-47 | Generic-backend degradation | Works with a 404 /health and without kv_slots; never requests /v1/health | lib/runtime.ts; tests/ui_migration.py | built | probing; missing /props is "unknown", not 1 (runtime.rs:717-747); ADR 0003 | already-built |
| COL-UI-48 | Local settings persistence | localStorage baseUrl, model, theme, locale, all in try/catch | lib/storage.ts; App.tsx:58-65, 88, 95; i18n/index.ts:207-248 | built | config files (ADR 0025 §7) | already-built |
| COL-UI-49 | Memory-only conversations | Lost on reload; export to keep | App.tsx:70, 87; i18n/en.ts:33 | built | trail committed per run (ADR 0007) | already-built |

### F. Brio

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-50 | Brio scoring capability | POST /v1/brio {model, state, question, options}; returns probabilities, entropy, and usage with 0 generated tokens | lib/api.ts:237-274; openai_server.py:4398; docs/brio.md; README.md:509-539 | missing | nearest: llguidance validation (grammar.rs:1-15), ask_human ≤8 options (agent.rs:112-127), ask.rs | owner-decision |
| COL-UI-51 | Brio document card | Textarea, load a text file, refuses binary files, shows name · size · chars and the "read once" hint | Brio.tsx:68-77, 99-115 | excluded | X-70 | desktop-ui |
| COL-UI-52 | Brio question rows | Question input, own option lines (deduplicated), removable chips, warning when only one option, add/remove question | Brio.tsx:22-39, 59-66, 117-146, 177-180 | excluded | X-70 | desktop-ui |
| COL-UI-53 | Brio run and stop | "Answer N"; questions sent one at a time with per-question seconds; Stop aborts to free the KV slot; offline note | Brio.tsx:52-57, 62-63, 79-96, 147-150, 181-189 | excluded | X-70 | desktop-ui |
| COL-UI-54 | Brio results | Bars per option, top answer, entropy level (sure <0.4, unsure <0.8), tokens read, 0 generated | Brio.tsx:22, 151-172; tests/brio_ui.py | excluded | X-70 | desktop-ui |

### G. Brain: Explore

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-55 | Brain mode tabs | Explore / Live routing pill tabs | BrainWorkspace.tsx:200-210 | excluded | X-70 | desktop-ui |
| COL-UI-56 | Expert atlas loading | experts.json (13,260 GLM-5.2 experts, 10 categories); loading, error and Retry; checked in release | BrainWorkspace.tsx:218-234, 309; web/public/experts.json; release.yml:103, 268 | excluded | X-70 | desktop-ui |
| COL-UI-57 | Cortex renderer | Canvas-2D 3D hemispheres, ≤36 sampled experts per topic, illustrative arcs (not recorded routes), tweens, DPR ≤2 | lib/cortex.ts:1-97; lib/brain-topics.ts | excluded | X-70 | desktop-ui |
| COL-UI-58 | Topic regions overview | Ten region buttons with counts; "Choose a region" | BrainWorkspace.tsx:310, 312 | excluded | X-70 | desktop-ui |
| COL-UI-59 | Region and expert drill-down | Enter a topic, pick an expert; breadcrumb, Back, Overview | BrainWorkspace.tsx:262-279, 302, 306, 311-315 | excluded | X-70 | desktop-ui |
| COL-UI-60 | Camera controls | Drag, wheel, zoom buttons, arrow/+/-/Enter/Esc keys; clamped pitch and distance | BrainWorkspace.tsx:289-294, 303-308, 316-317; lib/cortex.ts:91-92 | excluded | X-70 | desktop-ui |
| COL-UI-61 | Guided journey | Tours the topics every 4.4 s; stops on interaction or a hidden tab | BrainWorkspace.tsx:280-288, 299 | excluded | X-70 | desktop-ui |
| COL-UI-62 | Animation pause and reduced motion | Pause/Resume; follows prefers-reduced-motion; camera jumps instead of tweening | BrainWorkspace.tsx:227, 255-261, 318; lib/cortex.ts:25-26, 87, 90 | excluded | X-70 | desktop-ui |
| COL-UI-63 | Topics and details panel | Expert select, specialist/generalist, entropy, top-4 affinities, topic search and grid, source note | BrainWorkspace.tsx:267-271, 300, 320-328 | excluded | X-70 | desktop-ui |
| COL-UI-64 | Node label overlay | ≤6 non-overlapping L·E labels, 100 ms throttle | lib/cortex.ts:77-82; BrainWorkspace.tsx:311 | excluded | X-70 | desktop-ui |

### H. Brain: Live routing

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-65 | Live expert grid | /experts every 1.5 s; one cell per expert; colour = tier, brightness = heat | Brain.tsx:7-11, 44-131 | excluded | X-70 | desktop-ui |
| COL-UI-66 | Routing pulses and residue | Experts routed this turn flash white and decay; a residue builds with repeat routing | Brain.tsx:32-34, 68-78, 98-130 | excluded | X-70 | desktop-ui |
| COL-UI-67 | Live legend and states | Layers × experts header, tier totals, hints, error flag | Brain.tsx:29, 79, 147-168 | excluded | X-70 | desktop-ui |
| COL-UI-68 | Expert hover tooltip | Layer (row+3, MTP=78), tier, heat, atlas affinities or depth role | Brain.tsx:13-21, 133-145, 169-188 | excluded | X-70 | desktop-ui |

### I. Profiling

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-69 | Profiling data and table | /profile every 2 s; derived "other" and tok/s; table of the last 40 turns plus a disk note | Profiling.tsx:7-23, 271-293, 330-347; openai_server.py:48, 4342-4356 | missing | per-run wall clock and iterations only (MISSION goal 7) | drive-via-runtime |
| COL-UI-70 | Summary tiles | tok/s, wall time, tokens/forward, disk service | Profiling.tsx:307-312 | excluded | X-70 | desktop-ui |
| COL-UI-71 | Phase share bars | Last turn and window split by I/O wait, matmul, attention, LM head, other | Profiling.tsx:200-237, 314-317 | excluded | X-70 | desktop-ui |
| COL-UI-72 | Per-turn charts | Throughput columns and phase-stacked columns with hover readout | Profiling.tsx:239-269, 319-328 | excluded | X-70 | desktop-ui |
| COL-UI-73 | Profiling chrome | Heading, live-metrics strip, legend, empty/connect text | App.tsx:442; Profiling.tsx:295-305 | excluded | X-70 | desktop-ui |

### J. Theme, language, layout, rendering

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-74 | Theme switching | Rail Sun/Moon and Settings cards set data-theme; persisted; dark default; ignores the OS preference | App.tsx:88, 95, 418, 436 | excluded | X-THEME | desktop-ui |
| COL-UI-75 | Theme tokens | CSS variables for dark and light, Tailwind @theme mapping, fixed Brain palette | index.css:1-32; chat-design.css; brain-design.css; web/index.html | excluded | X-THEME | desktop-ui |
| COL-UI-76 | Localisation | Six locales, detection, interpolation, English fallback, selector | i18n/index.ts; i18n/*.ts; App.tsx:436 | missing | English only; no ADR | owner-decision |
| COL-UI-77 | Responsive layout | 760 px and 600 px breakpoints, Brain 1100/760, Profiling 900; tested at four viewports | chat-design.css:35-36, 143; brain-design.css:7-8; tests/ui_migration.py:94-104 | excluded | X-70 (TUI recomputes its layout, view.rs:1-4, 82) | desktop-ui |
| COL-UI-78 | Accessibility semantics | aria labels, live regions, roles, focus-visible outlines, dialog, keyboard paths | App.tsx:342-500; NavigationDock.tsx; BrainWorkspace.tsx:299-328; index.css:39 | excluded | X-70 | desktop-ui |
| COL-UI-79 | Safe Markdown rendering | React-node subset, code blocks with copy, links limited to http(s)/mailto; tables and raw HTML shown as text | components/Markdown.tsx:1-146 | missing | TUI shows plain lines; HTML of untrusted text is the injection surface in ADR 0021 pi 8.8 (#143) | port |
| COL-UI-80 | Error boundary | Shows the crash with "engine unaffected" and a Retry | ErrorBoundary.tsx; main.tsx | built | panic hook restores the terminal (terminal.rs:1-33) | already-built |
| COL-UI-81 | Brand mark and font | Pixel SVG, Bytesized font (OFL), favicon | components/Brand.tsx; web/index.html; web/public/fonts | excluded | X-70 | desktop-ui |
| COL-UI-82 | UI primitives | shadcn-style Button, Input, Textarea, Badge | components/ui/*.tsx; lib/utils.ts | excluded | X-70 | desktop-ui |

### K. Serving and engine endpoints

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-83 | One-process dashboard serving | serve_static web/dist with SPA fallback; `coli web` opens a browser once /health answers; `coli serve` headless | c/openai_server.py:4261-4300; c/coli:2300-2341, 2664-2676 | excluded | X-NET | owner-decision |
| COL-UI-84 | /health contract | Status public; scheduler, kv_slots, continue_assistant, tiers, hwinfo only when authed | c/openai_server.py:4318-4331 | missing | reads /props only | drive-via-runtime |
| COL-UI-85 | /experts contract | rows, cols, hex map (tier in the top 2 bits, heat in the low 6), hits bitmap, seq; authed only | c/openai_server.py:4333-4341 | missing | no routing telemetry | drive-via-runtime |
| COL-UI-86 | /profile contract | Up to 120 per-turn PROF snapshots; authed only | c/openai_server.py:48, 4342-4356 | missing | no engine timings | drive-via-runtime |
| COL-UI-87 | CORS and Host guard | Default origins include :5173, tauri.localhost, tauri://localhost; --cors-origin; --allowed-host | c/openai_server.py:49-56, 4163-4175, 4371; docs/api.md:112-128 | excluded | X-NET | drive-via-runtime |

### L. Build, tests, release

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-88 | Web stack and dev server | React 18, TS 7, Vite 8, Tailwind 4, shadcn config, lucide; port 5173 and TAURI_DEV_HOST | web/package.json; web/vite.config.ts; web/components.json | excluded | X-70 | owner-decision |
| COL-UI-89 | Unit tests and CI web job | 25 vitest cases on pure helpers; CI runs npm ci, build, vitest | web/src/lib/*.test.ts; ci.yml:1201-1219 | built | pure AppState::update tests (app.rs:1-5) | already-built |
| COL-UI-90 | Browser end-to-end scripts | Playwright ui_migration.py and brio_ui.py against a fixture server; not in CI | web/tests/*.py | excluded | X-70 | desktop-ui |
| COL-UI-91 | Release packaging of the dashboard | Builds web/dist and ships it next to coli in archives; asserts index.html and experts.json | release.yml:85-103, 154-157, 267-269 | excluded | X-PKG | owner-decision |

### M. Desktop (Tauri)

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-92 | Tauri v2 shell crate | colibri-desktop 0.1.0, tauri 2.11.5, tauri-build 2.6.3, wry 0.55.1, tao 0.35.3; no console in release | desktop/src-tauri/Cargo.toml; Cargo.lock:2873-3134, 4213; src/main.rs; src/lib.rs | excluded | X-70 | owner-decision |
| COL-UI-93 | IPC surface: none | `Builder::default()`, no invoke_handler, no @tauri-apps/api; HTTP only | desktop/src-tauri/src/lib.rs:1-6; web/package.json | excluded | X-70; `trellis rpc` is the front-end channel (ADR 0025 §5) | owner-decision |
| COL-UI-94 | Plugins: none | No tauri-plugin-* in the lock | desktop/src-tauri/Cargo.toml; Cargo.lock | excluded | X-70 | owner-decision |
| COL-UI-95 | Capability and permissions | `core:default` on `main` only; schema generated, not committed | desktop/src-tauri/capabilities/default.json | excluded | X-70 | owner-decision |
| COL-UI-96 | Content Security Policy | Loopback-only connect-src; img-src has no data: | desktop/src-tauri/tauri.conf.json:24-26 | excluded | X-70 | owner-decision |
| COL-UI-97 | Main window and identity | "colibrì", 1280×820, min 860×600, centred, resizable; org.colibri.desktop | desktop/src-tauri/tauri.conf.json:1-23 | excluded | X-70 | desktop-ui |
| COL-UI-98 | Frontend wiring | before-dev/build npm commands, devUrl :5173, frontendDist ../../web/dist | desktop/src-tauri/tauri.conf.json:5-10; desktop/README.md | excluded | X-70 | owner-decision |
| COL-UI-99 | Bundling | targets all, DeveloperTool, icons; no signing; not built by CI or release | desktop/src-tauri/tauri.conf.json:27-41; icons/; .github/workflows | excluded | X-PKG | owner-decision |
| COL-UI-100 | Updater: none | No updater plugin, config or artifacts | desktop/src-tauri/tauri.conf.json; Cargo.toml | excluded | X-PKG | owner-decision |
| COL-UI-101 | Engine sidecar: none | No externalBin; bundling or managing the engine deferred by design | desktop/README.md:21-28 | missing | TRELLIS spawns agents (ADR 0026, 0035) and pulls weights (ADR 0044); never starts a runtime; router deferred (ADR 0021 pi 2.10) | owner-decision |
| COL-UI-102 | Mobile scaffolding | mobile_entry_point and TAURI_DEV_HOST, no mobile project | desktop/src-tauri/src/lib.rs:1; web/vite.config.ts | excluded | X-70 | owner-decision |

### N. Site

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-103 | Landing page | Static page on GitHub Pages: header, demo video, hardware strip, scroll-spy rail, Brio charts and JSON, closing, footer | site/index.html:1-14, 216-498, 531-538; site/demo.webm; .github/workflows/site.yml | missing | no website; no ADR | owner-decision |
| COL-UI-104 | Site model catalogue | Nine model rows, copyable download commands, search, tag pills | site/index.html:228-231, 269-356, 500-530 | built | `trellis model`, `model pull` (ADR 0044), `model path` | already-built |
| COL-UI-105 | Site theme | Pre-paint theme from storage or OS preference, toggle, D shortcut, follows OS changes | site/index.html:15-25, 232-237, 539-575 | excluded | X-THEME | owner-decision |

### O. Docs and media

| ID | Feature | What | Upstream evidence | Status | TRELLIS evidence | Disposition |
|---|---|---|---|---|---|---|
| COL-UI-106 | Dashboard docs and screenshots | README "See it running" (5 shots), docs/api.md "Web dashboard" plus a mobile shot; four engine diagrams that are not UI | README.md:49-85, 505-506; docs/api.md:399-445; docs/media/*.png | partial | ui_examples/preview*.webp exist but README does not use them | port |

## Totals

| | built | partial | missing | excluded | total |
|---|---|---|---|---|---|
| TRELLIS status | 17 | 16 | 22 | 51 | 106 |

| | already-built | drive-via-runtime | port | desktop-ui | owner-decision |
|---|---|---|---|---|---|
| Disposition | 17 | 14 | 19 | 37 | 19 |

## Defects and inconsistencies noticed while reading

None of these were run or verified. They come from reading the code.

1. **Desktop image thumbnails probably break.**
   - `tauri.conf.json` CSP `img-src 'self' asset: http://asset.localhost` does not list `data:`.
   - The chat renders attachment previews and sent images as data-URI `<img>` (App.tsx:447-448, 465).
2. **Windows desktop default endpoint.**
   - App.tsx:56-60 treats any http(s) origin not on port 5173 as "served by the engine".
   - Tauri on Windows serves from `http://tauri.localhost`, an origin the gateway's `DEFAULT_CORS_ORIGINS` already anticipates.
   - So the desktop release build would default to `http://tauri.localhost/v1` and auto-probe the webview itself.
   - A saved `http://127.0.0.1:8000/v1` is also overridden.
3. **Remote engines are unreachable from the desktop.** CSP `connect-src` allows only 127.0.0.1 and localhost, yet the endpoint field accepts any URL.
4. **GLM-5.2 layer numbers on every model.** The Live routing tooltip hardcodes `row + 3` and MTP = layer 78 (Brain.tsx:170-171), whatever model is running.
5. **Docs and code disagree on the Profiling window.** README and docs/api.md say "the last 30 turns". Profiling.tsx:291 charts the last 40. The server keeps 120 (`PROFILE_TURNS`).
6. **Stale README alt text.** The dashboard screenshot is described as "live metrics, hardware panel, expert tiers". Since the 1.12.0 redesign those live under Settings › System; the screenshot shows the chat workspace.
7. **Duplicate rail buttons.** Search and History open the same dialog (App.tsx:414-415).
8. **Stale and incomplete strings.**
   - `ui.navigation` reads "Chat, Brain and Profiling" and omits Brio (i18n/en.ts:17).
   - de, zh-CN and zh-TW have 134, 115 and 115 lines against 238 for en, and fall back to English key by key.
   - `<html lang>` stays `en` whatever the locale.
9. **The site and the app theme differently.**
   - Site: `colibri_theme`, OS preference, D shortcut.
   - App: `colibri.theme`, always dark, OS preference ignored.
10. **The desktop build is never checked.**
    - No CI or release job builds `desktop/`.
    - `capabilities/default.json` points at an uncommitted `gen/schemas` file.
    - The Playwright scripts are not in CI either.

## Open questions for the owner

1. **Does TRELLIS get a desktop or web surface at all?**
   - All 37 desktop-ui rows and the shell rows depend on an ADR amending ADR 0021 #70 and BRIEF Part 1.
   - If yes, colibri's zero-IPC, zero-plugin shell is a poor template: a TRELLIS front end must drive the orchestrator, through `trellis rpc` as a sidecar or the crates linked in.
   - colibri deliberately does neither: it has no process management and no IPC.
2. **Should colibri be a documented TRELLIS runtime?**
   - The likely recipe is `--model openai-chat:http://127.0.0.1:8000#<model-id>` (tier 1, native tools on GLM, Kimi K3, DeepSeek V4; docs/api.md says Qwen3.8 and OLMoE return 400 on tools).
   - Tier 2 through `vllm:` probably runs unconstrained: colibri's `/v1/completions` takes `response_format` (openai_server.py:3097-3108), not `guided_json`. Unverified.
   - No run has been measured.
3. **KV slot pinning.** Should partition agents (ADR 0016, ADR 0037) send colibri's `cache_slot` so each keeps its prefix? docs/brio.md warns that with one slot, interleaved clients evict each other.
4. **Brio.**
   - A probability or entropy is the model's self-report and can never produce a `Verdict` (ADR 0004).
   - Is closed-set scoring wanted for classification, such as `ask.rs` task-vs-answer or ticket triage, with an entropy threshold that routes to a human?
5. **Localisation.** TRELLIS is English-only, and no ADR addresses it.
6. **Temperature.** It is fixed at 0.0 for runtimes and omitted for hosted APIs. Should it be configurable?
7. **Keys typed into a UI.** ADR 0010 and ADR 0024 read keys from the environment only. May any front end accept a key in memory, as colibri does?
8. **A project website** comparable to `site/`.
9. **Licence.** If any colibri code is ported, Apache-2.0 §4 applies (NOTICE kept, changes marked). The earlier inventories were MIT.

## Coverage bounds

- **Read only partly:**
  - Non-English i18n bodies: by line count, plus the Italian labels the Playwright script uses.
  - `chat-design.css`: selectors and media queries, not every rule.
  - Unit tests: by test name.
  - `site/index.html`: CSS and per-model rows outlined.
  - `docs/brio.md`: only "In the browser" and "Limits".
  - `c/openai_server.py`: only the dashboard handlers, CORS and endpoint dispatch.
  - `web/public/experts.json`: by shape and counts.
- **Not opened:** `docs/media/colibri-brio.png`, `docs/media/colibri-brain.png` and the four engine diagrams.
- **Not read:** README translations (it, ja, zh-CN, zh-TW).
- **TRELLIS ADRs 0018, 0025, 0035, 0038, 0041 and 0044:** cited through ADR 0021 and the code, not read directly.
