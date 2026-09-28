# ADR 0020 — A local work queue and a heartbeat

**Status:** Accepted. Amended in implementation: §1, §2, §3 (the checkout is a
hard link, not a rename), §5 and §7. Each amendment is marked in place.
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

*Amended in implementation: the git directory comes from
`git rev-parse --absolute-git-dir`, not `git2`.* The call is
`trellis_vcs::git_dir`, run with the repository root as the working
directory. `git2` was removed from the build on purpose (6b5db2a) to shrink
the binary and the build, and `trellis-vcs` already shells out to `git` for
everything else. Re-adding `git2` and libgit2 only to find one directory would
have reversed that decision.

In a linked worktree, `--absolute-git-dir` returns
`<main>/.git/worktrees/<name>/`, not the common directory. So **each worktree
has its own queue**, and `git worktree remove` deletes that worktree's queue
along with it. Execution is serial and one run per worktree (§4), so this is
the intended behaviour.

### 2. One task, one file

Each task is one JSON file, `<ulid-or-timestamp>-<slug>.json`:

```jsonc
{ "task": "...", "focus": "<vault stem>|null", "prompt": "<name>|null",
  "added_at": "...", "attempts": 0 }
```

*As implemented:*
- The name is `YYYYMMDD-HHMMSS-<9-digit nanoseconds>-<slug>.json`, in UTC.
  The width is fixed, so sorting by name sorts by age, and oldest-first
  ordering is deterministic. The id is the file stem.
- The file is written under a dot-name and then hard-linked into `pending/`
  (`publish`, §3). A claimer therefore never reads half a file, and a name
  collision fails instead of overwriting.
- As the task moves, the queue appends three things:
  - `claimed_at` and `claimed_by_pid` while the task is in `claimed/`.
  - `outcome: { run_id, exit, finished_at }` when it leaves.
- `attempts` counts runs started. A task put back before its run started is
  not counted.

### 3. Atomic checkout is a rename

Moving `pending/x.json` to `claimed/x.json` is one `rename` within one
filesystem. It either succeeds or fails, and whoever loses the race gets an
error and moves on. This is Paperclip's atomic checkout without a database.

*Amended in implementation: the checkout is a hard link, and the queue
contains no `rename` at all.* Two Windows findings forced this. Both came
from this crate's tests.

1. **A rename-based claim let both claimers win.** On Windows,
   `std::fs::rename` opens the source and renames it through that handle. If
   two claimers both open the file before either moves it, both renames
   succeed, because the second one moves the file to where it already is.
   `two_claimers_one_file_exactly_one_wins` produced `[true, true]`.
2. **Renaming over an existing file was refused.** Renaming a freshly
   written temp file over an existing task file failed with "Access is
   denied" in about one run in five of the crate's unit tests. Some of those
   tests had no concurrency at all, and a retry that waited five seconds did
   not clear the error.

So the queue writes a file in only one way, `publish`:
- write the whole file under a unique dot-name
- `hard_link` it to the final name, which fails when that name exists
- remove the temp name

A reader never sees half a file, and nothing is ever overwritten.

A **claim** reads `pending/x` and then publishes `claimed/x`. Exactly one
claimer creates that name, and every other claimer gets `AlreadyExists`.
Losing is `Ok(None)` to the caller, not an error. There is one more check. A
claimer whose read of `pending/` is stale could create `claimed/x` after the
real winner has finished and moved on. The winner removes `pending/x`
immediately after claiming, so a claimer that finds `pending/x` gone
withdraws its claim. Nobody else can remove `pending/x` while this claimer
holds `claimed/x`.

A **move** (finish, release, requeue) publishes the task at its new name and
then removes the old one. Every file operation also retries with backoff for
about five seconds on `PermissionDenied`, in case a virus scanner is holding a
new file. After the change: 30 of 30 runs of the crate's tests passed, and 12
of 12 runs of the CLI queue suite.

One crash window remains. If a process dies between publishing a new name and
removing the old one, the task exists complete in both places, and
`queue list` shows both. A claim cannot happen in that state:
- if both are in `pending/` and `claimed/`, a new claimer gets `AlreadyExists`
- if both are in `claimed/` and `done/`, `pending/x` is already gone

So the task is never run twice, but it stays stuck until someone removes one
copy.

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

*Settled in implementation. This ADR left these open:*

**Flags**
- **With neither `--once` nor `--every`**, `work` runs tasks back to back
  until the queue is empty or a stop condition below fires.
- **`--once`** is `--max-tasks 1` and conflicts with both other flags.
- **`--every S`** sleeps only while the queue is empty. While tasks remain,
  they run back to back.
- **`--model`** falls back to `[model] spec`, as `run` does. It is parsed
  before anything is claimed, so a typo cannot fail the task at the head of
  the queue.
- **"auto mode"** is `cmd_run` exactly as `trellis run` calls it, with
  `--resume` off and `[model] unconstrained` taken from config, as the
  session does. At the time of writing, `run` has no approval step to switch
  off.

**Refusing up front**
- **Nothing is claimed while the worktree is dirty.** Before each claim,
  `work` runs the same `git status --porcelain` check as the snapshot
  (`trellis_vcs::pending`). If it fails, `work` prints the paths and exits 1.
  A task that is claimed and then cannot start would be stranded.
- **`queue add` checks `--prompt` and `--focus` when the task is added**, the
  same way the run will: the template must exist and the note must compile. A
  bad name is refused on the spot instead of hours later.

**What each run outcome does**

| Run outcome | Task moves to | `work` then |
| --- | --- | --- |
| Converged | `done/` | stops, exit 0 |
| Rolled back, tree restored | `failed/` | continues |
| Not restored | `failed/` | stops, exit 2 |
| `cmd_run` returned an error | `failed/` (outcome records the error) | stops, exit 1 |
| Interrupted, tree restored | `pending/`, `attempts + 1` | stops, exit 130 |
| Interrupted, not restored | `failed/` | stops, exit 130 |

- **A rolled-back run** did not converge, so its task goes to `failed/`, not
  `done/`. The tree is clean afterwards because `record_trail` commits the
  trail. If that commit fails, the next tick's cleanliness check refuses.
- **An error from `cmd_run`** means an unreachable runtime, a vault that will
  not compile, or a missing template. `work` stops because, from where it
  sits, a bad task and a dead runtime look the same, and a dead runtime would
  fail every task after it.
- **An interrupted run whose tree was restored** goes back to `pending/`
  because the run's own rollback applied and the task produced no outcome, so
  running it again is safe.
- **An interrupted run whose tree was not restored** goes to `failed/`
  because the tree is in a state nobody has looked at, and retrying would be
  a guess.
- The **run id** is the stem of the trail file this run wrote in
  `.trellis/runs/`, found by comparing the newest name before and after the
  run. It is `null` when no trail was written.

**Ctrl-C.** `cmd_run` races each run against `tokio::signal::ctrl_c()`. Once
that has been called, tokio owns the process's SIGINT handler for the rest of
the process. After the first task, a plain `sleep` would therefore swallow
Ctrl-C and `--every` could not be stopped. So `work` also listens for Ctrl-C
on its own thread and sets a flag. The flag is checked:
- before each claim
- immediately after a claim (the task is put back untouched, not counted)
- every 100 ms while idle

During a run, both listeners fire: `cmd_run` rolls back, and the flag stops
the loop after it. On every path, no task that this process claimed is left
in `claimed/`. This is not covered by an automated test. The loop's handling
of each outcome is, against a fake runner, but delivering a real SIGINT or
console Ctrl-C to a child process is not portable across the platforms the
suite runs on.

**Stale claims.** A task left in `claimed/` by a process that crashed is
never re-run automatically, because `work` cannot tell a dead claimant from a
live one in another terminal. It is still made visible:
- `work` names every task already in `claimed/` when it starts.
- `queue list` shows the claim's age and pid.
- `trellis queue requeue <id>` moves it, or a task in `failed/`, back to
  `pending/` and keeps its `attempts`.

**Not enforced.** Two `work` processes in one worktree at the same time.
`trellis run` does not lock either. The snapshot's clean-tree check catches
the second run once the first has written anything, but not before.

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

*As implemented:*
- `--log-json` is a global flag, next to `--root`, rather than a field on
  `run` and `work`. It is accepted by every command and logs whatever that
  command emits.
- The file is opened in append mode. The JSON layer sits under the same
  `Targets` filter as the terminal layer, so by default it records the INFO
  per-turn events from `trellis_loop`.
- This needs `tracing-subscriber`'s `json` feature, which adds `tracing-serde`.
  `serde` and `serde_json` were already in the tree.

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
