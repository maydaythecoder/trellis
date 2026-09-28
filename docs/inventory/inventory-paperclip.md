**Paperclip v2026.916.1 capability inventory.** `d:` = https://docs.paperclip.ing/, `r:` = repository path. Adapter list, routes and CLI were checked at tag v2026.916.1. Spec docs were read on master (2026-09-27); the cited facts match the tag, except where noted in item 47. Tags mark what the capability itself requires.

**Companies**
1. Multi-company — many isolated companies per instance; every entity is company-scoped. (d:guides/welcome/key-concepts) [db]
2. Company lifecycle — CRUD, branding, stats, archive (pauses agents), delete. (d:reference/api/companies)
3. Company settings — hire-approval toggle, feedback sharing. (d:administration/company) [web-ui]

**Org chart and access**
4. Reporting tree — CEO reports to the board, one manager per agent, no cycles; export as SVG/PNG. (d:guides/org/org-structure)
5. Roles — Owner/Admin/Operator/Viewer plus 21 permission keys. (d:administration/roles-and-permissions) [multi-user]
6. Members — invites, join requests, instance admin, offboarding, profiles. (d:guides/org/members-and-access) [multi-user]
7. Agent permissions — scoped grants; protected agents, projects and issues. (d:reference/api/agents)
8. Team Catalog — installable starter teams. (d:guides/org/team-catalog)
9. Built-in agents — Briefs, Learning, Reflection Coach. (d:reference/api/built-in-agents)

**Agents and adapters**
10. Built-in adapters (16) — claude_local, codex_local, gemini_local, cursor, cursor_cloud, opencode_local, pi_local, grok_local, kimi_local, hermes_local, hermes_gateway, openclaw_gateway, process, http, acpx_local (retired), paperclip_runner (experimental). (r:server/src/adapters/builtin-adapter-types.ts)
11. ACP engine — `engine: auto|acp|cli` on claude, codex, gemini and kimi. (d:reference/adapters/overview)
12. External adapters — npm or local plugin with a custom UI parser. (d:reference/adapters/external-adapters) [server]
13. Lifecycle — pause/resume/terminate; statuses active, idle, running, error, paused, terminated. (d:reference/api/agents)
14. Instructions — AGENTS/SOUL/HEARTBEAT/TOOLS.md bundle. (d:guides/org/agents)
15. Config revisions — atomic rollback. (d:guides/org/agents) [db]
16. Environments (experimental) — Local, SSH or sandbox targets. (d:experimental/environments)
17. Sandbox providers — Cloudflare, Daytona, E2B, exe.dev, Kubernetes, Modal, Novita (plugins). (d:reference/adapters/sandbox-providers) [server]
18. Custom sandbox images — built in an in-browser terminal. (d:guides/projects-workflow/custom-sandbox-images) [web-ui]

**Tasks and issues**
19. Fields — identifier, title, description, status, priority (critical/high/medium/low), project, goal, parent, assignee (agent XOR user), work_mode, execution_policy, billing_code, workspace, lock columns. (r:doc/SPEC-implementation.md §7.6) [db]
20. Statuses — backlog, todo, in_progress, in_review, blocked, done, cancelled. (r:docs/api/issues.md)
21. Atomic checkout — `expectedStatuses`, 409 on conflict, stale-lock adoption, release. (r:docs/api/issues.md) [db]
22. Execution lock — checkoutRunId/executionRunId, compare-and-clear, sweeper. (r:doc/execution-semantics.md) [db]
23. Blockers — `blockedByIssueIds`; resolution wakes the issue; `blocked` needs a typed reason. (d:guides/day-to-day/blocked-inbox)
24. Sub-issues — parent wakes when all children are terminal. (r:doc/execution-semantics.md)
25. Comments — @-mention wakes an agent; missing-comment retry. (r:docs/guides/agent-developer/comments-and-communication.md, d:guides/power/execution-policy)
26. Documents — keyed, revisioned, restorable, lockable. (d:reference/api/issues) [db]
27. Attachments — upload, download, preview. (r:docs/api/issues.md)
28. Work products and artifacts — Artifacts page. (d:guides/day-to-day/artifacts)
29. Labels — company-scoped. (d:guides/day-to-day/issues)
30. Interactions — questions, confirmations, checkbox, verdicts. (d:reference/api/checkbox-confirmations)
31. Work modes — standard, ask, planning; plan decomposition. (d:guides/day-to-day/work-modes)
32. Execution policy — review/approval stages, escalation to a human. (d:guides/power/execution-policy) [multi-user]
33. Recovery — monitors, recovery actions, watchdogs (experimental). (d:experimental/task-watchdogs) [db]
34. Decisions — 1–8 options, 7-day expiry. (d:reference/api/decisions) [multi-user]
35. Attention feed and inbox — ranked board queue with dismiss/snooze; Mine/Recent/Unread/Blocked tabs. (d:reference/api/attention) [web-ui][multi-user]
36. Inbox agent policy — each user chooses open, allowlist or disabled for agents managing their inbox. (d:reference/api/inbox-agent-policy) [multi-user]
37. Cases, pipelines, status cards, summary slots (experimental). (d:reference/api/cases)
38. Feedback and decision training — helpful/needs-work votes on comments and revisions; human-only JSONL decision snapshots. (d:guides/day-to-day/feedback-voting)
39. Projects — status, lead, repo/cwd binding. (d:guides/projects-workflow/projects)
40. Goals — company/team/agent/task tree; issues return `ancestors`. (d:reference/api/goals-and-projects)

**Heartbeats and wakeups**
41. Triggers — timer, assignment, on_demand, automation, mention, approval, blockers resolved. (r:docs/agents-runtime.md) [server]
42. Queue — database-backed; wakes coalesce into the active run. (r:docs/agents-runtime.md) [server][db]
43. Scheduler — interval ≥30s, maxConcurrentRuns 1–50; skips paused or over-budget agents. (r:doc/SPEC-implementation.md §11.6) [server]
44. Runs — queued/running/succeeded/failed/cancelled/timed_out; stored logs; session resume across heartbeats. (r:doc/SPEC-implementation.md §7.8) [db]
45. Recovery — orphan reconciliation, rewake throttle. (d:how-to/debug-stuck-heartbeat) [server]
46. Protocol — identity → approvals → checkout → `/heartbeat-context` → work → delegate; `X-Paperclip-Run-Id` header. (r:docs/guides/agent-developer/heartbeat-protocol.md) [server]

**Routines**
47. Triggers — cron with timezone, signed webhook, API. At the tag, signing is bearer or hmac_sha256; master adds github_hmac and none. (r:docs/api/routines.md) [server]
48. Policies — concurrency (coalesce/skip/always) and catch-up (skip or capped enqueue). (r:docs/api/routines.md)
49. Each run creates an issue and wakes the assignee; `{{var}}` templates; revisions. (d:guides/projects-workflow/routines) [server][db]

**Budgets and costs**
50. Cost events — tokens plus costCents by agent, provider, model, issue, project. (d:reference/api/costs) [db]
51. Scopes — company monthly, agent monthly, project; cents; no goal or issue budgets. (d:how-to/set-monthly-budget)
52. Policies — warnPercent (default 80), hard stop, monthly or lifetime window. (d:reference/api/costs)
53. Hard stop — at 100%: pauses the scope, stops heartbeats, cancels work, raises an override approval; keep-paused or raise-and-resume; monthly UTC rollover. (d:reference/api/costs) [db]
54. Breakdowns — provider, biller, rolling windows, quota windows, finance ledger. (d:guides/day-to-day/costs) [db]

**Governance**
55. Approval types — hire_agent, approve_ceo_strategy, budget_override_required, request_board_approval. (d:reference/api/approvals)
56. Approval lifecycle — pending → approved/rejected/revision_requested → resubmit; comments, linked issues; resolution wakes the requesting agent. (d:reference/api/approvals)
57. Board powers — pause, terminate, reassign; CEO strategy gate. (d:guides/day-to-day/approvals)
58. Trust presets — standard, low_trust_review (sandboxed, bounded scope, quarantined output until promoted). (d:administration/trust-and-low-trust-review)

**Workspaces**
59. Execution workspaces — isolated (new git worktree and branch off the base ref, plus a provision command), reused, or project primary. Behind an experimental flag. (d:guides/projects-workflow/workspaces)
60. Consistency — environment-conflict detection, finalize-before-wake. (d:guides/projects-workflow/workspaces)
61. Runtime services — manually started dev servers. (d:guides/projects-workflow/workspaces) [server][web-ui]
62. HTTPS previews — through Tailscale, fail-closed. (d:reference/deploy/tailscale-https-broker) [server]
63. Archive — explicit archive cleans up the workspace, unmerged work is lost; Paperclip never pushes to a remote. (d:guides/projects-workflow/workspaces)

**Skills**
64. SKILL.md — import from GitHub, URL, local path, skills.sh. (d:guides/org/skills)
65. Catalog — bundled: doc-maintenance, issue-triage, status-card-query, task-planning, wireframe, qa-acceptance, github-pr-workflow; optional: agent-browser, release-announcement, simplified-english, ramp, design-critique, last30days, prepare-mcp-integration. (d:reference/skills)
66. Injection — persistent, ephemeral, tracked-only. (d:reference/skills)
67. Skill policy — allow/deny across 8 actions. (d:reference/api/company-skill-policy)
68. Skill Studio — test-run a skill. (d:guides/org/skills) [web-ui]

**Secrets**
69. Providers — local_encrypted, AWS Secrets Manager. (d:reference/deploy/secrets)
70. Scopes — company or per-user; folders. (d:administration/secret-scopes) [multi-user]
71. Delivery — env, run API, rotation, proposals, strict mode. (d:reference/api/secrets)

**Connectors and MCP**
72. Connectors — about 45 providers (e.g. GitHub, Slack, Linear, Jira, Notion, Google Workspace, Stripe); 4 access gates; Allowed/Ask/Off per action. (d:connectors) [server]
73. Review requests — allow once, always allow, decline. (d:connectors/review-requests) [web-ui]
74. Tool gateway — remote_http and local_stdio MCP; profiles per company/agent/project/routine/issue; allow/block/require_approval/rate_limit; audited calls. (d:reference/api/tool-gateway) [server][db]
75. `/mcp` endpoint and connection intents. (r:doc/MCP-ACCESS-GOVERNANCE.md) [server]

**Plugins**
76. Manifest — jobs, webhooks, tools, database, UI. (r:doc/plugins/PLUGIN_SPEC.md §10)
77. Capabilities — entity read/write (issues, comments, documents, agents, goals, costs, activity), plugin state, events.subscribe/emit, jobs.schedule, webhooks.receive, http.outbound, secrets.read-ref, agent.tools.register, ui.* slots; approvals, budget override and lock override are forbidden. (r:doc/plugins/PLUGIN_SPEC.md §15)
78. Workers — JSON-RPC over stdio; events. (r:doc/plugins/PLUGIN_SPEC.md §12–16) [server]
79. Host-scheduled jobs, signed inbound webhooks, agent tools via executeTool, scoped `/api/plugins/:id/api/*` routes. (r:doc/plugins/PLUGIN_SPEC.md §17–18) [server][db]
80. UI slots — page, detailTab, dashboardWidget, sidebar, toolbar, context menus, settings pages; same-origin, not sandboxed. (r:doc/plugins/PLUGIN_SPEC.md §19) [web-ui]
81. State — scoped plugin state, namespaced Postgres schema; npm/local install, upgrade needs capability re-approval. (d:administration/plugins) [db]
82. Shipped — LLM Wiki, Workspace Diff, hello-world, file-browser, kitchen-sink examples. (d:reference/plugins/llm-wiki)

**Activity**
83. Log — every mutation with actor type (agent/user/system/plugin), action, entity, run ID, responsible user. (d:reference/api/activity) [db]
84. Filters and CSV export gated by `audit:view_agent_actions`; exports are themselves logged. (d:reference/api/activity) [multi-user]

**Portability**
85. Package — COMPANY/TEAM/AGENTS/PROJECT/TASK/SKILL.md plus `.paperclip.yaml`. (r:docs/companies/companies-spec.md)
86. Export — agents, projects, skills, tasks, routines; secret values, paths and IDs scrubbed. (d:guides/power/export-import)
87. Import — folder, zip or GitHub; dry-run; rename/skip/replace on collision; backup and restore reuse this. (d:how-to/back-up-and-restore-a-company)

**Identity and deployment**
88. Modes — local_trusted, authenticated private/public. (d:reference/deploy/deployment-modes)
89. Sessions — Better Auth, CLI login. (d:administration/cli-auth) [server][multi-user]
90. Run JWTs and agent keys. (d:reference/api/authentication) [server]
91. Database — embedded Postgres or DATABASE_URL. (d:reference/deploy/database) [db]
92. Storage — local or S3. (d:reference/deploy/storage)
93. Deploy — Docker, Tailscale, VPS, ECS; OTel, Sentry. (d:reference/deploy/overview) [server]

**CLI (`paperclipai`)**
94. Setup — onboard, configure, doctor, env, channels, allowed-hostname, db:backup, run, test-drive, install, update, uninstall, service, worktree, env-lab, heartbeat. (d:reference/cli/setup-commands)
95. Client — connect, auth, token, context, access, company, project, goal, issue, agent, managed-agent, org, run, routine(s), pipelines, approval, activity, dashboard, cost, budget, finance, workspace, environment, adapter, asset, skill(s), secrets, teams, plugin, feedback, connections, email, openapi. (r:cli/src/commands/client/) [server]
96. Options — `--api-base`, `--api-key`, `--context`, `--company-id`, `--json`. (d:reference/cli/common-options)

**REST API (`/api`)**
97. Auth — trusted local, session, board bearer, agent key, run JWT. (d:reference/api/overview) [server]
98. Groups — companies, agents, projects, goals, issues, routines, approvals, decisions, costs, secrets, activity, dashboard, skills, environments, workspaces, adapters, tool-gateway, plugins, instance, openapi.json, llms. (r:server/src/routes/) [server][db]

**UI**
99. Views — dashboard, ⌘K command palette, inbox, org chart, work timeline (Gantt), artifacts, costs tabs, mobile layouts. (d:guides/day-to-day/dashboard) [web-ui]
100. Experimental flags — Environments, Isolated Workspaces, File Viewer, External Objects, Plan Decomposition Panel, Task Chat, Task Watchdogs, Status Cards, Server Info Debug View, Auto-Restart Dev Server, Auto-Create Recovery Tasks. Cloud Sync is retired. (d:experimental/overview) [web-ui]
