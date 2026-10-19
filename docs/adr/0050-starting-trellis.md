# ADR 0050 — Starting trellis: plain `trellis`, or `--tui`, `--dsk`, `--web`

**Status:** Accepted (owner-directed). Built (story S19, #377): sections 1–4 by #370–#374, #553, #555, #556, #558 and #561; section 5 by #375. The handoff to `trellis-tui` (section 1, amended) is #514's and is not built.
**Date:** 2026-10-04
**Decided by:** the owner's request of 2026-10-04 ("make the startup more
intuitive, limited flags; start with just `trellis`, with `--tui`, `--dsk`,
`--web`, or plain `trellis`"), carried out by the session under the owner's
2026-10-04 delegation (#369).
**Holds:** CLAUDE.md's ten rules unchanged; ADR 0009's clean-tree rule and
the reason `root_from` refuses a non-repository tree unchanged; ADR 0024 §3
unchanged (a model the user picks is written to the user's own config). It
grows the CLI surface by three flags, recorded here as brief 4.12 requires,
and amends ADR 0021's paragraph on desktop and web interfaces (#70) for the
web view.

## Context

A first run on Windows, 2026-10-04: bare `trellis` in the home directory and
in a directory of repositories refused ("cannot determine the repository
root"); inside a repository it refused again ("no model configured"),
although `trellis doctor --model vllm:http://127.0.0.1:11434#qwen3:8b`
showed a working local runtime; and `TRELLIS RUN` was rejected for its case.
Each refusal was correct on its own terms, and together they made the first
minute a series of dead ends.

pi starts with plain `pi` and connects a provider from inside its interface
(`/login`). colibri starts with `coli chat`, `coli serve` or `coli web`, the
last serving its dashboard on 127.0.0.1 behind a DNS-rebinding Host guard and
opening the browser. Both put the first working session one command away.

## Decision

### 1. Plain `trellis` and three interface flags

Bare `trellis` opens the TUI. `--tui` says so explicitly; `--dsk` starts the
desktop (ADR 0048); `--web` serves the desktop's page on 127.0.0.1 (section
5). They are the only flags this adds. At most one of the three is accepted,
and one given with a subcommand is refused naming both. Without a terminal
(a pipe, a redirect, CI) bare `trellis` keeps today's behaviour.
*Amended 2026-10-06 (#500), by ADR 0052, accepted 2026-10-07:* once
the Go TUI reaches parity (ADR 0052 section 7), bare `trellis` and `--tui`
keep first-run (section 2) and the start screen (section 3), then hand off
to `trellis-tui`, the sibling of `current_exe()`, falling back to the
ratatui TUI when it is missing, for one release. The switch itself is its
own ticket and its own dated line here.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07):* bare `trellis` at a terminal starts `trellis-desktop`
detached, with `--root` only when it found a root, prints the overview
(section 3, amended) and exits. Outside a repository, with a desktop found
and a display, it skips the start screen; inside one with no model
configured, first-run (section 2) runs in the terminal first. `--tui` and
`trellis tui` keep the TUI; `--dsk` opens the desktop only. With no desktop
found, a spawn that fails, or no display, it says so on one stderr line,
prints the overview and falls back to the start screen, then the TUI. With
no terminal, behaviour is unchanged. The 2026-10-06 handoff line above is
restated for `--tui` and `trellis tui` only: they, not bare `trellis`, hand
off to `trellis-tui` at parity. Accepted; built 2026-10-07 (#374 and #561,
below), except the handoff to `trellis-tui`, which is #514's and not built.
*As built 2026-10-07 (#374):* `trellis --dsk` starts `trellis-desktop`
from beside `current_exe()` only, never from PATH, detached (no standard
streams; on Windows its own process group with no console, on Unix
`process_group(0)`; no job object) and returns once it has started. It
passes `--trellis <current_exe>`, and `--root` only for `--root` given or a
root found; outside a repository it starts the desktop with no root rather
than refusing. The environment passes unchanged (ADR 0048 section 3). On
Linux with neither `DISPLAY` nor `WAYLAND_DISPLAY` set it starts nothing
and says so. Not found, it names the path it looked at and both
`cargo install --locked --path crates/trellis-cli` and `cargo install
--locked --path desktop`.
*As built 2026-10-07 (#561):* bare `trellis` at a terminal (the routes
`bare_route` sends to the interface or the start screen) starts the desktop
through #374's launcher, with `--root` only for the root it found, prints
the overview's text to stdout and exits 0. Outside a repository, with a
desktop found and a display, the start screen is skipped; inside one whose
configuration names no model, first-run's picker runs in the terminal
first, and quitting it starts nothing, as in the TUI. With no desktop
beside `current_exe()`, no display on Linux, or a spawn that fails, one
stderr line says which (for a missing desktop, the path it looked at and
both `cargo install` commands) and ends "falling back to the terminal"; the
overview follows, then the start screen and the TUI as before, and the
desktop is not tried again in that process. `--tui` is checked before the
decision and never reaches it; `trellis tui`, a pipe, a redirect and `--dsk`
are unchanged. The decision is `launch_route` in `main.rs`, beside
`bare_route`, whose tests are unchanged. `trellis --help` lists `trellis` as
the desktop and the overview, `--tui` as the terminal interface and `--dsk`
as the desktop only.
*As built 2026-10-12 (#514):* every path that opened the ratatui TUI now
goes through `terminal_ui` in `main.rs`: `--tui`, `trellis tui [task]
[--model <spec>]`, and bare `trellis`'s fall back to the terminal after a
desktop it could not start. It looks for `trellis-tui` (`trellis-tui.exe`
on Windows) beside `current_exe()` only, never on PATH (`launch::find_tui`).
Found, first-run's picker still runs in Rust when no model is configured or
given (it needs a terminal on both ends; without one the ratatui path's
refusal is unchanged), and the terminal is handed to `trellis-tui --root
<root> --trellis <current_exe> [--model <spec>] [-- <task>]`: on Unix by
`exec`, replacing the process; on Windows by waiting for it and exiting
with its status. `--model` is passed only when the user gave one, never a
spec from the repository's configuration, so rpc does not take a
repository's choice for the user's (ADR 0024 section 3); `trellis-tui`
confirms the configured model itself. The environment passes unchanged,
the `trellis-tui` entry of `INTERFACE_LAUNCHES` (ADR 0052, amendment of
2026-10-07). Not found, one stderr line names the path looked at and says
the ratatui TUI follows, which it then does, for one release (ADR 0052
decision 2.2); #516 removes the fallback. Held by the `launch_` tests in
`tests/cli.rs` and `launch.rs`.

### 2. A first run with no model

With no model configured, the interface does not refuse. It probes, each
with a short bound, the local runtimes it knows: Ollama on 127.0.0.1:11434,
llama.cpp on 127.0.0.1:8080 and vLLM on 127.0.0.1:8000. It lists the models
they report as the specs that reach them, and writes the chosen spec to the
user's own config file, never to `.trellis/config.toml`: the user decides
where a model is, and a local spec carries no key (ADR 0024 §3). Nothing is
written without a choice. With nothing found, it names the addresses it
tried and the exact `--model` form.

*Amended 2026-10-12 (#625):* a `/v1/models` list on 127.0.0.1:8000 owned by
colibri (`owned_by: "colibri"`, ADR 0047 decision 3; `coli setup` serves
there with no key) is offered as `provider:colibri#<id>`, never as the
`vllm:` spec, which would run colibri at tier 4 with no chat template while
tier 1 is there, and never as a bare `openai-chat:` spec, which would send
the user's `OPENAI_API_KEY` to it (ADR 0024 §3). Choosing it writes a
`[[providers]]` entry named `colibri` (`protocol = "openai"`, the base the
probe read with `/v1`, `key_env = "COLI_API_KEY"`, ADR 0047 decision 2)
and then the spec, both to the user's own config file and never to
`.trellis/`: a `key_env` is the user's to name, and this is the one writer
of one. An entry named `colibri` already there at the same base is left as
it is; at another base it is refused with the file untouched and nothing
written, since replacing it would move where the user's key is sent. A
list owned by anyone else is offered as before. The same list feeds rpc's
`list_models` (ADR 0025 §5), and the desktop's model rule forwards a
`provider:` spec (desktop/src/commands.rs, `check_model`), read and not
changed. Evidence: the `first_run_colibri_` tests in
crates/trellis-cli/src/first_run.rs and crates/trellis-cli/src/rpc.rs.

### 3. Outside a repository

Bare `trellis` outside a git repository opens a start screen instead of
refusing: the repositories trellis last resolved a root in, newest first, at
most ten, and an offer to run `git init` in the current directory after an
explicit yes, through `trellis_vcs`. Every other command keeps today's
refusal. No command walks a tree that is not a repository: the reason
`root_from` refuses (a home directory indexed by accident) stands.
*Amended 2026-10-07 (#557), owner-directed (the owner's decision of
2026-10-07):* `trellis overview` (and `trellis [--root <repo>] --json
overview`) runs with or without a repository and prints once: the start
screen's recent repositories, each with its last run's outcome; and, for
the current root, its last run, its codebase and its board. The overview
never indexes: the codebase comes from `.trellis/index.bin` only when that
cache is current, and is otherwise reported as not indexed or stale, never
built or saved. A root with no `.git` entry (the `current_dir()` fallback
from a home directory) is treated as no root, and nothing under it is read,
so the reason `root_from` refuses stands. Accepted; built 2026-10-07 (#558).
*As built 2026-10-07 (#558):* `trellis overview` (`overview.rs`) needs no
repository. Its JSON object, with C1 controls escaped as `log --json`
escapes them, carries `recent`: the start screen's list, each entry with its
newest trail's run id, start, outcome, integrity and task, null with no
trail, or an error for a corrupt newest trail, which is not skipped for an
older one. For a root with a `.git` entry it adds `last_run` (the same, plus
turns, edits, each check's result, verified and reviewed as two counts, and
the tier), `codebase` (`codebase_json`'s shape from `.trellis/index.bin` only
when `cache::load` accepts the current sources' fingerprint, else null with
"not indexed" or "index is stale") and `board` (the queue's tasks and
`pr_ci`). With no such root, only `recent` is shown and neither the
directory's trails nor its config are read. The text form prints the same
sections. `pr_ci` comes from the first `gh` on PATH (absolute entries only),
started in the root through `secrets::scrub` with no stdin and no token, as
`gh pr list --state open` and `gh run list -L 10`, each bounded to 10
seconds, after which it and what it started are killed. Missing, not logged
in, timed out, failing or not JSON, it is `{"available": false, "reason"}`
and the command still exits 0. Nothing is written.
*Amended 2026-10-12 (#654), owner-directed (the owner's decision of
2026-10-12, on #654):* the overview still never indexes and never writes:
its codebase is still `.trellis/index.bin` only when that cache is
current. What changes is who else saves that cache. rpc's `get_codebase`,
which the desktop's Codebase and Atlas sections send, now saves the
worktree's index there through the writer `trellis index` uses (ADR 0048
section 10, amended 2026-10-12), so after the desktop has drawn a
repository's graph the overview shows its counts for the same root rather
than "not indexed" or "index is stale". The desktop's pane says the counts
come from `.trellis/index.bin`. The reasons the overview gives when no
current cache exists are unchanged.
*As built 2026-10-07 (#555):* a root is recorded only from an interface
start (bare `trellis`, `trellis tui`, `--tui`, `--dsk`), never from another
command, and never when it is under the temporary directory. In the home
directory the screen does not offer `git init`, and says why. With nothing
listed, it says whether trellis never opened a repository or every one it
opened is gone.

### 4. Case in subcommand names

Subcommand names match without regard to case: `trellis RUN` is `trellis
run`. Only the first argument that is not a flag is compared, and only
lowered when its lowercase form is a subcommand, so a task text or a path is
never changed.

### 5. The web view

`--web` binds 127.0.0.1 only, on a free port, and opens a URL carrying a
per-launch random token. A request without the token, or whose Host header is
not `127.0.0.1:<port>` or `localhost:<port>`, is refused before anything else
is read. It serves the same static page and the same rpc-backed view-models
as the desktop, so it adds no path around the orchestrator (ADR 0048
sections 2, 5 and 10), and every trellis- or model-supplied string reaches
the DOM through `textContent`.
*As built 2026-10-11 (#375):* `trellis --web` starts `trellis-desktop
--web` from beside `current_exe()` only, as `--dsk` does (#374), with
`--root` only for a root found, in its own process group, its stdin and
stdout piped and its stderr the terminal's. The desktop binds 127.0.0.1:0
(`desktop/src/web.rs`) and prints `{"url": "http://127.0.0.1:<port>/?t=<token>"}`
once on stdout; trellis prints the URL, opens it through the platform's
opener (`xdg-open`, `open`, or `rundll32 url.dll,FileProtocolHandler`,
each scrubbed of the key variables) and says so when the opener is
missing. The token is 32 bytes from the OS's random source, through std's
`RandomState` on a fresh thread so no crate is added, as 64 hex
characters. `GET /?t=<token>` redirects to `/<token>/`, the page's root, so
the page's relative imports and fetches carry the token on their own and
the page is served byte for byte: `ui/web.js`, loaded first and the one
module that fetches, defines `window.__TAURI__` over `fetch` and
`EventSource` when Tauri has not, so every other module is unchanged. The
four commands are `POST /<token>/invoke/<name>` and the core's events are
one server-sent event stream at `/<token>/events`, whose first connection
runs startup as the window's page load does. A request whose path lacks
the token or whose Host is not `127.0.0.1:<port>` or `localhost:<port>` is
refused with 403 before its body is read, with one answer for both. Every
answer, a refusal too, then shuts the write side and reads and throws away
what the client sent that was not read, until the client closes, 200 ms
have passed or 64 KiB have come, before the socket is dropped: a socket
closed with input unread sends a reset, and on Windows the reset discards
the 403 the browser had already received (#666's Windows CI, 2026-10-12).
The discarded bytes are not parsed, so a refusal is still decided on the
request line and headers alone. Every response carries a
Content-Security-Policy as strict as the window's (`connect-src 'self'`,
nothing inline, `frame-ancestors 'none'`). The server stops when its stdin
ends: trellis closes it on the first Ctrl-C, says so, and waits for the
desktop, which sends its rpc child `quit` and waits (ADR 0048 section 9);
a second Ctrl-C stops waiting. Tests whose names start with `web_` in
`desktop/src/web.rs` hold the bind address, the token, the Host guard, the
close, the policy, the file set, the commands and the stream; `launch_web_`
tests in `crates/trellis-cli/src/launch.rs` hold the launch.

## Alternatives rejected

**More flags** (`--model` on bare `trellis`, `--root` as the default verb).
The owner asked for limited flags; the first-run picker removes the reason
for a model flag at startup, and `--root` stays where it is for the commands
that take it. **Indexing the current directory when it is not a
repository.** That is the accident `root_from`'s refusal exists to prevent.

## Consequences

- The first minute is one command: `trellis`.
- The web view is a second surface that must keep ADR 0048's rules; its
  ticket's tests hold the bind address, the token and the Host guard.
