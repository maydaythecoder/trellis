# ADR 0026 — Local orchestration: task model, agent profiles, routines, budgets

**Status:** Accepted. Amended in implementation: §1, §4, §5, §7, §8 and
§10. §4 and §11 amended again for `pass_env` when ADR 0024 §3's key gap
was closed. Each amendment is marked in place. §4's `tools` list and skill deny
list were integrated with ADR 0023 and ADR 0019 afterwards; see
"Integrated" at the end.
**Date:** 2026-09-27
**Extends:** ADR 0020 (the local work queue). Every rule there still holds:
the queue lives in the git directory, files are published by hard link and
never renamed, execution is serial, and `work` stops at the first converged
change.
**Reference design:** Paperclip (`paperclipai/paperclip`, MIT), the rows of
ADR 0021 dispositioned W-E. Semantics were read from Paperclip's own docs
(`docs/api/routines.md`, `doc/execution-semantics.md`) where the inventory
was not exact. No Paperclip code or server is used.

## Context

ADR 0020 built the queue as a list of tasks that run oldest first. Paperclip
organizes work with more structure than that: priorities, dependencies,
sub-tasks, named agents with their own instructions and budgets, schedules,
approvals and an audit log. ADR 0021 took each of those as W-E.

Two things found while reading the code decide the shape of what follows.

1. **The trail did not record tokens.** The brief for this wave says the
   budget ledger reads spend from the trail. It could not: `Factual` records
   prompt size in bytes (`context`) and nothing else about spend. So this
   ADR adds a token field to the trail (§8), and the ledger reads that.
2. **Every mutation collides with the no-overwrite rule.** `publish` refuses
   an existing name (ADR 0020 §3), and `set`, `note`, `cancel`, `approve`, a
   routine's last-fired time and a profile's pause all rewrite a file. One
   protocol has to cover all of them (§2).

## Decision

### 1. The task model

A task file gains these fields. Every one has a serde default, so a task file
written under ADR 0020 still loads unchanged.

| Field | Meaning |
|---|---|
| `status` | `backlog`, `todo`, `in_progress`, `in_review`, `blocked`, `done`, `cancelled`, `failed` |
| `blocked` | The typed reason when `status` is `blocked`: `blockers`, `children`, or `manual` with a detail naming what unblocks it |
| `priority` | `critical`, `high`, `medium` (default), `low` |
| `labels` | Free strings |
| `parent` | A task id. Structure only, as in Paperclip: it is not a dependency |
| `assignee` | An agent profile name (§4) |
| `blockers` | Task ids this task waits for |
| `needs_approval`, `approval` | §6 |
| `notes` | `[{at, actor, text}]`, appended by `queue note` and by the queue |
| `routine` | The routine that created the task, if any (§7) |

The goal is the vault stem already stored as `focus` (ADR 0020 §6).
`queue add --goal` is an alias of `--focus`.

**The directory is still the state; `status` refines it.** `State` gains
`cancelled/`. Which statuses may sit in which directory is fixed:

| Directory | Statuses | Default when `status` is absent |
|---|---|---|
| `pending/` | `backlog`, `todo`, `blocked` | `todo` |
| `claimed/` | `in_progress` | `in_progress` |
| `done/` | `in_review`, `done` | `done` |
| `failed/` | `failed` | `failed` |
| `cancelled/` | `cancelled` | `cancelled` |

A task in `done/` written before this ADR reads as `done`, not `in_review`.
Those tasks converged under ADR 0020 and have almost certainly been reviewed
since, and reading them as `in_review` would leave every task that names one
as a blocker waiting forever.

**A converged run now ends `in_review`, not `done`.** Its change is in the
worktree waiting for a person, which is what `in_review` means in Paperclip.
`trellis queue set <id> --status done` records that the person accepted it.

**What is claimable.** A pending task is claimable when all of these hold:
- its status is `todo`
- every blocker is `done`
- it does not need an approval it has not received
- its assignee, if any, passes the gate in §4

The heartbeat claims the highest priority first and, within a priority, the
oldest. It reads every pending file to sort them. That costs one read per
pending task per tick, and the queue is a person's backlog, not a message
bus. A file that is not valid task JSON still goes to `failed/` when a claim
reaches it, as in ADR 0020.

**Blockers and parents wake tasks.** These rules are taken from Paperclip
`execution-semantics.md` §6:
- A blocker is resolved only when it is `done`. A `cancelled` blocker does
  not satisfy the dependency. The edge stays unresolved until someone
  removes it (`queue set <id> --unblock <blocker>`).
- A task that is `blocked` for reason `blockers` becomes `todo` once every
  blocker is `done`.
- A task that is `blocked` for reason `children` becomes `todo` once every
  direct child is terminal. Terminal is `done`, `cancelled` or `failed`.
  Paperclip has no `failed` status. Trellis adds it, and counts it as
  terminal because a failed child will not move again without a person.
  `in_review` is not terminal.
- A parent waits on its children only if it was set to wait with
  `--status blocked --reason children`. Adding a child does not change the
  parent, because Paperclip says structure is not dependency.

These checks run in `Queue::reconcile`. `work` calls it before every claim,
and every CLI command that ends a task calls it too. Each wake is written to
the activity log (§9).

Adding blockers that are not yet `done` sets the task to `blocked` for reason
`blockers`. A task set to `todo` while it still has unresolved blockers stays
unclaimable, and `queue list` says why.

*Amended in implementation: a heartbeat that finds tasks but none it may
claim now says so.* `work` without `--every` used to return "queue empty"
whenever nothing was claimed. With gated tasks, that line is false. The loop
prints each waiting task and its reason once, and it stops with
`Stop::Waiting(n)`: "nothing claimable; n pending task(s) waiting". This
exits 0, like an empty queue, because nothing is wrong: the tasks are waiting
as designed. "Queue empty" is printed only when nothing is pending.

### 2. Changing a file that already exists

Every rewrite goes through one of three paths. None of them renames a file.

- **A pending task is edited by claiming it.** `claim(id)` gives this process
  the task under the same atomic checkout `work` uses. The edit is applied,
  and the changed task is published back to `pending/` (or to its new
  directory) before the claim is removed. While the
  edit is in progress, `work` does not see the task. If `work` holds the
  task, the edit fails with "claimed by pid N". The window ADR 0020 §3
  already accepts also applies here. A claimer that read the old file a few
  microseconds before the edit could run the old content.
- **A claimed task is not edited.** The running process publishes its own
  copy when it finishes, so an edit would be silently lost. `queue set` and
  `queue note` on a claimed task refuse and say why.
- **A task in `done/`, `failed/` or `cancelled/`, and the queue's other
  state files, take an exclusive lock.** The lock is
  `<git-dir>/trellis/locks/<name>.lock`, published by hard link, and the hard
  link fails if a lock already exists. Profile overlays and routine state
  use the same path.

  *Amended after the third review of PR #1.* The holder first removed the
  old file and then published the new one, so a failure or a crash between
  the two lost the file: a finished task vanished from every directory. Now
  the holder publishes the new content at the fixed side name
  `.<name>.next`, removes the old file, links the side copy into place and
  removes the side copy. A complete copy exists at every instant. Readers
  that find the name missing read the side copy, `queue list` shows it, and
  the next holder of the lock finishes the swap. If the side copy exists
  while the old file is still there, the edit never took effect and the
  side copy is discarded. If linking the side copy into place fails, the
  caller gets an error, but the edit has already taken effect. The error
  says so and names the side copy. The lock is taken by every mutation of a finished
  task: `queue set`, `queue note`, `queue requeue`, and the removal done by
  `import --on-conflict replace`. Before this, only `edit` took the lock, so a requeue
  that ran during an edit moved the task to `pending/`, and the edit then
  wrote it back to `failed/`. That left one task in two states.
  `a_requeue_during_an_edit_of_a_failed_task_leaves_it_in_one_state` and
  `an_edit_of_a_finished_task_that_fails_midway_never_loses_it` hold both. Approval requests are never rewritten in
  place: they only move between `pending/`, `approved/` and `rejected/` by
  publish-then-remove, like a task, so a second decision on the same request
  fails at the publish. A crash while the lock is held leaves
  the lock behind, and the next writer gets an error that names the lock
  file. Removing it by hand is the recovery. This is the same kind of
  visible, stuck-but-never-lost state as the two-copies window in ADR 0020
  §3.

**The activity log is appended, not published.** `activity.jsonl` is opened
with `O_APPEND`, and each record is one `write_all` of one line. It is never
renamed, replaced or truncated. The hard-link rule exists so that a file is
never seen half-written and never overwritten. An append-only log meets the
second part by construction. It meets the first part as long as each line
goes out in one write, and a reader skips a last line that does not parse.

### 3. The queue subcommands

- `queue add` gains `--priority`, `--label` (repeatable), `--goal`,
  `--parent`, `--assignee`, `--blocked-by` (repeatable), `--needs-approval`
  and `--backlog`. The parent, the blockers and the assignee must exist when
  the task is added. This follows ADR 0020's rule that anything checkable is
  checked at add time.
- `queue list` shows priority, status, labels, assignee, blockers, parent
  and the reason a pending task is not claimable. It filters with
  `--status`, `--label`, `--assignee` and `--priority`.
- `queue set <id>` changes priority, labels, assignee, parent, blockers,
  approval requirement, or status (`--status blocked` requires `--reason`).
- `queue note <id> "<text>"` appends a note.
- `queue cancel <id> [--reason]` moves the task to `cancelled/`.
- `queue approve`, `queue reject`: §6.
- `queue log [--limit N]`: §9.
- `queue export`, `queue import`: §10.

### 4. Agent profiles

A profile is `.trellis/agents/<name>.toml`, committed with the repository
like the rest of `.trellis/`:

```toml
role = "reviewer"
instructions = "agents/reviewer.md"   # under .trellis/, prepended to the task
model = "llamacpp:http://127.0.0.1:8080"
# command = ["python", "tools/agent.py"]   # a process agent (§11); excludes model
# timeout_s = 1800
# pass_env = ["ANTHROPIC_API_KEY"]         # process agents only; default none (§11)
reports_to = "lead"
paused = false
tools = ["read_file", "edit"]           # carried for W-B's registry; see below

[control]
mode = "auto"          # or "approve"
approve = ["edit"]

[skills]
allow = ["refactor"]   # absent means all
deny = []

[budget]
monthly_tokens = 2000000
lifetime_tokens = 50000000
warn_at = 0.8
```

**Validation** happens whenever profiles are loaded, which is every command
that reads them:
- A name is lowercase ASCII letters, digits, `-` and `_`.
- `model` and `command` are mutually exclusive. With neither, the task uses
  the `work` model.
- `reports_to` must name an existing profile, and the chain must not form a
  cycle (Paperclip item 4).
- `[control]` uses the same vocabulary as the repository's `[control]`
  (ADR 0018).
- `warn_at` is in (0, 1].
- A process profile may not set a budget, because trellis cannot see the
  process's tokens (§8). A budget it could never enforce would read as a
  limit that exists.
- *Amended (wave K):* `pass_env` is refused on a profile without `command`.
  A model profile starts no process to pass a variable to, and accepting
  the field would read as a pass-through that happened. An entry that is
  blank or contains `=` is refused as not a variable name.

**What a profile changes about a run of its tasks:**
- **`instructions`**: the file's text goes ahead of the task, the same way
  `--prompt` does.
- **`model`**: overrides `work --model` and `[model] spec` for this
  profile's tasks.
- **`[control] mode`**: overrides the dial. `work` still has nobody to ask,
  so an `approve` profile's tasks can be claimed only when `work` runs
  attached to a terminal. Otherwise they are unclaimable and give that
  reason, instead of being claimed and refused by `NEEDS_A_TERMINAL`.
- **`[control] approve`**: replaces the repository's approve list for the
  run.
- **`[skills]`**: filters the catalog disclosed to the model (ADR 0019).
- **`tools`**: stored and validated only as a list of strings. W-B is
  building the tool registry at the same time (ADR 0023), and this field is
  what it will read. Until then, **it has no effect on the run**, and
  `agents show` says so. *Integrated: it now narrows the run's tool set;
  see "Integrated".*

The first four settings reach `cmd_run` through one new field on
`RunSeeding`. It is an optional reference to the profile, and every caller
except `work` passes `None`.

*Amended in implementation:*
- **The skill filter narrows the list the model is shown, and nothing
  more.** The `read_skill` tool reads any skill by exact name through
  `skills::read_body` in the orchestrator, and that path is owned by W-B. So
  a denied skill is left off the list, but a model that already knows the
  name can still read it. `agents show` says so. Enforcing the filter at the
  tool belongs with the tool registry (ADR 0023). *Integrated: enforced at
  `read_skill`; see "Integrated".*
- **`work --model` is optional.** A queue whose tasks all name a profile
  with its own model, or a process command, needs no default model. A task
  that does need one, when none is configured, is left pending with that
  reason instead of failing.
- **The profile's approve list travels on `RunSeeding`, not `RunControl`.**
  `RunControl` is built with a struct literal at six call sites across the
  modules that W-D is editing. `RunSeeding` has two such sites, `run` and
  `work`; every other caller uses `default()`. The approve list is applied
  to the dial just after `cfg.control(flag)`.

**Pause state is not written into the profile file.** The TOML is tracked,
and writing to it would dirty the worktree that every run needs clean
(ADR 0009). `agents pause`, the budget hard stop and budget raises write an
overlay at `<git-dir>/trellis/agents/<name>.json`, using the lock protocol of
§2. A profile is paused if its file says `paused = true` or its overlay says
so. `agents resume` on a profile paused in its file says to edit the file.

**Subcommands:** `trellis agents list`, `show <name>`,
`pause <name> [--reason]`, `resume <name>`.

### 5. Budgets in tokens

For each profile, the ledger sums the tokens recorded in trails (§8) whose
`agent` is that profile. It keeps two sums:
- **This calendar month, UTC.** The window resets at 00:00 UTC on the 1st.
  A run belongs to the month its `started_at` falls in.
- **Lifetime.**

There are no dollars. COST_MODEL.md does not price tokens, and ADR 0021
excludes dollars until it does.

**What is counted, stated plainly.** The count is **completion tokens as the
runtime adapter reported them**. It is `Model::tokens_used()`, which the
runtime adapter sums from each completion's reported count. No figure for
prompt tokens exists anywhere in trellis, so a budget here limits generated
tokens, not billed tokens. A scripted run records no token count. A process
agent's tokens are unknown to trellis.

**A hosted reply with no usage figure** (amended after the second review of
PR #1; before it such a reply was recorded as 0, so spend read as free and
no budget tripped). The count is unknown, never zero:
- The trail records `tokens: None` for the run once any reply went
  unreported, because the sum of the others would be a lower bound
  presented as a count.
- The in-run `max_tokens` ceiling charges each unreported reply at the
  `max_tokens` it was requested with, the most it can have cost, so it
  trips early rather than never.
- A run executed as a profile with a `[budget]` is refused at the first
  unreported reply: the run ends not converged and rolls back, with a
  reason naming the two fixes (a provider that reports usage, or no
  `[budget]`). The ledger itself is unchanged, and still counts only
  measured spend. The limit, stated: that one reply's tokens, at most its
  `max_tokens`, are spent and counted nowhere, and `agents show` reports
  the run among those without a count.

**The ledger covers the trails in this checkout.** Trails are committed, so a
branch that does not contain a run's trail does not count its spend. Trails
are read as untrusted data (ADR 0007). A trail whose factual digest does not
match is still counted, because it is the only record available, and
`agents show` reports how many such trails there are.

**Thresholds.** *Amended in implementation: every profile with a budget is
checked at the top of every `work` tick, before anything is claimed.* The
next tick after a run is the "after each run" check. The gate then reads the
pause from the overlay.
- **At `warn_at` of either window:** `work` prints a warning naming the
  window, the spend and the limit.
- **At 100% (hard stop):** the profile is paused by the budget in its
  overlay, and `work` skips its tasks. A `budget_override` approval request
  is created, unless one is already pending for this profile.
  - `trellis queue approve <request> --raise <tokens>` raises the window
    that tripped and resumes the profile. A monthly raise applies only to
    the month it was granted in.
  - `trellis queue reject <request>` keeps the profile paused.
- *Amended in implementation:* **one request per pause, not per tick.** A
  request is opened only on the transition into a budget pause. A profile
  that stays over its limit keeps its pause and opens nothing more, so a
  rejection stands until something changes. The first version reopened a
  request on the next tick after every rejection; a unit test now covers
  it.
- **A budget pause lifts when the profile is under every limit again.** That
  happens when the month rolls over, or when someone raises `[budget]` in
  the profile file, which is the way out after a rejection. The resume is
  logged. A pause by a person is never lifted automatically, and a profile
  a person paused opens no request while it is over its limit.

The budget is a cost guard, not a correctness claim. Nothing about it
touches `Verdict`.

### 6. Approvals

**Task approval** (Paperclip item 32, the review stage run before a task
starts):
- `queue add --needs-approval`, or `queue set --needs-approval true`, makes a
  task unclaimable until it is approved.
- `trellis queue approve <task-id>` records `{decision: approved, at}` on the
  task.
- `trellis queue reject <task-id> [--reason]` records the rejection and
  moves the task to `cancelled/`.

**Budget override requests** (§5) are files under
`<git-dir>/trellis/approvals/{pending,approved,rejected}/<id>.json`. Their
ids begin with `budget-`, so `queue approve <id>` can tell them from task
ids.

Every request and every decision is logged (§9).

### 7. Routines

A routine is `.trellis/routines/<name>.toml`:

```toml
schedule = "0 9 * * 1-5"    # minute hour day-of-month month day-of-week
timezone = "UTC"            # or a fixed offset: "+02:00", "-05:30"
concurrency = "coalesce"    # coalesce | skip | always
catch_up = "skip"           # skip | enqueue
catch_up_max = 5            # with enqueue: at most this many missed fires
paused = false

[task]
text = "Triage new failures for {{date}} ({{routine}}), area {{area}}"
priority = "high"
labels = ["triage"]
assignee = "reviewer"
# focus, prompt, needs_approval, parent

[vars]
area = "parser"
```

**Cron.** The parser is written by hand. It supports:
- `*`, lists, ranges and steps (`*/15`, `1-5/2`, `10/5`)
- month names `jan`–`dec` and day names `sun`–`sat`
- day-of-week 0–7, where both 0 and 7 are Sunday
- the usual rule that when day-of-month and day-of-week are both restricted,
  a day matches if either one does. *Amended 2026-09-27:* "restricted" is
  Vixie cron's test, a field whose first character is not `*`. A field
  starting with `*` (including `*/2`) makes the two AND, so
  `0 9 */2 * mon` is odd-numbered Mondays; it had been read as restricted
  and ORed, firing every Monday and every odd day.

It does not support `L`, `W`, `#`, `?` or `@daily`-style names.

**Timezones: UTC and fixed offsets only, not IANA names.** IANA names would
need a timezone database. That means a new dependency, and the small crates
that bundle one pin the tz data to whenever they were built. A fixed offset
does not follow daylight saving time, so a routine that should fire at 09:00
Amsterdam time is written `+01:00` in winter and `+02:00` in summer, or it is
written in UTC. `Europe/Amsterdam` is refused with that explanation. It is
not treated as UTC. An offset is exactly `+HH:MM` or `-HH:MM`, two digits in
each field, checked on the text rather than on the parsed number: `-5:00`,
`+05:5` and `+005:00` are refused rather than read as something close.

**Firing.** `work` evaluates routines on every tick before claiming. The
state of each routine is stored at `<git-dir>/trellis/routines/<name>.json`,
using the lock protocol of §2. It records `armed_at`, `last_fired` (the
scheduled instant) and the id of the last task the routine created.
- A routine seen for the first time is armed at that moment and does not
  fire for times before it.
- The due instants are the schedule's instants after `last_fired` (or
  `armed_at`) and at or before now.
- **`catch_up = "skip"`** (Paperclip `skip_missed`): only the most recent
  due instant fires. The earlier ones count as missed, and the log records
  how many were dropped.
- **`catch_up = "enqueue"`** (Paperclip `enqueue_missed_with_cap`): the most
  recent `catch_up_max` instants fire, oldest first.
- **Concurrency** applies to each fire. A routine is active while a task it
  created is in `pending/` or `claimed/`.
  - `coalesce` (the default) creates no task. It appends a note to the
    active task saying the routine fired again. This matches Paperclip's
    `coalesce_if_active`, which links the new fire to the active run.
  - `skip` creates no task and logs the skipped fire.
  - `always` creates a task every time.
- The template is filled per fire:
  - `{{date}}` and `{{time}}`: the scheduled instant in the routine's
    timezone
  - `{{routine}}`: the routine's name
  - any key in `[vars]`
  An unknown `{{name}}` is an error when the routine is loaded, not when it
  fires, and a var may not shadow a built-in.

*Amended in implementation:*
- **A `paused = true` routine is not evaluated at all**, so its state does
  not move. When it is unpaused, the fires it missed while paused are
  handled by its catch-up policy, as any other missed fires are.
- **The look-back is clamped to 400 days** so that a routine left
  unevaluated for years cannot stall a tick. The enumeration is one
  `next_after` per due instant, and a year of every-minute fires is about
  526,000 of them.
- **A template's assignee must name an existing profile.** This is checked
  when routines are loaded, like the template variables.
- **A routine is loaded, and fails, as a set.** `work` refuses to start while
  any routine or profile is invalid. That applies the ADR 0020 rule "refuse
  before claiming" to definitions as well as to tasks. Once `work` is
  running, a definition that breaks mid-session is reported once, and the
  last good set is kept.

`work --every S` now idles until the sooner of S seconds and the next due
routine.

**Subcommands:**
- `trellis routines list` shows the schedule, timezone, policies, next due
  instant and last fire.
- `trellis routines run <name>` fires the routine now. Concurrency still
  applies, as in Paperclip's manual run.

### 8. What a trail now records

`Factual` gains two optional fields. Each is omitted when it is `None`, the
same shape `control` uses, so an older trail keeps its shape and its digest:
- `tokens: Option<u64>`: completion tokens, as §5 defines them. `None` for a
  scripted run and a process run.
- `agent: Option<String>`: the profile the run was executed as.

`UntrustedFactual` reads both back. This is the minimal additive change to
`trellis-trail`. The CLI changes that carry the fields are:
- `TrailFacts` gains two fields.
- `cmd_run` reads `tokens_used()` from its driver after the loop.
- `RunSeeding` gains the profile reference (§4).

These are edits to `main.rs` outside the "mod lines, variants and dispatch"
rule for this wave, and they are listed as such.

*As implemented, the complete list of `main.rs` edits outside mod lines,
variants and dispatch:*
- the `RunSeeding` field, and `profile: None` in the `run` arm's literal
- the destructure in `cmd_run`
- the approve-list override after `cfg.control`
- the skill `retain`
- the `tokens` read
- the two `TrailFacts` fields, and their use in `record_trail` and in one
  test helper
- the `Queue` variant's doc comment, which now names ADR 0026

### 9. The activity log

`<git-dir>/trellis/activity.jsonl` gets one line per mutation:

```json
{"at":"…","actor":"human|work|routine","action":"task.add","entity":"task:<id>","run_id":null,"detail":"…"}
```

**What is logged:**
- task add, set, note, cancel, claim, finish, requeue, wake
- approval request, approve, reject
- agent pause and resume, including budget pauses and resumes
- routine fire, coalesce, skip and missed-drop
- import

`trellis queue log [--limit N]` prints the newest N lines (default 20), with
control characters escaped. The log lives in the git directory for the same
reason as the queue.

### 10. Export and import

`trellis queue export <dir>` writes:
- `agents/*.toml`, and each profile's `instructions` file under
  `agents/instructions/`
- `routines/*.toml`
- `tasks/<state>/<id>.json`

It does not export overlays, routine state, approval requests or the
activity log. Those are this checkout's runtime state, not definitions.

**Nothing secret exists to scrub.** Profiles name a model spec, and a key
never goes in a file trellis reads (ADR 0010). Paperclip scrubs secrets and
paths on export. Trellis has no secrets to scrub. The paths it exports are
relative to `.trellis/`.

`trellis queue import <dir> [--dry-run] [--on-conflict skip|replace|rename]`
reads the same layout:
- Profiles and routines are written into `.trellis/`, which leaves the
  worktree dirty. They are definitions a person commits, like any edit to
  `.trellis/`.
- Tasks are published into the queue.
- On a conflict, the default is `skip`. `replace` overwrites. `rename` gives
  the incoming item a new name or id and rewrites references to it within
  the imported set: `assignee`, `reports_to`, `parent`, `blockers` and a
  routine's `task.assignee`.
- `--dry-run` prints the plan and writes nothing.

The profiles are validated after the import is planned, including cycles
across old and new profiles, and before anything is written. An import that
would create an invalid set is refused as a whole.

*Amended in implementation:*
- **Claimed tasks are not exported**, and the export says how many it left
  out. A task in `claimed/` belongs to a run in progress. A `claimed/`
  folder in an import is refused.
- **Export refuses a folder that is not empty.** An export mixed into
  another folder's contents would bring those contents along when it is
  imported.
- **An instructions file is shared by path, not owned by one profile.** An
  import that creates or renames a profile whose instructions path already
  exists locally with different content is refused as a whole. Only
  `--on-conflict replace` overwrites it.
- **Task ids from an import folder must be lowercase ASCII letters, digits
  and dashes.** Each id becomes a file name in the git directory.

### 11. Process agents

This is Paperclip's `process` adapter. A profile with
`command = ["prog", "arg", …]` instead of `model` runs its tasks like this:

0. **Trust** (*amended 2026-09-27*, ADR 0023 §4). The command is
   repository code, and before this amendment `work` ran it in a cloned
   repository with no trust decision, where a hook or a custom tool would
   not have loaded. Each process agent's argv, and the files it names, is
   now an item in the trust fingerprint (`process agent <name>`, shown by
   `trellis trust`). A task assigned to an untrusted process agent is not
   claimed: it stays pending with the reason `agent <name> runs a
   repository command; not trusted: run trellis trust`, and the runner
   checks again before it spawns.
1. **Snapshot** (ADR 0009). The worktree must be clean, exactly as for a
   model run.
2. **Run** the command in the worktree root. Its stdin is the task text,
   with the instructions file prepended, and its environment has
   `TRELLIS_TASK`, `TRELLIS_TASK_ID` and `TRELLIS_AGENT` set. Its stdout and
   stderr go to `work`'s terminal.
   - It runs under `timeout_s` (default 1800). On timeout the process is
     killed.
   - `Child::kill` terminates the direct child only. On Windows, and on Unix
     for a process that ignores its group, a grandchild can outlive it. That
     is recorded here, not solved.
   - Ctrl-C kills the child and rolls back.

   *Amended (wave K): the environment has no keys unless the profile
   passes them.* The command's environment is trellis's own less every
   hosted key variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and every
   `[[providers]] key_env`), removed as for every other child (ADR 0024
   §3). An external agent may legitimately need a key of its own, so the
   profile field `pass_env = ["NAME", …]` puts back exactly the named
   variables, each only if it is set in trellis's environment. The default
   is none.

   Passing a key is the profile owner's explicit choice, and it is made
   knowing this: **whatever a process agent prints is recorded.** Its
   stdout and stderr are not captured or scrubbed by trellis; they go
   straight to `work`'s terminal, and from there into whatever keeps that
   output (scrollback, a CI log, a file `work` was redirected to). What it
   writes into the worktree is left for review if the run converges. A
   process agent given a key and told to print its environment will print
   the key. `agents list` and `agents show` name the passed variables,
   never their values. *Amended after the third review of PR #1:* so does
   `trellis trust`. The process agent's trust item carries `pass_env`, and
   the item prints `receives: ANTHROPIC_API_KEY` on its own line. Before
   this, the key was in the digest only because the profile file was, and
   nobody trusting the repository was told a key would be handed to it
   (`trust_names_the_keys_a_process_agent_receives`). Evidence:
   `a_process_agent_inherits_no_key_unless_its_profile_passes_that_one` in
   `crates/trellis-cli/tests/child_env.rs`.
3. **Check** with the grounded checks, the same pair `trellis check` runs:
   typecheck and tests over `.`.

   *Amended after the second review of PR #1: governance binds a command
   as it binds a model.* Before it, a process agent was judged on the two
   checks alone, and a `[governance] deny_write` path or a
   `.trellis/governance/` rule that refuses a model's edit did not refuse
   a command's. Now, over every path `git status` names after the command
   (untracked files one by one):
   - each path is matched against `[governance] deny_write` and every
     `deny-write` document; a match is a breach, named in the output;
   - when the repository has rule documents, the same `governance_audit`
     the loop runs at convergence produces a `Verdict` over those files,
     recorded with the other two.

   *Amended after the third review of PR #1: the judge is read before the
   command.* The `[checks]` commands, `deny_write` and the rule documents
   were read from the worktree after the command ran. A command could
   therefore rewrite what verifies it, for example by setting `typecheck`
   to a command that always passes or by deleting a `deny_write` line, and
   then converge. All three are now loaded before the command is spawned,
   as the `.git/` bytes below are, and a profile whose rules do not load is
   refused before anything runs. Any changed path under `.trellis/` is a
   breach, whatever the rules say, because that directory holds the checks,
   the rules and the trust inputs that judge the run. The run does not
   converge and is rolled back
   (`a_process_agent_that_rewrites_its_own_checks_does_not_converge`). What
   is not covered, stated: the repository's own test and typecheck
   configuration outside `.trellis/` (`pyproject.toml`, `conftest.py`, the
   tests themselves). A command can change those, just as a model run can,
   and the diff that is left for review is where that shows.

   **`.git/` is outside the worktree the rollback restores**, and `git
   status` does not see it, so a command could plant a hook or set
   `core.hooksPath` and survive any rollback. The bytes of `.git/config`
   and of every file under the hooks directory (both as `git rev-parse
   --git-path` names them) are read before the command and compared
   after. A difference is put back (the old bytes rewritten, a new file
   removed), named in the output, and makes the run not converged. What
   is not covered, stated: the rest of `.git/` (`info/exclude`, refs,
   objects, `packed-refs`, the index), a change the command makes and
   reverts before it exits, and a file that cannot be written back, which
   is reported as not put back. A process that outlives the command (see
   step 2) can still write after the comparison.
4. **Decide.** The run converged if and only if the command changed at least
   one file, both checks pass, the governance audit (when there is one)
   passes, no changed path is denied or under `.trellis/`, and nothing
   under `.git/config` or the hooks changed.
   - A command that changed nothing has nothing to verify, and ends `failed`
     with that reason.
   - On anything other than convergence, the tree is rolled back exactly as
     after a failed model run.
   - The command's exit code is recorded. It is not evidence of success.
5. **Record** a trail, built in the CLI from `trellis-trail` types. Its
   fields:
   - `proposed`: the changed paths
   - `check_outcomes`: read from each `Verdict`'s `passed()` and kind
   - one `process` action, carrying the argv, the exit status and the
     duration
   - `agent`: the profile
   - `tokens`: none

   As with a model run, the trail is committed and nothing else is.

**The external agent never produces a verdict (ADR 0004).** Its exit code,
its output and anything it writes are data. Convergence comes only from the
two `Verdict`s that `trellis-checks` constructs from processes it ran itself.

## What is not built

- **IANA timezone names** (§7).
- ~~**The per-profile `tools` list takes no effect** until W-B's registry
  reads it (§4).~~ Integrated; see below.
- ~~**A profile's skill deny list does not stop `read_skill` by exact name**
  (§4). It only narrows the list the model is shown.~~ Integrated; see
  below.
- **`@profile` mentions in notes do not wake anything.** ADR 0021 defers
  them.
- **No prompt-token or dollar accounting** (§5).
- **Routines fire only while `work` is running.** There is no daemon
  (ADR 0020). A schedule with no `work` running accumulates missed fires,
  which the catch-up policy then handles.
- **Webhook and API triggers.** ADR 0021 excludes them.
- **Grandchildren of a timed-out process agent are not reaped** (§11).

## Alternatives rejected

- **Rewriting the profile TOML to pause a profile.** Rejected: it dirties
  the worktree (§4).
- **Treating `failed` as not terminal for parent wakes.** Rejected: nothing
  moves a failed child again without a person, so the parent would wait
  forever.
- **A token budget for process agents, enforced by reading the process's own
  report.** Rejected: that is a number the external agent chooses, and the
  budget would then be the external agent's own opinion of its spend.
- **Joining tasks to trails to attribute spend, instead of recording the
  profile in the trail.** Rejected: a trail outlives the queue, which is per
  worktree and is lost when a linked worktree is removed (ADR 0020 §1). The
  trail is the durable record, so it says who ran.

## Consequences

- `trellis agents` and `trellis routines` are new subcommands, and `queue`
  grows. This was owner-directed, like ADR 0020.
- A trail may now carry `tokens` and `agent`. Readers that predate this ADR
  ignore them.
- `work` does more on each tick: it evaluates routines, runs
  `reconcile`, checks budgets and reads every pending file. Each is a
  directory listing and small file reads.

## Integrated

The two §4 items this wave left inert, closed after ADR 0023's registry
landed. Tested unless stated.

- **`tools` narrows the run's tool set.** `ToolSet::narrowed` intersects the
  set `[tools]` built, after trust, with the profile's list. It is called in
  `cmd_run` right after `tooling::prepare` and before the driver is built,
  so the prompt, the schema, the script parser and the orchestrator all get
  the narrowed set, and bash joins the approve list only if it survived.
  - **Only narrows.** A listed tool the repository did not enable, or did
    not trust (bash, a custom tool), is not added. The run's `tools:` line
    names it: `narrowed by agent profile `x`; not added, because this
    repository does not offer them: grep`
    (`a_profile_cannot_widen_the_repository_s_tools_and_the_line_says_so`,
    `a_profile_tool_list_can_never_widen_the_repositorys_set`).
  - **`run_checks` is always kept,** listed or not, for the reason ADR 0023
    §1 refuses to disable it. §4's own example, `tools = ["read_file",
    "edit"]`, omits it, and refusing that list would have made the example
    invalid; the `tools:` line says it was kept.
  - **An unknown name is an error**, not a no-op, as in `[tools]`: checked
    against every built-in and every custom tool the repository declares,
    trusted or not, when profiles are loaded (`agents list`, `work`,
    `doctor`) and again at run start.
  - **The orchestrator refuses the rest** with the same "not a tool in this
    run" as any call outside the set
    (`a_profile_tool_list_narrows_the_run_and_the_orchestrator_refuses_the_rest`).
    A script that calls a removed tool fails before the snapshot
    (`a_profile_without_edit_cannot_edit_and_the_run_costs_nothing`).
  - **A process profile may not have a `tools` list.** Refused at
    validation, for the budget's reason: it does not call trellis's tools,
    so the list could never be enforced, and an unenforceable restriction
    reads as one that holds.
- **The skill filter is enforced at `read_skill`.** The profile's
  `[skills]` becomes a `SkillPolicy` in `trellis-loop`, used both to filter
  the listing and by the orchestrator, which serves `read_skill`; one
  predicate, so the two cannot drift. A denied name is an error observation
  naming the profile and the list: `skill `refactor` is denied to agent
  profile `reviewer`: its [skills] deny = ["refactor"]`, and a name outside
  an allow list says `allow = [...]` instead
  (`read_skill_by_exact_name_is_refused_when_the_profile_denies_it`,
  `a_profile_s_denied_skill_is_refused_at_read_skill_in_a_real_run`).
  `--skill` forcing is not filtered: it is a person at the terminal, not
  the profile, and `work` never forces a skill.
- `agents show` no longer says either field is inert.
