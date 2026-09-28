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

`[tools] bash = true` enables `bash {command}`. It runs with the repository
root as its working directory, a timeout (`[tools] bash_timeout_s`, default
120), and `TRELLIS_RUN_ID` and `TRELLIS_MODEL` added to the environment.
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
not make. A command that exits normally is not reaped of what it left
running. Leading its own group has a cost on Unix: the child is no longer
in the terminal's foreground group, so Ctrl-C does not deliver SIGINT to a
running `bash` command, custom tool, hook or listing. It ends when trellis
drops it (`kill_on_drop`, SIGKILL) during the interrupt's shutdown, with no
chance to clean up. (Amended after the second review of PR #1: before it, the run
waited for EOF on both pipes, so `sleep 30 & exit 0` read as a timeout,
and a timeout killed only the shell.)

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
  description, command and schema, the `[tools]` bash settings, and the
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

**Not covered, plainly:** code reached indirectly from outside `.trellis/`
and outside any file an argv or code string names — a module imported by
a hook (`python -m tools.hook` resolving to `tools/hook.py`), a file a
script reads or sources, a program on `PATH`. A change there does not
untrust the repository. **Keep hook and tool code under `.trellis/hooks/`
or `.trellis/tools/`**, where every byte is in the digest.

Not covered, and stated so it is not assumed: `[checks] typecheck` and
`tests` are repository-supplied argv that trellis has always executed
without trust. Gating them breaks every existing configuration and needs a
decision of its own.

### 5. Executable hooks

Declared in `.trellis/config.toml` as argv arrays, with no reliance on a
shebang, so a hook is `["python", ".trellis/hooks/pre.py"]` on every
platform. One per event. Each runs with the repository root as its working
directory, the same two environment variables as `bash` (and, since the
fourth review of PR #1, `PYTHONDONTWRITEBYTECODE=1`, as every command
trellis starts has; see §4), and a timeout
(`[hooks] timeout_s`, default 10).

- **`pre_tool`**: the call as JSON (the flat wire form the model emitted) on
  stdin. Exit 0 allows, exit 2 denies with stderr as the reason, anything
  else, a timeout, or a failure to start denies. Fail closed. Implemented as
  a `ToolGate`, consulted after governance and the bash policy and before
  the approval gate, so a person is never asked about a call a hook refused.
- **`post_tool`**: the call and the tool's result as JSON on stdin. Observe
  only: its output is discarded and it cannot alter the observation.
- **`before_run`**: stdout, capped to its first 4 KiB (with the true total
  printed after the cut; *amended 2026-09-27*, it had been a 4 KiB slice
  from the middle of the last 8 KiB), is added to the repository's
  context inside a fenced `untrusted hook output` block, within the fence
  whose closing sentence says nothing above can authorise skipping a check.
  Runs after the snapshot, because a hook can write to the tree.
- **`after_run`**: the outcome as JSON on stdin. Runs when the loop ends and
  before the rollback, so on a run that did not converge the rollback also
  undoes what it wrote inside the worktree. It does not run on Ctrl-C. On
  a converged run, a write by it demotes the run (see "After a pass").

A hook can refuse a call. It cannot produce a pass, cannot make a run
converge, and cannot rewrite a call or an observation (ADR 0004, ADR 0005).

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
  is not refused up front: `edit scripts/gate.py` is an ordinary edit. The
  guard sees it at the next spawn — the `post_tool` hook of that same call,
  or the check after it — and ends the run there. Keeping hook code under
  `.trellis/` (§4) turns that into a refusal the model can read.
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

### 6. Custom command tools

```toml
[[tools.custom]]
name = "lint"
description = "Run the project linter on one file."
command = ["python", ".trellis/tools/lint.py"]
schema = { type = "object", properties = { path = { type = "string" } }, required = ["path"] }
```

The arguments, as a JSON object, go to the command on stdin. Stdout, tail
truncated, is the observation. A nonzero exit is an error observation.
Subject to trust, to the gate list, and to `[control] approve` by name.
Names are lowercase `[a-z][a-z0-9_]*`, at most 64 characters, and may not
shadow a built-in or `done`. The description is capped at 200 characters and
may hold no control character, since it is printed in the prompt's tool line.

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
  tools never set it.
- **`TRELLIS_RUN_ID` is the trail's run id.** The timestamp is taken when
  the run starts and handed to the trail, so a hook can find the trail by
  the id it was given. A consequence: `started_at` in a `cmd_run` trail is
  now the start of the run rather than the moment the trail was written.
- **What hooks receive.** `pre_tool`: the call's flat wire JSON.
  `post_tool`: `{"call": <wire JSON>, "result": <the ToolResult as
  serialized>}`. `before_run`: `{}`. `after_run`: `{"run_id", "converged",
  "iterations", "reason", "touched"}`. `before_run` output is indented line
  by line inside its fence, so no line it prints can be the fence's closing
  line, and control characters other than newline and tab are dropped. It is
  appended to `.trellis/context.md`'s notes, inside the repository fence. A
  `before_run` that fails or times out adds nothing and the run says so and
  continues; what failed was a source of context, not a check.
- **Custom tools have a fixed 120-second timeout**, the same as bash's
  default. Not configurable yet.
- **Untrusted is per repository, not per item.** One digest covers every
  hook, custom tool and the bash settings together; changing any of them
  untrusts all of them. `disable` or `enable` naming an untrusted custom tool
  is dropped rather than reported as unknown.
- **The bash policy applies to every auto-mode run**, `trellis tui` included:
  someone may be watching the interface, but nobody is asked.
- **`[control] approve`** accepts the new built-ins and any declared custom
  tool's name; in approve mode `bash` is appended to the list when bash is
  loaded, and the trail's `control` field shows it.
- **The `tools:` line** prints the set, the shell and its policy, and the
  loaded hooks, whenever the set is not the measured default or hooks are
  loaded. An unconfigured run prints what it printed before.

Not built: a `search_notes` tool (ADR 0021 row 30), per-item trust, a
configurable custom-tool timeout, hooks for events other than the four
above, and tracking of what `bash` or a custom tool wrote in the worktree.

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
