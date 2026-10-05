# Paperclip capability inventory addendum (2026-10-04): PC-101 to PC-144

This addendum lists the Paperclip capabilities that `docs/inventory/inventory-paperclip.md` (v2026.916.1) has no row for, read from `paperclipai/paperclip` at commit `1c07b5903b1b11139b1e1ce052a3cd4885865d90` (2026-10-04, #15100). PC-01 to PC-100 are rows 1–100 of `docs/inventory/inventory-paperclip.md` and are not repeated here. Of the 44 ids below, PC-105, 107, 108, 117, 118, 119, 121, 122, 138, 140 and 142 arrived upstream after v2026.916.1; the other 33 were present earlier but missed by that inventory or folded into another of its rows (the native runner, PC-109, was part of row 10). The file records no dispositions; those are ADR 0021's.

**Paths.** `r:` is a repository path at commit 1c07b590, `:<n>` a line, and a trailing `/` a directory. The paths are as #334 gives them. All 134 were checked here against the upstream tree at that commit (`git/trees/1c07b590?recursive=1`, not truncated), and every one exists. Every line citation was read at that commit and says what its row or claim says; `startTaskDrainRequestSchema` itself starts at `instance.ts:128`, inside the cited range 123–144. Beyond those lines, file contents were not read.

**Earlier draft.** The re-inventory this addendum narrows, with all 144 rows, change notes for PC-01 to PC-100, TRELLIS status and dispositions, is `git show 2539e4c:docs/inventory/inventory-paperclip-20261004.md`.

| ID | Capability | Upstream paths |
|---|---|---|
| PC-101 | Agent Chat | r:server/src/services/agent-conversations.ts; r:server/src/services/chat-completion-delivery.ts; r:packages/db/src/migrations/0288_glorious_jamie_braddock.sql |
| PC-102 | Board concierge chat, Conference Room | r:server/src/routes/board-chat.ts; r:packages/shared/src/feature-catalog.ts:165 |
| PC-103 | Chat channel connectors | r:server/src/routes/chat-channels.ts; r:server/src/services/chat-channels.ts; r:doc/connections/CHAT-CONNECTOR-UX.md |
| PC-104 | Email channels | r:server/src/routes/email.ts; r:server/src/services/email-channels.ts |
| PC-105 | GitHub review bots | r:doc/connections/GITHUB-REVIEW-BOT.md; r:server/src/services/chat-github-reviews.ts; r:server/src/services/chat-github-review-policy.ts; r:packages/db/src/migrations/0283_jittery_psynapse.sql |
| PC-106 | AI connections | r:doc/connections/AI-CONNECTIONS.md; r:server/src/routes/ai-connections.ts; r:server/src/services/ai-connections.ts; r:server/src/services/device-login-service.ts |
| PC-107 | Browser Use Cloud | r:doc/connections/BROWSER-USE.md; r:server/src/routes/browser-use.ts; r:server/src/services/browser-use.ts; r:packages/db/src/migrations/0290_browser_use_cloud.sql |
| PC-108 | Memory connectors | r:doc/connections/MEMORY.md; r:doc/memory-landscape.md; r:packages/shared/src/validators/instance.ts:56 (`enableMemoryConnectors`); r:server/src/services/cognee-connection.ts |
| PC-109 | Paperclip native runner | r:doc/architecture/paperclip-runner.md; r:packages/paperclip-runner/; r:server/src/services/native-runtime/; r:server/src/realtime/runner-prp-ws.ts; r:doc/grok-native-runner.md |
| PC-110 | Durable session goals | r:server/src/services/runner-goals.ts; r:server/src/services/runner-goal-control-broker.ts; r:packages/db/src/schema/agent_task_sessions.ts |
| PC-111 | Completion contracts and status arbitration | r:doc/architecture/native-status-arbitration.md; r:doc/execution-semantics.md:1397; r:doc/execution-semantics.md:1492 |
| PC-112 | Durable continuations and scheduled retries | r:doc/architecture/durable-continuation-scheduler.md; r:server/src/services/routable-blocked.ts; r:server/src/services/run-continuations.ts; r:releases/beta/v2026.1002.0-beta.0.md:29 (#14046, #13888) |
| PC-113 | Issue tree control | r:packages/shared/src/constants.ts:429-436 (`ISSUE_TREE_CONTROL_MODES`); r:server/src/routes/issue-tree-control.ts; r:server/src/services/issue-tree-control.ts; r:doc/composer-stop.md |
| PC-114 | Execution monitors | r:packages/shared/src/constants.ts:527-543 (`ISSUE_EXECUTION_MONITOR_KINDS`); r:server/src/services/issue-execution-policy.ts; r:doc/execution-semantics.md:351 |
| PC-115 | Run diagnostics and failure reports | r:server/src/services/run-failure-diagnostics.ts; r:server/src/services/run-failure-report.ts |
| PC-116 | Live events | r:server/src/services/live-events.ts; r:server/src/realtime/live-events-ws.ts; r:doc/run-log-events.md |
| PC-117 | Persistent agent files | r:doc/agent-files.md; r:server/src/services/agent-file-store.ts; r:packages/db/src/migrations/0287_serious_tinkerer.sql; r:skills/para-memory-files/SKILL.md |
| PC-118 | Skills created from work | r:doc/runner-created-skills.md; r:packages/paperclip-runner/src/protocol-actions/create-skill.ts |
| PC-119 | Skill sources from GitHub | r:server/src/services/skill-sources.ts; r:server/src/services/github-skill-source.ts; r:server/src/services/skill-source-git-snapshot.ts; r:packages/db/src/migrations/0291_conscious_secret_warriors.sql; r:packages/db/src/migrations/0292_powerful_devos.sql |
| PC-120 | Core Paperclip agent skills | r:skills/paperclip/SKILL.md; r:skills/paperclip-board/SKILL.md; r:skills/paperclip-converting-plans-to-tasks/SKILL.md; r:skills/paperclip-create-agent/SKILL.md |
| PC-121 | Runner API escape hatch | r:doc/runner-api-tools.md; r:packages/paperclip-runner/src/protocol-actions/search-api.ts; r:packages/paperclip-runner/src/protocol-actions/call-api.ts; r:server/src/services/native-runtime/runner-api-rollout.ts |
| PC-122 | Agent personas | r:doc/agent-personas.md; r:packages/shared/src/agent-appearance.ts; r:packages/db/src/migrations/0280_unique_genesis.sql |
| PC-123 | Onboarding and first task | r:server/src/services/onboarding-first-task-assets.ts; r:server/src/services/onboarding-seed.ts; r:server/src/routes/onboarding-seed.ts |
| PC-124 | Company and task search | r:doc/SEARCH.md; r:server/src/services/task-search.ts; r:server/src/services/company-search.ts; r:server/src/routes/issues.ts:7814 |
| PC-125 | Document annotations | r:server/src/services/document-annotations.ts; r:packages/shared/src/document-anchors.ts |
| PC-126 | External objects | r:server/src/services/external-objects.ts; r:server/src/services/github-external-object-provider.ts; r:packages/shared/src/external-objects.ts |
| PC-127 | Folders | r:server/src/routes/folders.ts; r:server/src/services/folders.ts |
| PC-128 | In-app announcements | r:doc/ANNOUNCEMENTS.md; r:server/src/services/announcement-feed.ts; r:server/src/routes/announcements.ts; r:announcements/current.json |
| PC-129 | Smoke Lab | r:doc/connections/SMOKE-LAB-TUTORIAL.md; r:server/src/routes/smoke-lab.ts; r:server/src/services/smoke-lab.ts |
| PC-130 | Decision queues and triage | r:server/src/routes/decision-queues.ts; r:server/src/services/decision-queues.ts; r:packages/shared/src/validators/decision-queue.ts |
| PC-131 | Per-user surfaces | r:server/src/routes/user-profiles.ts; r:server/src/routes/sidebar-preferences.ts; r:server/src/routes/inbox-dismissals.ts |
| PC-132 | Workspace file resources | r:server/src/routes/file-resources.ts; r:server/src/services/workspace-file-resources.ts |
| PC-133 | Hosted agent profiles | r:server/src/routes/managed-agent-profiles.ts; r:server/src/services/managed-agent-profiles.ts; r:server/src/routes/remote-agent-profiles.ts; r:server/src/services/remote-agent-profiles.ts |
| PC-134 | Cloud-managed instances and execution allowlist | r:server/src/routes/cloud.ts; r:server/src/services/cloud-instance.ts; r:server/src/services/execution-allowlist.ts |
| PC-135 | Task drain and hot restart | r:packages/shared/src/validators/instance.ts:123-144 (`startTaskDrainRequestSchema`); r:server/src/routes/instance-settings.ts; r:server/src/services/hot-restart.ts |
| PC-136 | Evals | r:doc/evals.md; r:evals/; r:packages/paperclip-eval-kernel/ |
| PC-137 | First-party telemetry | r:server/src/telemetry.ts; r:doc/TELEMETRY_WORKFLOW.md |
| PC-138 | Per-message run settings, prompt-only task creation | r:ui/src/components/task-chat/ComposerRunSettingsPicker.tsx; r:server/src/services/issue-title.ts; r:packages/db/src/migrations/0293_broad_rattler.sql |
| PC-139 | Routine activity gate | r:packages/shared/src/constants.ts:642-646 (`ROUTINE_ACTIVITY_GATE_POLICIES`); r:packages/shared/src/validators/routine.ts; r:server/src/services/routines.ts |
| PC-140 | Harness permission defaults | r:server/src/services/native-runtime/native-execution-input.ts:265; r:releases/v2026.1001.0.md (Breaking Changes, #13686, #13693) |
| PC-141 | Responsible-user attribution, per-person GitHub identity | r:doc/execution-github-identity.md; r:server/src/services/responsible-user-denial-run-outcomes.ts; r:packages/shared/src/validators/company.ts |
| PC-142 | Queued responses and steering into a running turn | r:server/src/services/queued-interaction-response.ts; r:server/src/services/issue-comment-wakeup.ts; r:server/src/services/issue-queued-comment-queue.ts; r:doc/execution-semantics.md:425 |
| PC-143 | Agents hiring agents | r:skills/paperclip-create-agent/SKILL.md; r:server/src/services/hire-hook.ts; r:packages/shared/src/constants.ts:690 (`hire_agent`) |
| PC-144 | Agent delegation | r:doc/execution-semantics.md:273 (`create_task`, `set_dependencies`); r:skills/paperclip-converting-plans-to-tasks/SKILL.md |

## Claims the source does not bear out

Each cited line below was read at 1c07b590, and the two files named for the flags were searched for either flag and hold neither. The other absences ("stated only in", "the server stores no folder") are as #334 and the earlier draft give them; they were not searched for again here.

- **The 30-second minimum heartbeat interval.** It is stated only in r:doc/SPEC-implementation.md:1350 (§11.6, `intervalSec` minimum 30).
- **Secret folders.** They exist only as the Secrets page's grouping of `/`-separated secret names (r:ui/src/pages/secrets/secret-path.ts). The server stores no folder.
- **The Task Chat and Auto-Create Recovery Tasks experimental flags** (row 100 of `docs/inventory/inventory-paperclip.md`). No flag in r:packages/shared/src/feature-catalog.ts or r:packages/shared/src/validators/instance.ts names either.
- **Routine signing modes.** r:docs/api/routines.md:129 lists four. r:packages/shared/src/constants.ts:651 has six: `app_webhook` and `fireflies_hmac` besides those four.
- **Plugin approval decisions.** r:doc/plugins/PLUGIN_SPEC.md:878 (§15.2, forbidden capabilities) lists approval decisions, against the `approvals.respond` plugin capability in r:packages/shared/src/constants.ts:1372 and r:server/src/services/plugin-capability-validator.ts.
