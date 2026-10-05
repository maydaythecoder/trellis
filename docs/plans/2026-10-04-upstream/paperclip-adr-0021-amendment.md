# ADR 0021 amendment: Paperclip re-inventory (2026-10-04)

**Status:** Proposed.
**Date:** 2026-10-04
**Amends:** ADR 0021, its Paperclip section and the W-E bullet under "Where this stands"; and two lines of ADR 0026 (§1 below).
**Inventory:** `docs/inventory/inventory-paperclip-20261004.md`, read from `paperclipai/paperclip` at `1c07b5903b1b11139b1e1ce052a3cd4885865d90` (2026-10-04, #15100; workspace package 0.3.1; newest release v2026.1001.0, plus the draft `releases/beta/v2026.1002.0-beta.0.md`). MIT. Compared with `docs/inventory/inventory-paperclip.md` (v2026.916.1) and the Paperclip rows of this ADR.

## Context

The re-inventory lists 144 capabilities. PC-01 to PC-100 keep the earlier numbering: 56 changed upstream and 44 did not. PC-101 to PC-144 are new rows: 11 arrived after v2026.916.1, and 33 were present earlier but missing from the first inventory or folded into another row (the native runner, PC-109, is split out of row 10). Against the trellis code at drain on 2026-10-04: 44 built, 22 built in part, 13 missing, 65 excluded.

Two findings shape this amendment.

1. **Some rows understate trellis.** Work merged since 2026-09-27 left nine Paperclip rows and the W-E bullet saying "deferred" or "not built" for something built. §1 corrects them and changes no disposition.
2. **No upstream change in rows 1–100 removes the clause an exclusion rests on.** Most of what moved upstream is networked services, multi-user governance, the web UI and the plugin and connector platform, which brief Part 1, [multi-user], ADR 0010 and the owner's answer on #73 exclude. Where upstream moved a reference point trellis followed (mention wakes, #57), or added something that fits a local single-user harness, the question is a ticket of this plan (§4), not a disposition here.

## 1. Corrections: rows trellis has outgrown

Each row keeps its text and gains a note `Corrected 2026-10-04:`.

| Row | Says now | Correction |
|---|---|---|
| 7 agent permissions | W-E (per-profile `[control]`, tool enable list, `deny_write`) | Profiles carry `allow_write`, which only narrows and is checked after the repository's `deny_write` (#182, ADR 0026 §4). |
| 8–9 team catalog, built-in agents | deferred | Built in part: three copy-in starter profiles under `examples/agents/`, `implementer` (#55) and `test-writer` and `docs-writer` (#183), none judging another agent's work. A team with routines and skills, and Paperclip's built-in agents, stay deferred. |
| 25 comments | `@profile` mentions deferred | Built (#57): a person's `queue note` naming an existing, unpaused `@profile` wakes the task for it; a note written inside a run is refused (#189). |
| 33 watchdogs | stale claims listed | Opt-in release of a claim whose holder is provably dead, at most once an hour per task (#59, ADR 0020 §5). |
| 41 wake triggers | mention deferred | Built (#57). |
| 47–49 routines, and the W-E bullet | UTC and fixed offsets only | IANA zone names through `jiff` (#47, ADR 0026 §7). |
| 50–54 costs and budgets, and the W-E bullet | no prompt-token accounting | Prompt tokens are counted beside completion tokens and never added to them (#108, ADR 0026 §5). Budgets limit completion tokens; dollars stay deferred to v1.x (#67). |
| 59–63 workspaces | deferred to v1.1 | Profile services are built (#60, ADR 0026 §12). A worktree per queued task is still not built (§4). |
| ADR 0026 "What is not built" | "No prompt-token or dollar accounting" | No dollar accounting (#108). |
| ADR 0026 §12 item 2 and the services bullet | a per-task worktree "until #24" | #24 is the partition work, split into #98–#101; no issue tracks a worktree per queued task. |

The pi row 1.2 (steering) is outgrown as well (#113–#117, ADR 0034); the pi track's amendment carries it.

## 2. Rows 1–100 whose upstream changed and whose disposition holds

| Rows | Upstream change | Why the disposition holds |
|---|---|---|
| 1–3, 5–6 | resolver-policy cap, default responsible user, board and bootstrap claim, Owner Instance Admin | [db][multi-user]: one repository is one company, and one person answers. |
| 10–11 | paperclip_runner on by default for self-hosted instances; ACP engines on more adapters; a shared rich-ACP transport | The named adapters still each wrap a third-party agent. Whether a process profile may speak ACP is a question of this plan (§4). |
| 12, 16–18 | plugin environment driver, lease policies, kubernetes mode, CreateOS, Daytona hardening | [server], and the order the owner set on #20: the throwaway clone first (ADR 0032), containers later. |
| 26–28 | text-attachment tabs, artifact cards, `register_deliverable` | #56: the trail and the diff are the work product. |
| 31 | `skill_test` mode | #141. A skill test from the command line is `trellis run --skill <name> --prompt <template> --args` (ADR 0025). |
| 32 | `review_policy`, native review handoffs | A person reviews `in_review` work; a reviewer-agent stage stays out by rule 5 (ADR 0005). |
| 35–37 | Combined Inbox + Task List; summary slots written by the Summarizer | [web-ui][multi-user]; #58. |
| 47 | `app_webhook` and `fireflies_hmac` signing modes, a trigger wizard | Webhook triggers still need a listening server (brief Part 1). |
| 64–65 | GitHub skill sources; three more bundled skills | `trellis install <source>@<ref>` (ADR 0039). The catalog's skills sit at `packages/skills-catalog/catalog/{bundled,optional}/<category>/<skill>/`, most drive Paperclip's own API, and a generic one can be copied into a package as `skills/<name>/SKILL.md`. |
| 69–71 | gcp and vault provider stubs, class-3 static leases, remote import | ADR 0010. |
| 72–75 | Composio broker retired, MCP aggregators on by default, 82 app definitions, new gateway application types | The owner's answer of 2026-09-28 on #73: excluded until a v2.0 brief, for a long-lived connection, remote tool schemas the model sees unreviewed, and credentials per server. None of the changes touches those. |
| 76–82 | `approvals.respond` and other new plugin capabilities, distribution image catalogs, new UI slots | [server][web-ui]. A plugin answering approvals is further still from trellis's rule that only a person answers (#189). |
| 88–100 | bind presets, backup retention, release channels, a rewritten set of experimental flags, Chat-first navigation | [server][db][multi-user][web-ui]; #70. |

## 3. New rows

| Id | Capability | Disposition | Clause |
|---|---|---|---|
| PC-101 | Agent Chat | built in part | `trellis session` is the conversation (ADR 0025). A persistent conversation per agent is deferred with pi 8.1–8.4. A chat that hands work off to tasks the model files is excluded by row 31 (#141). |
| PC-102 | Board concierge chat, Conference Room | excluded [web-ui] | Brief Part 1, "any surface beyond the CLI" (#70); the desktop track's. |
| PC-103 | Chat channel connectors | excluded | Brief Part 1, "any networked component"; as rows 72–75 (#73). |
| PC-104 | Email channels | excluded | Brief Part 1, "any networked component". |
| PC-105 | GitHub review bots | excluded | Brief Part 1, networked; rule 5 (ADR 0005) where the reviewed change is agent-written. |
| PC-106 | AI connections | excluded | ADR 0010 (a key never goes in a file trellis reads); OAuth and device login are excluded (pi 2.1–2.4); ADR 0024 §3. |
| PC-107 | Browser Use Cloud | excluded | Brief Part 1, networked. |
| PC-108 | Memory connectors | excluded | Brief Part 1, networked; trellis's memory is the trail (ADR 0007) and the vault (ADR 0017). |
| PC-109 | Paperclip native runner | built in part | Trellis runs its own loop natively and drives another agent as a process profile (ADR 0026 §11). Paperclip's runner protocol belongs to its control plane and is excluded with it. ACP for process profiles is a question of this plan. |
| PC-110 | Durable session goals | excluded | A trellis run ends when an executed check converges it or a budget stops it (ADR 0004, ADR 0008); a provider-side goal is not consulted. |
| PC-111 | Completion contracts and status arbitration | built, stricter | Only an executed check converges a run; `Verdict` is sealed in `trellis-checks`; `ToolResult` has no pass variant (ADR 0004). |
| PC-112 | Durable continuations and scheduled retries | built in part | Attempts per task, `queue requeue`, opt-in stale release (#59); `work` never re-runs a claim on its own. A limit that blocks a task after repeated releases is a question of this plan. |
| PC-113 | Issue tree control | deferred | `queue cancel --children` is a question of this plan; a subtree hold would be a second state beside `status`. |
| PC-114 | Execution monitors | excluded | Re-checking an external service needs a networked component. An agent-set monitor is the model scheduling its own work, which row 31 (#141) keeps with a person. A person's one-off re-check is a routine or `work --every` (ADR 0026 §7). |
| PC-115 | Run diagnostics and failure reports | built | The trail (ADR 0007), `trellis log` and `resume <run_id>` (ADR 0038), and the waiting reasons `queue list` prints (ADR 0026 §1). |
| PC-116 | Live events | built | Loop events to the TUI (`crates/trellis-cli/src/tui_bridge.rs`); JSONL on stdout and rpc events (ADR 0025). |
| PC-117 | Persistent agent files | deferred | Agent-writable files under `.trellis/` are excluded by the CLAUDE.md invariant (ADR 0023). A profile memory note outside `.trellis/` is a question of this plan. |
| PC-118 | Skills created from work | excluded | Model writes never land in `.trellis/` (ADR 0023), where skills live (ADR 0019). A run may write a draft skill elsewhere in the repository for a person to move. |
| PC-119 | Skill sources from GitHub | built | `trellis install <source>@<ref>`, `--list`, `--remove` (ADR 0039); installing at a new ref is the refresh. |
| PC-120 | Core Paperclip agent skills | built in effect | The loop's prompt and tools carry the protocol (ADR 0005); trellis ships the vault writing contract (ADR 0022). |
| PC-121 | Runner API escape hatch | excluded [server] | There is no REST API to call (rows 97–98). |
| PC-122 | Agent personas | excluded, cosmetic | "Cosmetic and [ui] exclusions"; the desktop track's. |
| PC-123 | Onboarding and first task | excluded [web-ui] | Brief Part 1; `trellis doctor` probes the setup; the desktop track's. |
| PC-124 | Company and task search | built in part | `queue list` filters by status, label, assignee and priority. `--text` is a question of this plan. |
| PC-125 | Document annotations | excluded | Documents are excluded (rows 26–28, #56); annotation editing is [ui] (Apex row 12). |
| PC-126 | External objects | excluded | Brief Part 1, networked: status is fetched from remote services. |
| PC-127 | Folders | excluded [web-ui] | Labels group tasks (row 29); the desktop track's. |
| PC-128 | In-app announcements | excluded | Brief Part 1: a networked fetch and a UI card. |
| PC-129 | Smoke Lab | excluded | It exercises connector paths, which rows 72–75 exclude. |
| PC-130 | Decision queues and triage | excluded [web-ui][multi-user] | As rows 35–36; the desktop track's. |
| PC-131 | Per-user surfaces | excluded [multi-user][web-ui] | Brief Part 1; the desktop track's. |
| PC-132 | Workspace file resources | built in part | `@` file search in `tui` over `git ls-files` (#52) and `read_file`; a file browser is the desktop track's. |
| PC-133 | Hosted agent profiles | excluded | The provider runs the loop on its own infrastructure, so trellis's gate (ADR 0018), write path and rollback never see its tool calls. A provider's CLI can run as a process profile (ADR 0026 §11), with its writes judged at carry-back. |
| PC-134 | Cloud-managed instances and execution allowlist | excluded | Brief Part 1 (networked, multi-user); ADR 0010. |
| PC-135 | Task drain and hot restart | excluded | No daemon (ADR 0020; ADR 0026 §7, #68); Ctrl-C rolls back (ADR 0009). |
| PC-136 | Evals | built in effect | Trellis measures through its benchmark protocol (`docs/BENCHMARK_AND_EVAL.md`). Paperclip's evals test its own control plane and runner; a model-graded oracle would be a model verdict (ADR 0004). |
| PC-137 | First-party telemetry | excluded | Brief Part 1, networked; `tracing` is the one telemetry path (CLAUDE.md, Style). |
| PC-138 | Per-message run settings, prompt-only task creation | built in part | Model per run (`--model`, session `/model`), thinking levels where the protocol carries them (ADR 0024), a profile's model for queued tasks. A model-written task title is excluded: a task is the text a person typed. |
| PC-139 | Routine activity gate | deferred | `activity_gate = "commits"` is a question of this plan. |
| PC-140 | Harness permission defaults | built | The control dial, auto or approve with an approve list (ADR 0018); a process agent runs unattended in a throwaway clone (ADR 0032). |
| PC-141 | Responsible-user attribution, per-person GitHub identity | excluded [multi-user] | Trellis runs as the one local user, with that user's git identity. |
| PC-142 | Queued responses and steering into a running turn | built | `steer` and `follow_up` between loop turns in rpc, tui and session, recorded in the trail (ADR 0034). `ask_human` ends the run, and the answer reaches the next one (ADR 0026 §1). |
| PC-143 | Agents hiring agents | excluded [multi-user] | Hire approvals are excluded (rows 55–57); a profile is a committed file a person writes (ADR 0026 §4). |
| PC-144 | Agent delegation | excluded | Row 31 (#141): a model does not file its own tasks without a human step. Agents parallelize work, never judgment (ADR 0005); trellis's own parallelism is partitioning by blast radius (ADR 0016, ADR 0037). |

## 4. Questions this amendment leaves to tickets

Each is a needs-owner ticket of story S17. A row above that names "a question of this plan" changes only when its ticket is answered.

- Rows 25 and 41, PC-142: Paperclip stopped starting runs from mentions (#14577). Keep #57's wakes or follow it, and give a re-run task the notes a person added since its last run.
- Row 33, PC-112: block a task after its third stale-claim release, or keep #59's hourly release without a limit.
- PC-139: a routine gate on commits since its last fire, ignoring the trails trellis commits.
- PC-113: `queue cancel --children`.
- Rows 30 and 34: several questions in one `ask_human` call.
- Rows 10–11, PC-109: whether a process profile may speak ACP.
- PC-117: a profile memory note outside `.trellis/`.
- Rows 59–63: a worktree per queued task, as v1.1 scope behind #98–#101.
- PC-124: `queue list --text`.

## 5. Inventory claims not found in Paperclip's source

Recorded so that nobody ports them from the docs:

- the 30-second minimum heartbeat interval (only in `doc/SPEC-implementation.md` §11.6);
- secret folders;
- the Task Chat and Auto-Create Recovery Tasks flags;
- `docs/api/routines.md:129` lists four signing modes, and the code has six;
- `doc/plugins/PLUGIN_SPEC.md` §15.2 forbids plugin approval decisions, and the code has an `approvals.respond` capability.

## Consequences

- This amendment adds no CLI surface and no dependency. Each question in §4 that would add a flag or a profile or routine field, or change an owner's earlier answer, is a needs-owner ticket whose Done when includes the ADR amendment.
- Excluded still means "not under the current brief"; a later ADR can revisit any row here.
