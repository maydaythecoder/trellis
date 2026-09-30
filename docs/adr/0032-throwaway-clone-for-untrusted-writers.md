# ADR 0032 — A throwaway clone for the writers trellis does not control

**Status:** Accepted. Built: `bash` and custom tools (#20), hooks
(#174, "Hooks" below), and process agents (#175, "Process agents"
below). What remains is in "What is not built".
**Date:** 2026-09-29
**Issue:** #20. The owner chose the mechanism on 2026-09-28 (the issue's
`## Decision`); this ADR records it and its limits.
**Amends:** ADR 0023 §5 and ADR 0026 §11, "What rollback guarantees when
trellis is not the only writer"; ADR 0021 row 13.3 (the sandbox decision
it deferred is taken here for the first step).

## Context

ADR 0009's rollback is exact when trellis's own `edit` and `write_file`
are the only writers. `bash`, custom tools, hooks and process agents are
not trellis's: each was an unsandboxed process running in the repository
with the user's authority. Eleven review rounds on PR #1 found about
fifteen ways such a writer could place a change where the post-hoc git
checks do not look: an edit to `.gitignore`, `git update-index
--skip-worktree`, writes under `.git/` outside the guarded files, refs
other than `HEAD`, pre-existing ignored files, processes that outlive the
call. ADR 0023 §5 listed them and said the fix is isolation, not another
check, because every check reads state the writer can also write.

Three kinds of isolation were on the table (ADR 0021 rows 1.6, 13.3, 12
and 17-18): a container, a micro-VM, and an OS sandbox (seccomp and
namespaces, `sandbox-exec`, a Windows AppContainer or Job Object). Each
needs something trellis does not have: a container runtime on the user's
machine, a hypervisor, or per-platform code that the workspace cannot write
without `unsafe` or a new dependency. The owner's decision was to start
with what needs none of those.

## Decision

**copy-in/diff-out.** A writer trellis does not control runs in a
disposable `git clone --no-hardlinks` of the repository, made under the
system's temporary directory for the one call, and only the change it made
to files trellis tracks comes back, through trellis's own write path.

For each call (`crates/trellis-loop/src/isolate.rs`, driven from the
orchestrator):

1. **List.** `git ls-files --cached --others --exclude-standard` in the
   repository: the tracked files, and the untracked files git does not
   ignore (what a `write_file` earlier in the run created). `ls-files`
   reads the index and never writes it, so the repository's index is not
   touched; `git status` is not used because it may refresh the index.
2. **Clone.** `git clone --no-hardlinks` through `trellis_vcs::command()`,
   so no hook runs on the checkout and the key variables are scrubbed.
   `--no-hardlinks` because a hard-linked object is one file under two
   names. The clone's `HEAD` must be the repository's commit or the call
   is refused. The `origin` remote and the reflog are removed, since both
   name the repository's path.
3. **Copy in.** Every listed regular file's bytes are written into the
   clone where the checkout left different ones (the run's own uncommitted
   edits) or none, and the sha256 of what was written is recorded. A write
   whose directory resolves outside the clone through a link is refused,
   and so is the whole call. Links and submodules are left as the checkout
   made them and are never carried back.
4. **Run.** The command runs with the clone as its working directory,
   through the same `exec::run` as before (bounded output, timeout, key
   scrub), with every variable that points git at a repository
   (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and the rest) removed, so
   a `git` it runs acts on the clone.
5. **Diff out.** Each copied-in file is read again from the clone's
   filesystem and hashed. **No `git` runs in the clone after the command**:
   its `.git/config` belongs to the command by then, and a filter driver or
   `core.fsmonitor` planted there is a program git would run. A file that
   is now a link, or whose directory leads out of the clone, is refused
   and never read through: what a link points at is not the file git
   tracks, and `a.py -> ~/.ssh/id_rsa` read through would carry a key into
   a tracked file by accident. (A command that copies a key's bytes on
   purpose can; that is the "reads outside the clone" limit below.) A file over 16 MiB is
   refused. A deleted file is reported and not carried back. A new file is
   not carried back.
6. **Carry back.** Each changed file goes through the orchestrator's
   write path, the one `edit` uses: `write_refusal` (outside the
   repository, under `.git/`, under `.trellis/`, through a link), the
   repository's `deny_write`, and a check that the repository's copy still
   has the bytes that were copied in. `.gitignore`, `.gitattributes` and
   `.gitmodules` are never carried back, since they change what git
   ignores or filters and so what a rollback reaches; the model is told to
   use `edit` for them. A carried-back file is recorded in `touched`, so
   the rollback and the trail both have it, and the index session is
   updated.
7. **Remove** the clone, whatever happened.

The observation tells the model what was carried back, what was not and
why, and that the command ran in a clone.

What this closes, for `bash` and custom tools, against a command that
writes where it runs:

- a `.gitignore` or `.gitattributes` edit: stays in the clone;
- `skip-worktree` and `assume-unchanged`: set in the clone's index;
- writes under `.git/`, refs, tags, stashes, branch moves, hooks,
  `core.fsmonitor`, config includes: all in the clone's `.git/`;
- pre-existing ignored files (`.env`, a virtualenv): not in the clone at
  all;
- untracked and new ignored files: not carried back;
- a process the command left running: writes into the clone, which is
  gone, or into nothing.

And what the carry-back gives the rollback: every byte a command changes
in the repository is an ordinary edit to a tracked file (or to a file
`write_file` created), which `git reset --hard` and `git clean -fd`
restore exactly, as ADR 0009 states for trellis's own edits.

Tests: `isolated_clone_keeps_gitignore_skip_worktree_and_git_dir_writes_out_of_the_repository`
(a command that edits `.gitignore`, runs `git update-index
--skip-worktree` and writes into `.git/` leaves the repository's `git
status`, its index bytes and a digest of every file under `.git/`
unchanged), `isolated_clone_carries_a_tracked_file_edit_back_through_the_edit_path`,
and `isolated_clone_edit_carried_back_is_undone_by_rollback`, in
`crates/trellis-loop/src/isolate.rs`.

## Limits

This isolates a writer that edits where it stands. It does not isolate
one that goes looking for the repository or for anything else on the
machine. Stated so they are not assumed closed:

- **No network isolation.** The command can open any connection the user
  can, and send anything it can read.
- **Reads outside the clone.** The home directory, credentials files,
  other repositories: whatever the user can read, the command can.
- **Writes outside the clone.** The same for writes. The repository's path
  is removed from the clone's remote and reflog, but a command that is
  given it, finds it (a parent process's working directory, a shell
  history, a search of the disk) or guesses it can write there directly,
  `.git/` included. Against that writer the ADR 0023 §5 guards (the trust
  guard, the git guard, the ignored-file listing, the worktree digests)
  are still what stands, and they are kept for exactly that reason. The
  same holds for the user's global git configuration (`~/.gitconfig`),
  which every `git` trellis runs in the repository reads; only the
  settings in `trellis_vcs::OVERRIDES` outrank it.
- **Clones left by a crash.** The clone is removed when the call ends,
  panics included; a trellis process killed mid-call leaves it in the
  temporary directory.
- **Processes that outlive the call** (#21). A process the command
  backgrounded is not killed on Windows after a normal exit (ADR 0023 §3).
  Inside the clone it writes to a directory that is being removed; one
  that writes by absolute path is the previous item.
- **Clone cost per call.** Each call clones the repository (the whole
  object store, copied rather than hard-linked) and reads every tracked
  file three times to hash it (the repository's copy and the clone's, before
  and after). On a large repository that is seconds per call,
  paid by every `bash` and custom-tool call.
- **Ignored files are absent.** A virtualenv, `node_modules`, build output
  and `.env` are not in the clone, so a command that needs them (a test
  runner from `.venv/bin`) fails there. The model sees the failure; the
  checks trellis runs itself (`run_checks`) still run in the repository.
- **Only edits come back** from `bash`, custom tools and hooks. New files
  and deletions stay in the clone, as do changes to links, file modes and
  submodules. For process agents new files and deletions come back too
  ("Process agents" below).
- **Checkout filters run.** The clone's checkout runs the user's global
  smudge filters (Git LFS), as any clone does.

## Consequences

- `bash` and a custom tool no longer run in the repository. A command that
  wrote there by relative path now writes into the clone and its edits to
  tracked files come back; the tests that asserted the trust guard catches
  a `bash` rewrite of `.trellis/` now write by absolute path, which is the
  writer the guard is still for.
- A repository with no commit can still be cloned (empty) and copied into;
  a directory that is not a git repository cannot, and `bash` and custom
  tools refuse to run there rather than run in place. `trellis run`
  already requires a git repository (ADR 0009).
- `may_change_tree` is unchanged: a `bash` or custom-tool call still marks
  the turn as one that may have changed the tree.

## Hooks (amended 2026-09-29, #174)

The owner's decision on #20 covers hooks; this section records the shape
they run in. Before it, every hook (ADR 0023 §5) ran with the repository
as its working directory, the one writer trellis starts that the first
slice left there.

**`pre_tool`, `post_tool` and `turn_end`: one clone per call, shared,
refreshed before each hook, carried back after each.** For a model call
(`crates/trellis-loop/src/orchestrator.rs`, `hook_clone_path` and
`carry_back_hook`):

1. At the call's first hook, the clone is made as for `bash` (steps 1 to
   3 above). A call with none of the three hooks declared makes none,
   and one that governance refuses makes one only for `turn_end`, which
   runs after a denied call too.
2. Before each later hook of the same call it is **refreshed**
   (`Sandbox::refresh`): the repository is listed again and every listed
   file is copied in where the clone's bytes differ, or put back where
   the clone lost it, and those bytes become the new baseline. Between
   `pre_tool` and `post_tool` the tool ran, and `edit`, `write_file` or a
   `bash` carry-back wrote into the repository; the refresh is what lets
   `post_tool` look at the tree the call left, which is its contract. The
   call and its result still arrive on stdin as before. The refresh runs
   `git ls-files` in the repository and only filesystem operations in the
   clone, so step 5's rule holds: no `git` runs in a clone a hook has had.
   A clone that cannot be refreshed (a hook put a link to outside the
   clone where a tracked directory was) is removed and a new one made.
3. The hook runs with the clone as its working directory and the
   `GIT_LOCATION_VARS` removed, through `exec::run` (bounded output,
   timeout, key scrub) as before.
4. Right after it exits, what it changed among the copied-in files is
   **carried back through the same `carry_back` `bash` uses**: the same
   refusals (`write_refusal`: `.git/`, `.trellis/`, links, outside the
   repository; `.gitignore`, `.gitattributes`, `.gitmodules`), the
   repository's `deny_write` and then a profile's `allow_write` (#182),
   the check that the repository's copy still has the bytes copied in,
   recorded in `touched`, and the index session updated. A hook has no
   observation to report into, so what was carried back and what was
   refused is logged (`tracing`, warn).
5. The clone is removed when the call ends, whichever way it ended.

Shared rather than one clone per hook, because a clone is the expensive
step and a call with `pre_tool` and `post_tool` would otherwise pay two
or three; and since the refresh resets every tracked file to the
repository's bytes before each hook, what one hook leaves for the next is
only what is not tracked (a scratch file, the clone's `.git/`). Only
trusted hooks run in it: `bash` and custom tools get clones of their own.

`pre_tool` is a gate in the orchestrator's gate list, built by the
binary. The orchestrator hands it the clone through two provided methods
on `ToolGate` (`runs_repository_code`, `run_in`); the defaults leave every
other gate as it was. **Fail closed:** when no clone can be made the gate
is handed the reason and denies the call. A `HookGate` used outside an
orchestrator runs its hook in a clone of its own and drops what it wrote.
It never runs in the repository. `post_tool` and `turn_end` only observe,
so when no clone can be made they are not run, and that is logged.

The worktree brackets of ADR 0023 ("after a pass") stay around each hook
and now enclose its carry-back, so a hook whose edit came back marks the
turn as one that may have changed the tree, as before. They also stay for
the hook that writes into the repository by absolute path, which no clone
stops (the "Writes outside the clone" limit).

The trust guard is read before the refresh (so the copy the hook runs is
of what was just checked) and again by the hook before it spawns.

**`input`, `before_run` and `after_run`: a clone of their own, nothing
carried back.** They run outside any call, where no orchestrator and so
no carry-back exists, and none of their contracts is to write:

- **`input`** runs before the snapshot. It moves into a clone. In the
  repository, a write by it to a tracked or visible file dirtied the tree
  and the snapshot refused the run, but a write to an ignored file (a
  `.venv/.../sitecustomize.py`) or under `.git/` was taken before the
  ignored-file listing and the `.git/` digest (ADR 0023 §5), so it became
  the baseline those guards compare against and was never seen. Nothing
  it writes in the clone comes back; it can still refuse the task.
- **`before_run`** adds context from its stdout. It runs after the
  snapshot but before the orchestrator exists, so there is no write path
  to carry back through; its stdout is unchanged.
- **`after_run`** runs after the last check. A write by it could only
  ever demote a converged run (ADR 0023, "after a pass"), so nothing is
  carried back. The binary's fingerprint around it stays, for the writer
  that names the repository by absolute path.

When no clone can be made, `input` goes on as for any other failure of
the hook (`HookError::NotIsolated`, printed as a `hook:` line),
`before_run` adds nothing and says why, and `after_run`'s failure is
logged.

What this closes, for hooks, against a hook that writes where it runs:
everything the Decision lists for `bash` (`.gitignore` edits, index
flags, `.git/` writes, ignored and new files, `.trellis/` rewrites), and
the gap above for `input`.

Limits, in addition to those above, which apply to hooks as they do to
`bash`:

- **Clone cost per call.** A repository that declares `pre_tool`,
  `post_tool` or `turn_end` pays one clone per model call, and a
  refresh (a listing and a hash of every tracked file) per further hook.
  `input`, `before_run` and `after_run` pay one clone each per run.
- **Ignored hook scripts are absent.** A hook named relatively
  (`["python", ".trellis/hooks/pre.py"]`, ADR 0023 §4) runs the clone's
  copy, which exists only when the script is tracked or untracked and
  not ignored. A git-ignored script is not in the clone, the hook fails
  to start, and `pre_tool` then denies every call. The same holds for a
  hook that needs a virtualenv or other ignored file.
- **A hook's view is a copy.** A hook that read state trellis does not
  copy (ignored files, the repository's `.git/`, its reflog or remote)
  sees the clone's instead.

Tests, in `crates/trellis-loop/src/isolate.rs`:
`isolated_hook_writes_reach_the_repository_only_as_carried_back_tracked_edits`,
`isolated_hook_post_tool_sees_the_call_s_edit_in_the_clone_pre_tool_shared`,
`isolated_hook_write_is_refused_by_deny_write_then_allow_write`,
`isolated_hook_pre_tool_denies_when_no_clone_can_be_made`, and
`isolated_hook_input_runs_in_a_clone_and_nothing_it_writes_comes_back`.

## What is not built

- ~~Process agents~~: built by #175; see "Process agents" below.
- **Containers, a micro-VM or an OS sandbox**, for network isolation and
  reads outside the repository. Later, per the owner's decision; ADR 0021
  rows 1.6 and 13.3 stay deferred for that step.

## Process agents (amendment, #175)

A process agent (ADR 0026 §11) now runs in a clone made by the same
`isolate::Sandbox`, through `trellis_loop::AgentClone`
(`crates/trellis-loop/src/agent_clone.rs`), started from
`crates/trellis-cli/src/process_agent.rs`. Steps 1-5 and 7 above are
unchanged. The clone is made after the snapshot, the judge (`[checks]`,
`deny_write`, the rule documents, the profile's `allow_write`, all read
before the command) and the git guard; the command runs with the clone as
its working directory and every `GIT_LOCATION_VARS` variable removed; and
its carry-back runs after the command has ended (and, on Windows, after
its Job Object is terminated, ADR 0035) and after the git guard has put
`.git/` back, since the carry-back asks the repository's git what it
ignores.

### New files and deletions come back for agents

For `bash` and custom tools only edits come back. For a process agent,
**new files and deletions of tracked files come back too**, under the same
refusals. The reasons:

- **It has no other write path.** A `bash` call is one step in a model run
  whose model has `write_file` for a new file; a command's new files are
  mostly build output and caches, and leaving them in the clone costs the
  model nothing. A process agent's clone is the whole of its work: an
  agent that adds a module or a test, or removes a dead file, and whose
  carry-back dropped that, would converge on a change it did not make.
  Every existing test of a converging agent creates a file.
- **The rollback still covers them exactly.** A deleted tracked file is put
  back by `git reset --hard`. A new file is written only where the
  repository's own ignore rules (`.gitignore` files, `info/exclude`,
  `core.excludesFile`, read by `git check-ignore` in the repository) do
  not ignore it, so `git clean -fd` removes it. The clone's `.gitignore`
  is never consulted: it is the agent's to edit, and un-ignoring `*.cfg`
  there would otherwise bring back a file the rollback leaves behind.
- **A refusal never lets a partial change converge.** Anything the agent
  changed that does not come back is reported, and the run does not
  converge and is rolled back. A new file the repository's rules ignore is
  not a refusal: it is left in the clone, reported, and is not part of the
  change, as a model run's caches are not.

Would reverse if an agent kind needs to leave ignored files as its result
(a generated artifact the repository ignores); that would need a copy of
ignored files for the rollback, which trellis does not keep.

### The carry-back for an agent

For each path, whatever the kind, in order:

1. under `.trellis/`, by any spelling and through any link: a breach,
   named as `governance: refused: <path> (under .trellis/, ...)`;
2. `write_refusal`: outside the repository, inside `.git`, an unsafe name;
3. the repository's `deny_write` (named by its rule), then
4. the profile's `allow_write` (#182; ADR 0026 §4): `deny_write` first,
   so a path the repository denies is named by the repository's rule.

Then, per kind: an edited or deleted file must still hold, in the
repository, the bytes copied into the clone; a new file must not exist in
the repository, must not be ignored there, and is written with
`create_new`. `.gitignore`, `.gitattributes` and `.gitmodules` are never
carried back in any kind, links never (a new link is refused; a tracked
one is left as the checkout made it), and nothing over 16 MiB.

New files are found by walking the clone's filesystem, never through a
link and never into a directory named `.git`. **No `git` runs in the
clone**, as in step 5. A directory the repository's rules ignore is not
entered (one `git check-ignore` per level of the walk, in the
repository), so an agent that fills `build/` or a virtualenv costs one
line of report, not a walk of every file. A clone holding more than
`NEW_FILE_WALK_CAP` (100 000) files and directories git does not ignore,
or a `check-ignore` that fails, carries nothing new back and is a
refusal.

Only a command that exited is carried back. A timed-out or interrupted
one left half a change; nothing comes back, and the run is rolled back as
before.

`allow_write` moves to the carry-back as #182 asked, and the
post-command judge keeps its copy, with `deny_write` and the `.trellis/`
breach, over what git shows changed in the repository afterwards. The
two do not report the same file twice: a file the carry-back refuses is
never in the repository. The judge's copy is what stands against a
command that writes into the repository by absolute path, which the clone
does not stop ("Limits"), and the git guard, the `HEAD` check and the
ignored-file snapshot stay for the same writer.

### What changes for an agent

- **A commit, reset or checkout in its clone is no longer a breach.** It
  moves the clone's `HEAD`; the files are compared by content, and what
  the agent committed comes back uncommitted, for review. A `HEAD` moved
  in the repository (by absolute path) is still refused and undone.
- **Writes under `.git/`** (config, hooks, `info/exclude`) land in the
  clone's `.git/` and are never put back, because they never reached the
  repository. The git guard now has nothing to do unless the command
  wrote into the repository by absolute path.
- **Ignored files are absent**: a `.env`, a virtualenv, `node_modules`.
  An agent that needs them fails in the clone, and its failure is its own
  report. The checks trellis runs afterwards still run in the repository.
- **A relative `command[0]`** (`./agent.sh`) is resolved against the
  clone on Unix, where the child's working directory is applied first; on
  Windows it may resolve against trellis's own working directory (the
  repository). Both hold the same committed bytes, since a run starts from
  a clean tree.
- **Cost.** One clone per agent run (not per call, as for `bash`), plus
  the walk for new files.

Tests, in `crates/trellis-cli/tests/orchestration.rs`:
`isolated_agent_writes_to_trellis_and_git_in_the_clone_never_reach_the_repository`,
`isolated_agent_edit_new_file_and_deletion_are_carried_back`,
`isolated_agent_carried_back_change_is_undone_by_rollback`,
`isolated_agent_cannot_unignore_a_file_by_editing_gitignore_in_its_clone`
and
`isolated_agent_that_commits_in_its_clone_leaves_head_alone_and_its_change_comes_back`;
in `crates/trellis-loop/src/agent_clone.rs`,
`a_deletion_outside_allow_write_is_refused_and_the_file_kept` and
`a_directory_the_repository_ignores_is_not_entered_and_nothing_in_it_comes_back`.
The earlier tests of the git guard, the `HEAD` check and the ignored-file
snapshot for process agents now write into the repository by absolute
path (`in_repo`, `{ROOT}`), which is the writer those guards are still
for, as the `bash` tests did when `bash` moved into the clone.

## One test per writer (amendment, #54)

The owner's decision on #54 routes all four writers through this clone
with no switch to run them in the repository, which the sections above
already do. #54 adds one test per writer, in
`crates/trellis-loop/src/isolate.rs`, each running the same `.git/`
writes from where the writer stands (a planted file, `core.hooksPath`, a
branch, a tag, a `skip-worktree` flag), checking they landed in its clone,
and checking every byte under the repository's `.git/` is unchanged:
`isolated_writer_bash_git_dir_writes_do_not_reach_the_repository`,
`isolated_writer_custom_tool_git_dir_writes_do_not_reach_the_repository`,
`isolated_writer_hook_git_dir_writes_do_not_reach_the_repository` and
`isolated_writer_process_agent_git_dir_writes_do_not_reach_the_repository`.
The process-agent test spawns as `trellis-cli`'s `process_agent` does
(`AgentClone::confine`, then the key scrub, then `carry_back`). ADR 0023
§5 and ADR 0026 §11 now point here for the limits a clone leaves.
