# Dependency Audit — 2026-09-28

Predecessor: `audit/DEPENDENCIES-2026-09-22.md`. This file re-audits what
changed in the manifests since, measured with that file's commands. It applies
no verdict: the one TRIM below is its own ticket.

## Verdict

Every dependency line the inherit PR added is used and is KEEP, except `jiff`,
which is TRIM (#154) for a measured 21,504 B. The release graph grew from 157 to
176 crates, 19 in all: 3 on `v0.1-resolver` after the last audit, 16 from the
inherit PR, 10 of which are the `hosted` TLS stack.

**The binary more than doubled, and dependencies are not most of it.** The
default release build is 10,861,056 B against the 4,810,240 B baseline
(+6,050,816 B). The `--no-default-features` build, which links none of the TLS
stack, is 9,519,616 B, so `hosted` accounts for 1,341,440 B of the growth. The
remaining ~4.7 MB came with 9 crates that are either first-party
(`trellis-queue`, `trellis-vault`) or small (`jiff`, `unicode-normalization`,
`tracing-serde`, and their dependencies). ADR 0024 recorded 7,084,032 B at
`3234bee`, before any TLS crate was added. Most of the growth is therefore
first-party code: the agent loop, queue, vault, RPC and hosted-protocol work.
That is inferred from these figures and was not attributed per crate. The
`cargo bloat` run that would attribute it was stopped (see Unverified).

## Numbers

**Measured on:** Windows 11 Home 10.0.26200, target `x86_64-pc-windows-msvc`,
rustc/cargo 1.93.0. Release profile as the workspace defines it: `lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`. Tree at `inherit` = `969c059`.

**Method.** These are the predecessor's commands, unchanged:

```
release graph   cargo tree -p trellis-cli --edges normal --prefix none | sed 's/ (\*)//' | awk '{print $1}' | sort -u | wc -l
whole workspace cargo tree --prefix none | sed 's/ (\*)//' | awk '{print $1" "$2}' | sort -u | wc -l
tree lines      cargo tree | wc -l          # whole workspace, not -p
binary          cargo build --release -p trellis-cli ; ls -la target/release/trellis.exe
duplicates      cargo tree -p trellis-cli --edges normal -d
```

`--no-default-features` is added to `cargo tree` and to `cargo build` for the
local-only rows. **Evidence that the method is the same:** the release-graph
command run at `7ad9b2d` (the commit that added the predecessor) prints 157,
the predecessor's figure.

| | Baseline (2026-09-22, `7ad9b2d`) | `origin/v0.1-resolver` (`e64b80c`) | `inherit` (`969c059`), default features | `inherit`, `--no-default-features` |
|---|---|---|---|---|
| Unique crates, release graph (names) | 157 | 160 | **176** | 166 |
| Unique crates, release graph (name+version) | 159 | — | 178 | — |
| Unique crates, whole workspace | 258 | — | 273 | — |
| `cargo tree` lines | 660 | — | 717 | — |
| `trellis.exe` bytes | 4,810,240 | not built | **10,861,056** | 9,519,616 |
| Duplicate versions, release graph | 2 pairs | — | 2 pairs (`syn` 2/3, `hashbrown` 0.16/0.17) | — |

- The default build was built twice from the same tree, with the
  `--no-default-features` build between them. Both default builds gave
  10,861,056 B.
- **The default build is what ships** (`release.yml` passes no feature flags,
  ADR 0024). The `--no-default-features` figure is the local-only binary, given
  separately. It does not replace the default figure.
- Compared with ADR 0024's measurements at `29528e2` (8,685,568 B default /
  7,357,440 B local-only, same OS, target and profile): +2,175,488 B default
  and +2,162,176 B local-only since then. The `hosted` cost is 1,328,128 B there
  and 1,341,440 B here.
- The ADR 0024 table's "178 crates" is the name+version count. The two counts
  agree.

**Where the 19 crates came from** (the `comm` of the sorted name lists):

- `v0.1-resolver` after the audit (+3), all from `1d3d06f` (2026-09-27) and the
  vault work: `tracing-serde`, `trellis-queue`, `trellis-vault`.
- The inherit PR, `hosted` only (+10): `rustls`, `rustls-pki-types`,
  `rustls-webpki`, `ring`, `untrusted`, `subtle`, `zeroize`, `tokio-rustls`,
  `webpki-roots`, `getrandom` (0.2, through `ring`). This is the same ten that
  ADR 0024 lists.
- The inherit PR, both builds (+6): `jiff`, `jiff-core`, `jiff-tzdb`,
  `jiff-tzdb-platform`, `unicode-normalization`, `tinyvec`.
- None were removed.

**Duplicates, two scopes.** The row above uses the predecessor's release-graph
scope and is unchanged at 2 pairs. `cargo deny check` looks at the whole
workspace, dev-dependencies included, and warns of 4: `syn`, `hashbrown`,
`getrandom` and `windows-sys`. ADR 0024 records that the two added pairs come
through `ring`.

## Every dependency line added

These rows are the `+` lines of
`git diff origin/v0.1-resolver...inherit -- '*Cargo.toml'`. They are followed
by the named items that no manifest line in that diff adds. Usage was checked by
grepping each crate's `src/` and `tests/` for the dependency's path. `cargo
machete` reports only `crossterm` in `tui-demo`, which is the false positive the
predecessor explains (it is reached through `ratatui::crossterm::`).

| Manifest | Line | Verdict | Reason |
|---|---|---|---|
| `trellis-checks` | `trellis-core.workspace = true` | KEEP | `trellis_core::secrets::scrub` on every spawned check (the child-env invariant in CLAUDE.md), plus `paths::symlink_for_test`. The predecessor cut this line for zero references. It now has references. No new crate: `trellis-core` was already in the graph |
| `trellis-vcs` | `trellis-core.workspace = true` | KEEP | `trellis_core::secrets::scrub` on every `git` spawn (`lib.rs`), the same invariant. Also re-added after the predecessor's cut, for the same reason |
| `trellis-vcs` | `[dev-dependencies] tempfile.workspace = true` | KEEP (dev) | Fixture repositories for `git_guard.rs` tests. Not in the release graph |
| `trellis-cli` | `serde_json.workspace = true` (moved from dev to normal) | KEEP | `--json`, `log --json`, `rpc` framing and the after-run hook's outcome JSON (44 references). No new crate: `serde_json` was already in the graph through the libraries |
| `trellis-cli` | `jiff = "0.2.37"` | **TRIM** → #154 | `trellis vault dates` needs *local* calendar days, and std has no time zones. It is used at two sites in `vault_cmd.rs`, and on Windows it needs the bundled tzdb, because Windows has no system zoneinfo. Keep the crate and drop the default features those calls do not use (`perf-inline`, `tzdb-concatenated`). Measured: −21,504 B, crate count unchanged (method under TRIM measurement). Adds 4 crates (`jiff`, `jiff-core`, `jiff-tzdb`, `jiff-tzdb-platform`) |
| `trellis-cli` | `[features] default = ["hosted"]` | KEEP | ADR 0024's decision: tier 1 over HTTPS is in the released binary. Cost measured here: +10 crates, +1,341,440 B (12.3% of the default binary). A local-only binary is one flag away |
| `trellis-cli` | `hosted = ["trellis-model/hosted"]` | KEEP | Forwards the feature. Without it `--no-default-features` could not drop TLS |
| `trellis-loop` | `trellis-vault.workspace = true` | KEEP | `search_notes`/`read_note` use the vault's reader and BM25 index (`notes.rs`) and do not keep a second copy. `trellis-vault` does not depend on `trellis-model`, so the CLAUDE.md boundary holds |
| `trellis-loop` | `trellis-vcs.workspace = true` | KEEP | The worktree fingerprint around `post_tool` hooks, and the `git grep` in `listing.rs` goes through `trellis_vcs::git_argv` (the every-git-spawn invariant) |
| `trellis-loop` | `sha2.workspace = true` | KEEP | The trust store's fingerprints (`trust.rs`, ADR 0023). Already in the graph for the trail |
| `trellis-loop` | `thiserror.workspace = true` | KEEP | `exec.rs` and `hooks.rs` error enums. The predecessor cut it for zero references. It now has them |
| `trellis-model` | `rustls` (0.23.45, `ring`, `std`, `tls12`, optional) | KEEP | The TLS implementation for `hosted`. Uses the `ring` provider, not the default `aws-lc-rs`, which needs cmake and nasm (ADR 0013's clean-machine rule). `tls12` has no stated reason in ADR 0024. Dropping it would refuse a user-chosen endpoint that only speaks TLS 1.2, which changes what a user can connect to. That makes it an owner decision, not a chore. Its byte cost was not measured. It is the next candidate if binary size becomes a gate |
| `trellis-model` | `tokio-rustls` (0.26.5, `ring`, `tls12`, optional) | KEEP | Runs rustls over the existing `tokio::net::TcpStream`, so the HTTPS path has the same timeout and cancellation as the plain one. The alternative is `reqwest`, which the predecessor kept out of the core |
| `trellis-model` | `webpki-roots` (1.0.9, optional) | KEEP | Mozilla's root set compiled in, so trust does not depend on the machine's certificate store. The alternative (`rustls-native-certs`) is a different trust policy, not a trim |
| `trellis-model` | dev-dep self-reference gains `"hosted"` | KEEP (dev) | Without it, `cargo test -p trellis-model` compiles out the TLS configuration test, which then runs only when another crate in the same invocation happens to turn the feature on |
| `trellis-model` | `[features] hosted = ["dep:rustls", "dep:tokio-rustls", "dep:webpki-roots"]` | KEEP | The gate that keeps the local-only build free of TLS (measured: 166 crates vs 176) |
| `trellis-queue` | `toml.workspace = true` | KEEP | Agent profiles and routines under `.trellis/` (`portable.rs`, ADR 0026). Already in the graph for `config.toml` |
| `trellis-vault` | `serde.workspace = true` | KEEP | `dates.json` is read with `deny_unknown_fields` at every level (`dates.rs`, ADR 0022 §5). Already in the graph |
| `trellis-vault` | `unicode-normalization = "0.1.25"` | KEEP | NFKD before stripping combining marks, matching Apex's `slugify`. Without it `Café` slugs to `caf` here and `cafe` in Apex, and an alias resolves in one tool and not the other (`refs.rs`). Correct NFKD needs the Unicode decomposition tables, and hand-writing them is not a trim. Adds `tinyvec` |

**Named items no manifest line in the diff adds:**

| Item | Verdict | Where it comes from, and why |
|---|---|---|
| `ring` | KEEP | Transitive. `rustls`'s `ring` feature and `rustls-webpki` pull it, and only under `hosted`. It is the crypto provider chosen over `aws-lc-rs` (see `rustls`). It builds with the C compiler already needed on MSVC, not with cmake or nasm. It brings `untrusted` and a second `getrandom` (0.2) |
| `tracing-serde` | KEEP | Not added by the inherit PR. The `json` feature on `tracing-subscriber` is a context line in the inherit diff. It came to `v0.1-resolver` in `1d3d06f` (2026-09-27), after the predecessor, for `--log-json` (ADR 0020 §7). One crate, and `serde` was already in the graph |
| `CDLA-Permissive-2.0` (licence) | KEEP | Only `webpki-roots@1.0.9` carries it (`cargo deny list -l crate`). The licence covers Mozilla's root certificates, which are data. `deny.toml` allows it with the comment that it "permits use and redistribution with no copyleft condition". It appears only in the `hosted` build |

## TRIM measurement (#154)

A detached worktree at `969c059` was used, with `jiff` changed to
`default-features = false, features = ["std", "tz-system",
"tzdb-bundle-platform", "tzdb-zoneinfo"]`. It was built with `cargo build
--release -p trellis-cli --target-dir <scratch>`. The manifest was then
restored and the unmodified tree built from the same path into the same target
directory. That second build is the control, because embedded source paths make
a build from a different directory a different size. Result: 10,840,064 B
trimmed vs 10,861,568 B control, **−21,504 B**. The control is 512 B larger
than the in-repo build (10,861,056 B), which is the path difference. The trial
compiled. Its tests were not run: #154 requires them.

## `cargo deny check` and `cargo audit`

- `cargo deny check` (cargo-deny 0.20.2): exit 0,
  `advisories ok, bans ok, licenses ok, sources ok`. It prints four `duplicate`
  warnings, listed above.
- `cargo audit` (cargo-audit 0.22.2): exit 0. It prints one allowed warning,
  RUSTSEC-2023-0089 (`atomic-polyfill` unmaintained, through `heapless` ←
  `postcard`). Unmaintained advisories are warnings, not failures, in
  cargo-audit. `deny.toml` ignores the same ID with a reason, and that entry
  dates from `bc705e1` (2026-09-21), before this ticket and the predecessor.

## Feature powerset

**Command:** `cargo hack check --feature-powerset --no-dev-deps --workspace`
(cargo-hack 0.6.45, rustc/cargo 1.93.0, Windows 11, target
`x86_64-pc-windows-msvc`). **Date:** 2026-09-28. **Commit:** `d79b55c`
(`drain`, the base of #36). **Result:** exit 0, every combination compiled.
`git status --porcelain` was empty afterwards, so the manifests
`--no-dev-deps` rewrites while it runs were restored. The run took 13.3 s wall
clock on an already-built target directory (`time`, one run).

Combinations checked, counted from cargo-hack's `info: running` lines (19 in
all):

| Crate | Features declared | Combinations | Which |
|---|---|---|---|
| `trellis-cli` | `default = ["hosted"]`, `hosted` | 3 | none; `hosted`; `default` |
| `trellis-model` | `grammar`, `hosted` (no `default`) | 4 | none; `grammar`; `hosted`; `grammar,hosted` |
| `tui-demo` | `default = ["demo"]`, `demo` | 3 | none; `demo`; `default` |
| `chat`, `trellis-checks`, `trellis-core`, `trellis-index`, `trellis-loop`, `trellis-queue`, `trellis-trail`, `trellis-vault`, `trellis-vcs` | none | 1 each, 9 in all | plain `cargo check` |

For `trellis-cli`, `default` and `hosted` enable the same set, so two of its
three runs build the same thing; cargo-hack runs both because it does not
compare what the features expand to. Optional dependencies are declared with
`dep:` everywhere, so none is an implicit feature and none adds a combination.

**Nothing was excluded.** No `--exclude-features`, `--depth`,
`--skip` or `--group-features` was passed; the full powerset is 11 builds for
the three crates that have features, small enough to run whole.

**What `--no-dev-deps` leaves out.** Dev-dependencies are removed for the run,
so tests, examples and benches are not built in any combination. This checks
that library and binary targets compile under each feature set, nothing more.
Whether the test suite passes without `hosted` is a separate question this run
does not answer.

**The predecessor's Deferred item.** `audit/DEPENDENCIES-2026-09-22.md`
(Deferred, first item) proposed this command in place of
`cargo-unused-features`, which cannot parse an edition-2024 workspace. It does
not answer the same question. `--feature-powerset` checks that every
combination of *this workspace's own* features compiles; it does not report a
feature of a dependency that is enabled but unused. The powerset over each
dependency's features that the predecessor sketched would need each
dependency's feature list edited in turn, which cargo-hack does not do. That
item is closed as follows: the compile-under-every-combination question is
answered (yes, at `d79b55c`); the unused-dependency-feature question is still not
answered by any tool, only by hand checks such as the predecessor's of
`smallvec`, `rustyline` and `tui-logger`.

## Threads versus tokio

The predecessor (`audit/DEPENDENCIES-2026-09-22.md`, "Is async needed at
all?") did not build the threads version and listed that as a gap. This
section builds both event loops as standalone prototypes and measures them.
It measures the cost of the TUI loop's shape only. It is not a proposal to
remove tokio (see the last paragraph).

**Measured on:** Windows 11 Home 10.0.26200, target `x86_64-pc-windows-msvc`,
rustc 1.93.0 / cargo 1.93.0 (picked up from the repository's
`rust-toolchain.toml`), 16 logical CPUs, default `-j`. Date 2026-09-28, tree at
`aeef233`. No `.cargo/config.toml` exists in the repository or the user
profile, and `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`,
`CARGO_ENCODED_RUSTFLAGS` and `CARGO_TARGET_DIR` are unset in the shell that
ran the builds, so no rustflags apply.

**The prototypes.** Two packages, `.bench/t38/threads/` and
`.bench/t38/tokio/`, under the git-ignored `.bench/`. They are not committed
and are not workspace members: each manifest has its own empty `[workspace]`
table. Both mirror the workspace's `[profile.release]` (`lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`), so sizes are in the same units as the
Numbers section. Each draws a `ratatui` frame and has one background worker
that sends a counter to the UI every 250 ms over a channel. That describes
the source. The binaries were compiled but not run, because a TUI needs a
controlling terminal and none was attached. `tui-demo`'s tests work around
the same limit with an event source that never yields. The ticket asks for
build figures only.

- **threads**: `crossterm`'s blocking `event::poll`/`event::read` (through
  `ratatui::crossterm`), a `std::thread` worker, `std::sync::mpsc`.
  Dependencies: `ratatui = "0.30"` only.
- **tokio**: `crossterm`'s `EventStream` on a current-thread runtime
  (`Builder::new_current_thread().enable_all()`), a `spawn_blocking` worker
  using `blocking_send` (the shape trellis's TUI uses for a run), `tokio::sync::mpsc`,
  `tokio::select!`. Dependencies: `ratatui = "0.30"`,
  `crossterm = { version = "0.29", features = ["event-stream"] }`,
  `futures-util = { version = "0.3", default-features = false }` (as
  `tui-demo` has it), and `tokio = "1.53.1"` with the workspace's feature list
  verbatim: `rt, macros, sync, time, process, net, io-util, signal`. The loop
  itself needs only `rt`, `macros`, `sync` and `time`. The ticket asks for the
  workspace's features, and those inflate the tokio side's figures. `net`
  accounts for `mio` and `socket2`.

**Versions pinned to the workspace.** The root `Cargo.lock` was copied into
each prototype and pruned by one `cargo tree --offline`. Every build then ran
with `--locked --offline`. The pruned locks resolve `ratatui` 0.30.2,
`crossterm` 0.29.0, `futures-util` 0.3.34 and, on the tokio side,
`tokio` 1.53.1, which are the root lock's versions. `cargo tree -e features -i crossterm`
confirms that `event-stream` is enabled only on the tokio side. The threads
side gets `crossterm`'s defaults through `ratatui-crossterm` and nothing more.

| File | SHA-256 |
|---|---|
| `.bench/t38/threads/src/main.rs` (44 lines) | `fcffa91cfc0cdbd79a33450405f7d7b2913b31dcf3c10e1d4e2025891685e484` |
| `.bench/t38/tokio/src/main.rs` (54 lines) | `7b42615756eb4afdf5cd324b81776b473efb8edf6672658c4a8077e2166841ef` |
| `.bench/t38/threads/Cargo.toml` | `c3c6d52531062b4a67975ed0f23a014230a2e183c5b5379604b8fd7328e8379c` |
| `.bench/t38/tokio/Cargo.toml` | `796fab5b3235d51f1caff18075c7fae0ce4ae25aee9f11e311bceb0ceb7f5613` |

Both sources are under 80 lines and are given inline so the figures can be
reproduced without the untracked directory:

```rust
// .bench/t38/threads/src/main.rs
// Threads: crossterm's blocking poll/read, one std::thread worker, std mpsc.
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::widgets::Paragraph;

fn main() -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel::<u64>();
    // The worker does blocking work and reports each result to the UI.
    thread::spawn(move || {
        for i in 0u64.. {
            thread::sleep(Duration::from_millis(250));
            if tx.send(i).is_err() {
                break;
            }
        }
    });
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &rx);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, rx: &mpsc::Receiver<u64>) -> std::io::Result<()> {
    let mut last = None;
    loop {
        while let Ok(v) = rx.try_recv() {
            last = Some(v);
        }
        terminal.draw(|f| {
            f.render_widget(Paragraph::new(format!("worker: {last:?}  (q quits)")), f.area())
        })?;
        // The poll timeout is what bounds how late a worker result is drawn.
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
            && k.code == KeyCode::Char('q')
        {
            return Ok(());
        }
    }
}
```

```rust
// .bench/t38/tokio/src/main.rs
// Tokio: crossterm's EventStream on a current-thread runtime, one
// spawn_blocking worker (the shape trellis uses for a run), tokio mpsc.
use std::time::Duration;

use futures_util::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc;

fn main() -> std::io::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let (tx, mut rx) = mpsc::channel::<u64>(16);
        // The worker does blocking work and reports each result to the UI.
        tokio::task::spawn_blocking(move || {
            for i in 0u64.. {
                std::thread::sleep(Duration::from_millis(250));
                if tx.blocking_send(i).is_err() {
                    break;
                }
            }
        });
        let mut terminal = ratatui::init();
        let result = run(&mut terminal, &mut rx).await;
        ratatui::restore();
        result
    })
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    rx: &mut mpsc::Receiver<u64>,
) -> std::io::Result<()> {
    let mut events = EventStream::new();
    let mut last = None;
    loop {
        terminal.draw(|f| {
            f.render_widget(Paragraph::new(format!("worker: {last:?}  (q quits)")), f.area())
        })?;
        tokio::select! {
            Some(v) = rx.recv() => last = Some(v),
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k)))
                    if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('q') =>
                {
                    return Ok(())
                }
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
                _ => {}
            },
        }
    }
}
```

**Method.** Each prototype had its own target directory
(`.bench/t38/target-threads`, `.bench/t38/target-tokio`). Builds ran one at a
time in the foreground with nothing else building. From the prototype's
directory, three times each:

```
cargo clean --target-dir ../target-<p>
time cargo build --release --locked --offline --target-dir ../target-<p>
stat -c %s ../target-<p>/release/t38-<p>.exe
cargo tree -p t38-<p> --edges normal --prefix none | sed 's/ (\*)//' | awk '{print $1}' | sort -u | wc -l
```

The last line is the predecessor's "release graph" command with the package
name substituted. As for `trellis-cli`, it counts the root package itself.
Build time is Git Bash `time`'s wall clock for the build alone. The clean is
not timed. `--offline` means no network. Every source was already in the local
registry cache, so the figures exclude download and extraction.

| | threads | tokio | tokio − threads |
|---|---|---|---|
| Clean release build, wall clock (3 runs) | 9.890, 9.701, 9.619 s | 11.584, 11.729, 11.638 s | — |
| Clean release build, median | **9.70 s** | **11.64 s** | +1.94 s |
| Release binary (`t38-<p>.exe`) | **400,896 B** | **593,408 B** | +192,512 B |
| Unique crates, release graph (names) | **62** | **72** | +10 |
| Duplicate versions, release graph | 2 pairs (`hashbrown` 0.16/0.17, `syn` 2/3) | 2 pairs, the same | 0 |

- Binary sizes were identical across the three builds of each.
- An earlier set of three builds per prototype, which cleaned with `rm -rf` of
  the target directory rather than `cargo clean`, gave 9.65/9.57/9.61 s and
  13.19/11.69/11.66 s. The table uses the `cargo clean` set because the
  ticket names that step. The first tokio build of the earlier set was the one
  outlier.
- **The 11 crates only the tokio side has** (`comm -13` of the sorted name
  lists, so 10 net once each side's own root package is excluded): `tokio`,
  `tokio-macros`, `mio`, `socket2`, `bytes`, `pin-project-lite`,
  `futures-core`, `futures-task`, `futures-util`, `windows-sys` and the root
  `t38-tokio`. The threads side has no crate the tokio side lacks, apart from its
  root.

**What these figures do and do not say about the workspace.** They measure
one event-loop shape in isolation, with the workspace's tokio feature set. On
this machine the async shape costs about 2 s of clean release build, 188 KiB of
binary and 10 crates over the threads shape. They do not say that trellis
would save that by switching. Tokio is in the workspace for more than the TUI
loop. `trellis-checks` runs its processes on it, `trellis-model`'s HTTP
client is built on `tokio::net`, and the `hosted` feature's `tokio-rustls` runs
TLS over that same `TcpStream`, so tokio stays regardless of how the TUI reads
its terminal. Tokio's features are unified across the graph, so dropping one
consumer does not change what the binary gets while another consumer enables
the same features. What the TUI loop alone brings in is the part no other
crate uses. `cargo tree -p trellis-cli -e normal -i` at `aeef233` shows
`futures-util` reached only through `tui-demo`, `futures-core` only through
`crossterm` (its `event-stream` feature) and `futures-util`, and
`futures-task` only through `futures-util`. Three of the tokio side's ten
crates would therefore leave the release graph with a threads loop. The other
seven stay, because the rest of the workspace pulls them in through tokio.
The prototypes do not measure the byte or build-time share of those three
crates, and they say nothing about latency, CPU use while idle, or the size of the
rewrite. The predecessor put the rewrite at both interfaces' event loops. So
this closes the predecessor's "threads version not built" gap with figures,
and the reason not to switch is still the same one: nearly all of the tokio
side's cost is already paid by other crates.

## Unverified

- **Per-crate size attribution was not produced.** `CARGO_PROFILE_RELEASE_STRIP=none
  cargo bloat --release -p trellis-cli --crates` (into a scratch target
  directory) was stopped by the host for low memory before it finished. The
  claim that the non-TLS growth is mostly first-party code rests on the
  `--no-default-features` figure and ADR 0024's `3234bee` figure, not on a
  per-crate table. What I would do next is rerun that command on an idle
  machine.
- **`tls12` size was not measured.** See the `rustls` row.
- **Only one OS and target.** Sizes are for Windows/MSVC. `jiff`'s bundled
  tzdb in particular is Windows-specific (`tzdb-bundle-platform` bundles
  nothing where a system zoneinfo exists), so its cost elsewhere differs.
- **Build times were not measured.** The ticket does not ask for them, and the
  predecessor's timing method (medians of repeated clean builds) was not
  repeated.
