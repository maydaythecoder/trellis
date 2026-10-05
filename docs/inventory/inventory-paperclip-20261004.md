# Paperclip capability inventory, re-read at head (2026-10-04)

**Upstream.** `paperclipai/paperclip`, commit `1c07b5903b1b11139b1e1ce052a3cd4885865d90` (2026-10-04 02:13:46 -0700, "feat: Chat leads the left nav, agent work beside chats, and a Combined Inbox + Task List flag (#15100)"), from `git log -1` in the clone. Workspace package version 0.3.1 (`server/package.json`, `cli/package.json`). The newest release notes are `releases/v2026.1001.0.md` (2026-10-01). `releases/beta/v2026.1002.0-beta.0.md` is a draft: it is headed v2026.1005.0 and dated 2026-10-05, the day after the clone date.

**Licence.** MIT, "Copyright (c) 2025 Paperclip AI" (`LICENSE`). Licence fields in individual packages were not checked. That includes the Rust runner under `packages/paperclip-runner/runner/`.

**Supersedes.** `docs/inventory/inventory-paperclip.md` (v2026.916.1). PC-01 to PC-100 are that file's items 1 to 100, in the same order. PC-101 to PC-144 are new.

## Method

- **What was read.**
  - Top level: `README.md`, `package.json`, `releases/`.
  - The in-repository `docs/` tree, which is a subset of docs.paperclip.ing, and `doc/` (specs, architecture, plugins, connections; most plans by title only).
  - `packages/shared/src/constants.ts`, `validators/instance.ts` and `feature-catalog.ts`.
  - The `packages/db` schema files and the migration list (0000 to 0294).
  - The `server/src/app.ts` mounts and the endpoint list of every route module.
  - About 30 service files, read at the top of the file.
  - `cli/src/index.ts` and the command modules.
  - Package READMEs, `skills/`, and the skill and team catalogs.
- **Evidence.** Every row cites repository paths at head. The earlier inventory's `d:` citations point to the hosted docs, which were not re-read here. Where an earlier claim could not be found in the repository, the row says so.
- **Dating.** The clone is depth 1 and this task is read-only, so it was not possible to fetch the tag and diff against v2026.916.1. "Since the tag" is dated in three ways:
  - PR numbers in `releases/v2026.1001.0.md`, which was promoted from 2026.921.0-beta.1.
  - The draft 1005 notes. Claims taken from them were checked in source where a row cites them.
  - Migrations numbered 0280 or higher.

  Head (#15100) is later than even the draft notes.
- **Change labels.** Each row's notes begin with one of these.
  - **NEW** is a capability not in the earlier inventory. It is either "upstream since the tag", or "to inventory", meaning it was present before the tag or its date could not be established.
  - **CHANGED** means the capability is in the earlier inventory and differs at head.
  - **UNCHANGED** means it matches at head.
- **TRELLIS status.** This is today's state, re-derived from ADR 0021, the ADRs that amend it, and the crate sources.
  - `built`: built or built in effect.
  - `partial`: built in part.
  - `missing`: deferred or absent; the row cites what does exist.
  - `excluded`: excluded by the brief or an ADR; the row cites the clause.
- **Disposition.** This is where the work would go if it were taken up. It is independent of status, which is why combinations such as excluded with drive-via-runtime, or missing with desktop-ui, are deliberate.
  - `already-built`: trellis has it.
  - `port`: it fits the brief and would be reimplemented natively in a trellis crate.
  - `drive-via-runtime`: trellis would reach it through a runtime rather than reimplement it. That means a `trellis-model` spec, an adapter tier, or the `process` agent profile. This covers third-party harnesses, sandboxes and hosted agents.
  - `desktop-ui`: a view or interaction surface. It belongs to an interface layer, not the core crates. The assumed target is the planned desktop surface, but brief Part 1 still excludes "any surface beyond the CLI", and `trellis rpc` (ADR 0025 §5) is how a front end drives trellis today.
  - `owner-decision`: excluded or in conflict with the brief or an ADR (multi-user, server, networked components, the `.trellis/` write rule, #141), and now central upstream. Reopening it needs an ADR amending ADR 0021.

## Features

| ID | Capability | Change | Upstream evidence | TRELLIS status (evidence) | Disposition |
|---|---|---|---|---|---|
| PC-01 | Multi-company: many isolated organizations per instance, every entity company-scoped | UNCHANGED | `packages/db/src/schema/companies.ts`; `server/src/routes/companies.ts` | excluded (ADR 0021 rows 1-3 [db][multi-user]; brief Part 1) | owner-decision |
| PC-02 | Company lifecycle: CRUD, branding, stats, archive, delete | UNCHANGED | `server/src/routes/companies.ts:399,1319,1344,1356` | excluded (rows 1-3) | owner-decision |
| PC-03 | Company settings: board approval for new agents, feedback sharing, default responsible user, issue prefix, cap on who may resolve interactions | CHANGED: interaction-resolver cap and default responsible user | `packages/db/src/schema/companies.ts:13-30` | excluded (rows 1-3) | owner-decision |
| PC-04 | Reporting tree; 12 roles; org chart as JSON, SVG or PNG | UNCHANGED | `server/src/routes/agents.ts` (org, org.svg, org.png); `server/src/routes/org-chart-svg.ts` | built (`reports_to` with cycle check, ADR 0026 §4, `trellis-queue/src/profile.rs`); no image export | already-built |
| PC-05 | Roles owner, admin, operator, viewer; 21 permission keys | UNCHANGED | `packages/shared/src/constants.ts:964-1025` | excluded (rows 5-6 [multi-user]) | owner-decision |
| PC-06 | Members: invites, join requests, instance admin, board claim, user directory | CHANGED: board and bootstrap claim; Owner Instance Admin flag | `server/src/routes/access.ts`; `constants.ts` INVITE_TYPES | excluded (rows 5-6) | owner-decision |
| PC-07 | Agent permissions: scoped grants | UNCHANGED | `server/src/routes/agents.ts` (permissions); `packages/db/src/schema/principal_permission_grants.ts` | built (per-profile `[control]`, `tools`, `allow_write`; ADR 0026 §4, #182) | already-built |
| PC-08 | Team catalog: core-exec-team, product-design, product-engineering, content-machine | UNCHANGED | `packages/teams-catalog/catalog/`; `server/src/routes/teams-catalog.ts` | missing (rows 8-9 deferred) | port |
| PC-09 | Built-in agents briefs, learning, reflection-coach, summarizer | CHANGED: summarizer added | `server/src/services/built-in-agents.ts:310-405`; `server/src/built-ins/agents/` | missing (rows 8-9 deferred) | port |
| PC-10 | 16 adapter types (acpx_local kept as a retired tombstone) | CHANGED: runner on by default; Grok Build; harness pins and catalogs | `server/src/adapters/builtin-adapter-types.ts`; `server/src/adapters/registry.ts:185-197` | partial (process profile, ADR 0026 §11; named adapters excluded, rows 10-11) | drive-via-runtime |
| PC-11 | ACP engine: auto, acp or cli on claude, codex, gemini (kimi); rich-ACP transport | CHANGED: rich-ACP transport (#14430), acpx sidecar | `docs/adapters/overview.md:21-135`; `doc/architecture/runner-rich-acp-capabilities.md` | excluded (rows 10-11) | drive-via-runtime |
| PC-12 | External adapters from npm or a local path, with a UI parser | UNCHANGED | `server/src/adapters/plugin-loader.ts`; `docs/adapters/external-adapters.md` | excluded (row 12 [server]) | drive-via-runtime |
| PC-13 | Agent lifecycle; statuses including pending_approval; pause reasons | CHANGED: pending_approval status; import pause reason | `constants.ts:16-24`, PAUSE_REASONS; `server/src/routes/agents.ts` | built (paused overlay, `agents pause/resume`, `profile.rs`) | already-built |
| PC-14 | Instructions bundle | CHANGED: now the persistent agent directory (PC-117); revision history superseded | `server/src/services/agent-instruction-files.ts`; `doc/agent-instruction-revisions.md` | built (profile `instructions`, ADR 0026 §4) | already-built |
| PC-15 | Agent config revisions with rollback | UNCHANGED | `server/src/routes/agents.ts` (config-revisions); `schema/agent_config_revisions.ts` | built (row 15: profiles are files in git) | already-built |
| PC-16 | Environments: local, ssh, sandbox and plugin drivers; lease policies | CHANGED: plugin driver, lease policies, managed-only, kubernetes mode | `constants.ts` ENVIRONMENT_*; `server/src/services/environments.ts` | missing (row 16 deferred) | drive-via-runtime |
| PC-17 | Sandbox providers: Cloudflare, CreateOS, Daytona, E2B, exe.dev, Kubernetes, Modal, Novita | CHANGED: CreateOS (#13434) | `packages/plugins/sandbox-providers/` | missing (rows 12, 17-18 sandbox deferred; ADR 0032 first step only) | drive-via-runtime |
| PC-18 | Custom sandbox images built over SSH or a browser terminal | UNCHANGED | `server/src/services/environment-custom-images.ts` | missing (rows 17-18, follows the sandbox decision) | drive-via-runtime |
| PC-19 | Task fields | CHANGED: title_needs_generation (0293), review_policy, harness_kind, responsible user, source_trust, monitor and conversation columns | `packages/db/src/schema/issues.ts` | built (ADR 0026 §1; `trellis-queue/src/lib.rs:319`) | already-built |
| PC-20 | Statuses backlog to cancelled | UNCHANGED | `constants.ts:189-198` | built (plus `failed`; `lib.rs:128-137`) | already-built |
| PC-21 | Atomic checkout, release, force-release | UNCHANGED | `doc/SPEC-implementation.md` §10.4.1; `server/src/routes/issues-checkout-wakeup.ts` | built (ADR 0020 hard-link claim) | already-built |
| PC-22 | Execution lock, sweeper, controller lease | CHANGED: renewable controller lease; finalization guarded (#14219) | `schema/issues.ts`; `server/src/services/legacy-controller-lease.ts` | built (ADR 0020; `stale.rs`, #59) | already-built |
| PC-23 | Blockers with wakes and diagnostics | UNCHANGED | `server/src/services/issue-dependency-wakeups.ts`; `routes/issues.ts` diagnostics/blockers | built (ADR 0026 §1 reconcile) | already-built |
| PC-24 | Sub-issues wake parent when children are terminal | UNCHANGED | `doc/execution-semantics.md` | built (reason `children`) | already-built |
| PC-25 | Comments; comment wakes the assignee; @-mentions | CHANGED (breaking): mentions are context only (#14577) | `server/src/services/issue-comment-wakeup.ts`; `native-runtime/runner-api-reference.ts:167` | built (notes and `@profile` wakes, #57, `mention.rs`) | already-built |
| PC-26 | Documents: keyed, revisioned, lockable | UNCHANGED | `routes/issues.ts` documents; `schema/documents.ts` | excluded (rows 26-28, #56) | owner-decision |
| PC-27 | Attachments, with text tabs | CHANGED: task tabs (#14297); bigint size (0285) | `routes/issues.ts` attachments; `schema/issue_attachments.ts` | excluded (#56) | owner-decision |
| PC-28 | Work products and artifacts | CHANGED: rich cards, gallery, register_deliverable | `server/src/services/work-products.ts`; `doc/AGENT-ARTIFACTS.md` | excluded (#56: the trail and the diff are the work product) | owner-decision |
| PC-29 | Labels | UNCHANGED | `schema/labels.ts` | built (ADR 0026 §1) | already-built |
| PC-30 | Thread interactions (six kinds), resolver policies | CHANGED: connection_intent kind; canonical policies; history (#14613) | `constants.ts:259-338`; `server/src/services/issue-thread-interactions.ts` | partial (`ask_human`, 8 options, #140) | port |
| PC-31 | Work modes standard, ask, planning, skill_test; plan decomposition | CHANGED: skill_test; explicit mode authority | `constants.ts:214-216`; `doc/plans/2026-09-22-explicit-work-mode-authority.md` | excluded (row 31, #141); plan mode = read-only profile | owner-decision |
| PC-32 | Execution policy: review and approval stages, review policies | CHANGED: review_policy; native review handoffs (#13581) | `server/src/services/issue-execution-policy.ts`; `docs/guides/execution-policy.md` | built (`needs_approval`, `queue approve`; converged run ends `in_review`) | already-built |
| PC-33 | Recovery actions and task watchdogs | CHANGED: watchdog flag retired; retry budget escalates to blocked (#14046) | `server/src/services/task-watchdogs.ts`; `doc/SPEC-implementation.md` §9.9 | partial (stale claims list or release, #59) | port |
| PC-34 | Decisions with 1-8 options and expiry; Decisions sidebar | CHANGED: sidebar attention home | `server/src/services/decisions.ts`; `validators/instance.ts` enableDecisions | partial (`ask_human`, #140; record deferred) | port |
| PC-35 | Attention feed, inbox, What Needs Me, Combined Inbox + Task List | CHANGED: combined flag (#15100) | `server/src/services/attention.ts`; `ui/src/pages/WhatNeedsMe.tsx` | excluded (rows 35-36 [web-ui][multi-user]) | desktop-ui |
| PC-36 | Inbox agent policy | UNCHANGED | `server/src/routes/inbox-agent-policy.ts` | excluded (rows 35-36) | owner-decision |
| PC-37 | Cases, pipelines, status cards, summary slots | CHANGED: Summarizer-driven slots | `server/src/routes/cases.ts`, `pipelines.ts`, `status-cards.ts`, `summary-slots.ts` | excluded (row 37, #58) | owner-decision |
| PC-38 | Feedback votes and decision training | UNCHANGED | `server/src/routes/decision-training.ts`; `docs/feedback-voting.md` | excluded (row 38, #142) | owner-decision |
| PC-39 | Projects with GitHub repository selections | CHANGED: repo selection at creation; repo-only workspaces | `doc/project-repositories.md`; `server/src/routes/projects.ts` | excluded (rows 39-40: one repository) | owner-decision |
| PC-40 | Goals and ancestry | CHANGED (surface only): Goals hidden from the sidebar by default | `constants.ts` GOAL_LEVELS; `validators/instance.ts` enableGoalsSidebarLink | built (vault lineage, ADR 0020 §6; `--goal`) | already-built |
| PC-41 | Wake triggers | CHANGED: mention no longer a source | `constants.ts` HEARTBEAT_INVOCATION_SOURCES; `docs/agents-runtime.md:20-62` | built (timer, routine, blockers, approval, mention #57) | already-built |
| PC-42 | Database-backed wake queue with coalescing | UNCHANGED | `server/src/modules/wake-queue`; `schema/agent_wakeup_requests.ts` | built (ADR 0020; coalescing ADR 0026 §7) | already-built |
| PC-43 | Scheduler: interval, maxConcurrentRuns default 20, clamped 1-50 | CHANGED: default 20; 30 s floor in spec only | `doc/SPEC-implementation.md` §11.6; `server/src/services/heartbeat.ts:677-679` | built (`work --every`, serial) | already-built |
| PC-44 | Runs: statuses including scheduled_retry and interrupted; session resume | CHANGED: new run statuses | `constants.ts` HEARTBEAT_RUN_STATUSES; `schema/heartbeat_runs.ts` | built (trail per run; `resume <run_id>`, ADR 0038) | already-built |
| PC-45 | Run recovery: orphans, rewake throttle, crash-loop guard | CHANGED: rejected claim cancelled (#14738) | `server/src/services/recovery/`; `doc/architecture/durable-continuation-scheduler.md` | partial (rollback, ADR 0009; #59) | port |
| PC-46 | Heartbeat protocol and run-id header | UNCHANGED | `docs/guides/agent-developer/heartbeat-protocol.md`; `skills/paperclip/SKILL.md` | built (rows 42-46, the loop owns claim, run and record) | already-built |
| PC-47 | Routine triggers: cron, webhook (six signing modes), API; trigger wizard | CHANGED: app_webhook, fireflies_hmac; wizard (#13637); docs list four modes | `constants.ts` ROUTINE_TRIGGER_SIGNING_MODES; `schema/routines.ts` | partial (IANA cron, #47, `cron.rs`; webhook and API excluded, rows 47-49) | owner-decision |
| PC-48 | Routine concurrency and catch-up policies | UNCHANGED | `constants.ts` ROUTINE_CONCURRENCY_POLICIES | built (`routine.rs`) | already-built |
| PC-49 | Routine runs create issues; typed variables; revisions and restore | CHANGED: typed variables, restore, routine documents | `docs/api/routines.md:85-99`; `schema/routine_documents.ts` | built (ADR 0026 §7 templates; git) | already-built |
| PC-50 | Cost events | UNCHANGED | `schema/cost_events.ts` | partial (tokens, `ledger.rs`; dollars deferred, #67) | port |
| PC-51 | Budget scopes company, agent, project | UNCHANGED | `constants.ts` BUDGET_SCOPE_TYPES | partial (per profile only) | port |
| PC-52 | Budget policies: warn, hard stop, month or lifetime | UNCHANGED | `constants.ts` BUDGET_WINDOW_KINDS | built (ADR 0026 §5) | already-built |
| PC-53 | Hard stop and override | UNCHANGED | `constants.ts` BUDGET_INCIDENT_RESOLUTION_ACTIONS | built (pause and approval, `approval.rs`) | already-built |
| PC-54 | Cost breakdowns, quota windows, finance ledger | UNCHANGED | `server/src/services/finance.ts`, `quota-windows.ts` | partial (token spend only) | port |
| PC-55 | Approval types | UNCHANGED | `constants.ts` APPROVAL_TYPES | partial (task and budget; hire and strategy excluded, rows 55-57) | owner-decision |
| PC-56 | Approval lifecycle | CHANGED: cancelled status; responses queue in active runs (#13539) | `server/src/routes/approvals.ts`; `server/src/services/queued-interaction-response.ts` | partial (no revision_requested) | port |
| PC-57 | Board powers | UNCHANGED | `doc/SPEC-implementation.md` §12.3 | built (`agents pause/resume`, `queue set/cancel/reject`) | already-built |
| PC-58 | Trust presets standard and low_trust_review; source trust | CHANGED: private directory for repository-free low-trust tasks (#14766) | `doc/LOW-TRUST-PRESETS.md`; `server/src/services/low-trust-runtime-containment.ts` | missing (row 58 deferred; trust store ADR 0023, clone ADR 0032) | port |
| PC-59 | Execution workspaces | CHANGED: isolated by default; worktree run execution; branch repair | `server/src/services/execution-workspaces.ts`; `validators/instance.ts` | partial (partition worktrees, ADR 0037; per-task #24 not built) | port |
| PC-60 | Workspace consistency, finalize-before-wake | CHANGED: restore locks; workspace_finalize gate | `doc/workspace-restore-locks.md` | partial (ADR 0009, ADR 0042) | port |
| PC-61 | Runtime services (dev servers) | CHANGED: lazy provisioning | `server/src/services/workspace-runtime.ts` | built (ADR 0026 §12, #60, `services.rs`; ADR 0021 row out of date) | already-built |
| PC-62 | HTTPS previews through Tailscale | UNCHANGED | `packages/tailscale-https-broker` | excluded (rows 59-63, previews [server]) | owner-decision |
| PC-63 | Workspace archive; never pushes | UNCHANGED | `scripts/check-no-git-push.mjs`; workspace guide:136-146 | partial (no push path; worktrees discarded after merge; no per-task archive) | port |
| PC-64 | Skill import, versions, fork | CHANGED: GitHub sources (PC-119) | `server/src/routes/company-skills.ts` | built (ADR 0019; `install`, ADR 0039) | already-built |
| PC-65 | Skill catalog (10 bundled, 6 optional) | CHANGED: three added, last30days gone | `packages/skills-catalog/catalog/` | partial (mechanism; one built-in skill, ADR 0022) | port |
| PC-66 | Skill injection modes | UNCHANGED | `server/src/services/runtime-skill-selections.ts` | built (ADR 0019 disclosure, `--skill`) | already-built |
| PC-67 | Skill policy | UNCHANGED | `server/src/routes/company-skill-policy.ts` | built (profile `[skills]`) | already-built |
| PC-68 | Skill Studio: test runs, saved inputs, versions | CHANGED: skill_test issues | `server/src/routes/company-skills.ts`; `ui/src/pages/SkillStudio.tsx` | excluded (row 68 [web-ui]) | desktop-ui |
| PC-69 | Secret providers | CHANGED: GCP and Vault declared as stubs | `server/src/secrets/external-stub-providers.ts` | excluded (ADR 0010) | owner-decision |
| PC-70 | Secret scopes company and user | UNCHANGED (secret folders not located) | `constants.ts` SECRET_SCOPES | excluded (ADR 0010) | owner-decision |
| PC-71 | Secret delivery, bindings, proposals, leases | CHANGED: class-3 static leases; remote import | `constants.ts` CLASS3_STATIC_LEASE_ALLOWLIST; `docs/api/secrets-remote-import.md` | excluded (ADR 0010) | owner-decision |
| PC-72 | Connectors: 82 apps; aggregators; one-screen setup | CHANGED: Composio broker retired; aggregators default on; new providers | `packages/shared/src/app-definitions/`; `doc/connections/README.md` | excluded (rows 72-75) | owner-decision |
| PC-73 | Connector review requests | UNCHANGED | `server/src/routes/tool-gateway.ts:605-632` | excluded (rows 72-75; ADR 0018 is the local analogue) | owner-decision |
| PC-74 | Tool gateway: profiles, policies, audit, runtime slots | CHANGED: a2a, chat, rest_api app types; defer_runtime | `constants.ts` TOOL_*; `doc/MCP-ACCESS-GOVERNANCE.md` | excluded (rows 72-75) | owner-decision |
| PC-75 | Named MCP gateways and tokens; MCP server; connection intents | CHANGED: token expiry (0294); intents as an interaction kind | `server/src/routes/tool-gateway.ts:247-471`; `packages/mcp-server` | excluded (rows 72-75, brief v2.0) | owner-decision |
| PC-76 | Plugin manifest | UNCHANGED | `doc/plugins/PLUGIN_SPEC.md` §10 | excluded (rows 76-82; hooks, ADR 0023) | owner-decision |
| PC-77 | Plugin capabilities | CHANGED: approvals.respond and more; spec §15.2 disagrees | `constants.ts` PLUGIN_CAPABILITIES | excluded (rows 76-82) | owner-decision |
| PC-78 | Plugin workers over JSON-RPC | UNCHANGED | `server/src/services/plugin-worker-manager.ts` | excluded (rows 76-82) | owner-decision |
| PC-79 | Plugin jobs, webhooks, tools, routes | UNCHANGED | `server/src/services/plugin-job-scheduler.ts` | excluded (rows 76-82; custom tools, ADR 0023) | owner-decision |
| PC-80 | Plugin UI slots (17 types) | CHANGED: overlay, organization switcher; snippets removed | `constants.ts` PLUGIN_UI_SLOT_TYPES | excluded (rows 76-82 [web-ui]) | desktop-ui |
| PC-81 | Plugin state, install, distribution catalogs | CHANGED: image catalogs (#13646); managed resources | `doc/plugins/DISTRIBUTION-PLUGINS.md` | excluded (rows 76-82) | owner-decision |
| PC-82 | Shipped plugins | CHANGED: smoke examples, fake sandbox, scaffolder | `packages/plugins/` | excluded (rows 76-82) | owner-decision |
| PC-83 | Activity log | UNCHANGED | `schema/activity_log.ts` | built (`activity.rs`, ADR 0026 §9) | already-built |
| PC-84 | Activity filters and export | UNCHANGED | `server/src/routes/activity.ts` | built (`queue log`) | already-built |
| PC-85 | Company package format | UNCHANGED | `docs/companies/companies-spec.md` | built (own layout, `portable.rs`) | already-built |
| PC-86 | Export with scrubbing | CHANGED: fidelity report; zip | `server/src/services/export-fidelity.ts` | built (`queue export`) | already-built |
| PC-87 | Import with collisions | CHANGED: chunked transfers; imported agents parked | `server/src/services/company-import-transfers.ts` | built (`queue import`, skip, replace or rename) | already-built |
| PC-88 | Deployment modes and bind presets | CHANGED: loopback, lan, tailnet, custom | `constants.ts` BIND_MODES | excluded (rows 88-93) | owner-decision |
| PC-89 | Sessions and CLI auth | CHANGED: preference routes removed (#14643) | `server/src/routes/access.ts` | excluded (rows 88-93) | owner-decision |
| PC-90 | Run JWTs and agent keys | UNCHANGED | `server/src/agent-auth-jwt.ts` | excluded (rows 88-93) | owner-decision |
| PC-91 | Embedded Postgres and scheduled backups | CHANGED: retention presets; connection recovery | `validators/instance.ts:19-33`; `server/src/routes/instance-database-backups.ts` | excluded (rows 88-93 [db]) | owner-decision |
| PC-92 | Storage, local or S3 | UNCHANGED | `constants.ts` STORAGE_PROVIDERS | excluded (rows 88-93) | owner-decision |
| PC-93 | Deploy, OTel, Sentry, release channels | CHANGED: channels; hidden-settings wildcard | `doc/CHANNELS.md`; `doc/observability.md` | excluded (rows 88-93) | owner-decision |
| PC-94 | CLI setup commands | UNCHANGED | `cli/src/index.ts` | built (rows 94-96) | already-built |
| PC-95 | CLI client commands | CHANGED: agent and board prompt; run live; tree holds | `cli/src/commands/client/prompt.ts`, `issue.ts`, `run.ts` | built (`queue`, `agents`, `routines`, `log`, `resume`) | already-built |
| PC-96 | CLI common options | UNCHANGED | `cli/src/commands/client/common.ts` | built (`--json`, ADR 0025) | already-built |
| PC-97 | REST authentication | UNCHANGED | `docs/api/authentication.md` | excluded (rows 97-98) | owner-decision |
| PC-98 | REST groups (about 70 route modules) | CHANGED: many groups not in the earlier list | `server/src/app.ts:643-962` | excluded (rows 97-98; `trellis rpc` for front ends) | owner-decision |
| PC-99 | UI views | CHANGED: Chat-first nav; streamlined UI; 40 locales | `ui/src/pages/`; `ui/src/i18n/locales/` | excluded (rows 99-100 [web-ui]) | desktop-ui |
| PC-100 | Experimental flags | CHANGED: set rewritten; watchdog flag retired | `validators/instance.ts:40-91`; `feature-catalog.ts` | excluded (rows 99-100) | desktop-ui |
| PC-101 | Agent Chat: per-person, per-agent conversation backed by an issue, hands work to tasks | NEW to inventory; extended since the tag (0288, #14706, #15100) | `doc/plans/2026-09-10-agent-chat.md`; `server/src/services/agent-conversations.ts` | partial (`session.rs`, `ask.rs`; chat-filed tasks conflict with #141) | owner-decision |
| PC-102 | Board concierge chat and Conference Room | NEW to inventory | `server/src/routes/board-chat.ts`; `skills/paperclip-board` | excluded (brief Part 1) | desktop-ui |
| PC-103 | Chat channels: Slack, Discord, Telegram, Teams, Google Chat, GitHub, iMessage | NEW to inventory; Slack round trip since the tag | `server/src/routes/chat-channels.ts`; `server/src/services/chat-*.ts` | excluded (brief Part 1 networked) | owner-decision |
| PC-104 | Email channels (AgentMail) | NEW to inventory | `server/src/routes/email.ts`; `skills/agentmail` | excluded (brief Part 1) | owner-decision |
| PC-105 | GitHub review bots | NEW upstream since the tag (0283, #13717) | `server/src/services/chat-github-reviews.ts` | excluded (brief Part 1; rule 5 if the PR is agent-written) | owner-decision |
| PC-106 | AI connections: provider accounts, logins, setup tokens | NEW to inventory | `doc/connections/AI-CONNECTIONS.md`; `server/src/services/local-ai-login.ts` | excluded (ADR 0010; pi 2.1-2.4) | owner-decision |
| PC-107 | Browser Use Cloud with live browser tabs | NEW upstream since the tag (0290, #14627) | `server/src/services/browser-use.ts` | excluded (brief Part 1) | owner-decision |
| PC-108 | Memory connectors Mem0, Zep, Supermemory, Cognee, Honcho | NEW upstream since the tag (#13942) | `packages/shared/src/memory-connectors.ts` | excluded (networked; memory is the trail and the vault) | owner-decision |
| PC-109 | Paperclip native runner (Rust runnerd, PRP, drivers) | NEW (row split from PC-10); on by default | `packages/paperclip-runner/README.md`; `packages/paperclip-runner/runner/crates/runner-core` | partial (own loop; process profile, `agent_clone.rs`) | drive-via-runtime |
| PC-110 | Durable session goals (PRP v2, /goal) | NEW to inventory | `doc/SPEC-implementation.md` §11.7; `server/src/services/runner-goals.ts` | missing (runs end on executed checks, ADR 0004) | drive-via-runtime |
| PC-111 | Completion contracts and server status arbitration | NEW to inventory | `doc/architecture/native-status-arbitration.md`; `native-runtime/evidence-classifier.ts` | built, stricter (ADR 0004; `trellis-checks/src/verdict.rs`) | already-built |
| PC-112 | Durable continuations and scheduled retries | NEW to inventory; budgets changed since the tag | `doc/architecture/durable-continuation-scheduler.md`; `server/src/services/run-continuations.ts` | partial (attempts, `queue requeue`; no auto retry) | port |
| PC-113 | Issue tree control: subtree pause, resume, cancel, restore | NEW to inventory (migration 0066) | `server/src/services/issue-tree-control.ts`; `doc/composer-stop.md` | missing | port |
| PC-114 | Execution monitors: scheduled re-checks with recovery policy | NEW to inventory | `constants.ts` ISSUE_EXECUTION_MONITOR_*; `routes/issues.ts` monitor/check-now | missing (routines, `work --every` closest) | port |
| PC-115 | Run diagnostics, failure reports, provider traces | NEW to inventory | `server/src/services/run-failure-report.ts`; `routes/issues.ts` diagnostics | built (trail, `log`, `resume`, `queue list` reasons) | already-built |
| PC-116 | Live events over WebSocket | NEW to inventory | `constants.ts` LIVE_EVENT_TYPES; `server/src/realtime` | built (`tui_bridge.rs`; `--json`, rpc events) | already-built |
| PC-117 | Persistent agent-writable files and memory | NEW upstream since the tag (0287, #14420) | `doc/agent-files.md`; `server/src/services/agent-file-store.ts` | missing (model writes never land in `.trellis/`, ADR 0023) | owner-decision |
| PC-118 | Skills created from work (create_skill, update_skill) | NEW upstream since the tag (#13538) | `doc/runner-created-skills.md`; `server/src/services/skill-tools.ts` | excluded (ADR 0023 `.trellis/` write rule) | owner-decision |
| PC-119 | Skill sources synced from GitHub | NEW upstream since the tag (0291-0292, #14713) | `server/src/services/skill-sources.ts` | built (`install <source>@<ref>`, ADR 0039) | already-built |
| PC-120 | Core Paperclip agent skills and beta pin | NEW to inventory | `skills/`; `skills-releases/paperclip` | built in effect (loop prompt, ADR 0005; vault skill, ADR 0022) | already-built |
| PC-121 | Runner API escape hatch (search_api, call_api), assigned-tool search | NEW upstream as a default (#14186) | `doc/runner-api-tools.md` | excluded (rows 97-98) | owner-decision |
| PC-122 | Agent personas | NEW upstream since the tag (0280, #13171) | `doc/agent-personas.md` | excluded (cosmetic) | desktop-ui |
| PC-123 | Onboarding and seeded first task | NEW to inventory; first-task skill since the tag | `server/src/services/onboarding-seed.ts`; `cli/src/commands/onboard.ts` | excluded (brief Part 1; `doctor` probes setup) | desktop-ui |
| PC-124 | Company and task search | NEW to inventory | `doc/SEARCH.md`; `server/src/services/task-search.ts` | partial (`vault search`, `queue list`) | port |
| PC-125 | Document annotations | NEW to inventory | `server/src/services/document-annotations.ts` | excluded (#56; Apex 12) | owner-decision |
| PC-126 | External objects (PR and ticket status) | NEW as a row | `server/src/services/external-objects.ts` | excluded (networked) | owner-decision |
| PC-127 | Folders | NEW to inventory (0174) | `server/src/routes/folders.ts` | missing | desktop-ui |
| PC-128 | In-app announcements | NEW to inventory | `doc/ANNOUNCEMENTS.md` | excluded (networked, UI) | desktop-ui |
| PC-129 | Smoke Lab | NEW to inventory | `server/src/routes/smoke-lab.ts` | excluded (connectors excluded) | owner-decision |
| PC-130 | Decision queues and triage | NEW to inventory (0198) | `server/src/routes/decision-queues.ts` | excluded (as rows 35-36) | desktop-ui |
| PC-131 | Per-user surfaces: memberships, sidebar, profiles | NEW to inventory | `server/src/routes/resource-memberships.ts`, `user-profiles.ts` | excluded ([multi-user][web-ui]) | desktop-ui |
| PC-132 | Workspace file resources and file viewer | NEW as a row | `server/src/routes/file-resources.ts` | partial (tui `@` search, #52; `read_file`) | desktop-ui |
| PC-133 | Hosted agent profiles: Claude Managed Agents, AWS AgentCore | NEW to inventory | `server/src/services/managed-agent-profiles.ts`, `remote-agent-profiles.ts` | missing (hosted models only, ADR 0024) | drive-via-runtime |
| PC-134 | Cloud-managed instances and execution allowlist | NEW to inventory | `packages/shared/src/feature-catalog.ts`; `server/src/services/execution-allowlist.ts` | excluded (brief Part 1; ADR 0010) | owner-decision |
| PC-135 | Task drain and hot restart | NEW to inventory | `server/src/routes/instance-settings.ts`; `doc/DEVELOPING.md:297-321` | excluded (no daemon, ADR 0020, ADR 0026 §7 #68) | owner-decision |
| PC-136 | Evals: runner, product E2E, eval kernel | NEW to inventory | `doc/evals.md`; `packages/paperclip-eval-kernel` | partial (`docs/BENCHMARK_AND_EVAL.md`, `docs/bench`) | port |
| PC-137 | First-party telemetry | NEW to inventory | `packages/shared/src/telemetry`; `doc/TELEMETRY_WORKFLOW.md` | excluded (brief Part 1; tracing only) | owner-decision |
| PC-138 | Per-message model and effort; prompt-only task creation | NEW upstream since the tag (#14322, 0293) | `ui/src/components/task-chat/ComposerRunSettingsPicker.tsx`; `schema/issues.ts` | partial (`--model`, `/model`, thinking levels; no title generation) | port |
| PC-139 | Routine activity gate | NEW to inventory | `server/src/services/routines.ts:3261-3275` | missing | port |
| PC-140 | Harness permission defaults to full auto | NEW upstream since the tag (#13686, #13693) | `releases/v2026.1001.0.md`; `packages/paperclip-runner/README.md` | built (control dial, ADR 0018) | already-built |
| PC-141 | Responsible-user attribution and per-person GitHub identity | NEW to inventory | `doc/execution-github-identity.md`; `schema/run_identity_contexts.ts` | excluded ([multi-user]) | owner-decision |
| PC-142 | Queued responses and steering into a running turn | NEW upstream since the tag (#13539) | `server/src/services/queued-interaction-response.ts` | built (steering, ADR 0034; rpc, tui, session) | already-built |
| PC-143 | Agents hiring agents | NEW to inventory | `routes/agents.ts` agent-hires; `skills/paperclip-create-agent` | excluded (rows 55-57) | owner-decision |
| PC-144 | Agent delegation: agents create and assign sub-tasks | NEW to inventory | `docs/guides/board-operator/delegation.md`; `skills/paperclip-converting-plans-to-tasks` | excluded (row 31, #141; ADR 0005) | owner-decision |

## Counts

- **Rows:** 144.
  - PC-01 to PC-100: 56 CHANGED and 44 UNCHANGED.
  - PC-101 to PC-144: 44 NEW. 11 are upstream since the tag (PC-105, 107, 108, 117, 118, 119, 121, 122, 138, 140, 142). 33 are new to the inventory.
- **TRELLIS status:** 44 built, 22 partial, 13 missing, 65 excluded.
- **Disposition:** 44 already-built, 22 port, 9 drive-via-runtime, 13 desktop-ui, 56 owner-decision.

## Not located at head

- **30-second interval floor.** Stated in `doc/SPEC-implementation.md` §11.6. No such clamp was found in `heartbeat.ts` or the validators.
- **Secret folders** (earlier PC-70). Not found. Named provider vaults exist (`docs/api/secrets.md:180`).
- **"Task Chat" and "Auto-Create Recovery Tasks"** (earlier PC-100) are not flags at head. The Task Watchdogs flag is stripped (`validators/instance.test.ts:36-45`).
- **Signing modes.** `docs/api/routines.md:129` lists four. `constants.ts` has six.
- **Plugin approvals.** `doc/plugins/PLUGIN_SPEC.md` §15.2 forbids approval decisions, but `constants.ts` PLUGIN_CAPABILITIES has `approvals.respond`.

## Open questions

1. **Mention wakes.** Upstream removed @-mention dispatch (#14577, a breaking change). Trellis built mention wakes (#57, ADR 0026 §1). Keep them, or follow upstream?
2. **Agent-filed work.** Agent Chat delegation (PC-101), hiring (PC-143) and delegation (PC-144) are now central upstream. Each has a model create its own work, which ADR 0021 row 31 (#141) excludes. Is that to be reopened?
3. **Agent-writable files.** Persistent agent files (PC-117) and skills created from work (PC-118) conflict with the rule that model writes never land in `.trellis/`. Is there anywhere agent-writable memory could live, for example vault notes?
4. **The Rust runner.** `packages/paperclip-runner/runner/crates/runner-core` is Rust, and MIT under the root licence; the package licence was not checked. Should ACP or PRP become a drive-via-runtime target beside the `process` profile (PC-10, 11, 109)?
5. **desktop-ui.** Confirm that it means the planned desktop surface. Brief Part 1 and owner decision #70 still exclude interfaces beyond the CLI.
6. **ADR 0021 is out of date** on rows 25 (mentions built, #57), 47 (IANA zones built, #47), 59-63 (services built, ADR 0026 §12) and pi 1.2 (steering built in rpc, tui and session). Refresh it?
7. **Harness defaults.** Upstream harnesses now default to full auto (PC-140). Trellis's dial is unaffected. Confirm no change is wanted for process-agent profiles.

## Coverage left unread

- `ui/src` beyond file listings and the composer run-settings picker.
- `server/src/services/native-runtime/` beyond `completion-contracts.ts` and `evidence-classifier.ts`.
- Connector and chat-provider internals.
- `heartbeat.ts` (30,460 lines) beyond grep.
- Most of the 446 service files, which are cited by file name.
- `packages/paperclip-runner` beyond its README and file layout.
- Plugin SDK internals, tests, scripts and Storybook.
- Most of `doc/plans`, read by title only.
- The hosted docs at docs.paperclip.ing.
- Developer tooling, which is not inventoried.
- ClipHub (`doc/CLIPHUB.md`) and the External Task Protocol (`docs/specs/external-task-protocol.md`) are design documents with no code found, so they are not rows.
