# trellis-desktop

The TRELLIS desktop: one Tauri 2 window over one `trellis rpc` child per
repository (ADR 0048). It is a client of trellis, not a second way to run
it: every write goes through `prompt`, and so through trellis's snapshot,
trust gate, checks, rollback and trail, and the page shows what trellis
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

After launch the window asks which model to use, with the configured spec
preselected, and keeps prompt input disabled until one is chosen (ADR 0048
§5, amended). A model it saves goes to your own config file, never the
repository's.

## What it does not do

- **No installer and no updater.** `bundle.active` is false in
  `tauri.conf.json`; it is built and installed with cargo, and updated the
  same way.
- **No key field.** No key reaches the page. A hosted model gets its key
  from trellis's environment, by the variable your own config names, and the
  desktop sends a hosted spec only when it is the one your own config wrote
  (ADR 0024 §3, ADR 0048 §4). The page has no endpoint field either.
- **No runtime sidecar.** It starts no model runtime and the page talks to
  none; it lists the runtimes `trellis` finds on their default ports, and you
  start them yourself.
- **No Tauri plugin, no network origin.** The page's one channel is the core,
  over Tauri's IPC; the CSP names no network origin and the window navigates
  nowhere but the app's own origin.
- **No kill.** Closing the window sends `quit` to trellis and waits, so a run
  in flight rolls back and writes its trail.
