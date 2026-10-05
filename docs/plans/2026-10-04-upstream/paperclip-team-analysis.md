# Paperclip track: team analysis (2026-10-04)

Upstream: `paperclipai/paperclip` at `1c07b590` (2026-10-04). Reference rows: ADR 0021 (Paperclip section) and ADR 0026. Trellis at drain on 2026-10-04.

Tickets in this track, by key:

- CLIP-01: Update the ADR 0021 and ADR 0026 rows that trellis has outgrown
- CLIP-02: Add the 2026-10-04 Paperclip rows PC-101 to PC-144 to ADR 0021
- CLIP-03: Give a re-run task the notes a person added since its last run
- CLIP-04: Block a task after its third stale-claim release instead of releasing it again
- CLIP-05: Gate a routine's scheduled fire on commits since its last fire
- CLIP-06: Add queue cancel --children to cancel a task and its pending descendants
- CLIP-07: Ask whether a process profile may drive an agent over ACP
- CLIP-08: Ask whether ask_human may carry several questions in one call
- CLIP-09: Ask whether an agent profile may name a memory note its runs read and edit
- CLIP-10: Ask whether work runs each queued task in its own worktree
- CLIP-11: Name the runtime state queue export leaves behind, paused profiles first
- CLIP-12: Add queue list --text to filter tasks by a substring of their text or notes

## Product Manager

**Who this serves.** The one owner and operator who runs trellis on one Python repository and uses `queue`, `agents`, `routines` and `work` to keep agents busy unattended. Paperclip is the reference for that orchestration (ADR 0026), not a product trellis matches feature for feature.

**What the re-inventory says.** 144 capabilities: 44 built in trellis, 22 built in part, 13 missing, 65 excluded. Most of what moved upstream since v2026.916.1 sits behind clauses trellis already applies: networked services (chat channels, email, Browser Use, memory connectors, GitHub review bots), multi-user governance, the web UI, and the plugin and connector platform. Those need a row in ADR 0021, not code.

**What matters to the user, in order.**

1. **The map understates trellis.** Nine Paperclip rows and the W-E bullet in ADR 0021 still say "deferred" for work that merged (#47, #55, #57, #59, #60, #108, #182, #183). A reader underrates trellis and may re-file built work. CLIP-01 fixes that with no owner input.
2. **A gap in a flow trellis already ships.** `@reviewer` in a `queue note` wakes the task (#57), but the woken run never sees the note; nor does a run of a task a person sent back from `in_review` with a request. Upstream kept comments as context while it stopped mention-started runs (#14577). CLIP-03.
3. **A pause lost without a word.** `queue export` drops a profile's overlay pause, and an import elsewhere runs the agent. CLIP-11 makes export say so.
4. **Robustness and noise.** A task that kills its holder is released once an hour without limit (CLIP-04); a routine fires on nights when nothing changed (CLIP-05).
5. **Ergonomics.** Cancel a whole breakdown at once (CLIP-06); find a task by text (CLIP-12).

**Owner questions where upstream moved a reference point:** mention wakes (#14577, inside CLIP-03), ACP for process profiles (CLIP-07), several questions per `ask_human` (CLIP-08), a profile memory note (CLIP-09), a worktree per queued task (CLIP-10), and acceptance of the 44 new rows (CLIP-02).

**Not re-asked, because no new upstream fact touches the recorded answer:** MCP and connectors (#73, 2026-09-28), the sandbox order of clone first and containers later (#20), starter profiles (#55, #183).

## Principal System Architect

**Boundaries.** Every code ticket stays in `trellis-queue` and `trellis-cli`. None touches `trellis-checks` (the `Verdict` seal), `trellis-index`, `trellis-trail` or the rollback paths in `trellis-vcs`. No ticket adds a crate dependency: HEAD and changed paths come through existing `trellis_vcs` calls (`command()`, `tree_diff`), and an ACP client, if the owner takes CLIP-07 (b), needs only `serde_json`, which the workspace has.

**On-disk formats, stated per ticket.**
- `Task` has no `deny_unknown_fields`. CLIP-04's `stale_releases` is additive: an older binary reads the file and drops the field when it rewrites the task, which at worst grants up to three more releases.
- `RoutineFile` and `TemplateFile` have `deny_unknown_fields`. CLIP-05's `activity_gate` makes an older binary refuse the routine, and `work` refuses to start while any routine is invalid. The ADR 0026 §7 amendment says so.
- `RoutineState` lives in the git directory and gains `last_head` additively.
- CLIP-08 (b), if taken, changes the `question` blocked reason; ADR 0026 §1 already records how an older binary treats a reason it cannot parse.

**The prompt contract.** CLIP-03 changes ADR 0026 §1's sentence that a task that never asked gets the text it always did. Person notes enter as human input, attributed and indented inside one fence; model notes stay Debug-quoted. A run cannot write a person's note (#189). That check's stated limit, a child that strips `TRELLIS_RUN_ID` and `TRELLIS_TASK_ID` on purpose, now reaches the next prompt as well as a wake, and the amendment must say so. The fix for that writer stays isolation (ADR 0032), not another check.

**Grounding.** Nothing here produces a verdict. Answers, notes and memory notes are input to a run, never a pass (ADR 0004). The routine gate and the stale-claim block are scheduling choices about whether to run, not claims about correctness.

**ACP (CLIP-07).** ADR 0021 excluded the adapters because each wraps one agent. ACP is one JSON-RPC protocol over stdio that Claude Code, Codex, Kimi, Cursor, Copilot, Pi and Grok Build speak through Paperclip. In ACP the agent asks the client before it uses a tool (`session/request_permission`, answered in Paperclip's `packages/paperclip-runner/src/drivers/acpx/codex-acpx-driver.ts:1720-1723`), so a process agent's tool use could meet the `[control]` gate during the run. The throwaway clone and carry-back (ADR 0032) stay as the path every write comes back through; messages go to the trail as untrusted data (ADR 0007); only checks converge. Paperclip's runner protocol (PRP) and its session goals belong to its control plane and stay out.

**Per-task worktree (CLIP-10).** ADR 0020 §1 keeps the queue per git directory, and a linked worktree has its own git directory, so any follow-up ADR must settle where claims live. ADR 0037 already creates and discards linked worktrees for partitions. ADR 0026 §12's pointer to #24 is wrong (#24 is the partition work, #98–#101); CLIP-01 corrects it.

**Exclusions held.** Each new row in the ADR 0021 amendment cites a clause ADR 0021 already applies. Where an exclusion's clause could be read as no longer fitting (rows 10–11), the question goes to the owner (CLIP-07) rather than into a row.

## Senior Full-Stack Engineer

- **CLIP-03.** Replace `Task::answers_since_question` with `Task::input_since_last_run` in `crates/trellis-queue/src/lib.rs`. The anchor is the later of the last `Actor::Model` note and the first note after `outcome.finished_at`; compare with `parse_iso` rather than string order if the formats could differ. Keep only notes with `actor == Human` and `id.is_some()`, so the queue's own `human` records stay out. Keep the existing fence, `plain()` and indentation. Update the one call in `text()` in `crates/trellis-cli/src/work.rs`. Tests in `crates/trellis-queue/src/tests.rs` named `input_since_last_run_*`; the mention case can call `mention::wake_and_say`, which is `pub(crate)`.
- **CLIP-04.** Add `stale_releases` beside `stale_released_at` with the same serde attributes, and `MAX_STALE_RELEASES` beside `RELEASE_THROTTLE_SECS`. In `release_dead`, after the re-read under the task lock, branch: at the limit set status `Blocked` with `Blocked::Manual { detail }`, otherwise increment and release. Keep the "its holder finished it" check on both branches. Tests reuse the injected clock and liveness probe the five `stale_claim_release_` tests use.
- **CLIP-05.** Parse `activity_gate` into an enum at load, as `concurrency` is. `evaluate` takes the HEAD and a changed-paths reader from the caller, the way it takes `now`; `work.rs` supplies them through `trellis_vcs` (`rev-parse HEAD` through `command()`, `tree_diff` over the two commits). Any error reading them means fire. Record `last_head` only when a task is enqueued. `run_now` ignores the gate. Unit tests in `routine.rs` inject heads and path lists, so they need no repository.
- **CLIP-06.** `Queue::cancel_tree(id, reason)` reads every task file once to build the parent-to-children map, plans the whole set, refuses before writing if any member is in `claimed/`, then cancels each `pending/` member through the existing cancel path under §2's lock protocol. The flag goes on `QueueAction::Cancel`. Integration tests in `crates/trellis-cli/tests/queue.rs` through the `trellis()` helper.
- **CLIP-11.** In `portable::export`, count `<git-dir>/trellis/agents/*.json` overlays (and which say paused while the profile file does not), routine state files, approval request files and activity-log lines. Print in the `QueueAction::Export` arm through `plain()`, since profile names come from file names.
- **CLIP-12.** Filter in the `QueueAction::List` arm; lowercase both sides; unreadable files stay listed. Tests named `queue_list_text_*` in `crates/trellis-cli/tests/queue.rs`.
- **Everywhere.** Test names match the Verify filters exactly; run on Windows and Linux; `tests/hygiene.rs` and `tests/child_env.rs` stay green.

## QA / Security Engineer

- **Every Verify can fail today.** Each block's first line is a test filter that matches nothing yet, or a string checked absent from the named ADR on 2026-10-04: `Corrected 2026-10-04`, `#182`, `#55`, `#57`, `#59`, `#47`, `#108`, `#60`, `PC-101`, `PC-144`, `Agent Client Protocol` and `enableIsolatedWorkspacesByDefault` in ADR 0021; `14577`, `activity_gate`, `--children`, `--text`, `ask_user_questions`, `persistent agent files` and `Agent Client Protocol` in ADR 0026; `stale_claim_blocked` in ADR 0020. The amendment draft does not contain `Agent Client Protocol` or `enableIsolatedWorkspacesByDefault`, so landing CLIP-02 cannot satisfy CLIP-07 or CLIP-10. CLIP-02 waits on CLIP-01, so CLIP-01's markers come from its own corrections.
- **CLIP-03.** A note has the same trust as `queue add` text: anyone with the user's shell can write either. The risk is a run writing a person's note; #189 refuses the accidental path, and the amendment must say the deliberate path now reaches a prompt. Model text stays quoted, and person text cannot close the fence.
- **CLIP-04.** Blocks rather than fails, never releases a live claim, and changes nothing under the default `list` policy.
- **CLIP-05.** Fails open: any git error, a missing `last_head` or a rewritten history fires. A process agent that commits outside `.trellis/runs/` by absolute path can only make a routine fire, which is today's behaviour.
- **CLIP-06.** Refuses before writing when any member is claimed; never touches `done/`, `failed/` or `cancelled/`. A task blocked on a cancelled descendant stays blocked, as ADR 0026 §1 rules for a cancelled blocker.
- **CLIP-11 and CLIP-12.** Output only; names and text print through `plain()`, so a file name cannot drive the terminal.
- **CLIP-07 (b), if taken.** A new trust surface: JSON-RPC from an untrusted process. Its ADR must bound message sizes, refuse writes into `.git/` and `.trellis/` if it serves file requests, resolve any requested path the way `resolve_repo_input` does, and pass no key beyond `pass_env`.
- **Upstream evidence** was spot-checked under the clone at `1c07b590` for every path cited in a ticket.
- **Coverage.** All 144 ids are in a ticket or in the uncovered list with a reason; the desktop-ui rows point at the desktop track.

## Next Steps & Action Plan

1. **Start now:** CLIP-01 (docs only, no owner input) and CLIP-11 (output only).
2. **Owner answers, in one sitting:** CLIP-02 (accept the 44 rows); CLIP-03, CLIP-04, CLIP-05, CLIP-06 and CLIP-12 (each a build ticket waiting on its option); CLIP-07 to CLIP-10 (questions only).
3. **Build after answers,** each through `scripts/gates/run-all.sh`: CLIP-03, CLIP-04, CLIP-05, CLIP-06, CLIP-12.
4. **Follow-ups an answer may create:** an ACP ADR (CLIP-07 b), a multi-question `ask_human` ticket (CLIP-08 b or c), a profile memory ticket (CLIP-09 b or c), a per-task worktree ADR blocked by #98–#101 (CLIP-10 b).
5. **Next re-inventory:** diff against `1c07b590` rather than a depth-1 clone, so changed rows are dated by commit.
