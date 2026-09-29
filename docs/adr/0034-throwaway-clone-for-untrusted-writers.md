# ADR 0034 — A throwaway clone for the writers trellis does not control

**Status:** Accepted. First slice built: `bash` and custom tools. Hooks and
process agents are not yet isolated (see "What is not built").
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
- **Only edits come back.** New files and deletions stay in the clone, as
  do changes to links, file modes and submodules.
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

## What is not built

- **Hooks** (`pre_tool`, `post_tool`, `before_run`) still run in the
  repository. They run around every call, so a clone per hook per call is
  a cost to weigh separately, and `post_tool`'s view of the tree is part of
  its contract. Follow-up ticket.
- **Process agents** (ADR 0026) still run in the repository. They are
  started from `trellis-cli`, not the orchestrator, and their output is
  the run's change, so the carry-back there is the whole of their work.
  Follow-up ticket.
- **Containers, a micro-VM or an OS sandbox**, for network isolation and
  reads outside the repository. Later, per the owner's decision; ADR 0021
  rows 1.6 and 13.3 stay deferred for that step.
