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

Output from `grep`, `bash` and custom tools is capped at the tail (8 KiB,
with a line saying how much was cut) before it becomes an observation;
`find` and `ls` name at most 500 paths and say how many they left out. The adapter's own tail window then applies as it does to every
observation.

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
running. (Amended after the second review of PR #1: before it, the run
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
  with `not trusted: run trellis trust`.
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
directory, the same two environment variables as `bash`, and a timeout
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
