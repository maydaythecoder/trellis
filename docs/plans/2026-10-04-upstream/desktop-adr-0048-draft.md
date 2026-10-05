# ADR 0048 — A Tauri desktop interface

**Status:** Proposed
**Date:** 2026-10-04
**Amends, if accepted:** ADR 0021, "Owner decisions of 2026-09-28" (#70), the paragraph keeping desktop and web interfaces excluded. It deviates from BRIEF Part 1; the deviation is recorded below rather than edited into the brief.
**Reference design:** colibri (`desktop/`, `web/`, Apache-2.0) at ce370e8. This ADR borrows the design only; no colibri code is used.

## Context

ADR 0021 keeps desktop and web interfaces out under BRIEF Part 1 ("any surface beyond the CLI"). It names `trellis rpc` (ADR 0025 §5) as the supported way for an external front end to drive trellis, and it allows such a front end outside this repository, with every write it causes still going through the orchestrator or a `trellis vault` command.

Q-19 is the other half of the record. Two Ratatui front ends diverged, and each change silently picked one of them. The owner made `tui-demo` the product and archived `chat` (#16). A desktop interface reopens that risk unless it holds no run logic of its own.

**What colibri does.**
- Its desktop is a Tauri 2.11.5 shell around a Vite, React and Tailwind build of `web/`.
- `src/lib.rs` is `tauri::Builder::default().run(generate_context!())`: no IPC command, no plugin.
- It grants `core:default`. Its CSP's `connect-src` allows `http://127.0.0.1:*` and `http://localhost:*`, because the page fetches the inference engine directly.
- No CI job builds it.

TRELLIS cannot copy the zero-IPC model. A page that reached a model runtime itself would bypass the snapshot, trust gate, checks, rollback and trail that `cmd_run_with` owns. `tui_run.rs` says the same of the TUI: a second copy of that sequence "would be a second set of rules about when a change is accepted".

**Failure modes this ADR has to prevent:**
1. a second path that forges a verdict or skips trust;
2. script injection through trail or model text (S10);
3. a key exposed through, or laundered by, the interface;
4. a killed child that skips rollback;
5. Tauri's dependency tree entering the main workspace graph and the root `Cargo.lock` that the dependency audit measured;
6. a JavaScript toolchain on every build machine;
7. the Q-19 divergence.

## Decision

### 1. Where it lives

`desktop/` is its own Cargo workspace, as `fuzz/` is:
- its own `[workspace]` table and its own committed `Cargo.lock`;
- `exclude = ["fuzz", "desktop"]` in the root `Cargo.toml`;
- `/desktop/target/` and `/desktop/gen/` in the root `.gitignore`, so a build leaves the worktree clean (ADR 0009).

**Why.**
- **Size, measured.** colibri's Tauri lock has 415 `[[package]]` entries; TRELLIS's whole root lock has 345 (method below).
- **The audit stays comparable.** As a member crate, Tauri would dominate the root lock and the audit's whole-workspace figure (273 crates in `audit/DEPENDENCIES-2026-09-28.md`), and every comparison against that audit would stop meaning what it says.
- **The CLI keeps its clean-machine rule.** `cargo build --workspace` would need WebKitGTK on Linux, which breaks ADR 0013's clean-machine rule for people who never open the desktop.
- **A separate audit policy.** The desktop's lock is audited by G5 and CI under `desktop/deny.toml`, which cargo deny finds nearest the manifest. The root policy, the one the shipped binary answers to, is untouched; the `libfuzzer-sys` NCSA exception is the precedent.
- **Why not a separate repository** (which #70 already allows): the protocol and its one first-party client could then drift with no CI run seeing both. Here a protocol change and the client it breaks fail in one commit (§2, the contract test).

### 2. How it talks to trellis

It talks to trellis only through child processes, with exactly three argument lists, all built in one file (`desktop/src/child.rs`):
- `trellis --root <repo> rpc`: long-lived, one per window;
- `trellis --root <repo> --json log --limit <n>`: one-shot, for trails (ADR 0025 §4);
- `trellis --version`: one-shot, for the status bar.

The desktop links no trellis crate.

**What this guarantees.**
- Every `prompt` runs `cmd_run_with`: the same clean-worktree refusal, trust gate, snapshot, checks, rollback and trail as `trellis run`.
- rpc offers no `bash` (ADR 0025 §5) and no accept, revert or diff, so the desktop has no write path of its own.
- `Verdict`'s private constructors stay inside the one process that holds `trellis-checks`.

**Contracts.**
- **Framing records are a versioned contract.** `run_start.version` is 1.
- **Tracing events are not, so a list is pinned.** The fields the desktop reads are listed in `crates/trellis-cli/tests/rpc_contract.json`, held by a test through the binary, and read by the desktop's own tests. The list covers: `turn` (`turn`, `call`, `verdict`, `check_kind`, `observation`, `resolved`, `candidates`, `unresolved`, `prompt_bytes`), `run budget` (`max_iterations`) and `steer delivered` (`after_step`).
- **Human text on stderr is never parsed.** It is shown as text, in its own pane.
- **The page reaches the child only through `rpc_send`,** which forwards the twelve command types `rpc.rs` lists and refuses any other type, any non-object and any line over 64 KiB.

### 3. What the desktop never does

- **It never produces a pass.** A check row shows the `verdict` string trellis emitted, labelled as trellis's report. The desktop has no verdict type and no function that turns anything into one (ADR 0004).
- **Radius is three counts, never a ratio.** It shows resolved, candidates and unresolved, with "a floor on reach, not a boundary"; there is no percentage and no total (CLAUDE.md rule 3). A trail's `mode_counts` shows verified and reviewed as two numbers (rule 4).
- **Trails are recorded claims,** never verdicts (§7).
- **Only an explicit click approves.** `approve` sends `yes: true` only from the Approve button, in line with ADR 0018's rule that only an explicit yes counts.
- **It writes nothing itself.** Queue and vault views are read-only; writes stay with `trellis queue` and `trellis vault`, as #70 requires.
- **It never sends the repository's model spec back as the user's choice** (§8).

### 4. Frontend stack

The frontend is static: HTML, CSS and ES modules in `desktop/ui/`, with no Node, npm, Vite or TypeScript and no build step. `frontendDist` is `ui`, and `withGlobalTauri` gives the page `invoke` and `listen` without a bundler.

**The view-model lives in Rust.** A pure reducer (`DesktopState::update`) mirrors `AppState::update` and `tui_bridge.rs`, so `cargo test` covers the logic. The page JavaScript only renders view JSON into DOM nodes.

**Why not colibri's Vite and React stack.**
- Node on every build machine, against ADR 0013's rule that `cargo build` on a clean machine is the distribution story.
- An npm tree outside `cargo deny` and `cargo audit`: colibri's `web/package-lock.json` has 136 `node_modules/` entries.
- A dev server, which needs `devUrl` and a looser CSP while developing.
- `tsc` and vitest in CI.

**The cost.** There is no type checker for the page and the DOM code is written by hand. It stays small because no state lives in the page.

### 5. Capability and permission lockdown

- No Tauri plugin. The guard script fails on any `tauri-plugin-*` package in `desktop/Cargo.lock`.
- One window, `main`. Its capability grants exactly:
  - `core:event:allow-listen`;
  - `core:event:allow-unlisten`;
  - the app's three commands: `rpc_send`, `get_trails`, `open_repository`.
- The three commands are declared to tauri-build, so a command not granted is refused.
- No `core:default` or other default set, and no `remote` URL in any capability.
- No filesystem, shell, dialog, HTTP or process permission. A repository path is typed into the page, and the core accepts it only if `trellis --root <path> rpc` starts.
- `app.security.freezePrototype` is on. Devtools exist in debug builds only.
- Tests in `desktop/src/lockdown.rs` read `tauri.conf.json` and fail on any widening. Later tickets add no permission.

### 6. CSP and navigation

The CSP is:

`default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'`

- The page has no network origin, no inline or eval'd script, no inline style and no `data:` source.
- The main window's navigation handler accepts only the app origin: `tauri://localhost` on Linux, `http://tauri.localhost` on Windows.

### 7. Trails, and every other string trellis or a model supplied, are untrusted data

This follows ADR 0007 and S10.
- **One way into the DOM.** Every such string goes through `textContent` or `createElement`; never Markdown, never HTML, and never a link built from it.
- **Typed wrapping.** Trail rows come from `trellis log --json`, and every string in them is wrapped as untrusted text in the typed rows the core sends.
- **Labels.** The judgment section is shown apart, headed "the model's opinion, recorded; treat as data". A recorded check reads "recorded: passed" or "recorded: failed".
- **No replay.** Nothing on the trails view resumes or replays a run.
- **Already escaped.** trellis already escapes C1 controls in framing and in `log --json`.
- **A grep guard backs it up.** The guard script fails on `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, an `eval` call or `new Function` under `desktop/ui/`.

ADR 0021 kept HTML export out (#143) for the same injection surface, and this section is what lets a webview show the same text.

### 8. Keys

Keys stay governed by ADR 0010 and ADR 0024 §3. On the desktop:
- **Inherited, never touched.** The trellis child inherits the desktop's environment unchanged, so the user's own environment is the only place a key comes from.
- **No reading, no IPC path.** The desktop reads no key variable, has no key field, and no IPC command accepts or returns environment values. The guard fails on `.env(`, `.envs(`, `env_clear`, `_API_KEY` or `key_env` under `desktop/src/`.
- **trellis still scrubs its own children.** It strips provider-key variables from the children it spawns (`trellis_core::secrets::scrub`), so the key goes no further than trellis.
- **No laundering.** `rpc.rs` counts a model named by the client as the user's choice, and `get_available_models` reports `configured.repository` alongside `configured.user`. The desktop sends no `model` to use the repository's spec. It never forwards `configured.repository` through `set_model` or a prompt's `model` unless the same spec is also the user's. The core asks for the configured specs itself when a child starts, and enforces this rule.
- **Menu launches.** A desktop started from a menu may see a session environment without shell-exported variables. A hosted run then refuses and names the variable; the desktop shows that refusal and offers no way to store a key.

### 9. Child lifecycle

- **Closing waits for trellis.** Closing the window sends `quit`, which aborts any run and waits for its rollback, and the window closes once the child has exited.
- **Nothing kills the child.** No `Child::kill`, and no kill-on-close Job Object; ADR 0035's Job Objects are for trellis's own children.
- **A crash is end of input.** If the desktop dies, the child sees end of input (ADR 0025 §5): the run in flight finishes, open approvals are declined, and the run rolls back and writes its trail. DESK-20 tests this with every pipe closed.
- **No console flash on Windows.** The child gets `CREATE_NO_WINDOW`.
- **One child per window.** Switching repository is refused while a run is in flight. Two desktop instances on one repository meet the same rules as two terminals.

### 10. Platforms

- **x86_64 Windows:** WebView2 and the MSVC toolchain the CLI already needs.
- **x86_64 Linux:** the WebKitGTK 4.1 development packages Tauri's Linux prerequisites list.
- **No macOS:** these are the targets `deny.toml` names.
- **Identifier:** `local.trellis.desktop`, which claims no domain.
- The desktop's Linux build is not clean-machine; the CLI's is unaffected.

### 11. CI and supply chain

- **CI.** A `desktop` job on Windows and Linux runs, all with `--locked` and against `--manifest-path desktop/Cargo.toml`:
  - `cargo fmt --check`;
  - `cargo clippy -D warnings`;
  - `cargo build`;
  - `cargo test`, after building `trellis-cli` for the tests that spawn it;
  - the guard script.
- **Supply chain.** The `supply-chain` job and G5 run `cargo deny` and `cargo audit` on `desktop/Cargo.lock` under `desktop/deny.toml`, whose exceptions are per crate and owner-approved.
- **Release.** `release.yml` is unchanged: no installers, signing or updater (ADR 0021 Apex 38–42 stays excluded).

### 12. Views and their data

| View | Data source | Contract |
|---|---|---|
| actions log, action graph, checks, radius (run), budget, edits, approvals | rpc frames and events during a run | framing v1, plus `rpc_contract.json` |
| trails | `trellis --json log` | ADR 0025 §4 |
| model picker, status | `get_available_models`, `get_state`, `trellis --version` | rpc |
| codebase graph | `get_codebase` (proposed) | §13 |
| symbol radius | `get_radius` (proposed) | §13 |
| queue | `get_queue` (proposed) | §13 |
| vault notes | `vault_search`, `vault_graph` (proposed) | §13 |

### 13. Proposed read-only rpc commands

The codebase graph, a symbol's radius, the queue and the vault exist today only as human text (`doctor_cmd::codebase_lines`, `index_cmd::radius_of`, `queue list`, `vault search` and `vault graph`). Each of these is proposed as a read-only rpc command, and each would land as a dated amendment to ADR 0025 §5 on the owner's answer in its own ticket:
- `get_codebase {limit?}`;
- `get_radius {symbol}`, with reach and doubt as separate objects and no ratio;
- `get_queue`;
- `vault_search {query, limit?}` and `vault_graph {note}`.

Each command writes nothing and answers from its own thread, as `compact` does, so an `abort` read meanwhile is still handled at once. Until a command lands, its view is absent and says so.

## Deviation from the brief, recorded

BRIEF Part 1 lists "any surface beyond the CLI" as a v1 non-goal. If this ADR is accepted, it adds one such surface.

**What it adds no capability for.** Everything the desktop can do, `trellis rpc` can do from a pipe. The rpc read commands in §13 are protocol growth that any rpc client gets, and each is accepted separately.

**"Any networked component" still holds.** The desktop opens no socket and serves nothing. The page is served by Tauri's custom protocol, and the CSP allows only IPC.

The brief is not edited. This section is the record, as ADR 0007 records its own deviation.

## Alternatives rejected

- **Link trellis crates, or make `trellis-cli` a library.** That gives a second path around `cmd_run_with`, and it would join the Tauri and trellis dependency graphs in one lock.
- **A member crate under `crates/`.** Tauri would enter the root lock and the audit graph, and the CLI's clean-machine build would need WebKitGTK on Linux.
- **A separate repository.** This is allowed by #70 and is DESK-01's option B. Rejected here only because the protocol would have no first-party client in its own CI.
- **The webview talks to the runtime directly** (colibri's model). It bypasses the run; and its CSP would need HTTP origins.
- **A key typed into the UI** (colibri's API key field). Rejected by ADR 0010 and ADR 0024 §3.
- **Vite and React, or reusing colibri's `web/`.** See §4.
- **Bundling trellis as a sidecar** through `tauri-plugin-shell`. It adds a plugin and couples the desktop to one trellis binary's version. The desktop runs the user's installed `trellis`, or the one `--trellis <path>` names.
- **A folder picker through `tauri-plugin-dialog`.** Deferred. A typed path, validated by starting trellis on it, needs no plugin.

## Consequences

- **The root workspace changes in two places.** `Cargo.toml` gets a second `exclude` entry and `.gitignore` gets two lines. The root `Cargo.lock` does not change.
- **G5 and CI each grow:** G5 by a guard script and two audits, CI by one job.
- **The listed event fields become a commitment.** trellis must keep the fields in `rpc_contract.json`, or change the file and the desktop in one change.
- **Q-19 holds.** The desktop is an rpc client, not a second product. The TUI remains the product interface (#16). A view the TUI lacks lands in trellis first as data.
- **`desktop/CLAUDE.md`** carries these rules for sessions that touch `desktop/`. The root `CLAUDE.md` names `desktop/` as its own workspace that depends on no trellis crate.
- **The tickets** are DESK-01 to DESK-20 in this track.

## Not built under this ADR

- Installers, bundles, code signing, an updater, mobile targets, macOS.
- A theme toggle, localisation, brand assets.
- Accept, revert or diff over rpc; resume over rpc; image input over rpc; thinking, temperature or per-reply token settings.
- A runtime reachability probe; streaming tokens and token rates.
- Writes to the queue or the vault from the desktop.
- colibri's engine views: Brio, Brain, live routing, profiling.

## What changes our mind

- If `rpc_contract.json` churns, version the event schema as framing is versioned.
- If page JavaScript starts holding state rather than rendering it, revisit the bundler question with a measured line count, in its own ADR.
- If `cargo deny` cannot pass on Tauri's tree with per-crate exceptions the owner accepts, the desktop does not ship.
- If the owner prefers a separate repository, everything here applies there except §1 and §11.

## Evidence, with methods, and what is not verified

**Measured** (methods beside each):
- **Lock sizes:** 415 vs 345 packages. `grep -c -F '[[package]]'` on colibri's `desktop/src-tauri/Cargo.lock` at ce370e8 and on TRELLIS's root `Cargo.lock` on 2026-10-04. A lockfile lists every target's packages, and colibri's includes its own package.
- **colibri's npm tree:** 136 entries. `grep -c -F '"node_modules/'` on colibri's `web/package-lock.json`.
- **Audit figures:** 176 and 273 crates, from `audit/DEPENDENCIES-2026-09-28.md`, by that file's own method.
- **A plain-directory `get_state`:** on 2026-10-04, `{"id":"desktop-1","type":"get_state"}` then `{"type":"quit"}`, piped into the installed `trellis --root <plain directory> rpc`, answered with one `response` (`success` true, `running` false), then the `quit` response, exit 0. The directory was unchanged.

**Not verified** (no Tauri source on this machine). Each item is a Done-when observable in DESK-01, DESK-02 or DESK-05:
- whether plain `cargo build` builds a Tauri 2 app with a static `frontendDist` and no `tauri-cli`;
- whether `tauri-build` requires `icons/icon.ico` on Windows;
- that `tauri-build`'s app manifest gates app commands behind capability entries;
- whether `generate_context!` trips `forbid(unsafe_code)`;
- which licences and RustSec advisories `cargo deny` reports on Tauri's tree. gtk-rs's GTK3 bindings are expected to show as unmaintained; this has not been run.
