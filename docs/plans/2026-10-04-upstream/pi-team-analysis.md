## Product Manager

**Scope.** The re-inventory lists 136 pi capabilities at upstream commit 200387122ca4 (all packages at 1.0.2): 85 are NEW or CHANGED and 51 are UNCHANGED. Each one now has a ticket below or a reason in `uncovered_features`.

**Upstream's big changes mostly fall under clauses the owner already settled on 2026-09-28.**
- Codemode, nested calls and parallel calls: #63, row 1.7.
- The MCP client: #73, which asked about "exposing or consuming".
- Classifier and image models: #66, row 2.14.
- Durable subagents: #130.
- Remote sessions and Radius: #65.
- Login and stored credentials: #64.
- Session tree: #50.
- Model-written summaries: #153.
- Streaming: #151.

So most of this track is records and owner questions, not builds.

**The re-inventory text is wrong in five places.** The code and the closed issues correct it:
1. **MCP.** #73 asked about MCP "exposing or consuming", and the owner answered "Keep all excluded". The inventory says the existing exclusion covers only the server side.
2. **PI-1.2.** The inventory says "remaining: queue modes and clear_queue". ADR 0025 §5 records "No `clear_queue`, by the owner's decision" (`abort` already hands back everything queued). Only one-at-a-time queue modes are unbuilt, and nothing asks for them.
3. **PI-3.2.** The inventory says "port". #111 was closed NOT_PLANNED: "ADR 0025 §2 stands: system prompt files are repository-level only."
4. **PI-2.20.** The inventory says "no credential-readiness check". `trellis doctor` already prints "key: from X, set" or "NOT set" (crates/trellis-cli/src/hosted.rs:349-356).
5. **PI-14.4.** Prompt tokens are already counted (#105-#108). Only a per-session display is missing.

**What gets built.** Each item is small:
- `AI_AGENT=trellis` on tool children (PI-08).
- Session token totals per model in `/status` (PI-09).
- Two read-only rpc commands for front ends (PI-10).
- A `read_only` fact in hook payloads (PI-11).

**Owner questions.** Eight tickets are needs_owner (PI-02, 03, 04, 05, 06, 07, 10, 11). Each lists lettered options and recommends one:
- PI-03, PI-04, PI-05: keep excluded and record the rows.
- PI-06: defer.
- PI-07: keep fixed decoding.
- PI-10, PI-11: build only facts trellis already holds.
- PI-02: accept the proposed rows.

## Principal System Architect

**Where upstream's new features collide with trellis invariants:**
- **One call per turn** (ADR 0005, #63) blocks codemode, nested calls and parallel calls. The gate log is keyed by turn, and the trail's actions, rollback points and S5 write/check serialization all assume one call per turn.
- **Tool set fixed at run start** (crates/trellis-loop/src/registry.rs:1-11) blocks `tool_search`, deferred or hidden exposure, `prepareLoadout`, and MCP tool-list changes mid-run.
- **Key custody** (ADR 0010, ADR 0024 §3) blocks the new login flows and MCP's OAuth credential file. Workload identity federation is genuinely open: row 2.1-2.4 says "a key never goes in a file trellis reads", but its #45 amendment admits "the platform's own ambient chain, never a file trellis writes". The identity token file is written by the platform, not by trellis.
- **Grounded triggers** (ADR 0004, ADR 0043) block virtual models. Upstream routes each request on a model's reading of it. ADR 0043's "Alternatives rejected" refuses a trigger the model chooses, and its only model switch fires on a failing check.
- **Measurement evidence** (ADR 0028): a non-zero temperature voids the metric-5 waiver unless the trail records the value the adapter sent.
- **CLAUDE.md rule 2**: structured tool results add no pass variant, but their only consumer is codemode.
- **Grounding**: the `AI_AGENT` marker must never reach grounded checks. The Verdict has to describe a run a person can reproduce.

**Recommendations.**
- Keep every exclusion.
- Add facts, not judgments: a read-only table built from trellis's own code, token totals read from trails as untrusted data (rule 10), and read-only rpc commands.
- No recommended option adds a crate dependency. Codemode (PI-04 option C) would add a JavaScript engine, which needs an ADR and a dependency audit.
- All builds stay in trellis-cli and trellis-loop. trellis-index, trellis-vault and trellis-checks are untouched, so `Verdict` stays sealed and trellis-index keeps no trellis-model dependency.

## Senior Full-Stack Engineer

**Code check for UNCHANGED rows that ADR 0021 marks built: every one is present.**

| Row | Evidence |
|---|---|
| 1.1 turn loop | crates/trellis-loop/src/agent.rs |
| 1.3 built-in tools | `ToolCallWire` variants `ReadFile`, `Edit`, `WriteFile`, `Grep`, `Find`, `Ls`, `Bash` in crates/trellis-loop/src/wire.rs |
| 1.8 terminate | `Done` at wire.rs:91 and :180 |
| 1.10 overflow | `is_context_overflow` re-exported at crates/trellis-model/src/lib.rs:31 |
| 1.11 `!`/`!!` | crates/trellis-cli/src/session.rs:283-284 |
| 1.12 session env | crates/trellis-cli/src/tooling.rs:226-229 |
| 1.13 shell | `ToolsSection.shell` in crates/trellis-cli/src/config.rs |
| 2.5 credential precedence | crates/trellis-cli/src/user_config.rs:7, :381 |
| 2.9 model switching | session.rs:318-330, :379 |
| 3.1 context files | crates/trellis-cli/src/context_files.rs:25 `NAMES` |
| 3.3 system files | context_files.rs:412-413 |
| 4.5, 4.7, 4.9 hooks | crates/trellis-loop/src/hooks.rs: `before_run`, `post_tool` (:285), `input` (:340), `turn_end` (:370) |
| 5.1-5.4 skills | crates/trellis-loop/src/skills.rs, `ReadSkill`, session.rs:384 `/skill`, user_config.rs:58-83 |
| 9.2 JSON mode | crates/trellis-cli/src/output.rs framing |
| 9.5 approvals over rpc | crates/trellis-cli/src/rpc.rs:671 |
| 11.2 `@` file search | crates/trellis-cli/src/tui_run.rs:394 |
| 13.4 guard examples | config.rs:168 `deny_write` |
| 15.4 evals | docs/BENCHMARK_AND_EVAL.md, qa/run-classify.py |

**The only mismatch runs the other way: the ADR says "landing" while the code is merged.**
- Row 1.2: rpc.rs:535-580, agent.rs:582 `SteeringLog::drain`, tui_run.rs:251, session.rs:613-650, trellis-trail lib.rs:143.
- Row 2.13: transport.rs:159-243.
- Row 7: session.rs:349-360.
- The W-C and W-D bullets, the "Three rows name work" paragraph, and ADR 0034's status line.

PI-01 corrects all of these.

**Build notes:**
- **PI-08.** Add the marker to the env vector at tooling.rs:226 and to the `.env()` calls at process_agent.rs:270-272. tests/child_env.rs already dumps each spawn kind's environment. The tests must remove `AI_AGENT` from the binary's `Command`, so they hold when the suite itself runs under pi. ADR 0026 line 947 lists the process agent's variables. work.rs `RUN_MARKERS` stays unchanged on purpose.
- **PI-09.** `/status` is at session.rs:1034-1056. Trails carry `tokens`, `prompt_tokens` and `Provenance::model` (trellis-trail lib.rs:504). One totals function goes in session_extra.rs, and PI-10 reuses it.
- **PI-10.** Extend the `COMMANDS` table and `handle()` in rpc.rs. Tests run through the binary over stdio in tests/modes.rs.
- **PI-11.**
  - **Where the table goes:** next to `APPROVABLE_TOOLS` at gate.rs:178.
  - **Where hooks read the call:** `pre_tool` stdin is `call_json(call)` (hooks.rs:512); `post_tool`'s is `{call, result}` (hooks.rs:288).
  - **Why built-ins only:** a custom tool's call object carries the repository's own argument names (registry.rs:827-830), so a `read_only` key could collide with one of them.

**Effort.** PI-08 m (5 files), PI-09 s, PI-10 m, PI-11 m, each docs ticket s.

## QA / Security Engineer

**Pre-check at a8a4b63.**
- Every docs anchor in the Verify blocks exits 1 today: "built in every part named above", "proxy built (#150)", "consuming MCP as well as exposing it (#73)", "16.1-16.7 MCP client", "2.15 Anthropic workload identity federation", ADR 0025's "get_session_stats", ADR 0023's and ADR 0026's "AI_AGENT", and the rest.
- No test matches today's filters: `agent_marker`, `session_status_tokens`, `rpc_get_session_stats`, `rpc_get_last_assistant_text`, `read_only_table`, `hook_payload_read_only`.
- So every Verify fails now and passes only once its work lands. `verify-tests.sh` carries a minimum count, so a filter that matches nothing fails.

**Verify allow-list.** Every Verify line starts with `git grep -q -F`, `cargo` or `scripts/tickets/verify-tests.sh`. None contains `| ; & < > $(`, a backtick, or `..`. No decision-word stem (design, decid, investigat, explor, consider) appears in any body, Question sections included.

**Labels to expect.**
- PI-09 touches session*.rs, which is on the risk path table (#196), so it labels risk:high.
- The trellis-cli tickets label surface:cli.
- PI-11's trellis-loop library `src/` paths make surface abstain, so it routes to needs-human.

**Security.**
- `AI_AGENT` tells a child that an agent launched it. It carries no key and no path, and grounded checks do not get it.
- `read_only` is a fact handed to hooks, not a permission. `[control] approve` (ADR 0018) is unchanged.
- PI-09 and PI-10 read only numbers and the `done` summary from trails, as untrusted data. No trail text reaches a prompt.
- Open questions with a security edge:
  - **PI-03, MCP:** a long-lived connection, remote tool schemas the model sees unreviewed, and per-server credentials.
  - **PI-06:** a token exchange on the one path that carries a credential.
  - **PI-04 option C:** in-process execution of model-written JavaScript. Today model-written commands run only through `bash` in a throwaway clone (ADR 0032).

**Grounding contract.** No ticket touches `Verdict`, trellis-index's dependencies, radius ratios, budget division, or trail trust.

**Landing order.** PI-01's anchor is specific to the correction, so a PI-03 row cannot satisfy it. Each question ticket puts its row under the "pi 1.0.2 re-inventory (2026-10-04)" heading, creating it if PI-02 has not landed.

## Next Steps & Action Plan

1. **PI-01 now.** Docs only, no owner question; mechanical under ADR 0021's rule that a row moves to built once its work is merged.
2. **PI-08 and PI-09.** Builds with no owner question.
3. **Owner answers.** PI-03, PI-04, PI-05, PI-06, PI-07, PI-10 and PI-11. The recommended options keep every exclusion and add only facts. A recommended answer means the ticket records its rows. Picking B or C rewrites the ticket's Done when and Verify before work starts.
4. **PI-10 after PI-09** (they share the totals function). **PI-11 after its answer.**
5. **PI-02 last**, after PI-08, PI-09 and the owner's acceptance. It replaces docs/inventory/inventory-pi.md and adds the remaining new rows.
6. **When filing**, replace the track keys PI-01 to PI-11 with issue numbers in every ADR citation.

**Not ticketed, because the owner has already answered them:** clear_queue, user-level system.md (#111), login and OAuth (#64), cloud signing and the llama.cpp router (#45), the session tree (#50), model-written summaries (#153), HTML export (#143), streaming (#151), and the subagent tool (#130).
