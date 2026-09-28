# ADR 0020 — A local work queue and a heartbeat

**Status:** Accepted.
**Date:** 2026-09-27
**Reference design:** Paperclip (`paperclipai/paperclip`, MIT). Five of its
ideas are borrowed as designs, and no Paperclip code or server is used:
- tasks as the unit of work
- atomic checkout
- a heartbeat that wakes an agent to take the next task
- budgets that hard-stop
- every task tracing back to a goal

## Context

The hands-off end of the control dial (ADR 0018) needs something that picks up
work without a human typing `trellis run`. Paperclip does this with a server, a
Postgres database and an org chart, but brief Part 1 makes "any networked
component" and "multi-user anything" v1 non-goals. So the ideas are taken and
the infrastructure is left behind.

## Decision

### 1. The queue lives in the git directory, not the worktree

The queue lives at `<git-dir>/trellis/queue/`, with `pending/`, `claimed/`,
`done/` and `failed/` under it. `<git-dir>` comes from `git2::Repository::path()`.
In a linked worktree `.git` is a file, not a directory, so the path is never
hard-coded.

The location decides whether the queue survives. Anything inside the worktree
makes the worktree dirty, which blocks every run (ADR 0009), and rollback's
`git clean -fd` would delete it. The git directory is untracked by
construction and survives rollback.

### 2. One task, one file

Each task is one JSON file, `<ulid-or-timestamp>-<slug>.json`:

```jsonc
{ "task": "...", "focus": "<vault stem>|null", "prompt": "<name>|null",
  "added_at": "...", "attempts": 0 }
```

### 3. Atomic checkout is a rename

Moving `pending/x.json` to `claimed/x.json` is one `rename` within one
filesystem. It either succeeds or fails, and whoever loses the race gets an
error and moves on. This is Paperclip's atomic checkout without a database.

### 4. Execution is serial

A heartbeat runs one task at a time. ADR 0009 allows one run per worktree, and
the snapshot and rollback guarantees are per run. Running in parallel would
need separate worktrees and the partition rules (ADR 0005, 0016), so it is
outside this ADR.

### 5. The subcommands

- `trellis queue add "<task>" [--focus <stem>] [--prompt <name>]`
- `trellis queue list`, which shows each task's state
- `trellis work [--once | --every <seconds>] [--max-tasks N]`

`work` is the heartbeat. On each tick it claims the oldest pending task, runs
it through the same code path as `trellis run` in `auto` mode, and moves the
task file to `done/` or `failed/` with the run id and exit reason appended.
- A converged run's change is left uncommitted, exactly as `trellis run`
  leaves it, and `work` then stops. The next task needs a clean tree, and
  accepting the change is the human's decision, so `work` never commits the
  code on the human's behalf.
- A rolled-back run leaves the tree clean, so `work` continues to the next
  task.
- `--max-tasks` bounds how many tasks one invocation runs. The per-run budget
  (ADR 0008) bounds each run. Nothing multiplies.

### 6. Goal ancestry is the vault lineage

Paperclip traces every task to the company mission. Here a task's `focus`
stem compiles to that note's `parent` chain up to the root (ADR 0017), so the
goal ancestry comes from the same vault the human edits. There is no second
hierarchy to keep in sync.

### 7. A JSONL event log for unattended runs

`trellis run --log-json <path>` and `trellis work --log-json <path>` add a
JSON formatter to the existing `tracing` subscriber. It is the same event
path, sent to a second destination, so ADR 0008's rule against a second
telemetry path holds. This covers what pi's `--mode json` offers.

## Alternatives rejected

- **Running Paperclip and driving trellis from it.** Rejected by the owner:
  build the design natively.
- **A queue directory inside the worktree.** Rejected for the reasons in §1.
- **A daemon.** Rejected: `work --every` runs in the foreground and dies with
  its terminal, like everything else in trellis, which does not run while the
  machine is asleep (a v1 non-goal).
- **Committing converged changes automatically so the queue can keep going.**
  Rejected: the brief makes approval a human decision, and a queue does not
  change whose decision it is.

## Consequences

- Three new subcommands grow the CLI beyond brief §4.12. That was owner-directed
  and is recorded here, following the precedent of `session`, `log` and `why`.
- A long unattended session stops at its first converged change. That
  behaviour is intended.
