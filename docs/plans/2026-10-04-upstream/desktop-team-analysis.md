# Desktop track: a Tauri interface for TRELLIS

Read for this analysis: the colibri inventory (COL-UI-01 to COL-UI-106, colibri at ce370e8) and colibri's `desktop/src-tauri/*`; on the TRELLIS side, `CLAUDE.md`, `docs/TICKETING.md`, the G0 and G6 gates (`scripts/gates/ticket-ready.sh`, `ticket-verify.sh`), `scripts/tickets/verify-tests.sh`, `docs/OPEN_QUESTIONS.md` Q-19, ADR 0021 (owner decisions, #70), ADR 0025 §4–5, ADR 0007, ADR 0010, ADR 0013, ADR 0024 §3, `crates/trellis-cli/src/{rpc.rs,output.rs,main.rs,tui_bridge.rs,tui_run.rs,session.rs,session_extra.rs,doctor_cmd.rs,index_cmd.rs}`, `crates/tui-demo/src/tui/{app.rs,message.rs,view.rs}`, `fuzz/Cargo.toml`, the root `Cargo.toml`, `deny.toml`, `scripts/gates/security.sh`, `.github/workflows/ci.yml` and `audit/DEPENDENCIES-2026-09-28.md`. Nothing was changed. One command was run: `get_state` and `quit` were piped into the installed `trellis --root <plain directory> rpc`. It answered with one `response` line (`success` true, `running` false) and wrote nothing to the directory.

## Product Manager

**Problem.** TRELLIS can be watched and steered from a terminal (`trellis tui`, `trellis session`) or from a pipe (`trellis rpc`). There is no window. ADR 0021's owner decision #70 keeps desktop and web interfaces out under BRIEF Part 1 ("any surface beyond the CLI"). It names `trellis rpc` as the supported way for an external front end to drive trellis, and says such a front end can be built outside this repository. So this track starts with an owner question. DESK-01 asks it, and ADR 0048 is the record.

**Who it is for.** Someone who would rather watch a run in a window than a terminal. In one run they want to see what the agent did and whether the checks passed, approve or reject an edit, steer the run or abort it, and then read the committed trail. It is not a chat app. Every prompt is a `trellis run`, with the same clean-worktree refusal, trust gate, checks, rollback and trail.

**Q-19 is the warning.** The owner made `tui-demo` the product and archived `chat` (#16) because two interfaces had diverged, and each change silently picked one of them. A desktop would bring that back unless it holds no run logic of its own. The rule in ADR 0048: the desktop only renders what trellis emits. A view the TUI lacks first lands in trellis as data (an rpc command), and only then in a surface. The TUI stays the product interface unless the owner says otherwise.

**v1 scope** (DESK-01 to DESK-12, DESK-19, DESK-20):
- **Run view:** actions log, action graph, checks, radius, budget, edits, approvals, steer, abort.
- **Trails view:** from `trellis log --json`.
- **Model picker, repository switch and status bar.**

**v1.1, once the owner answers DESK-13 to DESK-16:** codebase graph, symbol radius, queue, vault notes. Each needs a read-only rpc command, because today that data exists only as human text.

**Out of scope:**
- colibri's engine views (Brio, Brain, live routing, profiling).
- Installers, signing, updater, mobile, macOS.
- Theme toggle, localisation, brand assets.
- Any key field.

**Coverage of the inventory.** 43 of the 106 ids are mapped to 12 tickets and 63 are uncovered, each with a reason in `uncovered_features`. Every TRELLIS view named in the brief has a home:

| TRELLIS view | Ticket |
|---|---|
| actions log | DESK-09 |
| action graph | DESK-10 |
| codebase graph | DESK-17 (data: DESK-13) |
| radius | run pane DESK-09; symbol query DESK-17 (data: DESK-14) |
| checks | DESK-09 |
| budget | DESK-09 (state: DESK-08) |
| trails | DESK-11 |
| vault notes | DESK-18 (data: DESK-16) |
| queue | DESK-18 (data: DESK-15) |

**Mapping of every colibri element:**

| Colibri id | Element | TRELLIS view | Ticket or reason |
|---|---|---|---|
| COL-UI-01 | Workspace shell | left rail over run, trails, codebase, queue and vault views | DESK-09 |
| COL-UI-02 | Rail brand button | none | uncovered: cosmetic brand |
| COL-UI-03 | New chat | `run_start` resets the run panes | DESK-08 |
| COL-UI-04 | Conversations dialog, search, restore | trails with substring filter; no restore (`resume` is not an rpc command) | DESK-11 |
| COL-UI-05 | Navigation dock | none | uncovered: hover/touch affordance; fixed rail instead |
| COL-UI-06 | Workspace topbar | status bar | DESK-12 |
| COL-UI-07 | Pages kept mounted | state lives in the Rust core | DESK-08 |
| COL-UI-08 | Settings page | model and repository panel | DESK-12 |
| COL-UI-09 | API endpoint field | the endpoint inside a typed model spec | DESK-12 |
| COL-UI-10 | API key field | none | uncovered: ADR 0010, ADR 0024 §3 |
| COL-UI-11 | Probe server | none | uncovered: no rpc probe; `trellis doctor` |
| COL-UI-12 | Connection indicator | status bar: child state and last error; no runtime reachability | DESK-12 |
| COL-UI-13 | Auto-connect | none | uncovered: does not apply |
| COL-UI-14 | Model selector | model picker | DESK-12 |
| COL-UI-15, 16 | KV slot selector and indicators | none | uncovered: colibri-runtime track |
| COL-UI-17, 18, 19 | Temperature, max output tokens, reasoning toggle | none | uncovered: no rpc knob; config-level or fixed in code |
| COL-UI-20 | Controls locked during a generation | composer becomes steer while a run is in flight | DESK-09 |
| COL-UI-21 | Hardware panel | none | uncovered: engine telemetry |
| COL-UI-22 | Scheduler panel | queue counts by status, same 2×2 shape | DESK-18 |
| COL-UI-23 | Expert tier bar | none | uncovered: engine-specific |
| COL-UI-24 | Session token totals | budget (iterations, prompt bytes) | DESK-08 |
| COL-UI-25 | Health polling | none | uncovered: rpc pushes frames |
| COL-UI-26 | Empty-state wordmark | none | uncovered: cosmetic |
| COL-UI-27 | Suggestion chips | prompt templates from `get_commands` as starters | DESK-09 |
| COL-UI-28 | Message list | actions log (`role="log"`, plain text) | DESK-09 |
| COL-UI-29 | Reasoning stream | none | uncovered: not in trellis or rpc |
| COL-UI-30 | Copy response | none | uncovered: native text selection |
| COL-UI-31 | Regenerate | none | uncovered: no retry-of-a-turn concept |
| COL-UI-32 | Continue reply | none | uncovered: runtime feature |
| COL-UI-33 | Finish-state tracking | run outcome from `run_end` | DESK-08 |
| COL-UI-34 | Truncation warning | none | uncovered: `run_end.error` reaches the banner |
| COL-UI-35 | Export conversation | none | uncovered: `trellis log --json` is the export |
| COL-UI-36 | Clear conversation | clear the view, not trellis | DESK-08 |
| COL-UI-37 | Composer | composer | DESK-09 |
| COL-UI-38 | Stop generation | Stop sends `abort` (the run rolls back) | DESK-09 |
| COL-UI-39 | Image attachments | none | uncovered: `--image` is `run`'s alone (ADR 0041) |
| COL-UI-40 | Error banner | `role="alert"` banner | DESK-09 |
| COL-UI-41 | Live token counter | budget, prompt bytes per turn | DESK-08 |
| COL-UI-42, 43 | Decode speed, TTFT | none | uncovered: no streaming (ADR 0024) |
| COL-UI-44 | Per-turn usage badge | budget per turn | DESK-08 |
| COL-UI-45, 46 | Queue-wait header, streaming chat client | none | uncovered: runtime-side |
| COL-UI-47, 48, 49 | Generic-backend degradation, settings persistence, memory-only conversations | none | uncovered: already built in trellis |
| COL-UI-50 to 54 | Brio capability and UI | none | uncovered: owner-decision engine capability |
| COL-UI-55, 56, 57 | Brain mode tabs, expert atlas, cortex renderer | none | uncovered: GLM-5.2 engine data |
| COL-UI-58 | Topic regions overview | codebase graph: packages as regions | DESK-17 |
| COL-UI-59 | Region and expert drill-down | package to module, breadcrumb and Back | DESK-17 |
| COL-UI-60 | Camera controls | pan and zoom by pointer and keys | DESK-17 |
| COL-UI-61 | Guided journey | none | uncovered: cosmetic tour |
| COL-UI-62 | Reduced motion | no transitions under the OS setting | DESK-17 |
| COL-UI-63 | Topics and details panel | module side panel and module search | DESK-17 |
| COL-UI-64 | Node label overlay | at most six non-overlapping labels | DESK-17 |
| COL-UI-65 to 68 | Live routing | none | uncovered: engine telemetry |
| COL-UI-69 to 73 | Profiling | none | uncovered: engine PROF snapshots |
| COL-UI-74 | Theme switching | none | uncovered: ADR 0021 #74 |
| COL-UI-75 | Theme tokens | CSS custom properties, OS light and dark | DESK-09 |
| COL-UI-76 | Localisation | none | uncovered: owner-decision |
| COL-UI-77 | Responsive layout | 860×600 minimum, no sideways scroll | DESK-09 |
| COL-UI-78 | Accessibility | roles, accessible names, keyboard | DESK-09 |
| COL-UI-79 | Safe Markdown | plain text only (S10) | DESK-09 |
| COL-UI-80 | Error boundary | page error goes to the banner; the child is unaffected | DESK-09 |
| COL-UI-81, 82 | Brand mark, UI primitives | none | uncovered: cosmetic; native controls |
| COL-UI-83 to 87 | One-process serving, /health, /experts, /profile, CORS | none | uncovered: no network component |
| COL-UI-88 | Web stack | static `ui/`, no Node | DESK-01 |
| COL-UI-89 | Unit tests and CI job | desktop CI job on Windows and Linux | DESK-19 |
| COL-UI-90, 91 | Browser e2e scripts, release packaging | none | uncovered |
| COL-UI-92 | Tauri shell crate | `desktop/` workspace | DESK-01 |
| COL-UI-93 | IPC surface: none | one rpc child and three typed commands | DESK-05 |
| COL-UI-94 | Plugins: none | guard: no `tauri-plugin-*` in the lock | DESK-02 |
| COL-UI-95 | Capability | lockdown tests | DESK-06 |
| COL-UI-96 | CSP | lockdown tests | DESK-06 |
| COL-UI-97 | Main window and identity | window `main` titled TRELLIS | DESK-01 |
| COL-UI-98 | Frontend wiring | `frontendDist` `ui`, no dev server or build hooks | DESK-01 |
| COL-UI-99 to 103 | Bundling, updater, engine sidecar, mobile, landing page | none | uncovered |
| COL-UI-104 | Site model catalogue | picker lists `get_available_models`' catalog | DESK-12 |
| COL-UI-105 | Site theme | none | uncovered: cosmetic |
| COL-UI-106 | Dashboard docs | `desktop/README.md` | DESK-03 |

**Corrections to the inventory.** COL-UI-24, COL-UI-41 and COL-UI-44 cite `Budget.tokens`. Since #243 the TUI's `Budget` holds `iteration`, `prompt_bytes`, `elapsed_secs` and `max_iterations` (`crates/tui-demo/src/tui/app.rs`), and no rpc event carries a token count. The desktop labels the budget in bytes. The inventory's TRELLIS line numbers (for example `tui_bridge.rs:257`) predate that change.

## Principal System Architect

**Placement: `desktop/`, its own Cargo workspace, like `fuzz/`.**
- `desktop/Cargo.toml` has its own `[workspace]` and its own committed `Cargo.lock`. The root `Cargo.toml` gets `exclude = ["fuzz", "desktop"]`.
- The size argument is measured. colibri's Tauri lock has 415 `[[package]]` entries, against 345 in TRELLIS's whole root lock (`grep -c -F '[[package]]'` on each; colibri at ce370e8, TRELLIS on 2026-10-04; a lock lists every target's packages, and colibri's count includes its own package).
- Kept separate, the root lock and the audit's figures stay comparable (`audit/DEPENDENCIES-2026-09-28.md`: 176 crates in the release graph, 273 in the whole workspace).
- `cargo build --workspace` never needs WebKitGTK on Linux, so ADR 0013's clean-machine rule still holds for the CLI.
- The supply-chain audit of Tauri's tree gets its own policy (`desktop/deny.toml`) and leaves the shipped binary's policy alone. The `libfuzzer-sys` NCSA exception is the precedent.
- A member crate in the main workspace was rejected for all of the above. A separate repository is option B in DESK-01.

**Process model.**
```
desktop/ui (static page, webview)
   | invoke: rpc_send, get_trails, open_repository        (capability-gated)
   v
desktop/src (Rust core: child.rs, commands.rs, frames.rs, state.rs, trails.rs)
   | events: view, rpc-frame, rpc-unparsed, rpc-human, rpc-exited
   | spawns, fixed argument lists, environment inherited unchanged:
   v
trellis --root <repo> rpc                     long-lived, one per window
trellis --root <repo> --json log --limit <n>  one-shot, trails
trellis --version                             one-shot, status bar
```

**Why a child process and not linked crates.**
- The grounding contract is structural inside the trellis process. `Verdict` has private constructors in `trellis-checks`, and `cmd_run_with` owns the snapshot, trust gate, checks, rollback and trail.
- A desktop that linked `trellis-loop` or made `trellis-cli` a library could assemble a run with its own control flow. That is the "second set of rules about when a change is accepted" that `tui_run.rs` warns against.
- Over rpc, every `prompt` is `cmd_run_with`. rpc has no `bash` (ADR 0025 §5) and no accept, revert or diff, so the desktop has no write path of its own.
- It also keeps the Tauri graph and the trellis graph from ever meeting in one lock.

**Contracts.**
- The framing records are versioned (`run_start.version` is 1). The tracing events between them are not.
- DESK-07 writes the event fields the desktop reads into `crates/trellis-cli/tests/rpc_contract.json`, held by a test through the binary. The desktop's reducer tests read the same file.
- Human text on stderr is never parsed.
- Views whose data has no JSON form today (`codebase_lines`, `radius_of`, `queue list`, `vault search` and `vault graph`) get read-only rpc commands (DESK-13 to DESK-16) on the owner's answers, or are not built.

**Trust boundaries.**
1. **Webview to core.** The capability grants event listening plus three app commands. `rpc_send` forwards only rpc's twelve command types. The CSP allows only IPC, with no inline or eval'd script. Navigation stays on the app origin.
2. **Core to trellis.** The rpc protocol, with `cmd_run_with` behind it.
3. **Trellis to the model runtime.** Unchanged.

**Frontend: static, no Node.**
- Plain HTML, CSS and ES modules in `desktop/ui/`, with `withGlobalTauri` because there is no bundler.
- The view-model is a pure Rust reducer, so `cargo test` covers the logic. The page JavaScript only renders.
- Against Vite and React (colibri's stack):
  - colibri's `web/package-lock.json` has 136 `node_modules/` entries (`grep -c -F '"node_modules/'`). That tree is outside `cargo deny` and `cargo audit`.
  - A dev server needs `devUrl` and a looser CSP while developing.
  - `tsc` and vitest need Node on every build machine, and ADR 0013's rule is that `cargo build` is the distribution story.
- The cost is real: no type checker for the page and hand-written DOM code. It stays small because state is in Rust. If page JavaScript starts holding state, revisit with a measured line count.

**Platforms and CI.**
- Supported: x86_64 Windows (WebView2, MSVC) and x86_64 Linux (WebKitGTK 4.1). These are the targets `deny.toml` names.
- A new `desktop` CI job runs on both, and the `supply-chain` job audits the desktop lock.
- `release.yml` is unchanged: no installers, signing or updater (ADR 0021 Apex 38–42 stays excluded).

## Senior Full-Stack Engineer

**Layout.**
```
desktop/
  CLAUDE.md  README.md  Cargo.toml  Cargo.lock  deny.toml  build.rs  tauri.conf.json
  src/  main.rs  child.rs  commands.rs  frames.rs  state.rs  trails.rs  lockdown.rs
  ui/   index.html  style.css  app.js  render.js  graph.js  trails.js  settings.js  status.js
        codebase.js  radius.js  queue.js  vault.js
```

**Manifest.**
- Dependencies: `tauri` 2, `serde`, `serde_json`; `tauri-build` 2 as the build dependency.
- Caret requirements, per CLAUDE.md's pinning rules. colibri resolved `tauri` 2.11.5 and `tauri-build` 2.6.3.
- No `trellis-*` dependency. The guard script enforces it.

**`tauri.conf.json`.**
- One window, `main`, titled `TRELLIS`, 1280×820 with an 860×600 minimum (colibri's numbers).
- `frontendDist` `ui`; no `devUrl`, `beforeDevCommand` or `beforeBuildCommand`; `bundle.active` false.
- `app.security.freezePrototype` true. Capabilities inline.
- CSP: `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'`.
- Identifier `local.trellis.desktop`, which claims no domain.

**Core.**
- **`child.rs`** is the single spawn site, with three fixed argument lists.
  - On Windows it sets `CREATE_NO_WINDOW`, because a release build uses `windows_subsystem = "windows"` and a console child would otherwise flash a window.
  - It adds nothing to the environment.
  - Closing the window sends `quit` and waits; it never kills the child.
- **`commands.rs`** holds the three commands:
  - `rpc_send` checks the type whitelist and a 64 KiB line cap.
  - `get_trails` runs the log argument list once.
  - `open_repository` switches the child, and is refused while a run is in flight.
  - DESK-12 adds the model-source guard here.
- **`frames.rs` and `state.rs`** are the reducer, mirroring `AppState::update` and `tui_bridge.rs`.
- **`trails.rs`** turns log lines into typed rows with every string wrapped as untrusted text.
- **`lockdown.rs`** holds tests over `tauri.conf.json`.

**Tests that need trellis** find `../target/debug/trellis` (`.exe` on Windows), so their Verify blocks run `cargo build --locked -p trellis-cli` first.

**Verify scripts.**
- `scripts/tickets/verify-tests.sh` cannot reach a separate workspace (it runs `cargo test -p` from the root). DESK-02 adds `scripts/tickets/verify-desktop-tests.sh <filter> <min>` with the same zero-match refusal.
- DESK-01 lands before that script exists, so it pairs `git grep -q -F "fn <test>"` with the cargo test.

**File counts.** Every ticket lists at most five files. Two DESK-01 edits outside `desktop/` (the ADR, and root `.gitignore` entries for `/desktop/target/` and `/desktop/gen/`) are named in its Done-when, as #95 did for the root `Cargo.toml`.

**Not verified** (no Tauri source on this machine). Each item is a Done-when observable rather than a claim:
- whether plain `cargo build` builds a Tauri 2 app with a static `frontendDist` and no `tauri-cli`;
- whether `tauri-build` needs `icons/icon.ico` on Windows;
- that `tauri-build`'s app manifest gates app commands behind capability entries;
- whether `generate_context!` trips `forbid(unsafe_code)` (the root lint; the desktop adopts it where it compiles);
- which licences and advisories `cargo deny` reports on Tauri's tree.

**Sizing.** The trellis-side tickets are small: DESK-07, DESK-13 to DESK-16, DESK-20. The desktop tickets are one session each. DESK-04, which changes a ticketing rule, is under an hour.

## QA / Security Engineer

**Threat model.**

| # | Threat | Control | Held by |
|---|---|---|---|
| T1 | The desktop becomes a second path that forges a verdict or skips trust | No trellis crate linked; every run is an rpc `prompt` through `cmd_run_with`; no write path, no accept | guard (`trellis-*` dependency), DESK-05 whitelist, ADR 0048 |
| T2 | Script injected through trail or model text then sends `prompt` with `control: auto`, or `approve` with yes | `textContent` only; strict CSP; `freezePrototype`; Approve only on an explicit click | DESK-02 guard, DESK-06 tests, DESK-09 |
| T3 | **Key laundering through `set_model`.** `rpc.rs` sets `model_from_user` whenever the client names a model, and `get_available_models` returns `configured.repository`. A picker that echoes that spec makes a repository-chosen endpoint the user's choice, and ADR 0024 §3's refusal stops applying | The core asks for the configured specs itself and refuses to forward the repository spec as a choice. To use the repository's spec, send no `model` | DESK-12 test `the_repository_spec_is_never_forwarded_as_the_users_choice` |
| T4 | A key passes through the UI | No key field; the environment is inherited unchanged; no IPC carries the environment; no key variable is read | guard (`.env(`, `.envs(`, `env_clear`, `_API_KEY`, `key_env`), ADR 0048 |
| T5 | Killing the child skips rollback | Close sends `quit` and waits; no `Child::kill`; no ADR 0035 kill-on-close Job Object on the trellis child | guard (`.kill(`), DESK-05 test |
| T6 | The desktop crashes mid-run and every pipe closes | ADR 0025 §5 end-of-input rules; untested with stdout and stderr unread | DESK-20 |
| T7 | Tauri's tree brings licences or advisories into the shipped policy | Separate lock, `desktop/deny.toml`, G5 and CI audits | DESK-02, DESK-19 |
| T8 | The webview navigates to remote content that then holds IPC | Navigation predicate; no `remote` in capabilities | DESK-06 |
| T9 | A build leaves the worktree dirty (ADR 0009; #197) | `.gitignore` entries; lock-currency check | DESK-01, DESK-02 |
| T10 | A field rename blanks the desktop with nothing failing | `rpc_contract.json` held through the binary | DESK-07, DESK-08 |
| T11 | Radius ratio creep (rule 3) or verified/reviewed collapse (rule 4) | Tests reject `ratio`, `percent`, `fraction` and `total` keys; `mode_counts` shown as two numbers | DESK-08, DESK-11, DESK-14 |
| T12 | A menu-launched app lacks shell-exported keys | trellis refuses and names the variable; the desktop shows that and offers no key field | ADR 0048 §8 |

**The guard script** (`scripts/checks/desktop-guards.sh`, with `--self-test`, run by G5 and CI) fails on any of:
- tauri in the root lock;
- a plugin in the desktop lock;
- a `trellis-*` dependency;
- `Command::new` in more than one file;
- `.kill(`, `.env(`, `.envs(`, `env_clear`, `_API_KEY` or `key_env` under `desktop/src/`;
- `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, an `eval` call or `new Function` under `desktop/ui/`;
- a stale desktop lock.

It is a text check, weaker than a type, and is labelled as one.

**Found while reading.**
- **T3 (above):** the laundering path exists as soon as any rpc client lets a person pick a model; it is not specific to the desktop.
- **`AREA_PATHS` gaps.** It maps neither `desktop/`, `crates/tui-demo` nor `crates/trellis-tickets`, so every desktop ticket's `area:` abstains and routes to `triage:needs-human` (DESK-04).
- **Desktop tickets will route to a person regardless.** The risk rule abstains on any `Cargo.toml` in the files, so DESK-01 and DESK-02 route to a person even after DESK-04.
- **The broken-pipe path is untested (DESK-20).**
- **Inventory staleness:** `Budget.tokens` (see Product Manager).

## Next Steps & Action Plan

1. **Owner questions, together:**
   - DESK-01: accept ADR 0048, and where the desktop lives (recommended: A).
   - DESK-02: how `desktop/deny.toml` treats licences and advisories (recommended: A, per-crate exceptions).
   - DESK-04: which area `desktop/**` takes (recommended: `area:tui`, with `crates/tui-demo`).
   - DESK-13 to DESK-16: rpc read commands, `--json` flags, or leave the views out (recommended: rpc read commands).
2. **Independent of the answers, on trellis:**
   - DESK-07: the event-field contract.
   - DESK-20: client-vanishes rollback test.
   Both strengthen the rpc protocol for any client, and neither touches `desktop/`.
3. **Phase 1, the skeleton:** DESK-01, then DESK-02 and DESK-03, then DESK-05, then DESK-06; DESK-19 after DESK-02.
4. **Phase 2, the run view:** DESK-08 (needs DESK-07), then DESK-09, then DESK-10, DESK-11 and DESK-12 in parallel.
5. **Phase 3, the views behind new rpc commands:** DESK-13 and DESK-14, then DESK-17; DESK-15 and DESK-16, then DESK-18.
6. **Per ticket:** run `scripts/gates/run-all.sh <issue>`. A desktop ticket is done when `scripts/checks/desktop-guards.sh`, its Verify, and (from DESK-19) the `desktop` CI job pass on Windows and Linux.
7. **After phase 2,** record measured figures: the desktop binary size, a clean-build wall clock, and `cargo tree` counts for the desktop lock. Record them with the audit's commands in a new `audit/` entry. None are claimed before then.
