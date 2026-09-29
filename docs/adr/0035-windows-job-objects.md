# ADR 0035 — Windows Job Objects through `process-wrap`

**Status:** Accepted. Built.
**Date:** 2026-09-29
**Decided:** 2026-09-28 by the owner, via the coordinator (issue #21)
**Amends:** ADR 0023 §3 (what a command leaves running, on Windows),
ADR 0026 §11 step 2 (the same for a process agent), and the "Backgrounded
processes on Windows" entry in ADR 0023's list of known ways around rollback

## Context

On Unix a `bash` command, custom tool, hook or listing leads its own
process group, and after a normal exit or a timeout the group is killed
before anything is judged or rolled back (ADR 0023 §3). On Windows the only
tool was `taskkill /T /F`, which walks the tree by parent pid from a live
process. After a normal exit the command is gone, so there is nothing to
walk from, and a grandchild it backgrounded kept running: it could write
after a convergence judged the tree, or after a rollback restored it. A
process agent (ADR 0026 §11) had the same gap, and after its normal exit
nothing was signalled at all.

A Job Object holds a process tree by handle rather than by pid: every
process a member creates joins the job unless the job lets it break away,
and `TerminateJobObject` ends them all. Creating one needs Win32 calls that
are `unsafe` in Rust. The workspace has `unsafe_code = "forbid"`, so the
calls either live in a dependency or are not made. PR #1's review rounds 8
and 9 recorded that choice and left it open.

## Decision

**Take `process-wrap` as a direct dependency, Windows only, and spawn
`exec::run`'s children (trellis-loop) and process agents (trellis-cli) into
a Job Object of their own with kill-on-close and no breakaway.**

- `Cargo.toml`: `process-wrap = { version = "10.0.1", default-features =
  false, features = ["job-object", "kill-on-drop", "tokio1"] }`, and in
  trellis-loop and trellis-cli only under `[target.'cfg(windows)'.dependencies]`
  (and trellis-checks since #172, below).
  No Unix build compiles it.
- `kill-on-drop` is the third feature. The owner's approval named
  `job-object` and `tokio1`; `JobObject` sets
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` only when the command also carries
  the `KillOnDrop` wrapper, which exists only under this feature. It adds no
  crate.
- **The default `tracing` feature is off.** With it, `process-wrap`
  instruments its spawn hooks at debug level with the `Command` as a field.
  Nothing here needs those events. Checked, in case it is turned on: on
  Windows, the only platform that compiles the crate, a `Command`'s `Debug`
  output (`{:?}` and `{:#?}`) shows the program and arguments and not the
  environment, so a `pass_env` key (ADR 0026 §11) would not reach the log
  through it.
- **`unsafe_code = "forbid"` stays.** No workspace crate contains `unsafe`.
  The unsafe calls are in `process-wrap`'s `src/windows.rs`:
  `CreateJobObjectW`, `CreateIoCompletionPort`, `SetInformationJobObject`,
  `AssignProcessToJobObject`, `CreateToolhelp32Snapshot`/`Thread32First`/
  `Thread32Next`, `OpenThread`, `ResumeThread`, `TerminateJobObject`,
  `GetQueuedCompletionStatus`, `CloseHandle`. That is the cost the ticket
  named: the unsafe code moved into a dependency rather than disappearing.

### What happens, per spawn

1. The command is built and scrubbed exactly as before
   (`trellis_core::secrets::scrub`, then the process agent's `pass_env`),
   and only then handed to `process_wrap::tokio::CommandWrap`. The wrapper
   adds a creation flag and nothing to the environment.
2. It is created with `CREATE_SUSPENDED`, assigned to a new job, then its
   threads are resumed. Nothing it starts can run before it is in the job.
3. The job's limit flags are exactly `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
   Neither `JOB_OBJECT_LIMIT_BREAKAWAY_OK` nor
   `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` is set.
4. The wait is on the command itself (the wrapper's inner child), not on
   the job: the wrapper's own `wait` goes on to wait for every process in
   the job, and a backgrounded grandchild would hold the call for its life.
5. **However the command ends** (exit, timeout, interrupt), the job is
   terminated before anything else happens: in `exec::run` before its pipes
   are drained, in a process agent before `.git/config` is restored and the
   tree compared. The job handle, not a pid, names the tree, so this is
   safe after the command has been reaped; the Unix path needs a group
   holder for the same guarantee (ADR 0023 §3, round 9).
6. The job handle is closed when the child is dropped. With kill-on-close,
   that ends the tree too if trellis panics or is killed.

If the job cannot be created or the process cannot be assigned, the wrapper
kills the child and the spawn fails: `exec::run` returns
`ExecError::Start`, a process agent reports that it could not start. That
is the safe direction; running the command unheld is what this replaced.

### The failure mode, and why no breakaway

A process in a job may be created outside it with
`CREATE_BREAKAWAY_FROM_JOB`, but only if the job allows breakaway. Had the
job been created with `JOB_OBJECT_LIMIT_BREAKAWAY_OK`, any child could pass
that flag and escape, and with `SILENT_BREAKAWAY_OK` every child would
escape without asking. The job is created with neither, so a
`CREATE_BREAKAWAY_FROM_JOB` request fails with access denied rather than
producing an unheld process.

## What is still not held

Stated so it is not assumed closed (the fix for all of them is a sandbox,
ADR 0021 row 13.3):

- **A process started for the command by something outside the job.** The
  Task Scheduler (`schtasks`), WMI (`Win32_Process.Create`), a COM server,
  the shell via `explorer.exe`, or a service the command asks to start a
  process creates it as a child of that other process, outside the job.
- **Nested jobs.** If trellis itself runs inside a job (a CI runner, some
  terminals), ours is nested in it, which Windows 8 and later support. How
  a breakaway request behaves across such a chain was read from Microsoft's
  documentation, not tested here.
- **The grounded checks** were out of this ticket's scope. #172 (below)
  brought them in.
- **Unix is unchanged.** A process agent runs in trellis's own process
  group (ADR 0026 §11, round 9), so after its normal exit nothing is
  killed there, and a process that leaves its group (`setsid`) is not
  reached.

## Facts checked (2026-09-29)

- **Version:** `process-wrap` 10.0.1, published 2026-09-23, Apache-2.0 OR
  MIT, `rust-version` 1.87 (the workspace's is 1.93).
- **Maintainers:** one crates.io owner, Félix Saparelli (`passcod`), who
  maintains watchexec, which uses it. Repository
  `github.com/watchexec/process-wrap`, not archived, last pushed
  2026-09-29; GitHub lists 376 commits by him and single digits by anyone
  else. About 15.8 million downloads, 7.5 million in the last 90 days
  (crates.io API).
- **Source read:** `src/windows.rs` and `src/tokio/job_object.rs` in the
  10.0.1 package, for the limit flags, the suspended-create-assign-resume
  order and the wait semantics above.
- **Added to `Cargo.lock`:** `process-wrap`, `windows` 0.62.2 with
  `windows-core`, `windows-collections`, `windows-future`,
  `windows-implement`, `windows-interface`, `windows-numerics`,
  `windows-result`, `windows-strings`, `windows-threading`, and `futures`
  0.3.34 with `futures-channel`, `futures-executor`, `futures-io`,
  `futures-macro`, `futures-sink` (`futures-core`, `-task` and `-util` were
  already present). No existing entry changed version. `nix` is behind
  `cfg(unix)` in `process-wrap` and not compiled on Windows.
- **`cargo deny check`:** `advisories ok, bans ok, licenses ok, sources ok`;
  the duplicate-version warnings name `getrandom`, `hashbrown`, `syn` and
  `windows-sys`, none of them a crate this adds.
- **`cargo audit`:** no vulnerability; the one warning is RUSTSEC-2023-0089,
  already accepted in `deny.toml`.
- **Binary size and build time:** not measured.

## Alternatives

- **Hand-written FFI over `windows-sys`.** Already in the lockfile, but the
  calls are `unsafe` and would need `unsafe_code` lifted for a crate.
  Rejected by the rule this ADR keeps.
- **`win32job` 2.0.3.** A safe wrapper over job objects only. It would still
  leave the suspended-create-assign-resume sequence to us, and resuming a
  suspended thread is itself an unsafe call; without it a child could start
  a grandchild before assignment.
- **`command-group`.** The same author's earlier crate, superseded by
  `process-wrap`.
- **Record the limit as permanent.** The ticket's other branch. Rejected by
  the owner: it leaves a documented route for a model-run `bash` command to
  write after rollback on the platform the project is developed on.

## Tests

- `exec::tests::job_object_kills_tree` (trellis-loop, Windows only): a
  command backgrounds a second `cmd` that appends to a heartbeat file about
  once a second, waits, and exits 0. After `run` returns the file must be
  non-empty (the heartbeat was alive) and must not grow over the next four
  seconds. Spawning without the job, the same test fails.
- `job_object_kills_a_process_agent_s_tree` (trellis-cli
  `tests/orchestration.rs`, Windows only): the same through `trellis work`
  and a process agent. `work`'s output goes to a file, not a pipe: an
  escaped grandchild inherits the pipe, and waiting for it to close hid the
  escape. Spawning without the job, it fails.

Both heartbeats stop after 30 beats, so a regression fails the test rather
than holding the test run on a process that never ends.

## Amendment: the grounded checks (#172, 2026-09-29)

A check that exited normally left what it started running, and on Windows
nothing reached it: `taskkill /T` walks by parent pid from a live process,
and #184 saw a check's `ping` grandchild outlive it by about 120 s. Decided
by the owner (issue #172): after a check exits, kill its whole group or
job, and fail the check if the tree changed after its exit was observed.

- **Windows:** `trellis-checks` spawns the checker exactly as `exec::run`
  does (scrub, then `KillOnDrop` and `JobObject`, wait on the inner child).
  After the exit the job is terminated, pass or fail, and on a timeout it
  replaces `taskkill /T`. The dependency line is the same, under
  `[target.'cfg(windows)'.dependencies]`; `Cargo.lock` gains only the edge.
- **Unix:** the checker already led its own group. It now gets the same
  group holder as `exec::run` (a `cat` in the group, reaped last), and the
  group is killed through `trellis_core::process::kill_group` after the
  exit. The timeout path is unchanged (`kill_tree` while unreaped).
- **The tree check.** For an exit that is a pass (code 0), the tree under
  the root is walked by metadata when the exit is observed and again after
  the kill; any difference fails the check (`exit_code: None`, through the
  existing constructors; `Verdict` is unchanged). Between the two walks the
  pipes get a grace of 2 s within the check's bound, so a leftover that
  holds them is seen writing; before #172 that wait was the whole bound.
  Not seen: a write between the exit and the first walk reaching that path,
  a same-length rewrite that restores its modification time on Windows
  (Unix compares `ctime`), and anything a process the kill does not reach
  writes after the second walk. The fix for those is the sandbox (ADR 0021
  row 13.3).
- **Tests:** `check_leftover_*` in `trellis-checks`: a backgrounded
  heartbeat stops when the check returns; a leftover writing after the
  exit fails the check; a normal check that writes before it exits passes.
