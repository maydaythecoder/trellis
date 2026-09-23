# Dependency Audit — 2026-09-22

## Verdict

Removed `git2` and its C-building subtree, eleven declarations that were never
used, the TLS stack behind `reqwest`'s defaults, llguidance's 88-crate subtree,
`color-eyre` from the shipped graph, and — the single largest win — the regex
engine, by swapping `EnvFilter` for `Targets`. **The binary is 19.6% smaller and
a clean release build is ~43% faster.**

**The biggest remaining question is Q-19, not a dependency.** `crates/chat` is
5,500 lines, reachable only from its own binary, and carries `reqwest`,
`tui-logger`, `ansi-to-tui` and `tui-scrollview` alone. Whether it ships is an
owner decision this audit deliberately did not make.

## Numbers

Timings are medians of repeated clean builds; a single sample was not stable
enough to quote.

**Method, because a reviewer could not reproduce two of these without it.**
Every figure is scoped to the shipped binary — `cargo build --release -p
trellis-cli` — except the "whole workspace" row. Exact commands:

```
release graph   cargo tree -p trellis-cli --edges normal --prefix none                   | sed 's/ (\*)//' | awk '{print $1}' | sort -u | wc -l
whole workspace cargo tree --prefix none | sed 's/ (\*)//'                   | awk '{print $1" "$2}' | sort -u | wc -l
tree lines      cargo tree | wc -l          # whole workspace, not -p
binary          ls -la target/release/trellis.exe
duplicates      cargo tree -p trellis-cli --edges normal -d
```

The release-graph count is **names only, deduplicated** — a crate present at two
versions counts once. Counting name+version instead gives 159 rather than 157,
which is the discrepancy a reviewer hit. The tree-lines row is whole-workspace;
the `-p trellis-cli` equivalent is 315.

| | Before | After | Δ |
|---|---|---|---|
| Unique crates, release graph | 189 | **157** | **−32** (−16.9%) |
| Unique crates, whole workspace | 285 | 258 | −27 |
| `cargo tree` lines | 758 | 660 | −98 |
| Clean release build | 87 s, 100 s (~93 s) | 54, 54, 53, 49 s (~53 s) | **~−43%** |
| Binary size | 5,981,184 B | **4,810,240 B** | **−1,170,944 B (−19.6%)** |
| Duplicate versions, release graph | 2 pairs | 2 pairs | 0 |

Verified at every step: `cargo build --all-targets`, `cargo test --workspace`
(703 tests), `cargo clippy --all-targets -- -D warnings` (clean), and the
11-case deterministic guard suite in `qa/run-guards.py` — 11/11 both before and
after, including the runtime-flavour change.

**Correction to an earlier claim in this file.** Mid-audit I wrote that
llguidance caused both duplicate pairs. It does not. `cargo tree -i` shows the
*first* path only, and I read a truncated one. `syn 3` is pulled by
`clap_derive` and by `ratatui`'s `instability`; `hashbrown 0.17` by `lru` inside
`ratatui-core`. Removing llguidance from the release graph changed the duplicate
count by zero, which the before/after measurement shows.

## Every dependency

| Crate | Verdict | Justification / what breaks without it | Action taken |
|---|---|---|---|
| `tree-sitter` | KEEP | The only thing that parses Python. Without it there is no call graph and therefore no blast radius — the product's whole premise | none |
| `tree-sitter-python` | KEEP | The grammar. ABI-paired with `tree-sitter`; `grammar_abi.rs` turns a runtime mismatch into a test failure | none |
| `llguidance` | **TRIM** | `ToolGrammar` compiles the tool-call schema — and every caller is inside a `#[test]`. The capability is adapter tier 3, descoped by ADR 0013. What remains is a build-time assertion worth keeping and not worth shipping | optional behind `grammar`, off by default; `trellis-loop`'s dev-dep re-enables it. 88-crate subtree out of the release graph; the test still runs |
| `schemars` | **TRIM** (model) / KEEP (loop) | In `trellis-model` it is used only by `grammar.rs`. In `trellis-loop` it generates the tool-call schema that tier 2 sends | optional in `trellis-model`, tied to the same feature |
| `rustyline` | KEEP | Line editing for the session. Without it an arrow key arrives as an escape sequence, falls past the command match, and starts an agent run — a recorded defect, not a hypothetical | none; already `default-features = false` |
| `trybuild` | KEEP (dev) | The v0.2 gate asks for a *compile-fail* proof that `Verdict` cannot be forged. Asserting it by inspection is not that | none |
| `sha2` | KEEP | The trail's tamper-evident digest. A 64-bit non-cryptographic hash would not be evidence of anything, and `tampered_trail` is a guard that depends on it | none |
| `serde` | KEEP | Index cache, trails, tool calls, config. Hand-rolling is thousands of lines of correctness risk | none |
| `serde_json` | KEEP | The wire format for every tool call and every trail | none |
| `tokio` | **TRIM** | See the Tokio section. Async is forced by `crossterm`'s `EventStream` and by subprocess checks with timeouts | `full` removed from two crates; `rt-multi-thread` → `rt` |
| `rayon` | KEEP (index) / **CUT** (cli) | Parallel parse and resolve in `trellis-index` (`par_iter` in three modules). Declared by `trellis-cli` and never used there | removed from `trellis-cli` |
| `postcard` | KEEP | `.trellis/index.bin`. `bincode` 3.0.0 is a protest release that deliberately fails to compile, which is why this is here | none |
| `git2` | **CUT** | Referenced in exactly two comments explaining why the crate shells out to `git` instead. No `use`, no call. The comment says it stays "for worktrees" — a feature that does not exist | removed; took `libgit2-sys`, `libz-sys`, `jobserver`, `pkg-config`, `vcpkg` with it |
| `clap` | KEEP | The CLI surface. Hand-rolled argument parsing with subcommands and derive is well past 80 lines | none |
| `toml` | KEEP | `.trellis/config.toml` and the governance rule documents | none |
| `thiserror` | KEEP (core/checks/model) / **CUT** (loop, trail) | Error enums in the libraries. Declared and never used by `trellis-loop` and `trellis-trail` | removed from both |
| `anyhow` | KEEP | The binary's error type. `thiserror` in libraries, `anyhow` in the binary, per CLAUDE.md | none |
| `tracing` | KEEP / **CUT** (chat) | Instrumentation *and* the budget accounting substrate — CLAUDE.md forbids a second telemetry path. `chat` declares it and never uses it | removed from `chat` |
| `tracing-subscriber` | **TRIM** | Kept — it is the log sink. But its `env-filter` feature pulls `matchers` → the whole regex engine: 184 KiB `regex_syntax` + 146 KiB `regex_automata`, measured with `cargo bloat`. `filter::Targets` parses `target=level` without regex, which is the only form this project has ever used | `env-filter` dropped; `EnvFilter` → `Targets`. **−1,045,000 B** |
| `smallvec` | KEEP | Edge storage in the index. `Candidates` is usually 1–4 symbols, and a `Vec` per call site is a heap allocation per call site | none |
| `walkdir` | KEEP (index) / **CUT** (cli) | `discover()` walks the repository. Declared by `trellis-cli` and never used there | removed from `trellis-cli` |
| `insta` | KEEP (chat) / **CUT** (index) | `chat` asserts snapshots with committed `.snap` files. `trellis-index` declared it and never used it — **found by `cargo-udeps`, missed by `cargo-machete`**, because "insta" hides inside `instance`, `isinstance` and `installed`, which a grep-based tool cannot tell apart | removed from `trellis-index` |
| `tempfile` | KEEP (dev) | Fixture repositories for the orchestrator and trail tests | none |
| `ratatui` | KEEP (tui-demo, chat) / **CUT** (cli) | The interface. Declared by `trellis-cli`, which mentions it once — in a comment | removed from `trellis-cli` |
| `crossterm` | KEEP (tui-demo, chat) / **CUT** (cli) | **A machete false positive worth naming.** Reached through `ratatui::crossterm::`, so there is no direct `use`; the direct dependency exists solely to turn on `event-stream`, without which `EventStream` does not exist. Unused in `trellis-cli` | removed from `trellis-cli`; kept in the two that need the feature |
| `futures` | **SWAP** → `futures-util` | The only import anywhere is `StreamExt`, three files. The facade additionally pulls futures-executor, -channel, -io and -sink for nothing | swapped, `default-features = false`; 3 one-line import changes |
| `color-eyre` | KEEP (chat, demo bin) / **TRIM** (shipped graph) | `chat` wraps its panic hook so a panic inside a full-screen interface restores the terminal before printing. But `tui-demo`'s *library* used it for exactly one thing — a return type — and `trellis-cli` inherited 37 crates for it, including `backtrace`, `tracing-error` and `color-spantrace` | optional behind `demo`, default-on for the demo binary; `trellis-cli` takes `tui-demo` with `default-features = false`. Library returns `anyhow::Result`, which also removed a `map_err` at the call site |
| `reqwest` | **TRIM** | `chat`'s HTTP client. Default features drag `hyper-tls`, `native-tls`, `tokio-native-tls` and `schannel`; this client talks to `127.0.0.1:11434` and there is no `https` literal anywhere in the crate | `default-features = false`; 17 crates removed. **Consequence recorded: an https endpoint needs `default-tls` added back** |
| `tui-logger` | KEEP (chat) | The log pane, integrated as a tracing layer. Pinned to 0.18 for ratatui 0.30 | none |
| `ansi-to-tui` | KEEP (chat) | Renders ANSI from subprocess output into ratatui spans. Doing this correctly — SGR parameters, 256-colour, truecolour — is not a small function | none |
| `tui-scrollview` | KEEP (chat) | Scrollable viewport widget | none |
| `trellis-core` | **CUT** (checks, model, trail, vcs) | Zero references in all four. Still used by `trellis-index` | removed from four manifests |
| `trellis-index`, `-checks`, `-model`, `-loop`, `-trail`, `-vcs`, `tui-demo` | KEEP | Workspace members; the crate boundaries are load-bearing per CLAUDE.md (`trellis-index` must never depend on `trellis-model`; `trellis-checks` owns `Verdict`) | none |

## Tokio

### Is async needed at all?

**Real concurrent task count: 4–7.** A run has the agent-loop future, the
orchestrator actor, and a Ctrl-C race. The interface adds a terminal event
stream, a driver task, a tier probe, and a `spawn_blocking` worker holding the
run. That is a low number, and by the brief's own reasoning a threads-and-
channels design would serve it.

**One thing forces async independently of task count**, and a second claim made
here earlier was wrong:

- `crossterm`'s `EventStream` is the async input source both interfaces use.
  The blocking `event::read()`/`poll()` alternative exists, but switching means
  rewriting both event loops. This one holds.
- ~~Subprocess checks need a timeout.~~ **Withdrawn.** A reviewer checked and
  `grep -rn "timeout" crates/trellis-checks/` returns nothing: the code is
  `tokio::process::Command…output().await` with no deadline and no kill. The
  claim described a timeout that would need async *if it existed*. Every real
  `tokio::time::timeout` in the workspace is in `trellis-model`'s HTTP client or
  in tests.

  This is also a product gap, not merely a bad justification: a `trellis run`
  whose mypy or pytest hangs has no deadline at the check. Pre-existing, not
  introduced here, and recorded because looking for it is what disproved the
  claim.

So the honest position is **one** forcing constraint, not two, and the un-built
threads comparison is correspondingly less excused.

**I did not build the threads version, and that is a gap rather than a
judgement.** The brief asks for both sets of numbers. Async colours six crates
here — `trellis-model`'s HTTP client, `trellis-checks`'s process runner,
`trellis-loop`'s `Model` trait and orchestrator, and both interfaces — so
removing it is a multi-thousand-line rewrite, far past the ~80-lines-per-swap
authority this audit was given. Listed under Unverified.

### Why tokio over the alternatives?

Ecosystem compatibility, stated explicitly rather than implied. `reqwest`
requires tokio. `crossterm`'s `event-stream` feature is built against it.
Moving to `smol` or `async-std` would mean replacing the HTTP client and
rewriting the terminal event integration — a larger change than removing async
altogether, for a smaller benefit.

`pollster` deserves a mention and does not fit: the binary does not block on one
future at one boundary, it has eight runtime construction sites and a spawned
actor.

### Features, before → after

```
before  ["rt-multi-thread", "macros", "sync", "process", "time", "net", "io-util", "signal"]
after   ["rt",              "macros", "sync", "process", "time", "net", "io-util", "signal"]
```

That understates it, because the workspace list was not what the binary got.
**`tui-demo` and `chat` each declared `tokio = { features = ["full"] }`**, and
feature unification applies across the graph — `trellis-cli` depends on
`tui-demo`, so the shipped binary received *every* tokio feature regardless of
what the workspace said. The curated list was decorative.

Resolved feature set for `trellis-cli`, before → after:

```
before  bytes default fs full io-std io-util libc macros mio net process rt
        rt-multi-thread signal signal-hook-registry sync time tokio-macros windows-sys
after   bytes default io-util libc macros mio net process rt signal
        signal-hook-registry sync time tokio-macros windows-sys
```

Gone: `full`, `fs`, `io-std`, `rt-multi-thread`.

**`rt-multi-thread` → `rt` required a code change and it worked.**
`Runtime::new()` builds a multi-threaded runtime, so eight sites became
`Builder::new_current_thread().enable_all().build()` and three `#[tokio::main]`
attributes gained `flavor = "current_thread"`. 703 tests pass and all 11
deterministic guards hold — including the orchestrator actor, subprocess
checks, rollback, trail integrity and resume, which is where a current-thread
deadlock would have shown.

**Delta from the tokio trim alone: −113,664 bytes of binary.** The largest
single win in the audit per line changed.

`net` was checked rather than assumed: `trellis-model/src/runtime.rs:26` uses
`tokio::net::TcpStream` in production code for a hand-rolled HTTP/1.1 client,
which exists specifically to avoid a heavier HTTP dependency in the core. KEEP.

## Cut

| What | Why | Replaced by | Lines added |
|---|---|---|---|
| `git2` | Two comments, no code. Took `libgit2-sys`, `libz-sys`, `jobserver`, `pkg-config`, `vcpkg` | nothing — `trellis-vcs` already shells out to `git` | 0 |
| `rayon`, `walkdir`, `ratatui`, `crossterm` from `trellis-cli` | Zero references | nothing | 0 |
| `thiserror` from `trellis-loop`, `trellis-trail` | Zero references | nothing | 0 |
| `trellis-core` from `-checks`, `-model`, `-trail`, `-vcs` | Zero references | nothing | 0 |
| `tracing` from `chat` | Zero references (`tracing-subscriber` is a separate dependency and is used) | nothing | 0 |
| `futures` → `futures-util` | Only `StreamExt` used | `futures-util`, no default features | 3 changed imports |
| `reqwest` default features | TLS stack for a plain-HTTP localhost client | nothing | 0 |
| llguidance + schemars from the release graph | Test-only callers | a `grammar` feature, on for tests | 4 lines of cfg |
| `color-eyre` from the shipped graph | `tui-demo`'s lib used it for one return type; `trellis-cli` inherited 37 crates | `anyhow::Result`, already in the workspace | 1 signature, 1 bridged `install()`, −1 `map_err` |
| `env-filter` / `EnvFilter` → `Targets` | `matchers` pulls the regex engine for a filter only ever used as `target=level` | `filter::Targets` | ~12 lines, net |

## Deferred

**`cargo-unused-features` cannot parse this workspace.** Four angles:

1. **Newer version?** `cargo search` says 0.2.0 is the latest published.
2. **Per-crate, bypassing the root?** Members declare `dep.workspace = true`, so
   a member manifest alone resolves nothing — the run produced a report with
   `"workspace_crates": {}`.
3. **Temporarily set `resolver = "2"`?** That got past the first error and
   straight into the real one: `unknown variant 2024, expected one of 2015,
   2018, 2021`. The tool's TOML schema predates edition 2024 entirely.
4. **Downgrade the edition?** Not available. `ASSUMPTIONS` A-1 records edition
   2024 as deliberate for `let`-chains, which this codebase uses throughout, and
   changing a project's edition to satisfy an audit tool inverts the point of
   the audit.

What I would do next: either wait for the tool to support edition 2024, or get
the same answer the slow way with `cargo-hack`'s `--feature-powerset` per
dependency, which builds rather than parses and therefore does not care about
the edition. The manifest was restored and the tree verified green afterwards —
`resolver = "3"`, `edition = "2024"`, 703 tests, clippy clean.


**Duplicate versions: `syn` 2/3 and `hashbrown` 0.16/0.17.** Both remain, and
both are upstream.

Angles tried: (1) traced every consumer with `cargo tree -i` — `syn 3` comes
from `clap_derive` and from `darling_core` inside `ratatui`'s `instability`,
`syn 2` from `derive_more` inside `crossterm`; `hashbrown 0.17` from `lru`
inside `ratatui-core`, `0.16` from `kasuari` in the same crate. (2) Attempted a
version bump — `cargo update -p llguidance` reports 1.8.0 is already the latest,
and `ratatui` 0.30.2 and `crossterm` 0.29 are current. (3) Checked whether
removing llguidance would unify them; measured before and after, and it changed
nothing, which corrected my own earlier claim. (4) Considered pinning, which
would mean forcing a version on a transitive dependency of two independent
crates and is how a build breaks quietly later.

`ratatui-core` depends on both `lru` (hashbrown 0.17) and `kasuari` (0.16)
simultaneously, so one crate is internally inconsistent upstream. Nothing in
this repository can fix that. What I would do next: file it upstream with
ratatui, and re-check on the next ratatui release.

## What `cargo bloat` showed

Run on the final binary, and it is why the last two cuts happened — the table
named `regex_syntax` at 184 KiB and `regex_automata` at 146 KiB in a tool that
has no regex in its own source. Tracing that to `matchers` ← `env-filter` was
the single most valuable thing in this audit.

Remaining composition, `.text` = 3.8 MiB before the regex cut:

```
std            945.6 KiB   trellis        598.2 KiB
clap_builder   265.9 KiB   trellis_loop   202.4 KiB
trellis_index  178.7 KiB   tokio          169.7 KiB
rustyline      118.9 KiB   toml           101.1 KiB
```

`clap_builder` at 266 KiB is the largest third-party contributor left and is
KEEP: it is the CLI surface, and hand-rolled subcommand parsing with derive is
well past the 80-line swap limit. Naming it as the next candidate if binary size
ever becomes a gate.

## Review findings, and what changed because of them

An independent reviewer went over the uncommitted changes. It reproduced every
number — binary 4,810,240 B to the byte, 703 tests, 11/11 guards — and found
the defects in what those numbers do not cover. Acted on:

| Finding | Status |
|---|---|
| Two `trellis-model` grammar tests silently vanish under `cargo test -p trellis-model` (43 vs 45) — coverage depended on another crate being in the same invocation | **Fixed**: the crate now dev-depends on itself with `grammar` on. 45 standalone |
| `RUST_LOG=` (set, empty) turned *silence* into global ERROR logging — `EnvFilter` short-circuits on an empty directive list, `Targets` reads one empty directive as ERROR for every target | **Fixed**: empty is handled explicitly and is silent again, verified |
| `chat` on an https endpoint fails with the same wording as a refused connection; `base_url` comes from `OPENAI_BASE_URL`, so "no https anywhere in this crate" was wrong | **Fixed**: the error now names the missing TLS backend and the feature that restores it |
| The `trellis-model` manifest still claimed the feature gate removed both duplicate pairs — the claim this report had already retracted | **Fixed**: comment corrected in place |
| "The one caller of `ToolGrammar` is a test in `wire.rs`" — there are two more in `grammar.rs`, and they are the ones exercising the derive → schema → grammar path | **Fixed** |
| The async justification cited a subprocess timeout that does not exist | **Withdrawn**, above |
| Measurement method not stated; two counts unreproducible | **Fixed**, above |
| `terminal.rs` comment described a `color_eyre` hook the shipped binary no longer has | **Fixed** |

Accepted and not acted on, with reasons:

- **The sanctioned commands do not build what ships.** `cargo build --workspace`
  unifies `demo` and `grammar` and produces a `trellis.exe` linking `color-eyre`;
  `cargo build -p trellis-cli` does not. Two binaries from one tree. The reviewer
  ran `clippy -p` for each member separately and found nothing, so nothing is
  broken — but CLAUDE.md's command list only ever exercises the unified build.
  Changing that list is a project decision, not an audit's.
- **Four cuts changed a declaration without changing the graph.** `rayon` and
  `walkdir` still arrive via `trellis-index`, `ratatui` and `crossterm` via
  `tui-demo`. They contributed zero crates and zero bytes to the headline
  numbers. Correct as hygiene, worth nothing as weight, and now said plainly.
- **`chat` still enables `env-filter`**, so a workspace-wide build still compiles
  the regex engine. The regex removal is real for the shipped binary and only
  for it.
- **`RUST_LOG=trellis[{turn}]=trace`** parses but matches nothing, silently.
  Named in the source comment now rather than left as a surprise.

## Unverified

- **The threads-and-channels alternative was never built.** The brief asks for
  both sets of numbers and this report has one. The reason is the ~80-line swap
  limit against a six-crate change, but the number is still missing, and the
  claim "async is needed here" therefore rests on two forcing constraints rather
  than on a measurement.
- ~~`cargo udeps` never ran.~~ **Closed.** Nightly installed and
  `cargo +nightly udeps --all-targets` run. The cross-check earned its keep: it
  found `insta` unused in `trellis-index`, which machete missed because the
  substring "insta" appears in `instance`, `isinstance` and `installed`. That is
  the exact false-negative class a grep-based tool has, and the reason the brief
  asks for both. udeps flagged nothing else, agreeing with machete on the rest.
- **`cargo-unused-features` cannot run here.** See Deferred. The remaining
  flags were checked by hand instead and all three survive on evidence:
  `smallvec`'s `serde` is required because `Target::Candidates(SmallVec<..>)`
  sits inside a `#[derive(Serialize, Deserialize)]` enum, so the index cache
  cannot round-trip without it; `rustyline`'s `with-file-history` is required
  because `session.rs` both loads and saves history; `tui-logger`'s
  `tracing-support` is required because `TuiTracingSubscriberLayer` is the whole
  integration. `smallvec`'s `union` is a memory optimisation — one word per
  `SmallVec`, two per symbol across the forward and reverse maps — and is kept
  on that reasoning rather than on a measurement, which is a judgement and is
  labelled as one.
- **The `Targets` swap gives up span-field filtering.** `RUST_LOG=[span{field=x}]`
  no longer parses. Nothing in this repository has used it; if someone does, the
  fix is re-adding one feature. Verified that per-target and submodule filtering
  still discriminate correctly — `trellis_loop::agent=trace` yields the agent's
  events and `trellis_loop::nosuchmodule=trace` yields none, which a silent
  parse failure could not produce.
- ~~`chat`'s `reqwest` trim is compile-verified, not run-verified.~~
  **Closed.** `cargo test -p chat --test live_model` against a live ollama
  passes both cases after the trim — 10 streamed chunks and a real
  `read_file` tool call returned through the tool channel, over plain HTTP with
  no TLS stack in the tree. The transport works, not merely compiles.
- **The binary-size figure is Windows/MSVC only.** No other target was built,
  and `libgit2-sys`/`libz-sys` removal in particular will behave differently
  where those link against system libraries.
- **Timings are wall-clock on a machine running an editor and a virus scanner.**
  Medians of repeated runs, with the spread quoted, but not a controlled
  benchmark.
