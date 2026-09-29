# ADR 0021 — Inheriting pi, Apex and Paperclip: the capability matrix

**Status:** Accepted.
**Date:** 2026-09-27
**Inventories:** `docs/inventory/inventory-pi.md` (pi 0.87.1),
`docs/inventory/inventory-apex.md` (apex-notes `f449644a`),
`docs/inventory/inventory-paperclip.md` (Paperclip v2026.916.1). All three
are MIT-licensed. Each inventory was read from its project's own source and
docs, not from memory.

## Where this stands (2026-09-28)

**After review.** PR #1 went through eleven rounds of code review. Each
round's findings were fixed with a regression test and merged, and every
merge passed CI on Linux and Windows. The findings split into two classes:

- **Trellis's own code.** This covers config, the queue, the transport,
  templates, the vault and the model tool paths. Findings here got fewer and
  less severe with each round.
- **Writers trellis does not control.** These are `bash`, custom tools,
  hooks and process agents: unsandboxed processes with the user's authority
  and write access to `.git`. Rounds 5–11 kept finding new ways for them to
  get past git-based checks, and no finite set of checks closes that.

For the second class, ADR 0023 §5 and ADR 0026 §11 now state the limit as a
first-class section, "What rollback guarantees when trellis is not the only
writer". ADR 0009's guarantee holds for trellis's own edits and is
best-effort for these writers. The real fix is sandboxing, row 13.3 below,
which is still deferred. The rows below for `bash`, hooks, custom tools and
process agents mean "built, with that limit".

**When the waves landed** (2026-09-27):

Every wave has landed on branch `inherit` (PR #1): W-A `2aa7b72`, W-B
`749e5a0`, W-C `69f585b`, W-D `8147c7f`, W-E `773abb0`, and the cross-wave
integration `cf68b00`. A row below marked W-x means **built under that
wave's ADR**, except where the ADR says otherwise. Each of ADRs 0022–0026
has an "As built" or "Not built" section, and it overrides this table
wherever the two disagree. The items those sections name as not built are:

- **W-A (0022):** auto-creating a note for a newly typed link (row 22).
  There is no typing event on a command line; `vault new` is the explicit
  form.
- **W-B (0023):** per-item trust (one digest covers hooks, custom tools and
  bash together); tracking what bash writes in the worktree; `after_run`
  on Ctrl-C. (A configurable custom-tool timeout was listed here; #135
  built it, and 0023 "As built" records how.)
- **W-C (0024):** streaming, `HTTPS_PROXY`, a `Retry-After` given as a date,
  summaries written by the model, and OAuth/Bedrock/Vertex/Azure. (The
  `ask` classifier on hosted models was built by #41.) Hosted runs have
  been exercised only against loopback stubs, never a real provider.
  Amended 2026-09-29 (#160): `HTTPS_PROXY` is landing (#150), not yet
  merged; row 2.13 below says so.
- **W-D (0025):** `/new`, `/name`, `/copy`, RPC `compact`, and a user-level
  `system.md`. Amended 2026-09-29 (#160): `/name` and `/copy` are landing
  (#109), not yet merged.
- **W-E (0026):** IANA time-zone names (UTC and fixed offsets only),
  `@profile` mentions, prompt-token and dollar accounting, and webhook or API
  triggers. Routines fire only while `work` runs.

## Context

The owner directed that trellis inherit every capability of the three
reference projects, as native Rust, with none of them running as a
dependency. ADRs 0017–0020 inherited the first slice. The inventories list
roughly 300 items.

Some of those items are literal v1 non-goals in the brief (Part 1: "any
networked component", "multi-user anything", "any surface beyond the CLI").
Some would contradict an accepted ADR. Others are desktop cosmetics.

Declining such an item without saying so would read as though the item had
been built. Building it would break guarantees that other ADRs rest on. So
every inventoried item gets exactly one disposition here, and the owner can
override any row.

## Dispositions

- **built** — in trellis before this ADR.
- **W-A … W-E** — built in this round, under the ADR named for that wave.
- **deferred** — fits trellis, but is not built this round. The reason is
  given.
- **excluded** — contradicts the brief or an ADR, or is cosmetic. The clause
  is given.

## Waves

| Wave | Scope | ADR | Owns |
|---|---|---|---|
| W-A | Apex vault parity | 0022 | `trellis-vault`, `trellis vault` subcommand module |
| W-B | Tool registry, pi's built-in tools, trust store, executable hooks, custom tools | 0023 | `trellis-loop` tools/wire/orchestrator/agent, new trust module |
| W-C | Hosted providers (native tool calling, tier 1), compaction | 0024 | `trellis-model`, `trellis-loop/src/adapter.rs` — **after W-B lands** |
| W-D | Context files, system prompt files, prompt-template arguments, JSON output mode, RPC mode | 0025 | new `trellis-cli` modules |
| W-E | Paperclip orchestration: task model, agent profiles, routines, approvals, budget ledger, activity log, export/import, process agents | 0026 | `trellis-queue`, `trellis-cli/src/work.rs` |

## pi (`inventory-pi.md`)

| Items | Disposition |
|---|---|
| 1.1 turn loop | built (`trellis-loop`, ADR 0005) |
| 1.2 steering / follow-up queues | deferred: `session`/`tui` can accept input mid-run only after a reverse channel exists for more than approvals; ADR 0018's channel is the base. **Amended 2026-09-29 (#160):** owner-approved 2026-09-28 to build in parts. Delivery of queued input between loop turns in `trellis-loop` is landing (#113), not yet merged. Still to come: `steer` and `follow_up` over `trellis rpc` (#114), queued input in `tui` (#115) and in `session` (#116), and recording it as user input in the trail (#117). |
| 1.3 built-in tools read/edit | built; **write, grep, find, ls, bash → W-B** (bash is opt-in, see ADR 0023) |
| 1.4 tool selection flags | W-B (`[tools]` enable/disable) |
| 1.5 output truncation | built (2,000-character tail, and a separate skill window, ADR 0019) |
| 1.6 swappable tool backends (ssh, micro-VM) | deferred: needs the sandbox decision (S-series) first |
| 1.7 parallel tool calls | excluded: one call per turn is the loop shape (ADR 0005). **Amended 2026-09-29 (#63):** the owner kept it excluded on 2026-09-28, and the shape carries weight beyond ADR 0005. ADR 0023's gate log records a denial against the turn number, because the loop makes exactly one `Request::Call` per turn; the trail's actions, the rollback points and the write/check serialization (S5) assume one call per turn too. Building it needs an ADR that first gives the gate log and the trail per-call ids and serializes writes, with a test that two edits to one file never interleave. |
| 1.8 terminate flag | built in effect (`done` ends a run; convergence is decided by checks) |
| 1.9 retry/backoff | built (format retries); provider backoff → W-C |
| 1.10 overflow recovery / compaction | W-C |
| 1.11 `!cmd` user shell | W-D (`session` `!` and `!!`) |
| 1.12 session env for model shells | W-B (`TRELLIS_RUN_ID`, `TRELLIS_MODEL` in the bash tool's environment) |
| 1.13 shell config | W-B (`[tools] shell`) |
| 1.14 images | deferred: no local runtime in the tier matrix takes image input |
| 1.15 prompt-cache warming | excluded: a dollar heuristic for hosted APIs; COST_MODEL.md owns cost. **Amended 2026-09-29 (#67):** deferred to v1.x. Its trigger is a dollar estimate, and no dollar figure is shown until COST_MODEL.md has a sourced per-model price table and prompt tokens are counted. |
| 2.1–2.4 30+ providers, OAuth, cloud platforms | **W-C covers the two wire protocols most of them speak** (OpenAI-compatible chat completions, Anthropic Messages) over HTTPS with keys from environment variables. OAuth and device login are excluded because ADR 0010 says a key never goes in a file trellis reads. Bedrock, Vertex and Azure request signing are deferred. **Amended 2026-09-29 (#45):** the owner kept signing deferred on 2026-09-28 and took per-model input limits first (#162). A signing scheme must keep ADR 0010 (credentials come from the environment or the platform's own ambient chain, never a file trellis writes), and is accepted only with its signing tested against recorded vectors and a loopback stub. |
| 2.5 credential precedence | W-C, environment only (ADR 0010) |
| 2.6 `models.json` endpoints and overrides | built (`[model] spec`, catalog); overrides → W-C |
| 2.7 catalog overlay | built (`trellis model`) |
| 2.8 thinking levels | W-C, where the provider protocol carries it |
| 2.9 scoped model cycling | W-D (`session` `/model` already switches; add a list) |
| 2.10 llama.cpp router | built in part (`trellis model pull`); router deferred. **Amended 2026-09-29 (#45):** the owner kept the router deferred on 2026-09-28. Listing, loading and unloading models on a running llama.cpp router is not built; `trellis model pull` is the download half. |
| 2.11 Radius gateway | excluded: networked service we would control (ADR 0010) |
| 2.12 custom providers | W-C (a `[[providers]]` table naming a protocol and base URL) |
| 2.13 transports / proxy | W-C (HTTPS; proxy from the environment); websocket excluded. **Amended 2026-09-29 (#160):** W-C did not build the proxy (see "Where this stands"). `HTTPS_PROXY` and `NO_PROXY` are landing (#150, an ADR 0024 §1 amendment), not yet merged. |
| 2.14 pi-ai extras | constrained sampling built (llguidance, tier 2); image generation and classification excluded (not a coding-loop concern) |
| 3.1 context files AGENTS.md / CLAUDE.md | W-D |
| 3.2–3.3 SYSTEM.md / APPEND_SYSTEM.md, flags | W-D (`.trellis/system.md`, `.trellis/append_system.md`; the grounding clause cannot be removed) |
| 3.4 sectioned prompt | built (the adapter's fenced sections) |
| 4 extensions: `tool_call`, `tool_result`, `before_agent_start`, `agent_end`, `turn_end`, `input`, `registerTool`, `registerCommand`, `exec`, `get/setActiveTools` | **W-B as executable hooks behind a trust store** (language-agnostic, JSON on stdin/stdout), plus custom command tools |
| 4 remaining events (message rendering, provider headers, UI widgets, shortcuts, renderers, themes) | excluded: cosmetic or TypeScript-runtime specific; `before_provider_request` is excluded because a hook could exfiltrate the key |
| 5.1–5.2 skills format, progressive disclosure | built (ADR 0019) |
| 5.3 `/skill:name` force-load | W-D (`run --skill <name>` and session `/skill`) |
| 5.4 extra skill locations | W-D (`~/.agents/skills` read-only, after trust) |
| 6 prompt-template arguments | W-D |
| 7 slash commands | built in part (`session`); `/new`, `/name` and `/copy` → W-D; `/login`, `/logout`, `/share` and `/bug` excluded (ADR 0010, networked). **Amended 2026-09-29 (#160):** W-D did not build `/name` or `/copy`; both are landing in `trellis session` (#109), not yet merged. |
| 8.1–8.4 session tree, branching, fork | deferred: the trail is the record, and each run is a fresh loop. A branchable session is v2 memory work (v0.3 closed without it) |
| 8.5–8.7 compaction | W-C |
| 8.8 export and share | export → W-D (`trellis log --json`); share excluded (networked). **Amended 2026-09-29 (#143):** HTML export excluded. `trellis log --json` is the export, and an HTML view can be rendered from it outside trellis; a renderer inside trellis would have to escape untrusted trail text (ADR 0007) for a browser, a new injection surface for nothing the JSON lacks. |
| 9.1 print mode | built (`trellis run`) |
| 9.2–9.3 JSON event mode | built as `--log-json`; stdout JSONL → W-D (`--json`) |
| 9.4–9.5 RPC | W-D (`trellis rpc`, a subset: prompt, abort, get_state, get_messages, set_model, compact, approve) |
| 9.6 SDK | built in effect (the crates are libraries) |
| 10 packages | deferred: `trellis install` for skills and prompts needs the trust store first; revisit after W-B |
| 11 TUI features | excluded [cosmetic]; `@` file search is built in `tui` (paths from `git ls-files`, so ignored files are never offered; #52), and `session` line mode has none |
| 12 settings layering | W-D (user-level `~/.config/trellis/config.toml` under the repo's) |
| 13.1–13.2 trust | W-B |
| 13.3 containers | first step built: `bash` and custom tools run in a throwaway git clone (ADR 0032); containers, for network and home-directory isolation, still deferred |
| 13.4 guard examples | built in effect: `permission-gate` = ADR 0018, `protected-paths` = `deny_write`, `dirty-repo-guard` = ADR 0009, `git-checkpoint` = snapshot and rollback. **Amended 2026-09-29 (#141):** `plan-mode` = an agent profile whose `tools` list is read-only (for example `read_file`, `grep`, `find`, `ls`; ADR 0026 §4), built in effect |
| 14 telemetry / durability | built (`tracing`, trail, queue) |
| 15 experimental (chord, pi-protocol, pi-server) | excluded: networked/multi-attach (brief Part 1) |

## Apex (`inventory-apex.md`)

| Items | Disposition |
|---|---|
| 1–7 note format, title fallback, null forms, link forms, implicit aliases, `notes/` root, skipping dotfiles and symlinks | W-A. Reading is lenient, as in Apex; writing stays strict (two keys). See ADR 0022 for how this meets the run gate. |
| 8 `manifest.json` | built (validated); maintenance on write → W-A |
| 9 `layout.json` | W-A: preserved and pruned on delete, never interpreted |
| 10–11 `dates.json` read, seed, reconcile | W-A, bit-compatible anchors, or read-only if they cannot be matched exactly (ADR 0022 says which) |
| 12 `annotations.json` | W-A preserves the file; editing is excluded [ui] |
| 13 graph index | built in part (lineage, children, one hop); refsIn/refsOut, depth, roots → W-A |
| 14 issues (missing-parent, cycle, disconnected, duplicate-alias) | W-A, including duplicate-alias, which Apex defines but never raises |
| 15–19 layout, canvas, labels, annotations | excluded [ui] |
| 20, 22–26 create, auto-create linked, retitle with link rewrite, reparent, link, delete | W-A as `trellis vault` subcommands; delete is a file removal (git is the undo; no OS trash). **Amended 2026-09-29 (#61):** 22, auto-create linked, is excluded, for ADR 0022 §7's reason: Apex's trigger is an editor autosave seeing a newly typed `[[X]]`, a command line has no such event, and doing it on `check --fix` would create a note for every unresolved link. `trellis vault new` is the explicit form. |
| 21, 27–29 editor, copy/paste, drop import, undo | excluded [ui] |
| 30 BM25 search | W-A (`trellis vault search`) and W-B (a `search_notes` tool) |
| 31 workspaces / project create | W-A (`trellis vault init`); tabs excluded [ui] |
| 32–34 validation status, live sync, size warnings | W-A (`trellis vault check` with the Apex report line and size warnings); live sync excluded (no long-lived process) |
| 35–37 writing skill, hierarchy prompt, AGENTS.md | W-A ships the writing contract as a built-in skill; the prompt is excluded [ui] |
| 38–42 build, packaging, native, self-update | excluded [ui/desktop] |

## Paperclip (`inventory-paperclip.md`)

| Items | Disposition |
|---|---|
| 1–3 multi-company, company lifecycle, settings | excluded [db][multi-user]: one repository is one trellis "company" |
| 4 reporting tree | W-E (`reports_to` on agent profiles; no cycles) |
| 5–6 roles, members | excluded [multi-user] |
| 7 agent permissions | W-E (per-profile `[control]`, tool enable list, `deny_write`) |
| 8–9 team catalog, built-in agents | deferred |
| 10–11 16 external adapters, ACP | W-E adds one `process` profile: runs any command in the worktree, then trellis's checks decide. The adapters themselves are excluded because each one wraps a third-party agent. |
| 12, 17–18 external adapters as plugins, sandboxes | excluded [server]; sandbox deferred |
| 13 lifecycle pause/resume/terminate | W-E (profile `paused`) |
| 14 instruction bundle | W-E (profile `instructions` file) |
| 15 config revisions | built in effect (profiles are files in git) |
| 16 environments | deferred |
| 19–20 task fields and statuses | W-E (priority, labels, goal = vault stem, parent task, assignee profile, blockers, status set) |
| 21–22 atomic checkout, execution lock | built (ADR 0020 hard-link claim) |
| 23–24 blockers, sub-issues waking parents | W-E |
| 25 comments | W-E (a notes log per task; `@profile` mentions are deferred) |
| 26–28 documents, attachments, work products | deferred; the trail and the diff are the work product. **Amended 2026-09-29 (#56):** excluded, for the same reason: the trail and the diff are the work product. A task's `focus` note (ADR 0017) and files in the repository cover input documents, and copies stored in the git dir would be lost with a linked worktree (ADR 0020 §1) and not versioned by git. |
| 29 labels | W-E |
| 30–31, 34, 37–38 interactions, work modes, decisions, cases, feedback training | deferred. **Amended 2026-09-29:** 30–31 work modes excluded (#141): a proposed breakdown is model text, and the human files tasks with `queue add --parent`; a mode whose output filed tasks would let the model create its own work queue with no human step. 37 cases and pipelines excluded (#58): a case is a parent task with blockers (rows 23–24), and a second hierarchy beside parent tasks would be a second source of truth for the same ordering. 38 feedback votes excluded until something reads them (#142): a vote nothing consumes is a record without a use, and feeding votes into prompts would make human approval read as evidence of correctness (ADR 0004). 34 decisions stays deferred. |
| 32 execution policy (review stage) | W-E (`needs_approval` on a task: `work` does not claim it until `trellis queue approve`) |
| 33 watchdogs | built in part (stale claims listed); deferred beyond that |
| 35–36 attention feed, inbox policy | excluded [web-ui][multi-user] |
| 39–40 projects, goals with ancestry | goals: built (vault lineage, ADR 0020 §6); projects excluded (one repository) |
| 41 wake triggers | W-E: timer (`work --every`), routine, blockers resolved, approval; mention deferred |
| 42–46 DB queue, scheduler, runs, protocol | built locally (ADR 0020); coalescing → W-E (a routine never enqueues a duplicate while one is pending) |
| 47–49 routines | W-E (5-field cron with timezone, the coalesce/skip/always and skip/catch-up policies, `{{var}}` templates); webhook and API triggers excluded [server] |
| 50–54 costs and budgets | W-E, **in tokens** from the trail, with monthly and lifetime windows, an 80% warning and a hard stop that pauses the profile. Dollars are excluded until COST_MODEL.md prices them. **Amended 2026-09-29 (#67):** dollars deferred to v1.x. Runs stay token-only through v1.0. No dollar figure is shown until COST_MODEL.md has a sourced per-model price table and prompt tokens are counted; when dollars arrive they sit beside tokens and never replace them. |
| 55–57 approvals, board | W-E (task approval and budget override via `trellis queue approve`); hire and strategy approvals excluded [multi-user] |
| 58 trust presets | deferred |
| 59–63 execution workspaces, dev servers, previews | deferred to v1.1 (parallel worktrees are ADR 0016 territory); previews excluded [server] |
| 64–67 skills import, catalog, injection modes, policy | built (ADR 0019); per-profile skill allow/deny → W-E |
| 68 Skill Studio | excluded [web-ui] |
| 69–71 secrets | excluded: ADR 0010 |
| 72–75 connectors, tool gateway, MCP | excluded: brief v2.0 ("MCP exposure") and [server] |
| 76–82 plugins | excluded [server][web-ui]; the executable hooks of W-B are the local extension point |
| 83–84 activity log, export | W-E (JSONL activity log in the git dir; `trellis queue log`) |
| 85–87 portability | W-E (`trellis queue export/import` of profiles, routines and tasks to a folder; nothing secret exists to scrub) |
| 88–93 identity, deployment, database, storage | excluded [server][db][multi-user] |
| 94–96 CLI | built in effect: the local subcommands; `--json` → W-D |
| 97–98 REST API | excluded [server] |
| 99–100 UI | excluded [web-ui] |

## Cosmetic and [ui] exclusions, itemised

Added 2026-09-28 (#74). This section adds no disposition and changes no row
above. It lists, one line each, the items that four rows exclude as a group,
so that the absence of any one of them is not read as an oversight.

What trellis does with Apex's own files is unchanged: `layout.json` is
preserved and pruned on delete, never interpreted (row 9); `annotations.json`
is never opened (row 12, `trellis-vault` `edit.rs`). The tests named
`sidecars_survive_*` and `..._prunes_sidecars` in
`crates/trellis-vault/tests/parity.rs` hold both.

From pi §4, under the row "4 remaining events":

- `ctx.ui` widgets: excluded, cosmetic.
- `ctx.ui` header and footer: excluded, cosmetic.
- `ctx.ui` overlays: excluded, cosmetic.
- `registerShortcut`: excluded, cosmetic.
- `registerMessageRenderer`: excluded, TypeScript-runtime specific (a
  function called inside pi's renderer; trellis hooks are processes that
  exchange JSON and have no render step to join).
- `registerEntryRenderer`: excluded, TypeScript-runtime specific, as above.
- `registerMarkdownTransformer`: excluded, TypeScript-runtime specific, as
  above.
- Themes: excluded, cosmetic.

From pi §11, under the row "11 TUI features":

- Keybindings: excluded, cosmetic.
- Fullscreen mode: excluded, cosmetic.
- External editor hook: excluded, cosmetic.
- Mermaid rendering: excluded, cosmetic.
- Inline images: excluded, cosmetic.
- Games: excluded, cosmetic.

`@` file search is not on this list. Row 11 records it as built in `tui`
(#52).

From Apex §12, under the row "12 `annotations.json`":

- Annotation editing: excluded, [ui].

From Apex §15–19, under the row "15–19 layout, canvas, labels, annotations":

- Layered-layout canvas (the SVG graph view): excluded, [ui].
- Large-graph mode: excluded, [ui].
- Label tiers: excluded, [ui].
- Drawing freehand annotations on the canvas: excluded, [ui].

Building any item listed here needs an ADR amending ADR 0021.

## Owner decisions of 2026-09-28

Added 2026-09-29 (#160). The owner answered a set of parked tickets on
2026-09-28. Each row above carries its change as a dated amendment after
the disposition it replaces, so the earlier disposition stays readable. In
summary:

- Kept excluded, with a further reason: parallel tool calls, pi 1.7 (#63).
- Excluded, from deferred or W-A: Paperclip 26–28 attachments (#56),
  30–31 work modes (#141), 37 cases and pipelines (#58) and 38 feedback
  votes (#142); pi 8.8 HTML export (#143); Apex 22 auto-create linked
  (#61).
- Built in effect: pi 13.4 `plan-mode`, as a read-only profile (#141).
- Deferred to v1.x: pi 1.15 and dollars in Paperclip 50–54 (#67).
- Kept deferred: cloud signing in pi 2.1–2.4 and the llama.cpp router in
  pi 2.10 (#45).

Desktop and web interfaces stay excluded under brief Part 1 ("any surface
beyond the CLI") (#70). `trellis rpc` (ADR 0025 §5) is the supported way
for an external front end to drive trellis. An editor or web UI can be
built outside this repository without trellis owning it, and every write
it causes still goes through the orchestrator or a `trellis vault`
command. No row changes for this.

Three rows name work that is approved but was not merged into `drain` when
this section was written, and say "landing" rather than "built": pi 1.2
(#113, with #114–#117 to follow), pi 2.13 (#150) and pi 7 (#109). A row
moves to built only once its work is merged.

## Rules every wave follows

- The grounding contract is untouched. No capability here produces a
  `Verdict` outside `trellis-checks`, and nothing makes a run converge
  except an executed check (ADR 0004).
- The clean-worktree requirement holds (ADR 0009), and every new write path
  goes through the orchestrator or through a `trellis vault` command the
  human runs.
- No new dependency without a one-line reason. Specifically: no `git2`, no
  `regex`, no `tokio` "full". TLS goes behind a cargo feature (W-C).
- Anything not built says so. It is not stubbed.

## Consequences

- Five new ADRs (0022–0026), one per wave. Each is written by its builder
  before its code and amended in place where the code disagrees.
- The CLI grows again: `vault`, `rpc` and `--json`. This is owner-directed,
  like ADR 0020.
- Everything excluded here can be revisited by a later ADR. Excluded means
  "not under the current brief", not "never".
