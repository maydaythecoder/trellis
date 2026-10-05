# ADR 0050 — Starting trellis: plain `trellis`, or `--tui`, `--dsk`, `--web`

**Status:** Accepted (owner-directed). Not built (story S19, #377).
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

### 2. A first run with no model

With no model configured, the interface does not refuse. It probes, each
with a short bound, the local runtimes it knows: Ollama on 127.0.0.1:11434,
llama.cpp on 127.0.0.1:8080 and vLLM on 127.0.0.1:8000. It lists the models
they report as the specs that reach them, and writes the chosen spec to the
user's own config file, never to `.trellis/config.toml`: the user decides
where a model is, and a local spec carries no key (ADR 0024 §3). Nothing is
written without a choice. With nothing found, it names the addresses it
tried and the exact `--model` form.

### 3. Outside a repository

Bare `trellis` outside a git repository opens a start screen instead of
refusing: the repositories trellis last resolved a root in, newest first, at
most ten, and an offer to run `git init` in the current directory after an
explicit yes, through `trellis_vcs`. Every other command keeps today's
refusal. No command walks a tree that is not a repository: the reason
`root_from` refuses (a home directory indexed by accident) stands.

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
