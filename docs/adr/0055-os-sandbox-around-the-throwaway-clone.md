# ADR 0055 — An OS sandbox around the throwaway clone's command: bwrap on Linux, sandbox-exec on macOS

**Status:** Accepted (owner-decided). Built for Linux (`bwrap`), tested
here against bubblewrap 0.9.0. The macOS path (`sandbox-exec`) compiles on
every platform and its profile text and argument list are tested, but it
has not been run on a Mac; see "Not tested".
**Date:** 2026-10-12
**Issue:** #621. The owner's decision, recorded in the issue on
2026-10-12: "bwrap on Linux and sandbox-exec (Seatbelt) on macOS, warn
when missing or failing. The repo: line says `sandbox: none (<reason>)`
and `doctor` names the binary; same when bwrap is present but fails (e.g.
Ubuntu 24.04 apparmor_restrict_unprivileged_userns). Windows stays on ADR
0035 job objects. No new crate."
**Amends:** ADR 0032 ("Limits" and "What is not built"), ADR 0021 rows
1.6 and 13.3, ADR 0023 §5 and ADR 0026 §11 where they defer an OS
sandbox. Story S05 (#86).
**Reference design:** anthropic-experimental/sandbox-runtime at 5507fb0e
(Apache-2.0), README "How It Works": bubblewrap with the network
namespace removed, and Seatbelt profiles on macOS. The argument list and
the profile here are written for trellis in the same shape; no text or
code is copied, and the TypeScript runtime and its allowlisting proxies
are not used.

## Context

ADR 0032 runs `bash`, custom tools, hooks and process agents in a
`git clone --no-hardlinks` of the repository and carries back only their
changes to tracked files. Its "Limits" section lists what a clone does not
contain: the command could open any network connection, read the home
directory and credentials files, and write anywhere the user can,
including the repository by absolute path. "What is not built" deferred
containers, a micro-VM or an OS sandbox to a later owner decision, and
ADR 0021 rows 1.6 and 13.3 stayed deferred with it. CLAUDE.md says the
fix for a writer that reaches the repository by absolute path is
isolation, not another post-hoc git check.

The workspace forbids `unsafe` and a new crate needs an ADR, so seccomp,
Landlock or namespaces called directly were out. Both Linux and macOS ship
or package a binary that does the confinement as a separate process:
bubblewrap (`bwrap`) and `sandbox-exec`.

## Decision

**When the platform's binary is on `PATH` and a trial run under it
succeeds, the command a writer runs in its clone is started under it.
When it is missing, or present and failing, the command runs as it did
before, in its clone, and trellis says so: a warning, never a refusal.**

The code is `crates/trellis-loop/src/os_sandbox.rs` (`OsSandbox`).

### Which commands

Every command that runs in a throwaway clone, through the two spawn points
ADR 0032 already has:

- `bash`, custom tools and hooks: `exec::run` (`crates/trellis-loop/src/exec.rs`),
  with `Spec::os` set by the orchestrator (`isolated`) and by `Hooks::spawn`.
  trellis's own `git` listings (`grep`, `find`) run through `exec::run` in
  the repository and get `os: None`; they are trellis's commands.
- process agents: `crates/trellis-cli/src/process_agent.rs`, which builds
  its `Command` from `OsSandbox::wrap`.

Not services (ADR 0026 §12) and not the grounded checks: neither runs in a
clone, and the ticket did not scope them.

The sandbox's process is spawned exactly as the command was: scrubbed of
the key variables (`trellis_core::secrets::scrub`), the process agent's
`pass_env` added after the scrub, the `GIT_LOCATION_VARS` removed, and on
Unix in its own process group (`exec::run`) or trellis's (process agents).
The command inherits that environment from it. `tests/child_env.rs` and
`tests/hygiene.rs` hold as before: the trial run is a `Command::new` with
the scrub beside it, and no `git` is spawned.

### Linux: `bwrap`

`bwrap --die-with-parent --unshare-net --unshare-pid --ro-bind / / --dev /dev
--proc /proc`, then the home directory and the temporary directory, then
`--bind <clone> <clone> --chdir <clone> -- <command>`:

- **No network.** `--unshare-net` gives the command a network namespace
  holding only a loopback device of its own: neither the internet nor a
  port anything on the machine listens on (127.0.0.1 included) is
  reachable.
- **The home directory reads as empty.** `--tmpfs $HOME`, with `HOME`
  taken from trellis's environment and canonicalized; nothing is hidden
  when it is unset, relative, `/`, or not a directory, and `trellis
  doctor` says so.
- **Writes land only in the clone and the system's temporary directory**
  (`std::env::temp_dir()`, bound read-write, since compilers and test
  runners write there; the clone lives under it). The rest of the
  filesystem is the read-only bind of `/`. Mount order: the outer of the
  home and temporary directories first, so the inner one is not covered,
  and the clone last, so it is writable wherever it lies.
- **Its own processes only.** `--unshare-pid` with a fresh `/proc`: the
  command cannot open `/proc/<trellis>/environ`, which holds the provider
  key trellis read, nor `/proc/<trellis>/cwd`, which leads to the
  repository through the host's writable mounts. When the command exits,
  the namespace ends and whatever it left running is killed with it.
- **`--die-with-parent`.** A process agent's timeout or interrupt kills
  the process trellis spawned, which is now `bwrap`; without this the
  command survived that kill and wrote after its timeout
  (`a_timed_out_command_is_ended_before_it_can_write_again` failed until
  it was added). The signal also fires when the thread that spawned
  `bwrap` exits; process agents are spawned from the thread that waits for
  them, and `exec::run` from tokio workers, which live as long as the
  runtime.
- **No `--new-session`**: a process agent stays in the terminal's
  foreground group (ADR 0026 §11, review round 9). The kernel still puts
  every process in the namespace in trellis's process group; read from
  inside the namespace that group's id is 0, so
  `a_process_agent_runs_in_trellis_s_own_process_group` now reads it from
  outside with `ps -A`.

### macOS: `sandbox-exec`

`sandbox-exec -p <profile> -D CLONE=<clone> -D TEMP=<temp> [-D HOME=<home>]
-- <command>`, with paths canonicalized (`/var` is `/private/var`). The
profile:

```
(version 1)
(allow default)
(deny network*)
(deny file-write*
  (require-not
    (require-any
      (subpath (param "CLONE"))
      (subpath (param "TEMP"))
      (subpath "/dev"))))
(deny file-read*
  (require-all
    (subpath (param "HOME"))
    (require-not (subpath (param "CLONE")))
    (require-not (subpath (param "TEMP")))))
```

The last rule is present only when there is a home directory to hide.
Paths reach the profile only as `-D` parameters, never spliced into its
text, so a path holding a quote or a parenthesis cannot change a rule.
Every rule is a `deny` whose filter excludes what stays allowed, on top of
`(allow default)`, so no rule depends on the order Seatbelt resolves
overlapping rules in. There is no pid isolation on macOS.

### Windows

Unchanged: the command is held by a job object (ADR 0035) and is not
sandboxed. The label is `none (Windows: the command is held by a job
object (ADR 0035), not sandboxed)`.

### Detection and the warning

`OsSandbox::detected()` runs once per trellis process: it looks the binary
up on `PATH`, then runs `true` under it with the arguments a command would
get (a trial run, 10-second limit). Ready only if that exits 0. Otherwise
the label is `none (bwrap not found)`, or `none (bwrap failed: <the first
line it printed>)`, the line cut to 160 characters with control characters
removed. On Ubuntu 24.04 with `apparmor_restrict_unprivileged_userns` set,
that line is bwrap's own (`bwrap: setting up uid map: Permission denied`
or similar); it is not special-cased.

Where it is said:

- **The run's `repo:` line**, on every run: `repo:     ... write rule(s),
  sandbox: bwrap, ...` when the line has other things to say, and
  `repo:     sandbox: <label>` alone when it does not (a run under no rules
  still does not announce "0 write rule(s)").
- **A process agent's run**: a `sandbox:  <label>` line after `snapshot
  at`.
- **`trellis doctor`**: `sandbox:    bwrap at /usr/bin/bwrap: bash, custom
  tools, hooks and process agents run with no network, <home> hidden, and
  writes only to their clone and <temp>`, or `sandbox:    none (<reason>);
  ... (ADR 0032, Limits)`.
- **`trellis trust`**'s note, which says which of the two holds on this
  machine and no more.

A sandbox that passes its trial run and then fails for one command (the
binary removed mid-run) shows as that command's failure: `bwrap`'s
message in its output, or a start error naming `bwrap`. It is not
retried unsandboxed.

## Tests

In `crates/trellis-loop/src/isolate.rs`, each running its probe once with
no sandbox (which must succeed, so a probe that could never work cannot
pass) and once under the platform's sandbox given a fake `HOME`:
`sandboxed_writer_bash_cannot_open_a_tcp_connection_to_loopback`,
`sandboxed_writer_bash_cannot_read_a_file_under_a_fake_home`,
`sandboxed_writer_bash_cannot_see_trellis_s_process` (Linux) and
`sandboxed_writer_hook_cannot_open_a_tcp_connection_or_read_a_fake_home`.
In `crates/trellis-cli/tests/orchestration.rs`,
`sandboxed_writer_process_agent_cannot_connect_to_loopback_or_read_a_fake_home`
runs the binary with `HOME` pointed at a fake home. In
`crates/trellis-cli/tests/cli.rs`,
`sandbox_is_named_on_the_repo_line_and_by_doctor_alike` and (Linux)
`sandbox_missing_or_failing_is_a_warning_that_names_the_reason`, which
puts a `bwrap` that fails as AppArmor's refusal does first on `PATH`, then
a `PATH` with none. The argument list, the mount order, the Seatbelt
profile and its parameters, and the reason text are unit tests in
`os_sandbox.rs`.

On a machine where the sandbox is missing or fails, the `sandboxed_writer_`
tests check the warning path instead (the command ran, unconfined, and the
label says why) and print that the confinement half was not exercised.
Removing `--unshare-net` or the `--tmpfs` of the home directory makes them
fail on a machine with a working `bwrap` (checked while writing them).

## Not tested

- **macOS.** No Mac was available. The profile's syntax, its semantics
  under Seatbelt, `-D` parameters with `(param ...)`, and `--` ending
  `sandbox-exec`'s options (POSIX `getopt` behaviour) are as documented
  and as the reference design uses them, not observed. A profile that
  Seatbelt rejects fails the trial run and becomes `none (sandbox-exec
  failed: ...)`, the safe direction; a profile that loads but does not
  confine would not be caught by the trial run. The `sandboxed_writer_`
  tests run `sandbox-exec` for real on macOS and are the check: run them
  there before relying on it.
- **Ubuntu 24.04's AppArmor refusal** is simulated by a stand-in binary.
  The suite ran as root, in a container without that restriction; a
  manual trial run as `nobody` there succeeded too.

## Limits

What the sandbox still does not stop, stated so it is not assumed closed:

- **No sandbox at all** where the binary is missing or fails, on Windows,
  and on other platforms: ADR 0032 "Limits" applies in full there, and the
  `repo:` line says so.
- **Reads outside the home directory.** `/etc`, `/opt`, other users'
  readable files, and a repository kept outside the home directory remain
  readable (Linux and macOS).
- **The temporary directory is shared and writable.** Other clones, other
  programs' temporary files, and a repository kept under it (as in the
  test suite) can be read and written. A private tmpfs there would close
  this on Linux; Seatbelt has no equivalent, and the ticket named a
  writable temporary directory.
- **Local Unix sockets.** A socket outside the home directory (`/run/user`
  holding the D-Bus session bus and keyrings, `/var/run/docker.sock`, an
  `ssh-agent` socket under `/tmp`) can still be connected to on Linux: a
  read-only mount does not refuse `connect`, and a network namespace
  isolates only abstract sockets. On macOS `(deny network*)` covers Unix
  sockets too.
- **The terminal.** A process agent shares trellis's terminal and could
  push input to it (`TIOCSTI`) where the kernel still allows that;
  `--new-session` would stop it and put the agent in the background
  group, which review round 9 removed.
- **Services and the grounded checks** are not sandboxed.
- **No switch to turn it off.** A hook or `bash` command that needs the
  network (`pip install`) fails under the sandbox, and the model or the
  hook sees that failure. Whether a user-level switch should exist is left
  to the owner.
- **The trial run is once per process.** A sandbox that starts failing
  later shows as command failures, not as `none`.

## Alternatives rejected

- **Refuse to run when the sandbox is missing or fails.** The owner chose
  a warning (#621): a refusal would stop every `bash` call on Windows, on
  WSL without bubblewrap, and on stock Ubuntu 24.04.
- **Landlock, seccomp or namespaces called directly.** Each needs `unsafe`
  (forbidden in the workspace) or a new crate (an ADR of its own, and the
  owner said no new crate).
- **A container or micro-VM.** A runtime on the user's machine that
  trellis cannot assume, for the same confinement on these two counts.
- **The reference design's proxies**, which re-allow listed hosts through
  a host-side proxy. Nothing here needs the network, and every proxy is a
  route out.

## Consequences

- On a Linux machine with a working `bwrap`, a `bash` call that needs the
  network, the home directory (a `~/.cache`, a `~/.pyenv` interpreter) or
  to write outside its clone and the temporary directory now fails, and
  the model sees why. Ignored files were already absent from the clone
  (ADR 0032).
- The guards ADR 0023 §5 lists stay. Against a sandboxed writer the
  repository is unreachable unless it lies under the temporary directory;
  against an unsandboxed one, and against services, they are what stands.
- ADR 0032's "Limits" items "No network isolation" and "Reads outside the
  clone" hold now only where the label is `none`; "Writes outside the
  clone" is narrowed to the temporary directory under a sandbox.

## Reversibility

Set `os: None` at the two `exec::run` call sites and drop the
`OsSandbox::wrap` call in `process_agent.rs`, and every command runs as
it did under ADR 0032. Nothing on disk changes.

## What would change it

A user who needs network in hooks (a switch, or an allowlist in the
reference design's shape); a report that `--die-with-parent` killed a
command early because its spawning thread exited; Apple removing
`sandbox-exec`; or a decision to sandbox services and checks too.
