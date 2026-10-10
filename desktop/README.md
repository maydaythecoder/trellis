# trellis-desktop

The TRELLIS desktop: one Tauri 2 window over one `trellis rpc` child per
repository (ADR 0048). It is a client of trellis, not a second way to run
it: every write to the repository goes through `prompt`, and so through
trellis's snapshot, trust gate, checks, rollback and trail; `set_model`'s
save writes only your own user config file. The page shows what trellis
reports and computes no verdict. The rules for working on it are in
[`CLAUDE.md`](CLAUDE.md) beside this file.

## Building it

It is its own Cargo workspace (ADR 0048 §1), so every command names its
manifest and takes `--locked`:

```
cargo build -p trellis-cli                              # the trellis it runs, and its tests spawn
cargo build --locked --manifest-path desktop/Cargo.toml
cargo test  --locked --manifest-path desktop/Cargo.toml
```

Prerequisites, as the `desktop` job in `.github/workflows/ci.yml` builds it:

- **Windows:** the MSVC toolchain (the Rust `x86_64-pc-windows-msvc` target
  with the Visual Studio C++ build tools) and WebView2, which current
  Windows ships; the window renders in it.
- **Linux:** the WebKitGTK 4.1 development packages and `pkg-config`. On
  Debian or Ubuntu: `libwebkit2gtk-4.1-dev`, `libxdo-dev` and `pkg-config`.
  The CI job checks for `webkit2gtk-4.1` with `pkg-config` and never
  installs anything itself; a runner's packages are its owner's to install.

To install both binaries side by side, as `trellis --dsk` looks for the
desktop beside `trellis` and nowhere else:

```
cargo install --locked --path crates/trellis-cli
cargo install --locked --path desktop
```

## Opening a repository

```
trellis-desktop [--root <repo>] [--trellis <path>]
```

- `--root <repo>` opens that repository. Without it, the window opens the
  repository holding the current directory (the nearest parent with a `.git`
  entry); outside any repository it starts no `trellis rpc` and shows the
  overview instead.
- `--trellis <path>` is the `trellis` binary it runs; `trellis` on PATH
  without it. `trellis --dsk` and bare `trellis` start the desktop with
  `--trellis` set to themselves.
- Another repository is opened from the window's "Open another repository"
  control, which refuses a path with no `.git` entry and refuses while a
  run is in flight.

After launch the window asks which model to use, with your own configured
spec preselected (or, with none, the repository's), and keeps prompt input
disabled until one is chosen (ADR 0048 §5, amended). A model it saves goes
to your own config file, never the repository's.

## What it does not do

- **No installer and no updater.** `bundle.active` is false in
  `tauri.conf.json`; it is built and installed with cargo, and updated the
  same way.
- **No key field.** No key reaches the page. A hosted model gets its key
  from trellis's environment: an `openai-chat:` or `anthropic:` spec from
  its protocol's own variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`), and a
  `provider:` spec from the `key_env` its `[[providers]]` entry names, or its
  protocol's own variable when the entry names none. The desktop sends a
  hosted spec only when it is the one your own config wrote (ADR 0024 §3,
  ADR 0048 §4). The page has no endpoint field either.
- **No runtime sidecar.** It starts no model runtime and the page talks to
  none; it lists the runtimes `trellis` finds on their default ports, and you
  start them yourself.
- **No Tauri plugin, no network origin.** The page's one channel is the core,
  over Tauri's IPC; the CSP names no network origin and the window navigates
  nowhere but the app's own origin.
- **No kill.** Closing the window sends `quit` to trellis and waits, so a run
  in flight rolls back and writes its trail.
- **No live token count while a reply arrives** (colibri's COL-UI-41, its
  pulsing `N tokens` badge). `trellis rpc` writes events to stdout through a
  layer fixed at `trellis=info` (`crates/trellis-cli/src/main.rs`), so that a
  `RUST_LOG` meant for debugging cannot change what a machine reader
  receives, and the loop emits one INFO `turn` event per completed turn
  (`crates/trellis-loop/src/agent.rs`). That event carries the step's output
  tokens and the model call's wall time, never a count while the reply
  arrives. Nothing streams replies today. COL-13 (#280), which would stream
  loopback colibri replies, is not built; if it is, it would emit each delta
  at DEBUG for this reason, since at INFO one reply would put one line per
  token on every `--json` and rpc reader.
- **No time to first token** (colibri's COL-UI-43, its `TTFT` badge), for
  the same reason: the one `turn` event per completed turn carries the
  step's output tokens and the model call's wall time, never when a first
  token arrived, and the `trellis=info` stdout layer passes no per-token
  event that could time one.

## Where the token figures it does show come from

- **Each turn's output tokens, tokens per second and queue wait**, from the
  `turn` event when the turn ends: the latest turn's three in the run
  section's Budget pane, and each turn's output tokens in Turns (#312). The
  rate is output tokens over the model call's wall time less the queue wait,
  so it includes prompt processing and reads below colibri's decode rate; the
  queue wait is colibri's `x-colibri-queue-wait-ms` header, shown only when
  every reply carried it. A figure the turn did not report is left out of the
  line, never drawn as 0.
- **The session's prompt and completion tokens per model**, from
  `get_session_stats`, drawn in the run section's Session tokens pane
  (#313). trellis sums them per model from the trails of the runs that
  `trellis rpc` process started, two figures never added together, and
  `unknown` where a run of that model reported none. The desktop adds up no
  turn itself. One `trellis rpc` runs per repository, so the figures cover
  the runs since the repository was opened, and start empty for each
  repository opened.
