# ADR 0052 — Two `trellis rpc` clients: a rendering Tauri desktop, and a Go TUI that replaces ratatui

**Status:** Accepted (owner-directed). The owner accepted it on #500 (G7)
on 2026-10-07.
**Date:** 2026-10-06
**Decided by:** the owner, 2026-10-06, in session ("have a fully rendering
tauri desktop option and a tui using go and bubble tea"), with the decisions
in section 2 recorded on #500. The plan came from a research-and-design pass
over ADR 0048, ADR 0050, ADR 0025, ADR 0021, `desktop/`, `crates/tui-demo`,
`tui_bridge.rs`, `tui_run.rs`, `ci.yml`, `release.yml` and the gates.
**Holds:** CLAUDE.md's ten rules unchanged; the Rust core is the only engine;
ADR 0025 §5 is the only way a client drives trellis; ADR 0048 sections 3, 4,
6, 8, 9 and 10 apply to both clients unchanged, and its sections 1, 5 and 7
stay the desktop's (`tui/` has its own counterparts in sections 3 and 5
below).
**Amends:** ADR 0048 (its Context list and section
5: the Go toolchain is an accepted cost and Node is still refused; section 2:
a client may show stderr in a capped pane); ADR 0021 row 11; ADR 0050
section 1 (bare `trellis` hands off at parity).

## Context

TRELLIS has one interface that ships, the ratatui TUI: `crates/tui-demo`,
driven from `trellis-cli` by `tui_bridge.rs` and `tui_run.rs`. Q-19 (#16)
made it the product interface, and ADR 0048 section 8 kept it so when it
placed a Tauri desktop in `desktop/` as a `trellis rpc` client. That desktop
is not built past its core (story S15).

On 2026-10-06 the owner set two goals: a desktop that renders a whole run,
built with Tauri, and a TUI written in Go with Bubble Tea. Both are clients of
`trellis rpc` (ADR 0025 §5). The failure modes this ADR has to prevent are
ADR 0048's, now for two clients: a second path around the orchestrator that
forges a verdict or skips trust; a client that kills a trellis child and skips
rollback; a key read or forwarded by a client; and two interfaces that drift
apart (Q-19). Adding Go adds three more: a toolchain fetched unpinned at
build time; a Go dependency tree no supply-chain job audits; and Go code that
spawns `git` or changes a child's environment outside ADR 0023's guards.

## Decision

### 1. Two clients, one engine

TRELLIS ships two clients of `trellis rpc`:

- **The desktop** (`desktop/`, ADR 0048) renders a whole run: every pane of
  the reducer's view reaches the page, held by a test from the reducer's
  `view` event to the page's render functions (N11 in the ticket plan).
- **The Go TUI** (`tui/`, the binary `trellis-tui`), built with Bubble Tea.
  Once it reaches parity (section 7) it replaces the ratatui TUI and takes
  over its role as the product interface under Q-19; Q-19's answer is
  unchanged in kind, one TUI and not two.

The Rust core stays the only engine. Neither client computes a verdict (ADR
0004), and every write either causes goes through `prompt`, and so through
the orchestrator (ADR 0048 section 10). Both follow ADR 0048 section 3: a
client never kills a trellis child, it sends `quit` and waits; it adds
nothing to a child's environment and reads no key variable. Both follow its
section 4 on hosted model specs.

### 2. The owner's decisions of 2026-10-06

1. **The Go TUI replaces ratatui at parity.** The two are not kept side by
   side past the fallback release (section 4).
2. **Launch handoff.** The Rust `trellis` keeps first-run (ADR 0050 section 2)
   and the start screen (ADR 0050 section 3), then execs `trellis-tui`, the
   sibling of `current_exe()`. When that binary is missing it falls back to
   the ratatui TUI. The fallback stays for one release after the switch, and
   is removed in the release after.
3. **Routing stays in Rust.** Whether an input is a question or a task is
   decided by a new rpc command, `classify {text}`, which wraps
   `ask::classify`, so the rule has one home. It is added to ADR 0025 §5 by
   its own dated amendment, with contract tests, in its own ticket (R1).
4. **Go on the runners** comes from a pinned go.dev tarball, checked against
   its SHA-256 and unpacked user-local, with `GOTOOLCHAIN=local` so a
   `toolchain` line in `go.mod` never downloads another toolchain.
5. **Releases are Windows and Linux only**, built on the self-hosted runners.
   macOS is dropped from releases until a macOS runner exists.
6. **#289's decisions stand.** #290 first (now closed); the owner installs
   WebKitGTK on WSL; the identifier is `dev.trellis.desktop`; the change to
   `desktop/src/child.rs` is allowed; and if Tauri's generated code needs
   `unsafe`, the work stops and is escalated to the owner.

### 3. Go for `tui/` only

Go is accepted for `tui/` and nowhere else in the repository. `tui/` is its
own Go module: `go.mod` with a `toolchain` line and a committed `go.sum`,
which is the pin as `Cargo.lock` is for the root workspace. The Charm
libraries, Bubble Tea, Bubbles and Lip Gloss, are pinned as one set and move
together in one commit, as `tree-sitter` and `tree-sitter-python` do. Any
other Go dependency needs an ADR, as a new crate does.

Node is still refused. ADR 0048 section 5 (static HTML, CSS and ES modules,
no Node, npm or bundler) is unchanged; the Go toolchain is the one build
toolchain this adds.

`internal/rpc` in `tui/` is modelled on `desktop/src/child.rs`: it spawns
`trellis rpc`, sends `quit` and never kills. Go code spawns no `git` (ADR
0023) and changes no child's environment; a check, `go-guards.sh`, refuses
both.

### 4. stderr in a capped pane

A client may show trellis's stderr to the user in a pane capped in size, as
human text. It still never reads stderr as data: what a client computes from
comes from JSON on stdout (ADR 0048 section 2, amended).

### 5. Supply chain for Go

The `supply-chain` CI job runs `go mod verify`, `govulncheck` and
`go-licenses` over `tui/` on every run. As ADR 0048 section 7 holds for
Tauri's tree, a vulnerability is never ignored, and a licence finding blocks
the ticket that brings it. Release builds of `trellis-tui` use
`CGO_ENABLED=0 -trimpath`. `go test -race` needs cgo, so it is optional and
Linux-only.

### 6. CI and release

- A `go` CI job builds and tests `tui/` on both self-hosted runners, and
  Verify lines reach Go through a new `scripts/checks/go-test.sh` rather than
  by widening G6.
- Gates G1 to G5 gain Go branches, skipped while `tui/` is absent;
  `workflows-locked.sh` learns `go` commands; `no-stubs.sh` covers `*.go`.
- The desktop job's timeout is raised for a cold Tauri build on both runners.
- The release job builds `trellis` and `trellis-tui` for Windows and Linux on
  the self-hosted runners, and lists both in `SHA256SUMS` and in #386's
  version check.

### 7. The parity bar, and retiring the Rust TUI

`tui/PARITY.md` maps every rule the ratatui TUI's tests hold to a Go test:
`app.rs` (40 tests), `view.rs` (23), `events.rs` (6), `tui_bridge.rs` (16)
and `tui_run.rs` (23). Parity is reached when every row maps to a Go test
that passes in `go-test.sh` on both runners.

At parity, bare `trellis` and `trellis tui` hand off to `trellis-tui`, with
the ratatui fallback. One release later the Rust TUI is retired: deleting
`crates/tui-demo` (and with it the `ratatui` dependency it carries),
`tui_run.rs`, `tui_bridge.rs` and the fallback branches in `main.rs`, rewriting the TUI tests
in `cli.rs` and `session.rs`, and checking the result with
`scripts/checks/path-absent.sh crates/tui-demo` and
`scripts/checks/text-absent.sh ratatui crates`.

## Alternatives rejected

**Keep ratatui and build only the desktop.** It leaves the owner's second
goal unmet. **Keep both TUIs.** Two interfaces with two homes for every
change is the drift Q-19 was answered to end. **A Go TUI that reaches a
model runtime or the repository itself.** It would bypass the snapshot,
trust gate, checks, rollback and trail that `cmd_run_with` owns, as ADR 0048
refused for the desktop page. **A Node-based TUI.** ADR 0048 refused a
JavaScript toolchain on every build machine, and this does not reopen it.
**Go fetched at job time, or from a distribution package.** Neither pins the
toolchain to a hash this repository records. **Routing re-implemented in
Go.** Two copies of the question-or-task rule would drift.

## Consequences

- The repository holds a second language. Reviewers of `tui/` read Go, and
  the hygiene guarantees the Rust tests give (ADR 0023, the secrets scrub)
  are held for Go by `go-guards.sh`, not by the Rust tests.
- CI gains the `go` job and three supply-chain steps; cold Tauri and Go
  builds run one at a time on the runners, under the Windows commit limit.
- Releases lose macOS until a macOS runner exists.
- The protocol grows by `classify`, `list_paths` and a `tier` field in
  `get_state`, each with contract tests on both clients. Merges that touch
  `client_contract.json` go in one at a time.
- At retirement, `ratatui` leaves the root `Cargo.lock`, and the crate table
  in CLAUDE.md loses `tui-demo`.

## What would reverse it

- The owner declines it on #500: it stays unaccepted, nothing is built on it,
  and the amendment lines it added to ADR 0048, ADR 0021 and ADR 0050 are
  struck.
- The Go TUI does not reach parity: the handoff (decision 2.2) does not ship,
  `tui/` is deleted, and ratatui stays the product interface.
- A Charm-set vulnerability or licence finding with no fixed release: the
  same as not reaching parity, until one exists.
- A macOS runner: macOS returns to releases. That reverses decision 2.5 only.

## Ticket plan

Each ticket gets its own `t<n>-<slug>` branch and worktree, and goes through
`audit` → `dev` → `main` (ADR 0051). The lettered tickets are filed once this
ADR is accepted. `∥` marks tickets that can run in parallel.

- **Owner actions (P0, not tickets):** apply #219 on WSL; install
  `libwebkit2gtk-4.1-dev`, `libxdo-dev` and `pkg-config` on WSL; check for
  gcc.
- **Wave 0, records:** T0, this ticket (#500).
- **Wave 1, protocol and toolchain:** #289 ∥ (the Tauri window, blocked by
  P0), R1 ∥ (`classify`), R2 = #269 ∥ (`tier` in `get_state`), R4 ∥
  (`list_paths {prefix}` for `@` completion), #312 ∥, and G1
  (`scripts/runners/install-go.sh`, blocked by T0 and P0).
- **Wave 2, desktop reducer and Go guards:** #296, amended, which closes
  #299; G2 ∥ (Go guards and gate branches, blocked by G1).
- **Wave 3, lockdown, skeleton and release:** #298; G3 ∥ (the `tui/`
  skeleton and the `go` CI job, blocked by G2); T7 ∥ (release from the
  self-hosted runners, blocked by G1 and #386).
- **Wave 4, first usable slices:** #301, amended (composer, Stop, auto or
  approve); T2 ∥ (Go slice 1: frames, reducer, a streaming prompt, Ctrl-C).
- **Wave 5, completing both clients:** the desktop's #300, #302 and N11; the
  Go TUI's T3 (approvals, steering, abort), T4 (the remaining panes, blocked
  by #312), T5 (status bar, `@` completion, slash commands, blocked by R2 and
  R4) and T6 (routing, blocked by R1).
- **Wave 6, launch switch:** T8, the handoff and its ADR 0050 amendment.
  risk:high (G7).
- **Wave 7, retire the Rust TUI:** T9, one release after T8. risk:high (G7).

## Amendment 2026-10-07: keys for the TUI's own rpc child (#514)

TRELLIS scrubs provider-key variables from every child it starts (`tests/child_env.rs`), but `trellis-tui` starts its own `trellis rpc`, which needs the user's keys for hosted models. Owner decision (#514): the handoff from `trellis` to the sibling `trellis-tui` passes the environment unchanged, as one named exception held by a test. Every other child stays scrubbed.

*Amended 2026-10-07 (#560), owner-approved (G7, 2026-10-07):* the exception names two launches, `trellis-tui` and `trellis-desktop`. The desktop's is there for the same reason as the TUI's: its own `trellis rpc` child needs the user's key variables for a hosted model the user chose. One counted test holds both: `INTERFACE_LAUNCHES` in `tests/child_env.rs` lists exactly these two, and an unscrubbed `Command::new(` passes only in `crates/trellis-cli/src/launch.rs`, on a line carrying `// key-exempt: <binary>` that names a listed binary, each binary at most once. A third launch is an edit to that list and to this ADR, not a comment. The owner approved it with one condition, recorded on #564: after launch the desktop asks which model to use and keeps prompt input disabled until one is chosen in that window (ADR 0048 section 5, amended), so no prompt runs on the keys it passes on before the user has picked a model. The test's exemption is built; `launch.rs` and the launches that use it are not (#374). Every other child stays scrubbed.

## Amendment 2026-10-07: handoff targets, the desktop's environment, and G7 approvals (#557)

Owner decision, 2026-10-07: bare `trellis` at a terminal opens the desktop and prints the overview (ADR 0050 section 1, amended 2026-10-07). Decision 2.2 and section 7's handoff therefore apply to `--tui` and `trellis tui` only: they, not bare `trellis`, hand off to `trellis-tui` at parity, with the ratatui fallback (#514). The amendment above names `trellis-tui` as the one launch that passes the environment unchanged; the desktop's own `trellis rpc` child needs the user's key variables for a hosted model the user chose, so the launch of `trellis-desktop` is to be the second, held by the same counted test. Accepted, not built (#560). Every other child stays scrubbed.

G7 record: the owner approved #289 and #296, each with the scope added to it on 2026-10-07, and #560 and #561, on 2026-10-07.
