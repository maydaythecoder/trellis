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
  bash together); a configurable timeout for custom tools; tracking what
  bash writes in the worktree; `after_run` on Ctrl-C.
- **W-C (0024):** streaming, `HTTPS_PROXY`, a `Retry-After` given as a date,
  summaries written by the model, OAuth/Bedrock/Vertex/Azure, and the `ask`
  classifier on hosted models. Hosted runs have been exercised only against
  loopback stubs, never a real provider.
- **W-D (0025):** `/new`, `/name`, `/copy`, RPC `compact`, and a user-level
  `system.md`.
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
| 1.2 steering / follow-up queues | deferred: `session`/`tui` can accept input mid-run only after a reverse channel exists for more than approvals; ADR 0018's channel is the base |
| 1.3 built-in tools read/edit | built; **write, grep, find, ls, bash → W-B** (bash is opt-in, see ADR 0023) |
| 1.4 tool selection flags | W-B (`[tools]` enable/disable) |
| 1.5 output truncation | built (2,000-character tail, and a separate skill window, ADR 0019) |
| 1.6 swappable tool backends (ssh, micro-VM) | deferred: needs the sandbox decision (S-series) first |
| 1.7 parallel tool calls | excluded: one call per turn is the loop shape (ADR 0005) |
| 1.8 terminate flag | built in effect (`done` ends a run; convergence is decided by checks) |
| 1.9 retry/backoff | built (format retries); provider backoff → W-C |
| 1.10 overflow recovery / compaction | W-C |
| 1.11 `!cmd` user shell | W-D (`session` `!` and `!!`) |
| 1.12 session env for model shells | W-B (`TRELLIS_RUN_ID`, `TRELLIS_MODEL` in the bash tool's environment) |
| 1.13 shell config | W-B (`[tools] shell`) |
| 1.14 images | deferred: no local runtime in the tier matrix takes image input |
| 1.15 prompt-cache warming | excluded: a dollar heuristic for hosted APIs; COST_MODEL.md owns cost |
| 2.1–2.4 30+ providers, OAuth, cloud platforms | **W-C covers the two wire protocols most of them speak** (OpenAI-compatible chat completions, Anthropic Messages) over HTTPS with keys from environment variables. OAuth and device login are excluded because ADR 0010 says a key never goes in a file trellis reads. Bedrock, Vertex and Azure request signing are deferred. |
| 2.5 credential precedence | W-C, environment only (ADR 0010) |
| 2.6 `models.json` endpoints and overrides | built (`[model] spec`, catalog); overrides → W-C |
| 2.7 catalog overlay | built (`trellis model`) |
| 2.8 thinking levels | W-C, where the provider protocol carries it |
| 2.9 scoped model cycling | W-D (`session` `/model` already switches; add a list) |
| 2.10 llama.cpp router | built in part (`trellis model pull`); router deferred |
| 2.11 Radius gateway | excluded: networked service we would control (ADR 0010) |
| 2.12 custom providers | W-C (a `[[providers]]` table naming a protocol and base URL) |
| 2.13 transports / proxy | W-C (HTTPS; proxy from the environment); websocket excluded |
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
| 7 slash commands | built in part (`session`); `/new`, `/name` and `/copy` → W-D; `/login`, `/logout`, `/share` and `/bug` excluded (ADR 0010, networked) |
| 8.1–8.4 session tree, branching, fork | deferred: the trail is the record, and each run is a fresh loop. A branchable session is v2 memory work (v0.3 closed without it) |
| 8.5–8.7 compaction | W-C |
| 8.8 export and share | export → W-D (`trellis log --json`); share excluded (networked) |
| 9.1 print mode | built (`trellis run`) |
| 9.2–9.3 JSON event mode | built as `--log-json`; stdout JSONL → W-D (`--json`) |
| 9.4–9.5 RPC | W-D (`trellis rpc`, a subset: prompt, abort, get_state, get_messages, set_model, compact, approve) |
| 9.6 SDK | built in effect (the crates are libraries) |
| 10 packages | deferred: `trellis install` for skills and prompts needs the trust store first; revisit after W-B |
| 11 TUI features | excluded [cosmetic]; `@` file search is deferred |
| 12 settings layering | W-D (user-level `~/.config/trellis/config.toml` under the repo's) |
| 13.1–13.2 trust | W-B |
| 13.3 containers | deferred (sandbox decision) |
| 13.4 guard examples | built in effect: `permission-gate` = ADR 0018, `protected-paths` = `deny_write`, `dirty-repo-guard` = ADR 0009, `git-checkpoint` = snapshot and rollback |
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
| 20, 22–26 create, auto-create linked, retitle with link rewrite, reparent, link, delete | W-A as `trellis vault` subcommands; delete is a file removal (git is the undo; no OS trash) |
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
| 26–28 documents, attachments, work products | deferred; the trail and the diff are the work product |
| 29 labels | W-E |
| 30–31, 34, 37–38 interactions, work modes, decisions, cases, feedback training | deferred |
| 32 execution policy (review stage) | W-E (`needs_approval` on a task: `work` does not claim it until `trellis queue approve`) |
| 33 watchdogs | built in part (stale claims listed); deferred beyond that |
| 35–36 attention feed, inbox policy | excluded [web-ui][multi-user] |
| 39–40 projects, goals with ancestry | goals: built (vault lineage, ADR 0020 §6); projects excluded (one repository) |
| 41 wake triggers | W-E: timer (`work --every`), routine, blockers resolved, approval; mention deferred |
| 42–46 DB queue, scheduler, runs, protocol | built locally (ADR 0020); coalescing → W-E (a routine never enqueues a duplicate while one is pending) |
| 47–49 routines | W-E (5-field cron with timezone, the coalesce/skip/always and skip/catch-up policies, `{{var}}` templates); webhook and API triggers excluded [server] |
| 50–54 costs and budgets | W-E, **in tokens** from the trail, with monthly and lifetime windows, an 80% warning and a hard stop that pauses the profile. Dollars are excluded until COST_MODEL.md prices them. |
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
