# ADR 0023 — A composable tool set, a trust store, and executable hooks

**Status:** Accepted.
**Date:** 2026-09-27
**Reference design:** pi (`earendil-works/pi`, MIT): its built-in tools
(`read`, `write`, `edit`, `grep`, `find`, `ls`, `bash`), `--tools` and
`--exclude-tools`, `registerTool`, the `tool_call` / `tool_result` /
`before_agent_start` / `agent_end` extension events, and `project_trust`.
Borrowed as a design; no pi code is used. Row owner in ADR 0021: W-B.

## Context

The tool set was a closed enum. `ToolCall`, `ToolCallWire` and its derived
JSON Schema, the orchestrator's dispatch, the prompt's tool line and the
trail's action mapping each listed the same five tools, and the schema that
tier 2 ships to the runtime was `schemars::schema_for!(ToolCallWire)` with
nothing composed at run time. A repository could not add a tool, and a user
could not take one away.

ADR 0018 §3 declined executable hooks because a hook in a cloned repository
is code the repository supplied, and there was no trust store to put in front
of it. That precondition is what this ADR builds first.

## Decision

### 1. A tool set composed at run start

`ToolSet` (in `trellis-loop`) is the list of tools a run offers. It is built
once in `cmd_run` from `[tools]` and handed, as the same value, to the
prompt (through `RepoContext`), to the schema the adapter sends, to the
parser that reads a reply, and to the orchestrator, which refuses a call to a
tool the set does not contain. The orchestrator's refusal is the one that
binds; the others keep the model from asking.

- **Built-ins stay typed.** Each built-in is a `ToolCall` variant and a
  `ToolCallWire` variant, as before. A repository's own tools arrive as
  `ToolCall::Custom { name, args: serde_json::Value }`.
- **The schema is composed, not derived whole.** The branches of
  `ToolCallWire::schema()` are filtered to the tools the set enables, in their
  existing order, and one branch per custom tool is appended before `done`.
  A custom branch is the tool's declared object schema with
  `tool: { type: string, const: <name> }` added to `properties`, `tool` added
  to `required`, and `additionalProperties: false` forced, matching the shape
  schemars emits for the built-ins under `deny_unknown_fields`. The root
  keeps `x-guidance: { whitespace_flexible: false }`.
- **The composed schema is compiled by llguidance when the set is built**,
  before the snapshot. A custom schema llguidance cannot compile is a run that
  cannot start as configured and costs nothing.

  *Amended at implementation.* It is not. `trellis-model`'s `grammar`
  feature, which is what links llguidance, is off in the shipped binary on
  purpose (its `Cargo.toml` says why), and the serving runtime compiles the
  schema itself. What runs before the snapshot instead is a check that a
  custom schema uses only a keyword subset (`type`, `description`, `title`,
  `enum`, `const`, `default`, `items`, `properties`, `required`,
  `additionalProperties`, `minimum`, `maximum`, `minLength`, `maxLength`,
  `minItems`, `maxItems`) with known type names. A test compiles, with
  llguidance, a composed schema using every keyword in the subset, so the
  subset is what the grammar engine accepts rather than a guess about it.
- **The default set is the measured one**: `blast_radius`, `read_file`,
  `edit`, `run_checks`, `read_skill` (named in the prompt only when there are
  skills, as before) and `done`. With no `[tools]` section the tool line and
  the schema are byte-identical to what was measured. New built-ins are
  opt-in.
- **Selection.** `[tools] enable = [...]` turns on built-ins that are off by
  default; `[tools] disable = [...]` turns off any tool, custom ones included.
  `run_checks` cannot be disabled: it is the only way a `Verdict` enters the
  loop, so a run without it could never converge (ADR 0004). An unknown name
  in either list is an error, not a no-op. `bash` is not enabled through
  `enable`; it has its own switch (§3), so there is one place to look.

### 2. New read-only built-ins, and `write_file`

All paths go through the orchestrator's existing `resolve_in_repo` rule:
relative, no `..`, no absolute paths.

*Amended 2026-09-27: the rule is on the string, not on `is_absolute()`.*
On Windows `/x` and `\x` are rooted without being absolute and `C:x` is
relative to drive C's current directory, so all three passed and
`root.join` left the repository. `unsafe_repo_path` now refuses, on every
platform: a leading `/` or `\` (rooted, UNC, `\\?\`); any `:` (a drive
prefix, and on NTFS an alternate data stream, which git cannot see and a
rollback cannot remove; it also rules out git pathspec magic in `find`);
any component made only of dots and spaces other than `.`, compared after
the trailing dots and spaces Windows strips (`..`, `.. `, `...`); and any
`RootDir`, `Prefix` or `ParentDir` component as the host parses it.
Components split on both separators. The approval gate uses the same
function for the calls it lets through unasked and for the edit preview,
which read outside the repository on Windows before.

- `grep {pattern, path?}` shells out to `git grep -n -I --untracked -e
  <pattern> -- <path>`. No regex crate: git's own matcher, over tracked and
  untracked files, skipping ignored ones and binaries.
- `find {glob}` matches `git ls-files --cached --others --exclude-standard`
  against a hand-rolled glob: `*` within one path segment, `**` across
  segments, `?` one character. No brackets, no braces.
- `ls {path}` lists one directory, sorted, directories marked with `/`,
  `.git` omitted.
- `write_file {path, content}` creates a **new** file. An existing path is
  refused with a message that points to `edit`. It travels through the
  orchestrator, so governance `deny_write` (now matched for `write_file` as
  well as `edit`) and every gate apply. The path joins `touched`, so it is in
  the trail's `proposed` list and in the governance audit's scope. Rollback's
  `git clean -fd` removes it, which is only true for a path git does not
  ignore; so a path `git check-ignore` reports as ignored is refused, as is
  any path with a `.git` component. Creating a `.py` file reopens the index
  session (a full rebuild), because `Session::try_update` only knows files
  present at open, and a graph that disagrees with the tree is the failure
  the refusal in `apply_edit` exists to prevent.

  *Amended 2026-09-27:* "a `.git` component" is compared case-insensitively,
  after trailing dots and spaces are trimmed, and includes the 8.3 name
  `git~1`, on every component and for `edit` as well as `write_file`
  (`names_git_dir`). The exact-match check let `.GIT/hooks/pre-commit`
  create a git hook on a case-insensitive filesystem (observed on Windows),
  and `edit` had no check, so `.git/config` could be edited. Both survive a
  rollback, which is git's own reset.

  *Also amended 2026-09-27:* the ignore check fails closed. Only
  `git check-ignore` exiting 1 ("not ignored") lets the write proceed; a
  failure to start git, an exit of 128, or a signal refuses it with the
  reason, where it used to be read as "not ignored".

  *Amended after the fifth review of PR #1: reads, and `.trellis/`.*
  `read_file` and `ls` refused only paths leaving the root, so
  `read_file .git/config` put a remote URL, credential and all, into an
  observation, and from there into a hosted request body and the committed
  trail. `read_file`, `ls` and `grep`'s path now refuse `.git` by the same
  spellings as writes (`read_refusal`), and all of them also resolve the
  path through links: a committed link into `.git`, or out of the
  repository altogether, is refused. `ls` drops a `.git` entry by any
  spelling; `find` drops any listed path with a `.git` component; `grep`
  drops any output line whose path has one. git lists and searches neither
  its own directory nor a nested repository's, so those three filters are
  a second line, not the first. `edit` and `write_file` refuse `.trellis/`
  as well (§5, "Trusted code cannot be rewritten mid-run"). Evidence:
  `reads_and_listings_do_not_reach_git_s_own_directory` and
  `a_link_out_of_the_repository_is_neither_read_nor_written`, which failed
  with the fix's behaviour removed.

  *Amended 2026-10-11 (#611): a path git ignores is not read or listed.*
  `find` (`git ls-files --exclude-standard`), `grep` (`git grep
  --untracked`) and the throwaway clone already left ignored files out, so
  a git-ignored `.env` was invisible to four tools and readable by the
  fifth. `read_file` now asks `git check-ignore` (through `trellis_vcs`,
  without `--no-index`, so a tracked file is read whatever its name
  matches) and refuses an ignored path with the reason; `ls` asks once for
  the whole directory (`--stdin -z`) and leaves ignored entries out without
  saying so, because a note would itself name which ignored files exist.
  Both fail closed as `write_file`'s check does: a check that could not be
  made refuses. Git's rule, not a name list: a tracked `.env` is already in
  the history the model is shown. A narrowing of the rule above, so no new
  ADR. Evidence: `read_file_refuses_an_ignored_*` and
  `ls_leaves_ignored_entries_out_*`.

Output from `grep`, `bash` and custom tools is capped at the tail (8 KiB,
with a line saying how much was cut) before it becomes an observation;
`find` and `ls` name at most 500 paths and say how many they left out. The adapter's own tail window then applies as it does to every
observation.

*Amended after the fifth review of PR #1:* `find` reads git's file list
into a 64 MiB capture that keeps the tail. Past the cap it returned an
arbitrary subset whose first path was cut mid-name. It now refuses with
the list's size and no paths, and tells the model to start the glob with a
directory; the glob's leading literal directories are passed to `git
--literal-pathspecs ls-files` as a pathspec, so that advice shrinks what
git lists and not only what is shown. Evidence:
`a_file_list_larger_than_the_cap_is_refused_not_cut` in
`crates/trellis-loop/src/listing.rs` (with a small cap through
`find_capped`), which returned the cut subset before the change.

*Amended after the sixth review:* keeping that tail cost more than the
list. Each read past the cap moved every kept byte to drop the oldest, so
192 MiB of output through the 64 MiB cap took over 10 s, and a large
repository's list ran into the listing timeout instead of the refusal. The
capture is now a ring, which drops from the front without moving what
stays, and `find` asks for its output whole or not at all: past the cap
nothing more is stored and only the count goes on
(`a_stream_far_past_a_large_cap_is_read_in_linear_time`, about 0.3 s
after; `find_far_past_the_cap_is_refused_with_the_true_size_not_timed_out`
is a guard at a size a test can build).

### 3. `bash`, off by default

`[tools] bash = true` enables `bash {command}`. It runs with the root of a
throwaway clone of the repository as its working directory (ADR 0032; its
edits to tracked files are carried back), a timeout (`[tools] bash_timeout_s`, default
120), and `TRELLIS_RUN_ID` and `TRELLIS_MODEL` added to the environment.
*Amended 2026-10-05 (#317):* and `AI_AGENT=trellis`, the generic marker a
tool outside trellis reads to tell that an agent launched it; custom tools
and hooks get all three. Grounded checks do not get `AI_AGENT` from trellis:
a test suite that read it would behave differently under trellis than under
a person, and the Verdict would describe a run no person can reproduce (one
the person's own shell sets is passed through as before). It is not one of
work.rs's run markers (`RUN_MARKERS`), because other agents set it in a
person's own shell.
The shell is `[tools] shell` (an argv prefix, the command appended as the
last argument) or the platform default: `sh -c` on Unix; on Windows, Git's
`bash.exe` under `%ProgramFiles%\Git\bin` or an `sh.exe` on `PATH`, and
`cmd /C` when neither exists. The shell in use is printed on the run's
`tools:` line.

How a call is gated depends on the control dial (ADR 0018):

- **approve mode:** `bash` is added to the approve list automatically. Every
  command is shown to the person in full and runs only on an explicit yes.
- **auto mode** (which is `run --auto`, `work`, and any run whose config says
  `auto`): nobody is asked, so a `BashPolicyGate` decides. A command runs
  only if `[tools] bash_unattended = true`, or if it is a *simple* command
  that starts with an entry of `[tools] bash_allow` at a word boundary.
  Simple means it contains none of `; & | ` `` ` `` `$( < >` and no newline:
  `cargo test && rm -rf ~` starts with `cargo test`, and an allowlist that
  admitted it would be an allowlist of prefixes to arbitrary commands.
  Anything else is denied with a reason that names both settings.

**Rollback (ADR 0009) restores the worktree and nothing bash did outside
it.** A command that writes to `~`, installs a package, pushes a branch or
calls a network service has done so for good. A command's writes inside the
worktree are undone by rollback on a run that does not converge, but they
are not in `touched`: the trail's `proposed` list names only what `edit` and
`write_file` wrote, and on a converged run `git status` is the authority on
what changed. A command is finished when its process exits: its pipes are
read for two seconds more, and output still arriving after that (from
something it backgrounded, which inherited the pipe) is cut and the
observation says so. On timeout the command and what it started are
killed: on Unix it leads its own process group and `kill -KILL -- -<pgid>`
signals the group; on Windows `taskkill /T /F` walks the tree from it. A
process that left the group (`setsid`, a daemon) or, on Windows, whose
parent had already exited is not reached, and outlives the run. Killing
more than that would need `unsafe` platform calls, which this crate does
not make. A command that exits normally has its group killed the same
way on Unix, before its pipes are drained, so a backgrounded
`(sleep 1; touch x) &` cannot write after the call returns — after a
convergence judged the tree or a rollback restored it (amended after the
eighth review; before it, what a finished command left running was not
reaped, and a model-run `bash` could write after rollback). *Amended after
the ninth review:* a normal exit is observable only by reaping the
command, and once the leader is reaped and the group has no member left,
its id can be reused, so `kill -- -<pgid>` could reach a stranger's group.
The timeout path already signals the group before reaping the leader. For
the normal-exit path, right after the command is spawned a `cat` reading a
pipe trellis holds is started in its group (safe std `process_group`) and
is not reaped until after the group signal: alive or a zombie, it keeps
the id the command's group's. If it cannot be started, the post-exit
group kill is skipped and a warning is logged, the safe direction. Guarded
only: `what_a_command_backgrounded_cannot_write_after_the_call_returns`
(Unix-only, not run on this Windows host) still asserts the group is
killed; reuse of a group id cannot be staged in a test. On Windows it
is not: `taskkill /T` walks the tree from a live process, and after a
normal exit the command is gone, so what it started keeps running until it
ends on its own; a Job Object would hold the tree, and that needs `unsafe`
Win32 calls or a dependency. A server a command starts with `&` for a later
call to use is ended too, on Unix. Leading its own group has a cost on Unix: the child is no longer
in the terminal's foreground group, so Ctrl-C does not deliver SIGINT to a
running `bash` command, custom tool, hook or listing. It ends when trellis
drops it (`kill_on_drop`, SIGKILL) during the interrupt's shutdown, with no
chance to clean up. (Amended after the second review of PR #1: before it, the run
waited for EOF on both pipes, so `sleep 30 & exit 0` read as a timeout,
and a timeout killed only the shell.) *Amended by ADR 0035:* on Windows the
command is spawned into a Job Object with kill-on-close and no breakaway
(through the `process-wrap` dependency; no `unsafe` in this workspace),
and the job is terminated however the command ends, before its pipes are
drained. What it backgrounded is ended after a normal exit too, and
`taskkill /T` is no longer used here. ADR 0035 lists what a job still
does not hold (a process started for the command by a service, WMI or the
Task Scheduler).

Bash output is an observation. It is never a verdict, and an exit code of 0
means nothing to convergence: only `run_checks` produces a `Verdict`.

### 4. A trust store outside the repository

Anything a repository can make trellis execute beyond its checks (hooks,
custom tools, and the whole `[tools]` bash block, since `shell` is an argv
and `bash_unattended` hands execution to repository-steered text) is
loaded only if the user has trusted it.

- **Where.** `%APPDATA%\trellis\trust.json` on Windows; elsewhere
  `$XDG_CONFIG_HOME/trellis/trust.json`, or `~/.config/trellis/trust.json`.
  Resolved from environment variables, with no directory crate. Outside the
  repository, so a repository cannot trust itself.
- **Keyed by** the canonical repository root.
- **Records** one sha256 over every hook argv, every custom tool's name,
  description, command and schema (and its `timeout_s` when declared, #135),
  the `[tools]` bash settings, and the
  path and bytes of every file those argv reference (an argument that names
  an existing file inside the repository). Any change to any of them makes
  the repository untrusted again.
- **`trellis trust`** prints exactly what will execute (each argv, each
  referenced file with its digest, each custom tool's schema) and records
  trust. `--show` prints the same and the current state without recording.
  `--revoke` removes the entry.
- **Untrusted** means not loaded: no hook runs, no custom tool is offered,
  bash stays off, and the run's `repo:` line says `hooks: N present, not
  trusted (run trellis trust)` (with custom tools and bash counted the same
  way). A run is not refused for being untrusted; it runs without what it
  was not trusted to run.
- **Process agents** (*amended 2026-09-27*). A `.trellis/agents/*.toml`
  profile with a `command` (ADR 0026 §11) is an argv `trellis work` runs in
  the worktree. It is in the fingerprint as `process agent <name>`, with the
  files its argv names, and an untrusted one is not run: its task waits
  with `not trusted: run trellis trust`. The variables its `pass_env` hands
  it are part of the item and are printed as `receives: <NAME>, …`
  (*amended after the third review of PR #1*).
- **A repository with none of these never opens the store.**

*Amended 2026-09-27: what the digest covers, and what it does not.* Only
argv elements that were themselves file paths were hashed, so
`["sh", "-c", "./scripts/check.sh"]` or `["python", "-m", "tools.hook"]`
hashed nothing that runs, and the code could change after trust without
anyone being asked. Now the digest also covers:

- **Every file under `.trellis/`**, as one item shown by `trellis trust`,
  except `.trellis/runs/` (trails) and `.trellis/index.bin` (the index
  cache), which trellis writes on every run and would otherwise untrust
  the repository after each one. Added only when something would execute,
  so a repository that declares nothing still never opens the store. This
  includes `config.toml` itself, so any edit to it asks again.

  *Amended after the third review of PR #1: exact skips, and links
  followed.* The skip was implemented as every top-level name starting with
  `index.bin` and all of `runs/`. That left `.trellis/index.bin.py` and
  `.trellis/runs/gate.py` uncovered. A symbolic link to a directory was
  skipped silently, which left whatever it pointed at uncovered too. Now
  exactly three things are skipped:
  - `.trellis/index.bin`
  - `.trellis/.gitignore`, only while its bytes (ignoring line endings) are
    the file trellis writes, `trellis_index::cache::IGNORE`. A `.gitignore`
    a person edited is covered.
  - regular files named `*.json` directly under `.trellis/runs/`, which are
    the trails
  Everything else under `runs/` is hashed. Symbolic links are followed, and
  a directory reached twice is walked once. A link that resolves outside the
  repository, or to nothing, makes the repository untrustable:
  `trellis trust` prints the link and its target and records nothing, and
  a trust recorded earlier no longer matches. Following such a link could
  hash a whole disk, and skipping it would leave code uncovered. Evidence:
  `only_what_trellis_writes_is_left_out_of_the_dot_trellis_digest`,
  `a_symlinked_directory_under_dot_trellis_is_followed` and
  `a_symlink_under_dot_trellis_that_leaves_the_repository_refuses_trust`
  in `crates/trellis-loop/src/trust.rs`. The `.trellis/` item's shown
  text changed with this amendment, and that text is part of the digest.
  So **every trust recorded before it no longer matches**, and each
  repository needs `trellis trust` run again. That was unavoidable: the
  set of files covered changed too, since a `.gitignore` trellis wrote used
  to be hashed.

  *Amended after the fourth review of PR #1: real paths, three more
  refusals, bytecode, and a `Refused` state.*
  - **Recorded paths.** A directory reached through two links was recorded
    under whichever alias `read_dir` returned first, so the digest depended
    on directory order. Entries are now walked in name order. Each file is
    recorded once, under its real path relative to the repository, whichever
    alias reached it. The three skips above are decided on that real path,
    relative to the real `.trellis/`, so an alias cannot move a file into a
    skip or out of one. This changes the recorded path of every file reached
    through a link (`.trellis/hooks -> ../hooks` now records
    `hooks/gate.py`, not `.trellis/hooks/gate.py`). **A trust recorded
    before this amendment, for a repository with a link under `.trellis/`,
    no longer matches.** A repository with no links is unaffected.
  - **More links refused.** The walk now also refuses a link that resolves
    to `.trellis/` itself or to an ancestor of it (`.trellis/src -> ..`).
    That link resolved inside the repository, so it was followed, and the
    walk hashed the whole repository, `.git/` included, so every commit
    untrusted it again. A link resolving into `.git/` is refused too, since
    git rewrites those files on its own. So is a `.trellis` that is itself
    a link resolving outside the repository, to its root, or into `.git/`.
    Before, only links *under* `.trellis/` were checked.
  - **Bytecode is covered, and trellis does not write it.** The review
    asked for `__pycache__/` and `*.pyc`/`*.pyo` to be skipped, because
    running a Python hook under `.trellis/` wrote them and flipped trust to
    changed. They are not skipped. The request assumed bytecode is always
    derived from a hashed source, and it is not. `import gate` loads a
    sourceless `gate.pyc` when no `gate.py` exists. An unchecked-hash pyc
    (PEP 552) in `__pycache__` is loaded without being compared to its
    source. Either would be repository code outside the digest. The flip is
    removed at its source instead: every command trellis starts through
    `exec::run` (hooks, custom tools, `bash`, listings) and every process
    agent runs with `PYTHONDONTWRITEBYTECODE=1`. The same variable stops a
    hook's `__pycache__` in a path git does not ignore from voiding a pass
    through the per-call worktree digest (§5). Running a hook by hand,
    outside trellis, can still write bytecode and untrust the repository.
    That errs toward asking again. `[checks]` commands do not get the
    variable. They are the repository's own test environment and are not
    bracketed.

    *Amended after the fifth review of PR #1:* they do now, when
    `.trellis/` holds any `*.py` file (links followed, as the digest
    follows them). A test suite that imports code under `.trellis/` wrote
    `__pycache__` there and untrusted the repository, and once the trust
    recheck before every hook spawn landed (§5, "trusted code cannot be
    rewritten mid-run") that would have voided the run at the next hook:
    trellis's own checker breaking trust, not the model. Only then, because
    the variable has a cost: a suite run without it recompiles every module
    it imports on every check, and a check runs many times a run. With no
    Python under `.trellis/`, nothing a check imports can write there, and
    the suite keeps its bytecode cache. Set in `trellis_checks::run`, so
    `typecheck`, `tests` and the import check get it, in a model run and in
    a process agent's checks alike. Evidence:
    `a_check_runs_without_bytecode_writes_when_dot_trellis_holds_python`
    in `crates/trellis-checks/src/lib.rs` failed before the change;
    `a_check_keeps_bytecode_caching_when_dot_trellis_holds_no_python` is a
    guard on the cost side.

    *Amended after the sixth review:* the walk that answers "is there
    Python under `.trellis/`" was bounded by depth only, so two links back
    to `.trellis/` itself branched twice per level, 2^32 directory reads
    before it gave up, on every check spawn. It now keeps the set of
    canonical directories walked, as the trust walker does, and walks each
    once
    (`self_referencing_links_under_dot_trellis_end_the_walk_quickly`: with
    `.trellis/a -> .` and `.trellis/b -> .` it had not answered after 10 s
    before the change). A directory whose real path is outside the
    repository, or is the root or an ancestor of it, is not walked and
    answers yes: the variable costs a suite its cache, and a wrong no costs
    the repository its trust. The trust digest refuses such a link anyway
    (`a_link_out_of_the_repository_is_not_walked_and_answers_yes`).
  - **`TrustState::Refused { reason }`.** A fingerprint that cannot be
    trusted read `Changed` when a trust was on record and `Unknown` when not.
    Either way the `repo:` line, `work`'s waiting reason and `doctor` told
    the person to run `trellis trust`, and that command then refused. The
    state is now decided from the fingerprint before the store is read. Each
    of those three places prints the reason, which names the link.
  Evidence, in `crates/trellis-loop/src/trust.rs`:
  `a_file_reached_through_two_links_is_recorded_once_under_its_real_path`,
  `a_link_under_dot_trellis_to_itself_an_ancestor_or_the_git_directory_refuses_trust`
  and `a_refused_repository_reports_refused_not_changed_or_unknown` each
  failed before the change. `bytecode_under_dot_trellis_is_covered` is a
  guard. In `crates/trellis-loop/src/exec.rs`,
  `every_command_starts_with_python_bytecode_writes_off` failed before the
  change. In `crates/trellis-cli/src/tooling.rs`,
  `a_refused_repository_names_the_link_at_every_refusal_site_not_trellis_trust`
  covers the three places that print the reason.
- **Paths inside a code string.** The argument after `-c`, `-e` or `/C` is
  split on whitespace and shell punctuation, and every token that is a
  regular file in the repository is hashed like an argv path.
  *Amended after the tenth review of PR #1: every argument.* A list of
  flags that introduce code is a list of the ones not yet missed:
  `bash -lc`, `sh -ec`, `pwsh -Command` and `--file=x.py` all named a
  script no token was taken from. Now every argv element is split (on
  whitespace, `; & | ( ) < > =`, quotes trimmed), whatever precedes it,
  and every token naming a regular file in the repository is hashed.
  Splitting an argument that is not code only hashes files that exist,
  which can only make the digest move more often. A repository whose
  commands carry such a token reads as changed once after this and needs
  `trellis trust` again. Evidence:
  `a_script_named_in_any_argv_element_is_hashed_whatever_flag_precedes_it`
  in `crates/trellis-loop/src/trust.rs`, which failed before on
  `["bash", "-lc", "python scripts/gate.py"]` with no file hashed.

*Amended after the eleventh review of PR #1: a rule, not a better
tokenizer.* The review found scripts named inside larger expressions that
the tokens above still missed, and every round before it had found one
more spelling. So the tokenizer stops being the defence. **A hook or
custom tool whose argv, in any element or any token of one, names a
regular file in the repository outside `.trellis/` is refused**: `trellis
trust` prints the item and the file, says to move the code to
`.trellis/hooks/` (or `.trellis/tools/`) and name it there, and records
nothing; a trust recorded earlier reads as `Refused` wherever trust is
read (the run's `repo:` line, `work`, `doctor`), so nothing loads.
`["bash", "-lc", "python scripts/gate.py"]` is refused;
`["python", ".trellis/hooks/gate.py"]` is trusted. Programs on `PATH` and
absolute paths outside the repository are the user's system and are fine.
Everything under `.trellis/` is still hashed whole. Two narrowings,
deliberate: the rule is on regular files, not directories, since a
directory named as an argument (`ruff check src`) is data for the tool;
and it is decided on the path as written (`names_trellis_dir`), so a link
under `.trellis/` to a directory elsewhere is followed and hashed by the
digest rather than refused. Process agents (ADR 0026 §11) and the `bash`
shell argv are not under the rule: a process agent writes the whole tree
anyway, and the shell argv names an interpreter. A repository whose hooks
or custom tools name such a file needs them moved before it can be
trusted again. Evidence:
`a_hook_or_custom_tool_naming_code_outside_dot_trellis_is_refused` in
`crates/trellis-loop/src/trust.rs` (failed with the rule removed: the
fingerprint was not refused) and
`trust_refuses_a_hook_whose_code_is_outside_dot_trellis_and_says_where_to_move_it`
in `crates/trellis-cli/src/trust_cmd.rs`.

*Amended after the twelfth review of PR #1: the rule has its own
resolver.* It saw only the files the hashing accepted, and the hashing
drops a path containing `..` and an absolute path not lexically under the
root. So `["python", ".trellis/../scripts/gate.py"]`, or the file's
canonical absolute path (`\\?\C:\...` on Windows) against a root spelled
without that prefix, or an absolute path under a relative `--root`, was
neither hashed nor refused. Now every argv element and token is also made
canonical (joined to the root when relative), the root and `.trellis/`
are made canonical, and a regular file inside the real root and outside
the real `.trellis/` is refused. What is hashed is unchanged. This
replaces the second narrowing above for paths an argv names: a link under
`.trellis/` to a file or directory elsewhere in the repository, named
through the link (`.trellis/hooks/gate.py` with `.trellis/hooks ->
../scripts`), is refused, because the file a run can edit is the one it
resolves to and `edit` checks the path as written. Reached with no path in
the argv (`-m gate`), it is still followed and hashed by the digest.
Evidence: `code_outside_dot_trellis_is_refused_however_the_path_is_spelled`
(failed before on `.trellis/../scripts/gate.py` and on the canonical
absolute path: not refused) and
`a_link_under_dot_trellis_to_code_elsewhere_in_the_repository_is_refused_when_named`
(failed before: not refused), in `crates/trellis-loop/src/trust.rs`. On
Windows, `an_absolute_path_with_the_drive_letter_in_the_other_case_is_refused`
is a guard only: it passed before too, because `Path` compares a drive
letter without regard to case.

**Not covered, plainly:** code reached indirectly from outside `.trellis/`
that no argv token names as a file — a module imported by a hook
(`python -m tools.hook` resolving to `tools/hook.py`, or an `import app`
inside `.trellis/hooks/gate.py`), a directory run as a program
(`python pkg/`), a file a script reads or sources, a program on `PATH`.
The same holds for a repository path the tokens above do not separate: one
glued to a flag (`-fscripts/gate.py`, `--config:scripts/gate.py`) or
embedded in an expression (`exec(open(f'scripts/gate.py').read())`) is
neither hashed nor refused. It is the class `-m` and imports are in, and a
better tokenizer is not the fix (the eleventh amendment above). A
change there does not untrust the repository. **Keep hook and tool code,
and what it imports, under `.trellis/hooks/` or `.trellis/tools/`**, where
every byte is in the digest.

Not covered, and stated so it is not assumed: `[checks] typecheck` and
`tests` are repository-supplied argv that trellis has always executed
without trust. Gating them breaks every existing configuration and needs a
decision of its own.

### 5. Executable hooks

Declared in `.trellis/config.toml` as argv arrays, with no reliance on a
shebang, so a hook is `["python", ".trellis/hooks/pre.py"]` on every
platform. One per event. Each runs with the repository root as its working
directory (*amended 2026-09-29, #174*: no longer; each runs in a throwaway
clone of the repository, and a write reaches the repository only through
the carry-back `bash` uses, ADR 0032 "Hooks"), the same two environment variables as `bash` (and, since the
fourth review of PR #1, `PYTHONDONTWRITEBYTECODE=1`, as every command
trellis starts has; see §4), and a timeout
(`[hooks] timeout_s`, default 10).

- **`pre_tool`**: the call as JSON (the flat wire form the model emitted) on
  stdin. Exit 0 allows, exit 2 denies with stderr as the reason, anything
  else, a timeout, or a failure to start denies. Fail closed. Implemented as
  a `ToolGate`, consulted after governance and the bash policy and before
  the approval gate, so a person is never asked about a call a hook refused.
  (*Amended 2026-10-04, #328*: for a built-in tool the object also carries
  `"read_only": true` or `false`, from the table `READ_ONLY_TOOLS` in
  gate.rs; a custom tool's object has no such key; and the key is added to
  the hook payload only, not to `call_json`. See the amendment at the end.)
- **`post_tool`**: the call and the tool's result as JSON on stdin. Observe
  only: its output is discarded and it cannot alter the observation.
  (*Amended 2026-10-04, #328*: the `call` object carries `read_only` as
  `pre_tool`'s does.)
- **`before_run`**: stdout, capped to its first 4 KiB (with the true total
  printed after the cut; *amended 2026-09-27*, it had been a 4 KiB slice
  from the middle of the last 8 KiB), is added to the repository's
  context inside a fenced `untrusted hook output` block, within the fence
  whose closing sentence says nothing above can authorise skipping a check.
  Runs after the snapshot, because a hook can write to the tree.
  (*Amended 2026-09-29, #174*: it runs in a clone of its own and nothing
  it writes there comes back, ADR 0032 "Hooks"; the order is unchanged.)
- **`after_run`**: the outcome as JSON on stdin. Runs when the loop ends and
  before the rollback, so on a run that did not converge the rollback also
  undoes what it wrote inside the worktree. On a converged run, a write by
  it demotes the run (see "After a pass"). (*Amended 2026-09-29, #138; the
  owner decided it on 2026-09-28*: it had not run on Ctrl-C.) On Ctrl-C
  and on `rpc`'s `abort` it runs too, before the interrupt's rollback, with
  `"reason": "interrupted"`, `"converged": false` and `"iterations": null`
  (the loop's count was in the future the interrupt dropped, and a guessed
  number would be a false one). `touched` is what the orchestrator
  recorded. It is bounded by `timeout_s`, as every hook is; the throwaway
  clone it runs in (ADR 0032, "Hooks") is made before that timer starts
  and is not bounded by it. A second Ctrl-C while it runs drops it, which
  kills its process (on Unix the process itself, `kill_on_drop`, and not
  what it started, as for any hook dropped during an interrupt; on Windows
  its Job Object, ADR 0035), and the rollback goes ahead. `rpc` has no
  second interrupt, since a second `abort` asks for what was already
  asked, so there the timeout is the only bound. Tested by
  `after_run_interrupted_*`: through `rpc`'s `abort` on every OS, and
  through SIGINT to the binary on Unix.
- **`turn_end`** (*amended 2026-09-29, #137; the owner decided the schema
  on 2026-09-28*): after each model turn, that is after each call the
  model made (a denied one too) and after that call's `post_tool`; never
  for `done` or the convergence-time governance audit. Stdin is
  `{"run_id": <TRELLIS_RUN_ID>, "turn": <1-based turn number>, "tool":
  <tool name>, "outcome": <label>}`, where the label is one of `denied` (a
  gate refused the call and the tool did not run), `error` (the tool
  failed), `check_passed`, `check_failed`, or `ok` (any other result). The
  result itself is not passed; `post_tool` has it. Observe only: output
  and exit code are discarded, a failure is logged. The worktree is
  fingerprinted around it as around `post_tool`, so a write by it marks the
  turn as one that may have changed the tree and a pass on that turn stops
  counting (see "After a pass").
- **`input`** (*amended 2026-09-29, #137*): once, before the snapshot,
  with stdin `{"run_id": <TRELLIS_RUN_ID>, "task": <the task as given>}`.
  Exit 2 refuses the run with stderr (its first 500 bytes) as the reason:
  nothing is snapshotted and no trail is written. Every other end (exit 0,
  another code, a timeout, a failure to start) lets the run go on, and a
  non-zero one is printed as a `hook:` line. That is the opposite of
  `pre_tool`, deliberately: `input` is not a gate in front of a write, and
  a hook that crashes on every task would otherwise stop every run; exit 2
  is the one deliberate refusal. It can refuse a task and never rewrite
  one: its stdout is discarded. It runs in the repository like every hook,
  before the snapshot, so a write by it leaves the worktree dirty and the
  snapshot refuses the run (ADR 0009). (*Amended 2026-09-29, #174*: it
  runs in a clone of its own and nothing it writes comes back, because a
  write to an ignored file or under `.git/` here came before the guards
  that would have seen it; ADR 0032 "Hooks".)

Both are declared, trusted and timed out like the other four: argv arrays
in `[hooks]`, under `[hooks] timeout_s`, part of the trust digest (so
adding or changing either asks for `trellis trust` again), and re-checked
against trust before each spawn. A trust recorded before this amendment
still matches for a repository that declares neither: the digest is built
from the declared hooks only, and the four earlier events keep their order
in it. Evidence, in
`crates/trellis-loop/src/extension_tests.rs`:
`hook_turn_end_receives_run_id_turn_tool_and_outcome_after_every_turn`,
`hook_turn_end_that_writes_after_the_pass_marks_the_turn_and_prevents_convergence`,
`hook_input_exit_two_refuses_the_task_with_stderr_as_the_reason` and
`hook_input_other_exits_proceed`; in `crates/trellis-loop/src/hooks.rs`,
`turn_end_and_input_are_in_the_trust_digest`; and in
`crates/trellis-cli/src/tooling.rs`,
`hook_input_refusal_comes_before_the_snapshot_and_leaves_no_trail`.

A hook can refuse a call, and `input` can refuse a task. It cannot produce
a pass, cannot make a run converge, and cannot rewrite a call, a task or an
observation (ADR 0004, ADR 0005).

#### After a pass (amended 2026-09-27)

A hook is repository code and nothing stops it writing in the worktree.
Three of them run at points where a write would sit after the last check:
`post_tool` after every call (the passing check included), `pre_tool`
before every call, and `after_run` after the loop has converged. Before
this amendment a hook that wrote at any of those points left bytes in a
"converged" change that no check saw. Now:

- **The worktree fingerprint.** `trellis_vcs::worktree_fingerprint` is the
  tree id `git add -A && git write-tree` would give, computed in a copy of
  the index so the user's is untouched. It covers the content of every
  tracked and untracked file git does not ignore; `git status --porcelain`
  would not, since a rewritten untracked file keeps its `??` line.
  **Ignored files are not covered**, as they are not part of any change
  trellis proposes, and a hook that writes one is not detected.

  *Amended after the seventh review of PR #1: ignored files and the
  rollback.* A model run whose tool set includes `bash` or a custom tool,
  or that has hooks, lists every ignored file with its stat right after
  the snapshot (before `before_run`), the same listing a process agent
  gets (ADR 0026 §11; the code moved to `trellis_vcs::ignored` so both
  share it). Those are the only writers in a model run that can reach an
  ignored path: `edit` and `write_file` go through the orchestrator,
  which refuses one. On every rollback (not converged, interrupted, a
  panic) the ignored files the run created are removed first, then any
  directory that left empty, then the snapshot's own rollback runs; one
  that existed before and was changed or deleted is named as not put back,
  since no copy is kept. A created file that cannot be removed, or a
  listing that fails after the run, makes the tree not restored: the
  rollback cannot say what it left. A converged run keeps them and says
  so, since `git status` does not show them and a later revert does not
  undo them. Before this, `bash` could leave `.venv/.../sitecustomize.py`
  behind a rollback (`git clean -fd` leaves ignored files) for the next
  check that starts the repository's Python. A run with none of these
  writers is not listed, and pays nothing. Evidence:
  `tests/cli.rs::a_model_run_s_bash_writes_to_ignored_paths_are_removed_on_rollback`,
  which failed before the change with the file still in place after
  "NOT CONVERGED". What is still not covered: a converged run's ignored
  writes are reported, not judged — the breach test and `deny_write` a
  process agent applies to them are not applied to a model run's — and
  the checks' own ignored writes (a `.pytest_cache`) are reported and
  removed with the run's.

  *Amended after the eighth review of PR #1: `.git/`, and what "restored"
  means.*
  - **`.git/` is guarded in a model run too.** The same writers can set
    `core.fsmonitor`, point `core.hooksPath` elsewhere, or plant a hook,
    none of which `git status` shows or the rollback touches, and the
    next git command trellis runs (the ignored listing, the rollback's
    reset, the trail's commit) would execute it. The process agent's
    guard moved to `trellis_vcs::git_guard` (what it covers is in ADR
    0026 §11) and is captured beside the ignored listing. It is put back
    before any git command once the loop ends: first on every rollback,
    and on a converged run a change there demotes the run to not
    converged (`NotConverged::GitDirChanged`), since it would run at the
    reviewer's next `git status` and a diff does not show it. A file that
    cannot be put back makes the rollback not restored. Evidence:
    `a_model_run_s_bash_writes_to_git_config_and_hooks_are_put_back_on_rollback`
    and `a_model_run_that_changed_git_config_does_not_converge` in
    `tests/cli.rs`, which failed before with `core.fsmonitor = ./evil.sh`
    left in the config (the second converged). Not covered, stated: the
    git commands trellis runs *during* the run (a post_tool fingerprint's
    `git add` into a copied index, `git check-ignore`) can meet a
    `core.fsmonitor` that `bash` set a call earlier. That is code the
    writer could already run directly, so it adds no reach; what the
    guard prevents is it outliving the run.

    *Amended after the ninth review of PR #1: trellis's own git runs
    nothing planted.* "No reach" was wrong in one respect: what trellis
    runs, it runs at a time the writer did not choose, with trellis's
    judgement waiting on the answer. Every `git` trellis runs now carries
    `-c core.fsmonitor= -c core.hooksPath=/dev/null` (`NUL` on
    Windows) on its command line (`trellis_vcs::command`), which outranks
    every config file, include and worktree config. The hooks override is
    load-bearing: `git add -A` into the fingerprint's copied index writes
    an index, and an index write runs `post-index-change`. The worktree
    digest and fingerprint, which run between tool calls, also turn off
    every filter driver the configuration defines: the drivers are listed
    with `git config --get-regexp` (which reads and runs nothing) at every
    call, and each gets an empty `clean`, `smudge` and `process` and
    `required=false`, since git offers no "no filters" switch for `status`
    or `add`. Both sides of a comparison then hash raw bytes, so equality
    means what it did. Evidence:
    `a_core_fsmonitor_bash_sets_is_never_run_by_trellis_during_or_after_the_run`
    in `tests/cli.rs` (a `post_tool` hook makes the digest run after the
    `bash` call; it failed before with the marker written; its control
    runs plain `git status` and sees the marker), and
    `trellis_s_own_git_runs_no_planted_fsmonitor_hook_or_filter` in
    `trellis-vcs`, which failed before with all three markers (fsmonitor,
    `post-index-change`, a `required` clean filter) and checks with plain
    git that each is live. What is not neutralised, stated:
    - **The trail's commit keeps the user's hooks** (`pre-commit` and the
      rest, `core.fsmonitor` still off): a commit in their repository runs
      their hooks, and it runs after the guard has put `.git/` back.
    - **Filters stay on outside the digest**: the snapshot's and the
      rollback's `status`, `reset --hard` and `clean`, a process agent's
      `status`, and `pending`. A rollback's checkout needs the user's own
      drivers (Git LFS's smudge), and each of these runs after the guard
      has restored the config that could define a planted driver. A driver
      defined outside the git directory (the global config, an include
      target elsewhere on disk) is not the guard's to restore.
    - **A submodule's own configuration**: `git status` recurses into a
      submodule with that repository's config, and whether the `-c`
      overrides reach it depends on git's handling of
      `GIT_CONFIG_PARAMETERS` for submodules; it is counted as not covered,
      as the submodule's git directory is by the guard.
    - Settings that run commands only in commands trellis does not run
      during a run (`diff.external`, textconv, `core.pager`,
      `core.sshCommand`, credential helpers) are not overridden.
      *[2026-10-11, security finding F1 in AUDIT-2026-10-11: the premise
      was false for `core.sshCommand`, and for `remote.<r>.uploadpack`,
      which this list left out. In a partial clone, a git command that
      reads an object the store lacks fetches it from the promisor remote
      `.git/config` names, through that remote's transport. The Atlas's
      `cat-file --batch`, `diff HEAD` and the rollback's `reset --hard`
      each ran a planted command that way, under the overrides above. Every
      git trellis runs now carries `GIT_NO_LAZY_FETCH=1`
      (`trellis_vcs::ENV`), so a read never fetches, and on a git that
      honours the variable these settings again run only in commands
      trellis does not run. See "As built".]*
    - A partition worktree's `git worktree add` runs no `post-checkout`
      hook now; its checkout still applies the user's filters.

    *Amended after the tenth review of PR #1: "every" was not every.* The
    `grep` and `find` tools built their own argv starting `git` and ran
    it without the overrides, so a `core.fsmonitor` that `bash` planted
    ran at the model's next listing. Every git the workspace starts outside
    tests now comes from one place: `trellis_vcs::command` (or
    `command_with_hooks` for the trail's commit and the guard's hooks
    path), `tokio::process::Command::from(command())` for an async one, and
    `trellis_vcs::git_argv` for `exec::run`'s argv. The one copy is
    `chat::git_command`, since the `chat` crate depends on nothing in the
    workspace (as its key scrub already is a copy). A source scan,
    `every_git_spawn_goes_through_trellis_vcs` in
    `crates/trellis-cli/tests/hygiene.rs`, fails on any non-test line
    naming `"git"` outside those; run against the sources before this
    change it named all four sites (`listing.rs` twice, the orchestrator's
    `check-ignore`, `chat`'s status). Evidence for the behaviour:
    `grep_and_find_run_no_planted_fsmonitor` in `trellis-loop`'s
    `listing.rs`, which failed before with the marker written, and whose
    control runs plain `git status` and sees it.
    *[2026-09-29, #16: `crates/chat` was deleted (Q-19), so
    `chat::git_command` and the test tying its copy of the overrides to
    `trellis_vcs` are gone; every `git` spawn now comes from
    `trellis_vcs`. The crate is recoverable from git history.]*
  - **A changed pre-existing ignored file is a tree not restored.** The
    rollback said "NOT put back" and exited 1 as restored; it exits 2
    now (`a_model_run_s_bash_writes_to_ignored_paths_are_removed_on_rollback`
    asserts it, and failed before with exit 1). So that the checks'
    rewrites of their own caches do not make every failed run exit 2,
    the listing became a ledger (`IgnoredLedger`): the orchestrator
    lists ignored files before and after every typecheck, test and
    import check, and only changes made outside those brackets — by
    `bash`, custom tools, hooks — are held against the run. Created
    files are all removed on rollback, whoever made them, as before.
    Evidence: `a_check_s_own_write_to_an_ignored_file_does_not_make_a_rollback_unrestored`,
    which before this change printed "NOT put back" for a file only a
    check had changed. What that gives up, stated: a check runs
    repository code the run may have edited, so a rewritten test can
    change a pre-existing ignored file unattributed; a run with none of
    these writers keeps no ledger and was already exposed to exactly
    that. Cost: two ignored listings per check, only in runs that load
    one of these writers.

    *Amended after the ninth review of PR #1: reverted to one listing.*
    The ledger answered the wrong question. `bash` running `pytest`
    rewrites the same caches a check does, and a process agent that runs
    the tests is one process tree, so a changed pre-existing ignored file
    was nearly always a cache, and counting it ended failed runs as not
    restored (exit 2, `work` stopped). The rollback's promise (ADR 0009) is
    the tracked tree, which it keeps. Now a pre-existing ignored file that
    was changed is named ("left as it is") and does not count, whoever
    changed it; a created one is removed, as before; and the tree is not
    restored when a created file cannot be removed, when a pre-existing one
    was deleted (a loss, not a rewrite), or when a changed one is under
    `.trellis/` or named by `deny_write`, which the run was refused anyway.
    The ledger, its per-check brackets in the orchestrator and the
    orchestrator's handle on it are removed; the run takes one listing
    before and one after, as a process agent does. That dissolves two
    findings of the same review: the bracket was not closed when a check
    was cancelled mid-way (Ctrl-C, abort, panic), which held the check's
    writes against the run, and it listed ignored files synchronously on
    the async runtime under a std mutex, twice per check. Evidence:
    `a_model_run_s_bash_writes_to_ignored_paths_are_removed_on_rollback`
    (exits 1 now; it failed before on the old "NOT put back" wording, with
    exit 2). Guards only, never seen failing:
    `a_model_run_that_changed_an_ignored_file_deny_write_names_is_not_restored`
    in `tests/cli.rs`, which passed on the old code too (it holds the
    strict path), and in `trellis_vcs::ignored`,
    `a_deleted_file_or_a_changed_protected_one_is_not_restored`.
    `a_check_s_own_write_to_an_ignored_file_does_not_make_a_rollback_unrestored`
    stays as the guard. What this gives up, stated: a writer that changes
    a pre-existing ignored file nothing protects (`.venv/.../site.py`) is
    reported and the run ends as restored; a repository that wants such a
    change to count says `deny_write = [".venv/"]`. A check that deletes a
    pre-existing ignored file (mypy removing a stale cache entry) now makes
    a failed run exit 2, where the ledger had hidden it: the direction a
    false alarm may err in.
  - *Amended after the tenth review of PR #1: HEAD may not move.* The same
    writers can commit, reset or check out. A committed change is not in
    `git status`, so it was not in the diff left for review, the write
    rules never judged it, and a converged run left it on the user's
    branch. After `.git/` is put back, a converged run whose HEAD is not
    the snapshot's commit on the snapshot's branch is demoted to
    `NotConverged::HeadMoved` and rolled back. The check runs for every
    model run, not only those loading these writers: one without them
    cannot move HEAD, and the check costs it two `rev-parse` calls. The
    rollback now puts HEAD back on the recorded branch before its reset
    (ADR 0009, amended), and says so. Evidence:
    `a_model_run_whose_bash_commits_does_not_converge_and_the_commit_is_undone`
    in `tests/cli.rs`, which failed before with "CONVERGED after 2
    iterations". The process-agent rule is ADR 0026 §11.
- **`pre_tool` and `post_tool`.** Each declared hook is bracketed on its
  own: the worktree is fingerprinted before and after the gates (which
  hold `pre_tool`; the other gates do not write), and before and after
  `post_tool`. A difference, or a fingerprint that could not be taken,
  marks the turn as one that may have changed the tree, and
  `last_check_passed` stops there (ADR 0004). One flag cannot say which
  side of a check a hook wrote on, so a hook that writes on a check's own
  turn voids that check. The tool itself is not bracketed: whether it may
  have changed the tree is decided by its kind (`edit` and `write_file`
  when they wrote, `bash` and custom tools always). *Amended after the
  second review of PR #1:* the first version bracketed the whole dispatch,
  so a checker that left an untracked, non-ignored file (`.coverage`,
  junit XML, `__pycache__`) voided its own pass whenever any hook was
  declared. Cost: two git invocations per declared hook per call.

  *Amended after the third review of PR #1: the per-call bracket writes no
  objects and does not block the runtime.* Up to four fingerprints per call
  ran `git add -A` and `git write-tree` on the async runtime's thread. Each
  one wrote a loose blob for every untracked file, and a tree. The bracket
  now uses `trellis_vcs::worktree_digest`, which is computed from:
  - one `git --no-optional-locks status --porcelain=v2
    --untracked-files=all`
  - `git hash-object --stdin-paths` without `-w`, over the paths that
    differ from `HEAD`
  The digest is a manifest: `HEAD`'s id, then the worktree mode and blob id
  of every path whose bytes differ from `HEAD`. It is compared for equality
  and nothing else. It runs through `spawn_blocking`, and only around a hook
  that is declared for that side, which the previous amendment already did.
  It sees what the tree form sees, with these differences:
  - It also moves when `HEAD` moves. A commit that leaves the worktree
    unchanged counts as a change, which fails closed.
  - An untracked file's mode comes from the filesystem, so on Windows a
    mode-only change to an untracked file is not seen.
  - A symlink is hashed by its link text, as git stores it, through `git
    hash-object --stdin` (on Windows with separators written `/`).
  - An untracked nested repository (`status` names it `dir/`) is recorded
    as `160000 <its HEAD> dir/`, the gitlink `git add -A` would stage, or
    `(unborn)` before its first commit. An uncommitted edit inside it is
    not seen, and was not seen by the tree form either.
  - A submodule entry is recorded as its status line.
  - A path containing a newline falls back to the tree form, objects
    included.
  Staging and unstaging alone change neither form. `after_run` still uses
  the tree form, once per run, because naming the changed paths
  (`tree_diff`) needs the objects. Evidence:
  `the_per_call_digest_writes_no_objects_and_sees_every_worktree_change`
  in `crates/trellis-vcs/src/lib.rs`. It saw three loose objects written
  by the tree form, and sees none from the digest.

  *Amended after the fourth review of PR #1: nested repositories and
  symlinks.* `hash-object --stdin-paths` reads through a path. It failed on
  an untracked nested repository and on a symlink to a directory or to
  nothing. The failed digest counted as a change, so with any hook declared
  every pass was voided and such a repository could never converge. The two
  bullets above replaced "a symlink is hashed through its target". Evidence:
  `an_untracked_nested_repository_is_digested_by_its_head_not_hashed_as_a_file`
  and `an_untracked_symlink_is_digested_by_its_link_text`, both of which
  failed with `fatal: Unable to hash` before the change.

  *Amended after the fifth review of PR #1:* one `git hash-object --stdin`
  per symlink was a process per link per bracket. The link texts are now
  written to files in a fresh directory under the system temp dir (never
  the worktree) and hashed in one `git hash-object --no-filters
  --stdin-paths`, run in the repository so the object format is its own;
  the directory is removed however the call ends. Computing the ids in
  process would need SHA-1 (not a dependency) and would be wrong for a
  SHA-256 repository. The ids are unchanged: guarded by
  `links_hashed_in_one_batch_get_the_ids_one_process_each_gave`, which
  compares the batch with the per-link call on a link to a directory, to a
  file and to nothing; a cost change, so no test failed before it.
- **`after_run`.** The binary fingerprints the worktree around the hook on
  a converged run. A difference, or a fingerprint that could not be taken,
  demotes the run to `NotConverged(AfterRunChangedTree)`: it is reported,
  the rollback removes the hook's bytes together with the run's, and the
  trail records a run that did not converge. Chosen over moving `after_run`
  to after the trail, which would still have needed the same guard. The
  outcome JSON the hook was given said "converged"; the hook is told what
  the loop decided, and the demotion is decided after it, from what it did.

Hooks observe, gate, and add context. They do not write in the worktree,
and a repository whose hooks do cannot converge.

#### Trusted code cannot be rewritten mid-run (amended after the fifth review of PR #1)

Trust was decided once, before the snapshot, and hooks re-read their
scripts on every spawn. So `edit .trellis/hooks/gate.py` could switch off
the `pre_tool` gate or run model-written code under the person's trust for
the rest of the run, and `bash`, a custom tool or a hook could do the same
by any route. **The rule, one for every kind of agent: a run may not change
what judges it.** `.trellis/` holds the hooks, the tools, the rules, the
check configuration and the process-agent profiles.

- **`edit` and `write_file` refuse any path under `.trellis/`**, before the
  file is read, with a message that names why. "Under `.trellis/`" is
  `names_trellis_dir`: the first component (after any `.`) compared
  case-insensitively, after the trailing dots and spaces Windows strips,
  and including the 8.3 short name `TRELLI~n` Windows gives a dot-leading
  name — the `.git` rule's spellings, on the root's directory only, since a
  nested `app/.trellis/` is not the one trellis reads. The path is also
  resolved through links: the nearest existing ancestor is canonicalized,
  so `alias -> .trellis` does not get through, and the same check refuses
  `.git` through a link. The approval gate's prediction of these refusals
  (`preview_edit`, and its `write_file` branch) calls the same function,
  `write_refusal`, so a call it lets through unasked is one the
  orchestrator refuses. *Amended after the sixth review:* the `write_file`
  branch did not predict the one refusal left, a path git ignores, so a
  person was asked to approve a write that was then refused. Both now ask
  `ignored_refusal`, one `git check-ignore` that fails closed the same way
  in each, and only after the path rules and the existing-file check pass
  (`a_write_file_git_ignores_is_not_put_to_the_human`). The cost is one
  `git` process per `write_file` the gate would otherwise have asked about.
- **What cannot be path-checked is re-checked by fingerprint.** When the
  repository is trusted, `tooling::prepare` builds one `TrustGuard` over the
  fingerprint it trusted, from the same configuration inputs, and hands it
  to the hooks and to the orchestrator. It recomputes the fingerprint
  (every file under `.trellis/`, every file an argv names, the settings):
  - before every hook spawn (`pre_tool`, `post_tool`, `before_run`,
    `after_run`) and every custom-tool spawn; a difference refuses the
    spawn;
  - after every call that may have changed the tree (an applied edit, a
    created file, `bash`, a custom tool, a hook that wrote), so a `bash`
    command that changes `.trellis/` ends the run even with no hook
    declared;
  - and once more when a run converges, before `after_run`: a check is not
    a tree-changing call, and a checker can still write under `.trellis/`
    (a coverage file configured there). A difference demotes the run to
    `TrustChanged` and it is rolled back. Evidence:
    `a_check_that_writes_under_dot_trellis_does_not_converge` in
    `crates/trellis-cli/src/tooling.rs`, which converged before.
  The first failure is latched, shared by every clone, and the run ends on
  that call with `NotConverged::TrustChanged`, whose explanation names the
  files whose hashes changed. The run is rolled back like any other that
  did not converge. Restoring the file does not clear the latch: a hook
  that ran rewritten, or was refused, is already part of this run.
- **A hook script outside `.trellis/`** (`["python", "scripts/gate.py"]`)
  is refused at trust time since the eleventh review (§4): the repository
  cannot be trusted until it moves under `.trellis/`, where `edit` refuses
  it. Before that it was trusted, `edit scripts/gate.py` was an ordinary
  edit, and only the guard's next recheck saw it. Code such a hook reaches
  by import outside `.trellis/` is still an ordinary file (§4, "not
  covered").
- **Process agents** (ADR 0026 §11) cannot be refused up front, since their
  command writes directly. Their run already failed to converge when git
  reported any changed path under `.trellis/`; that test now uses
  `names_trellis_dir` too, so both kinds of run share one predicate.
- **Cost.** One fingerprint per hook spawn, per custom-tool spawn, and per
  tree-changing call: a hash of every file under `.trellis/` and every
  argv-named file. Only in a trusted run.

  *Amended after the sixth review of PR #1.* That hash ran synchronously,
  under the guard's lock, on the run's single-threaded runtime, up to three
  times per call, so a large `.trellis/` held Ctrl-C, `rpc`'s `abort` and
  approval prompts until it finished. Two changes:
  - **A stat-keyed cache.** The guard keeps, per real path, the digest and
    the stat it was taken under: length and modification time, plus the
    device, inode and change time on Unix and the creation time on
    Windows. A recheck still walks `.trellis/` in full and still resolves
    every argv, so a file added or removed is always seen; it re-reads and
    re-hashes only a file whose stat differs. The stat is taken before the
    read, so a write landing between them leaves an older stamp and the
    next recheck hashes again. The trust decision itself (`trellis trust`,
    the state before a run, `require_trusted`) never uses the cache.
  - **What the cache can miss, and where that is closed.** A rewrite that
    keeps a file's length and sets its modification time back: on Unix the
    change time, which no unprivileged process can set, still differs; on
    Windows, where std exposes no change time, a cached recheck can pass
    it. So the recheck when a run converges (above, before `after_run`) is
    a **full** one, `check_full`: every file hashed afresh, the fresh
    digests becoming the cache. A run cannot converge on a rewrite the
    cache missed. What remains, stated: on Windows, a hook or custom tool
    spawned between such a rewrite and convergence runs the rewritten code;
    the run then fails at convergence and is rolled back, but the spawn has
    happened. Evidence: `a_rewrite_keeping_length_and_mtime_is_caught_by_the_full_check`
    (a guard: it tests the full check the convergence site calls) and
    `a_cached_recheck_still_sees_a_rewrite_and_an_added_file`.
  - **Off the runtime.** The orchestrator's and the hooks' rechecks, and
    the one at convergence, run on tokio's blocking pool
    (`check_async`, `check_full_async`), so the runtime keeps serving
    Ctrl-C, `abort` and approvals meanwhile. A recheck that panics there
    is latched as a breach. Evidence:
    `a_trust_recheck_leaves_the_runtime_free`, in which a 50 ms timer on
    the same current-thread runtime fired after 453 ms when the check was
    called the old, synchronous way.
- **Untrusted runs** load no hook, custom tool or `bash`, so `edit` and
  `write_file` are their only writers, and those refuse `.trellis/`.

A grounded check that imports `.trellis/` code must not break this by
writing bytecode there; see §4's amendment on `[checks]` commands.

Evidence, in `crates/trellis-loop/src/extension_tests.rs`:
`neither_edit_nor_write_file_can_reach_dot_trellis_by_any_spelling`,
`a_bash_command_that_rewrites_a_hook_stops_the_next_hook_and_the_run`,
`a_bash_command_that_changes_dot_trellis_ends_the_run_with_no_hook_declared`
and `a_custom_tool_does_not_start_once_trusted_code_has_changed`; and in
`crates/trellis-cli/src/tooling.rs`,
`a_hook_rewritten_mid_run_is_not_run_and_the_run_is_rolled_back`, which
drives `cmd_run` and fails when the orchestrator is not given the guard.
Each failed with its fix's behaviour removed.

#### What rollback guarantees when trellis is not the only writer

*Added after the eleventh review of PR #1.* ADR 0009's rollback rests on
one premise: during a run the orchestrator is the only writer (ADR 0005).
With `edit` and `write_file` as the only writers that premise holds, and
the rollback's guarantee is exact. When `bash`, a custom tool, a hook or a
process agent was enabled, it did not: each was an unsandboxed process
running in the repository with the user's authority, with write access to
the whole worktree and to `.git/`, and for them the rollback was
best-effort against git's view of the tree.

*Amended 2026-09-30, #54:* that caveat no longer holds for these four
writers. Each runs in a throwaway clone (ADR 0032), and no configuration
runs it in the repository instead; what it changes there reaches the
repository only through a carry-back under the refusals every writer
meets. For what such a writer does where it runs, `.git/` included, the
rollback's guarantee is exact, as for `edit` and `write_file`. What a
clone does not stop is stated once, in ADR 0032, "Limits": no network
isolation, reads outside the clone, writes that reach the repository by
absolute path, and processes that outlive the call. The list below is
kept as the record of what the review rounds found; it now holds only
for a writer that reaches the repository by absolute path. Evidence: the
four `isolated_writer_*` tests in `crates/trellis-loop/src/isolate.rs`,
one per writer, each showing a planted file, `core.hooksPath`, a branch,
a tag and a `skip-worktree` flag written from inside it leave every byte
under the repository's `.git/` as it was.

This is a limit, not a defect awaiting another guard. Eleven review rounds
have closed one route after another with a post-hoc git check, and each
round has found another, because the checks read state the writer can
also write. The known ways around it, from rounds 5 to 11, stated so they
are not assumed closed:

- **Backgrounded processes on Windows.** After a normal exit
  `taskkill /T` has no live process to walk the tree from, so what the
  command started keeps running and can write after the comparison and
  the rollback (ADR 0023 §3; ADR 0026 §11 step 2). On Unix a process that leaves
  its group (`setsid`) is not reached, and a process agent runs in
  trellis's own group, so nothing is killed after its normal exit.
  *Amended by ADR 0035:* on Windows, commands and process agents now run
  in a Job Object that is terminated however they end, so this route is
  closed there except for a process started on the command's behalf by
  something outside the job (a service, WMI, the Task Scheduler). The Unix
  half is unchanged.
- **`.git/` writes outside the guarded files.** The git guard restores
  `config`, `config.worktree`, include targets under the git directory,
  the hooks, `info/exclude` and `info/attributes`. The rest of `.git/`
  (refs, `packed-refs`, objects, the index, `.git/modules/`) is not
  compared or restored, nor is a change made and reverted before the
  command exits.
- **Ignored-path tricks, `.gitignore` edits included.** `git clean -fd`
  never removes an ignored file, and what is ignored is decided by files
  the writer can edit: a tracked `.gitignore`, or a new one in a
  directory it creates. The before/after listing of ignored paths
  (`trellis_vcs::ignored`) narrows this, but it reads the same view of the
  tree the writer shaped.
- **`skip-worktree` and `assume-unchanged`.** `git update-index
  --skip-worktree` or `--assume-unchanged` on a tracked file hides an edit
  to it from `git status` and `git diff`, and so from every judgment made
  from the changed paths, and from what the rollback's reset is relied on
  to restore.
- **Refs other than `HEAD`.** Only `HEAD` and the branch it was on are
  recorded (ADR 0009). A branch the writer creates, another branch it
  moves (`git branch -f`), a tag, a stash entry or a note is left as it
  is.
- **Pre-existing ignored files changed.** An ignored file that existed
  before the run (`.env`, a virtualenv, build output, a cache) and that a
  writer modifies is neither restored by the reset nor removed by the
  clean. The ignored-file listing removes new ignored paths and notices a
  changed one by its stat (a rewrite that keeps length and modification
  time passes on Windows), but holds no copy to restore it from.
- **TOCTOU in restore.** Each comparison and each restore is a separate
  step. A process still running (the first item) can write between the
  check and the restore, or after both.
- **Code reached by import outside `.trellis/`.** Trust covers every byte
  under `.trellis/` and refuses hook and tool code named outside it (ADR
  0023 §4, amended after the eleventh review), but code a hook or tool imports from
  elsewhere in the repository is an ordinary file a run may edit.

**What holds.** For trellis's own edits (`edit`, `write_file`), and for
the edits a `bash` or custom-tool command or a hook makes inside its
clone, and the edits, new files and deletions a process agent makes
inside its own, that are carried back (ADR 0032; hooks and process
agents *amended 2026-09-29*, #174 and #175), the rollback's guarantee
is exact, as ADR 0009 states. A run with none of
these writers enabled (the default: `bash` off, and an untrusted
repository loads no hook, custom tool or process agent) keeps it.

**The real fix is isolation**, and the first step is built (ADR 0032,
*amended 2026-09-29*): `bash` and custom tools run in a throwaway `git
clone` of the repository (copy-in/diff-out), and only their edits to files
trellis tracks come back, through the orchestrator's write path, where the
rollback covers them exactly. Against a `bash` or custom-tool command that
writes where it runs, every item in the list above is closed: its
`.gitignore` edit, index flags, `.git/` writes, refs and ignored files stay
in the clone, and what it started writes into a directory that is removed.
*Amended 2026-09-29, #174 and #175:* hooks and process agents run in
clones too (ADR 0032, "Hooks" and "Process agents"). A hook's edits
come back through the same write path; a process agent's edits, new
files and deletions come back through its own carry-back, under the
same refusals, where the rollback covers them exactly. The list still
holds, in full, for any writer that reaches the repository by
absolute path, which a clone does not stop (ADR 0032, "Limits": no network
isolation, reads and writes outside the clone still possible). The guards
above are kept for those writers. Containers or an OS sandbox, for network
and home-directory isolation, remain deferred (ADR 0021, 1.6 and 13.3).
`trellis trust` prints one line stating the limit when it records trust for
any of these writers.

### 6. Custom command tools

```toml
[[tools.custom]]
name = "lint"
description = "Run the project linter on one file."
command = ["python", ".trellis/tools/lint.py"]
schema = { type = "object", properties = { path = { type = "string" } }, required = ["path"] }
```

The arguments, as a JSON object, go to the command on stdin. It runs in a
throwaway clone of the repository, as `bash` does (ADR 0032). Stdout, tail
truncated, is the observation. A nonzero exit is an error observation.
Subject to trust, to the gate list, and to `[control] approve` by name.
Names are lowercase `[a-z][a-z0-9_]*`, at most 64 characters, and may not
shadow a built-in or `done`. The description is capped at 200 characters and
may hold no control character, since it is printed in the prompt's tool line.
An optional `timeout_s` bounds how long the command may run (default 120;
see "As built", #135).

Argument validation on tier 2 is the runtime's, from the composed schema. On
tier 4 and in scripts there is no schema validator in the workspace, so
validation is shallow: an object, every `required` key present, no key the
schema does not declare, and each top-level `type` checked. The command must
validate its own input.

### 7. The trail

New tools are recorded as actions like the old ones: `write_file` with
`wrote` set when the file was created, `grep`, `find`, `ls`, `bash` with the
command as the target, and a custom tool under its own name. A denial by a
named gate (the `pre_tool` hook, the bash policy) is an action of its own,
recorded under the same turn and just before the call it refused, the way a
human decision is. A run with no such denial adds no field and no action,
so older trails keep their shape and their digest.

**What `bash` and a custom tool changed** (#139). A `bash` or custom-tool
action carries `changed`, the repository paths the call changed, and
`changed_count`, how many there were. The list is cut at 50
(`trellis_trail::CHANGED_CAP`) and the count is not. Both are omitted when
nothing changed, so a trail written before them, and every action that
changed nothing, keeps its bytes and its digest (tests
`changed_paths_absent_keep_an_older_trails_bytes_and_digest` and
`changed_paths_over_the_cap_record_fifty_and_the_true_count` in
`trellis-trail`, `changed_paths_*` in `trellis-loop` and `trellis-cli`).
Read back, the paths are data a repository chose, like every other string
in a trail (ADR 0007). `wrote` keeps its meaning (set by `edit` and
`write_file` only), so an older trail and a newer one say the same thing by
it.

The list is the carry-back's (ADR 0032): the files the orchestrator wrote
into the repository from the command's clone for this call, recorded where
it writes them. The owner's decision named a `git status` taken before and
after each call (`trellis_vcs::pending`); since ADR 0032 the carry-back is
the only way trellis brings a command's changes into the repository, and
it already knows exactly which files those were. The trade: the list is
exact for what trellis wrote and blind to a command that escapes the clone.
A status diff would catch an escape that landed on a tracked file, but it
would also attribute to the command whatever a hook or another writer
changed during the call, and costs two git spawns per call.

**Best-effort.** It is what trellis wrote on the command's behalf, not
everything the command did. It does not see:

- a command that writes into the repository directly, by its absolute
  path (ADR 0032, "Writes outside the clone"): that bypasses the
  carry-back, and nothing records it;
- a `pre_tool` or `post_tool` hook's writes around the call, which are
  bracketed and reported on their own (`may_have_changed_tree`), not
  attributed to the call;
- anything the carry-back did not write, because the repository did not
  change: new files, deletions, `.gitignore`-class files, writes under
  `.git/` or `.trellis/`, paths `deny_write` or `allow_write` refuse, and
  ignored paths, which are not in the clone at all. The observation the
  model reads names the refused ones; the trail does not.

## Alternatives rejected

- **A tool set held on the model.** The adapter would need a constructor
  argument, and the orchestrator a second copy that can disagree. One value,
  handed to both, cannot.
- **Trust by prompt at run start.** An unattended `work` heartbeat has nobody
  to ask, and a run that stops to ask is a run that fails at 3 a.m. Trust is
  recorded once, by a command a person typed.
- **Shebang hooks.** They do not run on Windows without a shell association,
  and a hook that runs on one platform only is a rule that silently stops
  applying on the other.
- **A bash allowlist by plain prefix.** Rejected for the reason in §3.

## Consequences

- A repository can extend the tool set without a trellis release, and a user
  can narrow it without editing code.
- Executing repository code needs a person to have typed `trellis trust`
  after the last change to that code.
- `[checks]` remains the one repository-supplied execution path outside the
  trust store.

## As built

Decisions the text above left open, made when this was implemented. Test
names are in the code beside each.

- **The tool set rides on `RepoContext`.** `Transcript` and `Turn` are built
  as struct literals in the adapter's tests, and `RepoContext` is the one
  seed type everything builds with `..Default::default()`. Its default is the
  measured set. The adapter's changes are four lines: the tool line, the
  schema, the parser, and a `#[cfg(test)]` on the `ToolCallWire` import its
  tests still use.
- **Scripts are parsed against the run's set.** `ScriptedModel::from_json_with`
  goes through the same parser as the runtime adapter; `cmd_run` builds the
  set before the driver, so a script step calling a tool the run does not
  offer fails before the snapshot. `from_json` alone parses every built-in
  and no custom tool.
- **A call outside the set is refused by the orchestrator before any gate
  is asked**, so a person is never asked about a tool the run does not have.
- **Gate order** is governance, the bash policy (auto mode only), the
  `pre_tool` hook, then the approval gate. `ToolGate` gained a defaulted
  `name()`; gates that return one (`pre_tool`, `bash_policy`) have their
  denials logged by the orchestrator against the model call's position,
  which is the turn number because the loop makes exactly one
  `Request::Call` per turn and the convergence audit uses its own request.
  `Handle::gate_log()` returns them; they are not carried on `Turn`.
- **In the trail** a denial is an action with tool `pre_tool` or
  `bash_policy`, target `<tool> <target>`, outcome `denied: <reason>`, cut at
  120 characters like every outcome. The bash policy's reason names
  `bash_allow` and `bash_unattended` first, so the cut keeps them.
  `write_file` sets `wrote` when the file was created; `bash` and custom
  tools never set it, and record what they changed in `changed` instead
  (§7, #139).
- **`TRELLIS_RUN_ID` is the trail's run id.** The timestamp is taken when
  the run starts and handed to the trail, so a hook can find the trail by
  the id it was given. A consequence: `started_at` in a `cmd_run` trail is
  now the start of the run rather than the moment the trail was written.
- **What hooks receive.** `pre_tool`: the call's flat wire JSON (with
  `read_only` for a built-in since #328).
  `post_tool`: `{"call": <wire JSON>, "result": <the ToolResult as
  serialized>}`. `before_run`: `{}`. `after_run`: `{"run_id", "converged",
  "iterations", "reason", "touched"}` (on an interrupt, `reason` is
  `"interrupted"` and `iterations` is null; §5, #138). `turn_end` and `input` (added by
  #137): the schemas in §5. `before_run` output is indented line
  by line inside its fence, so no line it prints can be the fence's closing
  line, and control characters other than newline and tab are dropped. It is
  appended to `.trellis/context.md`'s notes, inside the repository fence. A
  `before_run` that fails or times out adds nothing and the run says so and
  continues; what failed was a source of context, not a check.
- **A custom tool's timeout is its declaration's `timeout_s`**, default
  120, the same as bash's (#135). It is read as `bash_timeout_s` is: a
  non-negative integer, with a value that key refuses refused here in the
  same words, and 0 taken as 1 second. Neither key has an upper bound. The
  key is part of the tool's trust item only when declared, so a declaration
  without it keeps the digest it was trusted under, and adding or changing
  it needs `trellis trust` again (`custom_tool_timeout_*` in
  `crates/trellis-cli/src/tooling.rs`).
- **Untrusted is per repository, not per item.** One digest covers every
  hook, custom tool and the bash settings together; changing any of them
  untrusts all of them. `disable` or `enable` naming an untrusted custom tool
  is dropped rather than reported as unknown.
- **The single digest stays; per-item digests sit beside it for diagnosis
  only** (#136, decided by the owner 2026-09-28). Items trusted together are
  one decision: a person read them as a set, and a set recombined from items
  trusted at different times (a hook from Monday's trust, a tool from
  Friday's) is a set nobody read. Per-item trust would allow exactly that, and
  an entry written before it could not say which items it covered. So each
  store entry keeps its `sha256` as the only thing `TrustStore::state`
  compares, and gains an optional `items` map (label to the sha256 of that
  item's JSON, `Fingerprint::item_digests`), written by `trellis trust` and
  read only by `TrustStore::changed_items`. `trellis trust --show` (an
  `items:` line) and `trellis doctor` (on the `trust:` line) use it to name
  each item changed, added or removed since trust was recorded, and say
  "not known" for an entry written before the map existed. The map is
  additive: such an entry reads and is honoured exactly as before, and an
  older trellis ignores the field. A record whose item digests all match
  while its `sha256` does not is still `Changed`. Tests:
  `trust_changed_items_*` in `trellis-cli`, the last of which is that
  property.
- **The bash policy applies to every auto-mode run**, `trellis tui` included:
  someone may be watching the interface, but nobody is asked.
- **`[control] approve`** accepts the new built-ins and any declared custom
  tool's name; in approve mode `bash` is appended to the list when bash is
  loaded, and the trail's `control` field shows it.
- **The `tools:` line** prints the set, the shell and its policy, and the
  loaded hooks, whenever the set is not the measured default or hooks are
  loaded. An unconfigured run prints what it printed before.
- **`callers_of` and `callees_of` are opt-in built-ins** (#128, ADR 0027),
  after `read_note` and on only through `[tools] enable`. Each returns
  `Output` with its doubt as counts: `callers_of` the symbol's
  `RadiusCoverage`, `callees_of` its own unbound call sites by reason
  (`graph_tools` in `crates/trellis-loop/src`).
- **`coverage_report` is an opt-in built-in** (#129), after `callees_of`:
  `doubt_in_files` over the named paths as counts only, never the
  index-wide `resolved` figure or a percentage.
- **No lazy fetch (security finding F1, AUDIT-2026-10-11).** `command()`,
  `command_with_hooks()` and the environment `grep` and `find` hand
  `exec::run` with their `git_argv` argv all set `GIT_NO_LAZY_FETCH=1`
  (`trellis_vcs::ENV`). A planted promisor remote (`remote.<r>.uploadpack`
  or `core.sshCommand`, with `protocol.<name>.allow=always`) is then not
  run by `cat-file --batch`, `history::tree_copy`, `diff` or the
  rollback. The read answers the object as missing, or the command fails
  and trellis reports the failure. No `-c` does this on git 2.43: a planted
  `allow` beat `-c protocol.allow=never`, and `-c
  extensions.partialClone=`, `-c remote.<r>.promisor=false` and `-c
  fetch.uriProtocols=` changed nothing. `--no-lazy-fetch` was added in
  2.45, and an older git refuses the command. Covered: git 2.45.0 and
  later, the CVE-2024-32465 releases (2.39.4, 2.40.2, 2.41.1, 2.42.2,
  2.43.4, 2.44.1), and distribution builds that carry that fix. Any other
  git ignores the variable and still runs the planted command, and
  trellis does not detect that. Given up: in the user's own partial
  clone, the Atlas reports an old commit whose blobs the clone lacks as
  missing rather than fetching them. Tests, in `trellis-vcs`:
  `a_planted_partial_clone_remote_never_runs_from_a_read` and
  `a_planted_partial_clone_remote_never_runs_from_diff_or_a_rollback`,
  for both transports, each of which failed before with the marker
  written and ends on a control that runs the planted remote without
  the variable; and `every_git_constructor_sets_no_lazy_fetch`.

Not built: a `search_notes` tool (ADR 0021 row 30), per-item trust (kept
out on purpose, above),
hooks for events other than the six in §5 (`turn_end` and `input` were
added by #137). What `bash` or a custom tool wrote is tracked since #139,
as far as §7 says.

## Integrated

- **An agent profile's `tools` list narrows the set** (ADR 0026 §4).
  `ToolSet::narrowed` is an intersection taken after `[tools]` and trust, so
  a committed profile cannot turn on a tool the repository left off or a
  bash or custom tool the user has not trusted. `run_checks` stays, for the
  reason §1 gives. It is applied once in `cmd_run`, before the driver, so
  the one value this ADR hands to the prompt, schema, parser and
  orchestrator is the narrowed one. Details and tests are in ADR 0026,
  "Integrated".
- **`read_skill` is served with a skill policy and user directories**
  (`Orchestrator::with_skills`). The orchestrator is where the body is
  served, so it is where a profile's deny list binds.
- **`trellis doctor` reports trust**: what is declared that needs it, and
  whether it is trusted, changed since, or never trusted. A repository that
  declares nothing says so, and the store is still not opened for it.
- **User-level skills are not gated on this store.** ADR 0025, "Integrated",
  gives the reason: the store vouches for a repository, and a user skill is
  in none.

## Amendment 2026-10-04: a hook may not run a file from the user's packages directory (#201)

§4 treated every absolute path outside the repository as the user's system.
One such place is not: the packages directory beside the trust store, where
`trellis install` puts skills and templates (ADR 0039). Nothing there is in
the trust digest, so a trusted hook or custom tool that ran a package's file
as code would change what it does whenever the package is updated, with the
digest unchanged. Packages carry no code (ADR 0039 §9), so such a hook is
refused at trust time, with the same fix as a repository file outside
`.trellis/`: move the code under `.trellis/`. Tokens are made canonical
first, as for the repository rule. The owner chose refusal over hashing the
file (which would support a pattern ADR 0039 says should not exist) and over
leaving it. Tests: `trust_package_path_*` in `crates/trellis-loop/src/trust.rs`.

## Amendment 2026-10-04: hooks are told whether a built-in call only reads (#328)

Status: Accepted (owner-directed): the session's choice under the owner's
2026-10-04 delegation (option A of #328).

A `pre_tool` hook is trellis's permission-gate extension point (§5), and a
gate that allows reads and asks about writes had to keep its own copy of
which tools only read, which went stale when a built-in was added. The
`pre_tool` input and the `call` object in `post_tool`'s stdin now carry
`"read_only": true` or `false` for every built-in tool call. The fact comes
from one table, `READ_ONLY_TOOLS` in `crates/trellis-loop/src/gate.rs`:
read-only are `blast_radius`, `read_file`, `read_skill`, `grep`, `find`,
`ls`, `search_notes`, `read_note`, `callers_of`, `callees_of`,
`coverage_report` and `ask_human`; not read-only are `edit`, `write_file`,
`bash` and `run_checks`, which runs repository code. A test fails when a
name in `APPROVABLE_TOOLS` is missing from the table.

- **Built-ins only.** A custom tool's call object is unchanged and has no
  `read_only` key: trellis does not know what a repository's command does,
  and the object holds the repository's own argument names, so an added key
  could collide with one. The choice is made on the call's variant, not its
  name, so a custom tool named like a built-in gets nothing.
- **The hook payload only, not `call_json`.** The key is inserted in
  `crates/trellis-loop/src/hooks.rs` after `registry::call_json` returns.
  `call_json` is unchanged, because it also builds the hosted transcript's
  tool arguments, where `read_only` would be an argument no schema declares.
- **Nothing loosened.** Hooks still run in the throwaway clone (ADR 0032),
  the trust digest and `[control]` are untouched, and a repository cannot
  set the fact.

What reverses it: the owner wanting custom tools to declare
`read_only = true` under trust (a config schema change), or no payload key
at all. Tests: `read_only_table_*` in `gate.rs`, and
`call_json_has_no_read_only_*` and `hook_payload_read_only_*` in
`crates/trellis-loop/src/extension_tests.rs`.

## Amendment 2026-10-13: registered linked worktrees under `.trellis/work/` are left out of the `.trellis/` digest (#718)

Status: Accepted (owner-directed): the owner's decision of 2026-10-13 on
PR #717, filed as #718.

`trellis work` keeps a converged task's linked worktree at
`.trellis/work/task/<id>/` (ADR 0053). §4's digest walked it like any
directory under `.trellis/`, so keeping one changed the starting worktree's
fingerprint, and every later trust check there (`work`'s gate for process
agents and services, an in-place `trellis run`) read "changed since it was
trusted". A run in the task worktree changed it again with every edit. A
worktree is a checkout of the repository, not code the person trusted under
`.trellis/`, so it joins the skips, on terms a directory cannot claim for
itself. A directory is left out only when all of these hold:

- its real path is strictly under the real `.trellis/work/`, and the walk
  reached it there, with no symbolic link on the way;
- its `.git` is a regular file (not a directory, not a link) whose
  `gitdir:` line resolves, `..` and relative paths included, to
  `<common git dir>/worktrees/<name>`, a directory directly under a
  `worktrees` that is not itself a link;
- that directory's `gitdir` file resolves back to this `.git`: the
  worktree is registered in this repository.

The common git directory is read from disk (`root/.git`, or the
`commondir` its `.git` file leads to), as are the two pointer files, each
read up to 4096 bytes. No git process is run: the digest is taken before
every hook spawn. Built as `trellis_vcs::common_git_dir_on_disk` and
`trellis_vcs::is_registered_linked_worktree`. Everything else under
`.trellis/work/` is hashed as before: a directory with no `.git`, with a
`.git` directory, with a `.git` pointing elsewhere, at the common git
directory, or at another worktree's directory (which does not name it
back), a `.git` that is a link, a worktree whose registration was removed,
and a link under `.trellis/` to a registered worktree, whose files are then
recorded under their real paths as any alias's are. A linked worktree
outside `.trellis/work/` is hashed too.

A hook or custom tool whose argv names a file inside such a worktree
(decided on the canonical path, so a link under `.trellis/` into one counts)
is refused, as code outside `.trellis/` is (the eleventh and twelfth
amendments above): it is not in the digest, and a run rooted in that
worktree edits it. `trellis trust` prints the item, the file and the
worktree, and says to move the code to `.trellis/hooks/` or
`.trellis/tools/`.

A repository with no linked worktree under `.trellis/work/` has the digest
it had, so no trust recorded before this changes. The `.trellis/` item's
shown text in `trellis trust` is unchanged, as for #620: that text is part
of the digest, and changing it would untrust every repository trusted
before. Evidence, in `crates/trellis-loop/src/trust.rs`:
`trust_linked_worktree_registered_under_work_is_left_out_of_the_digest`
(the fingerprint is the same before a task worktree is added, after, and
after edits in it; relative pointers with `..` resolve),
`trust_linked_worktree_forged_or_unregistered_is_still_fingerprinted`,
`trust_linked_worktree_outside_work_is_still_fingerprinted` and
`trust_linked_worktree_code_named_by_a_hook_is_refused`.

**Not covered, plainly:** whoever can write `.git/worktrees/` can register
a directory, and so leave it out. That is the same writer who can already
change `.git/config`, and a run refuses when `.git/` changed during it
(§5, ADR 0032); a clone cannot carry a `.git` file, since git refuses the
path. The skip is decided each time the digest is taken: a directory that
becomes registered, or stops being registered, after a trust changes the
digest, and reads as changed.

What reverses it: removing the skip from `dot_trellis_files`, after which
kept task worktrees untrust the starting worktree again.
