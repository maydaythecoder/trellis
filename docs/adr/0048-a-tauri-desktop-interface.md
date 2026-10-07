# ADR 0048 — A Tauri desktop interface, in `desktop/`, as a `trellis rpc` client

**Status:** Accepted (owner-directed). Not built (story S15, #365).
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
*Amended 2026-10-06 (#500), proposed by ADR 0052, pending acceptance:* a
`trellis rpc` client may show trellis's stderr to the user in a pane capped
in size, as human text. It still never reads stderr as data.

### 3. Children, environment and keys

It never kills a trellis child: it sends `quit` and waits (ADR 0025 §5). It
adds nothing to a child's environment and reads no key variable (ADR 0010,
ADR 0024 §3). Only test code sets `TRELLIS_CONFIG_DIR`, so tests never read
the developer's own config.

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
*Amended 2026-10-06 (#500), proposed by ADR 0052, pending acceptance:* the
Go toolchain is an accepted cost, for `tui/` only (the Go TUI, a second
`trellis rpc` client), pinned by a go.dev tarball and its SHA-256. This
narrows the Context's refusal of a toolchain on every build machine to
JavaScript: Node, npm and a bundler are still refused, here and in `tui/`.

### 6. The client contract

The event and trail fields listed in
`crates/trellis-cli/tests/client_contract.json` (DESK-07) are what an rpc or
`log --json` client may rely on, held by tests through the binary. Fields it
does not list stay as unversioned as ADR 0025 §4 leaves them.

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

### 8. The product interface and what it may show

The TUI stays the product interface under Q-19 (#16). The desktop shows what
trellis reports and computes no verdict (ADR 0004). It shows radius as three
counts, never a ratio (CLAUDE.md rule 3).

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
