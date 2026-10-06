# pi 1.0.2 capability inventory (@earendil-works/pi monorepo, MIT)

Re-inventory of pi against the previous `docs/inventory/inventory-pi.md` (pi-coding-agent 0.87.1) and the pi rows of ADR 0021.

> **Note, 2026-10-05 (#439).** This is a dated record and is not rewritten. The current record of pi 1.0.2's capabilities is `docs/inventory/inventory-pi.md`, as rewritten by #320 (152dbbf) at the same upstream commit. The two disagree: its totals are 54 / 47 / 35 against this file's 51 / 50 / 35, and it tags 4.12, 4.20 and 8.4 UNCHANGED where this file tags them CHANGED. Where they differ, `inventory-pi.md` holds. `inventory-pi.md` records no TRELLIS status or disposition; dispositions are ADR 0021's.

- **Upstream commit:** `200387122ca450d6387f033949423114a270b96c`, 2026-10-04 02:41 +0200, "Add [Unreleased] section for next cycle" (`git log -1` in the clone). The clone is depth 1, with one commit.
- **Version:** all 13 packages are at 1.0.2, and the coding-agent `[Unreleased]` section is empty. Packages: agent, ai, chord, client, codemode, coding-agent, durable, evals, mcp, protocol, server, telemetry, tui. pi-client, pi-codemode and pi-mcp are not in the old inventory.
- **Licence:** the root `LICENSE` is MIT (Copyright (c) 2025 Mario Zechner). 12 of 13 `package.json` files declare MIT, and `packages/evals` has no licence field. `packages/mcp/LICENSES/modelcontextprotocol-typescript-sdk.txt` is an MIT notice (Copyright (c) 2024 Anthropic, PBC) bundled with pi-mcp. Only its first lines were read.
- **TRELLIS side:** the TRELLIS commit is `525de6dc`. Statuses come from ADR 0021, its amendments, ADRs 0023–0044, and the code, with file and line cited per row.

## Method

- **Read in full.** I read every coding-agent doc not listed under the next three points. These include cli, codemode, mcp, models, providers, settings, configuration, extensions, virtual-models, usage, how-pi-works, slash-commands, sessions, session-format (to line 150), json, sdk, security, containerization, skills, prompt-templates, packages, llama-cpp, environment-variables, shell-aliases and custom-provider. I also read the coding-agent CHANGELOG from 0.87.1 to head, and the changelog sections since 0.87.1 for agent, ai, durable, telemetry, tui, mcp, codemode, protocol, server and client.
- **Read in part.**
  - `src/core/extensions/types.ts`, at these ranges: event overloads 1551–1623, ExtensionAPI 1630–1944, ExtensionUIContext 149–330, ExtensionContext 325–460, ToolDefinition 565–600.
  - The durable, mcp, codemode, client, protocol, server, chord and agent READMEs, at the sections cited.
  - The experimental READMEs for durable, vacation and services.
  - `examples/extensions/README.md`.
- **Read by headings only.** keybindings, themes, tui, terminal-setup, termux, tmux, windows, message-types, quickstart past the install section, rpc-commands past `steer`, and the ai, tui, telemetry and evals READMEs. For compaction.md past line 250 I used grep.
- **Not read.**
  - Source bodies, beyond directory listings and the types.ts ranges above.
  - Tests.
  - Examples beyond the README.
  - `durable/docs/spec.md` and `chord/PLANNING.md`.
- **Dating.**
  - Because the clone has one commit, "since 0.87.1" comes from changelog headings.
  - For inventory gaps, the note says "changelog mentions it under X". That is the earliest heading whose text contains the string, not a verified introduction date.
  - Three features have no changelog mention: `mcp_servers_change`, `defaultActive` and `thinking_level_changed`. Their evidence is types.ts or json.md instead.
- **Ids.**
  - Items that the old inventory numbered keep that number as `PI-<section>.<item>`.
  - Sections 4, 6, 7, 10, 11, 12, 14 and 15 were unnumbered there. Their items are numbered here in the order the old inventory listed them, and those numbers are now stable.
  - A new item takes the next free number in its section. Sections 16 (MCP client) and 17 (codemode, tool search, tool exposure) are new.
  - No new item reuses an old number. PI-2.17 and PI-2.18 split classification and image generation out of the old 2.14.
- **Change marks**, relative to the old inventory:
  - UNCHANGED.
  - CHANGED: the item was listed, and either upstream moved or the old entry missed part of it.
  - NEW, in two kinds: "upstream since 0.87.1", or "gap", meaning it existed before 0.87.1 and was not listed.
- **Dispositions.**
  - already-built: TRELLIS has it, or has part of it and an ADR declines the rest.
  - port: it fits TRELLIS and is missing or deferred.
  - drive-via-runtime: it belongs to the model runtime or provider and is reached through a `--model` spec.
  - desktop-ui: cosmetic or terminal-surface work, excluded by ADR 0021's itemised list or row 11. It goes to a desktop surface if one is built.
  - owner-decision: an ADR or brief clause conflicts, or no clause covers it and it has substance.

## Totals

| Measure | Count |
|---|---|
| Rows | 136 |
| UNCHANGED / CHANGED / NEW | 51 / 50 / 35 (23 upstream since 0.87.1, 12 gaps) |
| TRELLIS built / partial / missing / excluded | 43 / 38 / 28 / 27 |
| already-built / port / drive-via-runtime / desktop-ui / owner-decision | 63 / 19 / 3 / 15 / 36 |

Upstream changes since 0.87.1:

- **Codemode** (0.99.0, made leaner in 1.0.0). Model-written JavaScript runs in a QuickJS/WASM worker and calls tools. Only the script's output reaches the model. It ships with `tool_search`, a five-way tool exposure model, namespaces, MCP-style annotations, `outputSchema`/`structuredContent` results, and `ctx.executeTool` nested calls.
- **MCP client** (0.99.0–1.0.1). It covers stdio and streamable HTTP, OAuth (DCR, PKCE, step-up, RFC 9207, CIMD), resources, `/mcp`, `pi mcp add|remove|list|login|logout`, and project overrides, built on a standalone `@earendil-works/pi-mcp`.
- **Non-chat models.** These are classifier models (TypeSafe Jev, Cloudflare Clef, and every llama.cpp chat model scored from label log-probabilities) and image generation through OpenRouter. Both are first-class ModelRuntime operations (0.99.0–1.0.1).
- **Virtual models** (0.99.0, experimental). An extension routes each request to a physical model and keeps router state on the session branch.
- **pi-durable** was rebuilt as a durable harness (1.0.0, experimental), with checkpointed tasks, task-owned subagent conversations, child tasks and a task graph. agent-core removed its own experimental harness in the same release.
- **Smaller changes:**
  - `samplingParamsByThinkingLevel` (1.0.2)
  - Anthropic workload identity federation (0.99.2)
  - Sign in with ChatGPT (0.99.0)
  - Anthropic copy-code login and Radius in `/login` (1.0.0)
  - `provider_stream_event` (0.99.0) and `registerToolRenderer` (1.0.1)
  - Fullscreen mode by default (1.0.0) and the `system` theme by default (0.99.0)
  - Nix flake (1.0.1)
  - Per-input disposition on RPC prompt, steer and follow_up (0.99.0)

## Features

Path conventions: `ca/` stands for `packages/coding-agent/`. Other upstream paths are relative to the clone root. TRELLIS code paths are under `crates/`.

### 1. Agent loop and tools

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-1.1 | Turn loop | UNCHANGED | built | already-built | ca/docs/how-pi-works.md; packages/agent/README.md | trellis-loop/src/agent.rs; ADR 0005 |
| PI-1.2 | Steering and follow-up queues | CHANGED: per-input disposition on RPC/SDK (0.99.0) | partial | port | ca/docs/usage.md; ca/docs/rpc-commands.md | trellis-loop/src/agent.rs; trellis-cli/src/rpc.rs:557; trellis-cli/src/tui_run.rs:251; ADR 0034. No queue modes, no clear_queue |
| PI-1.3 | Built-in tools (read, bash, edit, write; grep, find, ls; powershell) | UNCHANGED | built | already-built | ca/docs/cli.md; ca/src/core/tools/ | trellis-loop/src/wire.rs:26-95; ADR 0023 §2-3 |
| PI-1.4 | Tool selection flags and defaultTools | CHANGED: +name/-name deltas (0.99.0); /reload enables new defaults (0.99.2) | built | already-built | ca/docs/cli.md; ca/docs/settings.md | trellis-cli/src/config.rs:1105-1126 |
| PI-1.5 | Output truncation | CHANGED: 1 MiB structured bash result for scripts; MCP text over 20 KB to temp file (0.99.0) | built | already-built | ca/src/core/tools/truncate.ts; ca/docs/codemode.md; ca/docs/mcp.md | ADR 0021 row 1.5; ADR 0019 |
| PI-1.6 | Swappable tool backends (ssh, Gondolin) | UNCHANGED | missing | port | ca/examples/extensions/ssh.ts; ca/docs/containerization.md | ADR 0021 row 1.6 (deferred); ADR 0032 |
| PI-1.7 | Parallel tool calls | CHANGED: codemode and nested calls add parallelism (0.99.0) | excluded | owner-decision | ca/docs/extensions.md; ca/src/core/tools/file-mutation-queue.ts | ADR 0021 row 1.7 (ADR 0005; owner kept, #63) |
| PI-1.8 | `terminate: true` | UNCHANGED | built | already-built | ca/docs/extensions.md; packages/agent/README.md | ADR 0021 row 1.8; trellis-loop/src/wire.rs (Done) |
| PI-1.9 | Retries and backoff | CHANGED (fixes): capacity errors retried (1.0.1); bad Retry-After falls back (0.99.2) | built | already-built | ca/docs/settings.md; ca/docs/json.md | trellis-model/src/hosted.rs (retry_delay); ADR 0024 §4 |
| PI-1.10 | Overflow recovery | UNCHANGED | built | already-built | ca/docs/compaction.md | trellis-model/src/lib.rs:31; trellis-loop/src/adapter.rs:116-141; ADR 0024 §5 |
| PI-1.11 | `!cmd` / `!!cmd` | UNCHANGED | built | already-built | ca/docs/usage.md | trellis-cli/src/session.rs; ADR 0025 §6 |
| PI-1.12 | Session env for model shells | UNCHANGED | built | already-built | ca/docs/environment-variables.md | trellis-loop/src/isolate.rs, hooks.rs (TRELLIS_RUN_ID, TRELLIS_MODEL) |
| PI-1.13 | Shell configuration | UNCHANGED | built | already-built | ca/docs/shell-aliases.md; ca/docs/windows.md | trellis-cli/src/config.rs:1124 |
| PI-1.14 | Images | UNCHANGED | partial | already-built | ca/docs/usage.md; ca/docs/models.md | trellis-cli/src/image_input.rs; trellis-model/src/image.rs; ADR 0041 |
| PI-1.15 | Prompt-cache warming | UNCHANGED | missing | port | ca/docs/settings.md; ca/src/core/cache-warmer.ts | ADR 0021 row 1.15 (v1.x, #67) |
| PI-1.16 | Process markers AI_AGENT, PI_CODING_AGENT | NEW (gap; changelog 0.67.1, 0.84.0) | partial | port | ca/docs/environment-variables.md | trellis-loop/src/isolate.rs; trellis-cli/src/process_agent.rs |

### 2. Providers, models, auth

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-2.1 | Built-in providers (30+) | CHANGED: TypeSafe (0.99.0), GPT-6.1 Sol (0.99.1), Codex renamed legacy | partial | drive-via-runtime | ca/docs/providers.md; packages/ai/README.md | trellis-model/src/hosted.rs; trellis-model/src/spec.rs; ADR 0024 §2 |
| PI-2.2 | Login (OAuth, device, API key) | CHANGED: Sign in with ChatGPT (0.99.0); Anthropic copy-code, Radius (1.0.0) | excluded | owner-decision | ca/docs/providers.md | ADR 0021 row 2.1-2.4 (ADR 0010) |
| PI-2.3 | Credential values (`!command`, `$VAR`, env object) | UNCHANGED | partial | owner-decision | ca/docs/providers.md; ca/docs/custom-provider.md | trellis-cli/src/config.rs:1170 (key_env); ADR 0024 §3; ADR 0010 |
| PI-2.4 | Cloud platforms (Azure, Bedrock, Vertex, Cloudflare) | CHANGED (fixes): Bedrock tiers and thinking signatures (1.0.1) | missing | port | ca/docs/providers.md | ADR 0021 row 2.1-2.4 (signing deferred, #45); ADR 0024 Not built |
| PI-2.5 | Credential precedence | UNCHANGED | built | already-built | ca/docs/models.md | trellis-cli/src/user_config.rs; ADR 0024 §3 |
| PI-2.6 | models.json endpoints and overrides | CHANGED: typed model entries (0.99.0) | partial | port | ca/docs/models.md | trellis-cli/src/config.rs:1152-1176 (input_bytes); ADR 0024 Not built |
| PI-2.7 | Model catalog overlay | CHANGED: chat, image and classifier types (0.99.0) | built | already-built | ca/docs/models.md; ca/src/core/remote-catalog-provider.ts | trellis-model/src/catalog.rs; trellis-cli/src/models_cmd.rs |
| PI-2.8 | Thinking levels | CHANGED: level recorded per assistant message (0.99.0) | built | already-built | ca/docs/settings.md; ca/docs/cli.md | trellis-model/src/hosted.rs:66-92, :175; trellis-cli/src/config.rs:1180 |
| PI-2.9 | Scoped model cycling | UNCHANGED | built | already-built | ca/docs/cli.md; ca/docs/models.md | trellis-cli/src/session.rs (/model, /models) |
| PI-2.10 | llama.cpp router (`/llama`) | CHANGED: router models are also classifiers (0.99.0); built-in extension | partial | drive-via-runtime | ca/docs/llama-cpp.md; ca/src/extensions/llama/ | trellis-model/src/pull.rs; ADR 0044; ADR 0021 row 2.10 |
| PI-2.11 | Radius gateway | CHANGED: in /login, MCP server, /share, /bug, relay (1.0.0) | excluded | owner-decision | ca/docs/providers.md; ca/src/core/radius.ts | ADR 0021 row 2.11 (ADR 0010) |
| PI-2.12 | Custom providers | CHANGED: mixed chat/image/classifier registrations; onProviderStreamEvent (0.99.0) | partial | already-built | ca/docs/custom-provider.md; ca/src/core/extensions/types.ts:1817-1944 | trellis-cli/src/config.rs:1152; ADR 0024 §2 |
| PI-2.13 | Transports and proxy | UNCHANGED | built | already-built | ca/docs/settings.md | trellis-model/src/transport.rs (HTTPS_PROXY, NO_PROXY) |
| PI-2.14 | pi-ai extras (constrained sampling, faux provider, handoff) | CHANGED: image and classifier operations moved into core Models (0.99.0) | built | already-built | packages/ai/README.md; packages/ai/CHANGELOG.md | trellis-model/src/grammar.rs; trellis-model/src/spec.rs (scripted:) |
| PI-2.15 | Anthropic workload identity federation | NEW (upstream 0.99.2) | missing | owner-decision | ca/docs/providers.md; packages/ai/CHANGELOG.md | ADR 0021 row 2.1-2.4 and #45 amendment; ADR 0010 |
| PI-2.16 | Sampling params by thinking level | NEW (upstream 1.0.2; samplingParams a gap, 0.84.0) | missing | drive-via-runtime | ca/docs/models.md | trellis-model/src/hosted.rs:263; trellis-model/src/runtime.rs:189-212 |
| PI-2.17 | Classifier models | CHANGED: split from 2.14; in the coding agent since 0.99.0, Clef 1.0.1 | excluded | owner-decision | ca/docs/models.md; ca/docs/llama-cpp.md; ca/docs/codemode.md | ADR 0021 row 2.14 |
| PI-2.18 | Image generation models | CHANGED: split from 2.14; in the coding agent since 0.99.0/1.0.0 | excluded | owner-decision | ca/docs/models.md; ca/docs/codemode.md | ADR 0021 row 2.14 |
| PI-2.19 | Virtual models (per-request routing) | NEW (upstream 0.99.0, experimental) | missing | owner-decision | ca/docs/virtual-models.md; ca/src/core/virtual-models.ts; ca/examples/extensions/jev-router.ts | ADR 0043 (policy only) |
| PI-2.20 | `pi auth check` / `print-api-key` / `print-bearer-token` | NEW (gap; changelog 0.68.0) | partial | port | ca/docs/cli.md; ca/src/cli/auth-check.ts | trellis-cli/src/doctor_cmd.rs |
| PI-2.21 | `--list-models` | NEW (gap; changelog 0.22.4) | built | already-built | ca/docs/cli.md; ca/src/cli/list-models.ts | trellis-cli/src/models_cmd.rs; session /models |

### 3. Context files and system prompt

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-3.1 | Context files (AGENTS.md, CLAUDE.md, AGENTS.override.md) | UNCHANGED | built | already-built | ca/docs/configuration.md | trellis-cli/src/context_files.rs; ADR 0025 §1 |
| PI-3.2 | SYSTEM.md / APPEND_SYSTEM.md | UNCHANGED | partial | port | ca/docs/configuration.md | ADR 0025 §2; ADR 0021 (user-level system.md not built) |
| PI-3.3 | `--system-prompt` / `--append-system-prompt` | UNCHANGED | built | already-built | ca/docs/cli.md | ADR 0021 row 3.2-3.3 (as files) |
| PI-3.4 | Sectioned system prompt | CHANGED: sections and tools persisted as transcript deltas; mcp_servers section (0.99.2) | built | already-built | ca/docs/session-format.md; ca/src/core/system-prompt.ts | trellis-loop/src/adapter.rs |

### 4. Extensions API

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-4.1 | Extension loading | CHANGED: built-in extensions (mcp, llama.cpp, codemode, tool-search), `-builtin:<name>`, replace warning (0.99.0) | partial | already-built | ca/docs/extensions.md; ca/src/core/extensions/loader.ts; ca/src/extensions/index.ts | trellis-loop/src/hooks.rs; trellis-loop/src/registry.rs:176-191; ADR 0023 §5-6 |
| PI-4.2 | Trust and resource events | UNCHANGED | partial | already-built | ca/src/core/extensions/types.ts:1556-1560 | trellis-cli/src/trust_cmd.rs; ADR 0023 §4 |
| PI-4.3 | Session events | UNCHANGED | partial | already-built | ca/src/core/extensions/types.ts:1561-1583 | ADR 0023 §5 (before_run, after_run) |
| PI-4.4 | Context and provider events | UNCHANGED | excluded | owner-decision | ca/src/core/extensions/types.ts:1584-1595 | ADR 0021 row '4 remaining events' |
| PI-4.5 | Agent run events | UNCHANGED | built | already-built | ca/src/core/extensions/types.ts:1597-1607 | trellis-loop/src/hooks.rs; ADR 0023 §5 |
| PI-4.6 | UI prompt events | UNCHANGED | excluded | desktop-ui | ca/src/core/extensions/types.ts:1608-1609 | ADR 0021 row '4 remaining events' |
| PI-4.7 | Turn and message events | UNCHANGED | built | already-built | ca/src/core/extensions/types.ts:1610-1614 | ADR 0023 §5 (post_tool); trellis-cli/src/tui_bridge.rs |
| PI-4.8 | Tool events (tool_call, tool_result) | CHANGED: nested calls carry parentToolCallId (0.99.0) | built | already-built | ca/src/core/extensions/types.ts:1615-1621; ca/docs/extensions.md | ADR 0023 §5 (pre_tool, post_tool) |
| PI-4.9 | Selection and input events | UNCHANGED | partial | already-built | ca/src/core/extensions/types.ts:1618-1623 | ADR 0023 §5 |
| PI-4.10 | Registration methods | UNCHANGED | partial | already-built | ca/src/core/extensions/types.ts:1630-1683 | trellis-loop/src/registry.rs:176; trellis-cli/src/templates.rs; ADR 0021 itemised list |
| PI-4.11 | Messages and entries (sendMessage, sendUserMessage, appendEntry) | UNCHANGED | partial | port | ca/src/core/extensions/types.ts:1690-1706 | ADR 0023 §5 (before_run output fenced) |
| PI-4.12 | Session metadata | CHANGED (gap: getSessionName, changelog 0.44.0) | built | already-built | ca/src/core/extensions/types.ts:1713-1719 | trellis-cli/src/session.rs:349-358 |
| PI-4.13 | Tools and commands (exec, active tools, getAllTools, getCommands, getSettings) | CHANGED: getAllTools reports exposure and annotations (0.99.0); getSettings a gap | partial | already-built | ca/src/core/extensions/types.ts:1722-1740 | trellis-loop/src/registry.rs:1-11; ADR 0026 §4 |
| PI-4.14 | Model methods | UNCHANGED | partial | already-built | ca/src/core/extensions/types.ts:1750-1759 | trellis-cli/src/session.rs; trellis-cli/src/config.rs:1180 |
| PI-4.15 | Event bus `pi.events` | UNCHANGED | missing | owner-decision | ca/docs/extensions.md; ca/src/core/event-bus.ts | none; hooks are separate processes |
| PI-4.16 | `ctx` fields and methods | CHANGED: executeTool and tools new (0.99.0); mode, model, hasPendingMessages, getSystemPromptOptions gaps | partial | already-built | ca/src/core/extensions/types.ts:325-400 | ADR 0023 §5 (hook stdin JSON) |
| PI-4.17 | Command-only ctx (newSession, fork, navigateTree, switchSession, reload) | UNCHANGED | missing | port | ca/src/core/extensions/types.ts:401-460 | ADR 0021 row 8.1-8.4 (deferred) |
| PI-4.18 | `ctx.ui` | CHANGED (gaps): working indicator, hidden thinking label, title, theme get/set | partial | desktop-ui | ca/src/core/extensions/types.ts:149-300 | ADR 0018; tui-demo/src/tui/app.rs (modal); ADR 0021 itemised list |
| PI-4.19 | Tool definition fields | CHANGED: outputSchema, exposure, namespace, annotations, defaultActive, prepareLoadout (0.99.0); renderShell a gap | partial | already-built | ca/src/core/extensions/types.ts:565-600 | trellis-loop/src/registry.rs:179-191 |
| PI-4.20 | Mode behaviour | CHANGED: `ctx.mode` guard | built | already-built | ca/docs/extensions.md | trellis-cli/src/rpc.rs:671 (approve) |
| PI-4.21 | `provider_stream_event` | NEW (upstream 0.99.0) | excluded | owner-decision | ca/docs/extensions.md; ca/examples/extensions/debug-provider.ts | ADR 0021 row '4 remaining events'; ADR 0024 Not built |
| PI-4.22 | `registerToolRenderer` | NEW (upstream 1.0.1) | excluded | desktop-ui | ca/docs/extensions.md | ADR 0021 itemised list (renderers) |
| PI-4.23 | Nested tool calls (`ctx.executeTool`) | NEW (upstream 0.99.0) | excluded | owner-decision | ca/docs/extensions.md; ca/src/core/nested-tool-calls.ts | ADR 0021 row 1.7 |
| PI-4.24 | Structured tool results (outputSchema, structuredContent, isError) | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/extensions.md | CLAUDE.md rule 2; trellis-loop/src/registry.rs:179-191 |

### 5. Skills

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-5.1 | Agent Skills format | UNCHANGED | built | already-built | ca/docs/skills.md | trellis-loop/src/skills.rs; ADR 0019 |
| PI-5.2 | Progressive disclosure | UNCHANGED | built | already-built | ca/docs/skills.md | trellis-loop/src/wire.rs (ReadSkill); ADR 0019 |
| PI-5.3 | `/skill:name` | UNCHANGED | built | already-built | ca/docs/skills.md | trellis-cli/src/session.rs:384; `run --skill` |
| PI-5.4 | Skill locations | UNCHANGED | built | already-built | ca/docs/skills.md | ADR 0025 Integrated; ADR 0039 §10 |

### 6. Prompt templates

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-6.1 | Prompt templates and arguments | CHANGED (gap): `${@:-default}`, `${@:N}`, `--prompt-template`, `-np` | built | already-built | ca/docs/prompt-templates.md; ca/src/core/prompt-templates.ts | trellis-cli/src/templates.rs:353-427; ADR 0025 §3 |

### 7. Slash commands

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-7.1 | Model and settings commands | UNCHANGED | partial | port | ca/docs/slash-commands.md | trellis-cli/src/session.rs:318-330 (/model, /models) |
| PI-7.2 | Session commands | UNCHANGED | partial | port | ca/docs/slash-commands.md | trellis-cli/src/session.rs (/resume, /name, /status); trellis-cli/src/rpc.rs:715 |
| PI-7.3 | Sharing and export commands | UNCHANGED | partial | already-built | ca/docs/slash-commands.md; ca/src/core/bug-report.ts | trellis-cli/src/session.rs:360 (/copy); ADR 0021 rows 7, 8.8 |
| PI-7.4 | Other commands (/trust, /reload, /hotkeys, /changelog, /quit, /debug) | UNCHANGED | partial | already-built | ca/docs/slash-commands.md; ca/docs/usage.md | trellis-cli/src/trust_cmd.rs; trellis-cli/src/session.rs |

### 8. Sessions, branching, compaction

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-8.1 | JSONL tree storage | CHANGED: file created at first user message (0.99.0) | missing | port | ca/docs/session-format.md; ca/src/core/session-manager.ts | ADR 0021 row 8.1-8.4 |
| PI-8.2 | Entry types | UNCHANGED | partial | already-built | ca/docs/session-format.md | trellis-trail/src/lib.rs; ADR 0007; ADR 0034 |
| PI-8.3 | `/tree` and branch summaries | UNCHANGED | missing | port | ca/docs/sessions.md; ca/docs/compaction.md | ADR 0021 row 8.1-8.4 |
| PI-8.4 | Session flags | CHANGED (gap: `-n/--name`, changelog 0.78.0) | partial | port | ca/docs/cli.md | ADR 0038 (`trellis resume`); trellis-cli/src/main.rs |
| PI-8.5 | Auto-compaction | CHANGED: compaction.modelOverrides (gap, 0.86.0); routed-model limits (0.99.0) | built | already-built | ca/docs/compaction.md; ca/docs/settings.md | trellis-loop/src/adapter.rs:116-141; ADR 0024 §5; ADR 0033 |
| PI-8.6 | Structured summary format | UNCHANGED | missing | owner-decision | ca/docs/compaction.md | ADR 0024 §5 (elision, not summary); ADR 0024 Not built |
| PI-8.7 | Extension-supplied summaries | UNCHANGED | missing | owner-decision | ca/docs/compaction.md | ADR 0024 §5 |
| PI-8.8 | Export and share | CHANGED: HTML export shows nested calls and hidden messages (0.99.0) | built | already-built | ca/docs/sessions.md; ca/src/core/export-html/ | trellis-cli/src/log_cmd.rs; ADR 0021 row 8.8 (#143) |
| PI-8.9 | Session picker (search, rename, delete) | NEW (gap) | missing | desktop-ui | ca/docs/sessions.md; ca/src/cli/session-picker.ts | `trellis log` lists runs |

### 9. Run modes

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-9.1 | Interactive and print modes | CHANGED (gap): `--mode text`, `--export` (changelog 0.52.0) | built | already-built | ca/docs/cli.md | trellis-cli/src/main.rs (run, tui, session) |
| PI-9.2 | JSON mode | UNCHANGED | built | already-built | ca/docs/json.md | trellis-cli/src/output.rs; ADR 0025 §4 |
| PI-9.3 | Event types | CHANGED (gap): session_info_changed, thinking_level_changed, summarization_retry_* (0.81.1) | partial | already-built | ca/docs/json.md | ADR 0025 §4 |
| PI-9.4 | RPC commands (33) | CHANGED: disposition on prompt, steer and follow_up (0.99.0) | partial | port | ca/docs/rpc-commands.md; ca/src/modes/rpc/ | trellis-cli/src/rpc.rs:557-715 (12 commands) |
| PI-9.5 | RPC extension-UI subprotocol | UNCHANGED | partial | already-built | ca/docs/rpc-extension-ui.md | trellis-cli/src/rpc.rs:671; ADR 0018 |
| PI-9.6 | SDK | CHANGED: AgentSessionRuntime, codemode/MCP factories, builtin inline extensions | built | already-built | ca/docs/sdk.md; ca/examples/sdk/14-codemode-mcp.ts | ADR 0021 row 9.6 |
| PI-9.7 | Fork-and-rebrand (`piConfig`) | NEW (gap; changelog 0.12.5) | missing | owner-decision | ca/docs/cli-integration.md | none |

### 10. Packages

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-10.1 | Package commands | CHANGED: `pi config` Built-in section; peer-dependency handling (0.99.0) | partial | already-built | ca/docs/packages.md; ca/docs/cli.md; ca/src/core/package-manager.ts | trellis-cli/src/install.rs; ADR 0039 |
| PI-10.2 | Package manifest and filters | CHANGED: gallery keyword, pi.image/pi.video, `autoload: false` delta | partial | already-built | ca/docs/packages.md; ca/src/core/pi-manifest.ts | trellis-cli/src/install.rs:11-14 |
| PI-10.3 | Self-update and install channels (installer, `pi update`, Nix flake) | NEW (Nix upstream 1.0.1; self-update a gap) | missing | owner-decision | ca/docs/quickstart.md; flake.nix; ca/src/utils/version-check.ts | CLAUDE.md 'Dependency pinning'; brief Part 1 |

### 11. TUI (cosmetic unless noted)

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-11.1 | Fullscreen mode | CHANGED: default since 1.0.0; scrollbar, copy-on-select, wheel lines | excluded | desktop-ui | ca/docs/settings.md; ca/docs/usage.md | ADR 0021 itemised list |
| PI-11.2 | `@` file search | UNCHANGED | built | already-built | ca/docs/usage.md | trellis-cli/src/tui_run.rs:364 (#52) |
| PI-11.3 | External editor | UNCHANGED | excluded | desktop-ui | ca/docs/settings.md | ADR 0021 itemised list |
| PI-11.4 | Themes | CHANGED: system theme default; OKLCH/OKHSL (0.99.0) | excluded | desktop-ui | ca/docs/themes.md | ADR 0021 itemised list |
| PI-11.5 | Keybindings | UNCHANGED | excluded | desktop-ui | ca/docs/keybindings.md | ADR 0021 itemised list |
| PI-11.6 | Mermaid diagrams | UNCHANGED | excluded | desktop-ui | ca/docs/settings.md | ADR 0021 itemised list |
| PI-11.7 | Inline images | CHANGED: JPEG/GIF/WebP transcoding for Kitty (1.0.1) | excluded | desktop-ui | packages/tui/CHANGELOG.md | ADR 0021 itemised list |
| PI-11.8 | Games | UNCHANGED | excluded | desktop-ui | ca/examples/extensions/README.md | ADR 0021 itemised list |
| PI-11.9 | Startup header, quietStartup, `--verbose` | NEW (gap 0.48.0; 'header' value 1.0.0) | excluded | desktop-ui | ca/docs/settings.md; ca/src/cli/startup-ui.ts | ADR 0021 row 11 |
| PI-11.10 | Status footer (context, usage, cost, routed model) | NEW (gap; routed model 0.99.0) | partial | desktop-ui | ca/docs/usage.md; ca/src/core/footer-data-provider.ts | tui-demo/src/tui/app.rs (model, tier, budget) |
| PI-11.11 | Terminal integration and platform guides | NEW (gap) | excluded | desktop-ui | ca/docs/terminal-setup.md; ca/docs/tmux.md; ca/docs/termux.md; ca/docs/windows.md | ADR 0021 row 11 |
| PI-11.12 | pi-tui library | NEW (gap) | excluded | desktop-ui | packages/tui/README.md | ADR 0021 row 11; tui-demo |

### 12. Settings

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-12.1 | Settings locations and layering | CHANGED: mcp.json in both locations; user-only settings | built | already-built | ca/docs/configuration.md; ca/docs/settings.md | trellis-cli/src/user_config.rs; ADR 0025 §7 |
| PI-12.2 | Environment variables | CHANGED (gaps): PI_PACKAGE_DIR, PI_SKIP_VERSION_CHECK, PI_TELEMETRY and nine more | partial | already-built | ca/docs/environment-variables.md | trellis-cli/src/user_config.rs:36-47 (TRELLIS_CONFIG_DIR) |
| PI-12.3 | Offline mode (`--offline`, PI_OFFLINE) | NEW (gap; changelog 0.55.1) | built | already-built | ca/docs/cli.md | ADR 0024 §1 (`--no-default-features`) |

### 13. Security

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-13.1 | No sandbox, no per-call approval | CHANGED: MCP calls pass permission gates | built | already-built | ca/docs/security.md; ca/docs/mcp.md | ADR 0018; ADR 0023 §3; ADR 0032 |
| PI-13.2 | Project trust | CHANGED: `.pi/mcp.json` trust-gated (0.99.0) | built | already-built | ca/docs/security.md; ca/src/core/project-trust.ts | trellis-cli/src/trust_cmd.rs; trellis-loop/src/trust.rs; ADR 0023 §4 |
| PI-13.3 | Isolation (Docker, Docker Sandboxes, OpenShell, Gondolin) | UNCHANGED | partial | port | ca/docs/containerization.md | trellis-loop/src/isolate.rs, agent_clone.rs; ADR 0032 |
| PI-13.4 | Guard examples | UNCHANGED | built | already-built | ca/examples/extensions/README.md | ADR 0021 row 13.4 |
| PI-13.5 | Tool annotations as permission hints | NEW (upstream 0.99.0) | missing | port | ca/docs/extensions.md; ca/docs/mcp.md | trellis-cli/src/config.rs:321; trellis-loop/src/agent.rs:62-105 |

### 14. Telemetry and durability

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-14.1 | Telemetry | CHANGED: settings.md gives enableInstallTelemetry default true (old inventory: opt-in) | built | already-built | ca/docs/settings.md; ca/src/core/telemetry.ts; packages/telemetry/README.md | CLAUDE.md Style (tracing is the one path) |
| PI-14.2 | Agent-core hooks | CHANGED: experimental harness removed from agent-core (1.0.0); onProviderStreamEvent (0.99.0) | built | already-built | packages/agent/README.md; packages/agent/CHANGELOG.md | trellis-loop/src/orchestrator.rs; trellis-loop/src/gate.rs |
| PI-14.3 | pi-durable | CHANGED: rebuilt as a durable harness with subagents and a task graph (1.0.0) | partial | owner-decision | packages/durable/README.md; packages/durable/CHANGELOG.md | trellis-trail; trellis-queue; ADR 0038; ADR 0009 |
| PI-14.4 | Usage and cost accounting | NEW (gap; per-physical-model cost 0.99.0) | partial | port | ca/docs/usage.md; ca/src/core/usage-totals.ts | trellis-queue/src/ledger.rs; ADR 0026 §5; #67 |

### 15. Experimental

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-15.1 | chord | CHANGED: application composition runtime (facets, services, replicated state); example plugin | excluded | owner-decision | packages/chord/README.md; ca/examples/plugins/pi-example-plugin | ADR 0021 row 15 |
| PI-15.2 | pi-protocol | CHANGED: protocol v8 routed envelopes, length-prefixed CBOR | excluded | owner-decision | packages/protocol/README.md | ADR 0021 row 15 |
| PI-15.3 | pi-server, `pi server` / `pi client` | CHANGED: session workers on pi-durable, PI_EXPERIMENTAL=1 | excluded | owner-decision | packages/server/README.md; ca/src/experimental/services/README.md | ADR 0021 row 15 |
| PI-15.4 | Docs evals | UNCHANGED | partial | already-built | packages/evals/README.md | docs/BENCHMARK_AND_EVAL.md; trellis-cli/src/ask.rs:74 |
| PI-15.5 | pi-client | NEW (not in the old inventory; no changelog entries since 0.87.1) | excluded | owner-decision | packages/client/README.md | ADR 0021 row 15 |
| PI-15.6 | Durable coding agent with subagents and task graph | NEW (upstream, durable 1.0.0) | missing | owner-decision | ca/src/experimental/durable/README.md; ca/src/experimental/vacation/README.md | ADR 0026 §11; ADR 0037; CLAUDE.md rule 5 |

### 16. MCP client (new section)

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-16.1 | MCP servers (stdio, streamable HTTP) from mcp.json | NEW (upstream 0.99.0; project overrides 1.0.1) | missing | owner-decision | ca/docs/mcp.md; ca/src/extensions/mcp/config.ts, runtime.ts | brief v2.0; ADR 0021 Paperclip 72-75; brief Part 1 |
| PI-16.2 | MCP OAuth | NEW (upstream 0.99.0-1.0.1) | excluded | owner-decision | ca/docs/mcp.md; ca/src/extensions/mcp/oauth.ts | ADR 0010; ADR 0021 row 2.1-2.4 |
| PI-16.3 | MCP tool exposure | NEW (upstream 0.99.0/0.99.2) | missing | owner-decision | ca/docs/mcp.md | trellis-loop/src/registry.rs:1-11 |
| PI-16.4 | MCP resource tools | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/mcp.md; ca/src/extensions/mcp/resources.ts | none |
| PI-16.5 | `/mcp` and `pi mcp` management | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/mcp.md; ca/docs/cli.md; ca/src/extensions/mcp/cli.ts, ui.ts | brief 4.12 (CLI surface) |
| PI-16.6 | MCP from extensions and SDK | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/extensions.md; ca/src/core/extensions/types.ts:1578, 1853-1859 | ADR 0023 §5-6 |
| PI-16.7 | pi-mcp standalone client library | NEW (package 0.99.0) | missing | owner-decision | packages/mcp/README.md; packages/mcp/CHANGELOG.md | none |

### 17. Codemode, tool search, tool exposure (new section)

| ID | Capability | vs old inventory | TRELLIS | Disposition | Upstream | TRELLIS evidence |
|---|---|---|---|---|---|---|
| PI-17.1 | `codemode` tool (QuickJS sandbox calling tools) | NEW (upstream 0.99.0; leaner 1.0.0) | excluded | owner-decision | ca/docs/codemode.md; ca/src/extensions/codemode/; packages/codemode/README.md | ADR 0021 row 1.7 (ADR 0005; #63) |
| PI-17.2 | Codemode store/load on the session branch | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/codemode.md | ADR 0021 row 8.1-8.4 |
| PI-17.3 | Codemode `models` API | NEW (upstream 0.99.0/1.0.0) | excluded | owner-decision | ca/docs/codemode.md | ADR 0021 rows 2.14, 1.7 |
| PI-17.4 | `tool_search` | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/cli.md; ca/src/extensions/tool-search/ | trellis-loop/src/registry.rs:1-11 |
| PI-17.5 | Tool exposure modes and namespaces | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/extensions.md | trellis-loop/src/registry.rs:1-11 |
| PI-17.6 | `prepareLoadout()` | NEW (upstream 0.99.0) | missing | owner-decision | ca/docs/extensions.md; ca/src/core/extensions/types.ts:579 | trellis-loop/src/registry.rs:1-11 |

## Open questions

1. **ADR text behind the code.** ADR 0021 rows 1.2, 2.13 and 7, its "Owner decisions" section, and ADR 0034's status line still say "landing (#113, #150, #109)". The code has the steering loop (trellis-loop/src/agent.rs), RPC steer and follow_up (rpc.rs:557), TUI steering (tui_run.rs:251 and tests at 1005–1163), `HTTPS_PROXY`/`NO_PROXY` (trellis-model/src/transport.rs), and `/name` and `/copy` (session.rs:349–360). Should those rows be amended to built? PI-1.2 would stay partial, because the queue modes and `clear_queue` are absent.
2. **MCP client.** The exclusions on record name MCP *exposure*: brief v2.0, and ADR 0021 Paperclip 72–75 ("connectors, tool gateway, MCP"). pi's MCP is a client that consumes servers, which no clause addresses. A stdio server resembles a custom tool (ADR 0023 §6), but runs argv the repository did not place under `.trellis/`. An HTTP server meets brief Part 1 ("Any networked component"). Does the owner want an MCP client, and if so, only for stdio servers?
3. **Codemode.** The owner kept parallel tool calls excluded on 2026-09-28 (#63). Codemode is a sandboxed script that issues many tool calls inside one model turn. Does row 1.7's clause (the gate log keyed by turn, and the trail, rollback points and S5 serialization assuming one call per turn) settle codemode too, or is it a separate question?
4. **Dynamic tool sets.** `tool_search`, the deferred and codemode exposures, and `prepareLoadout` change the declared tools mid-run. trellis-loop/src/registry.rs:1–11 fixes the tool set at run start, and the orchestrator refuses calls outside it. Keep that invariant?
5. **Virtual models versus ADR 0043.** ADR 0043 escalates after a failing `Verdict` ends a local run. pi routes every request through extension code. Is per-request routing (for example, a cheaper model after the first edit) in scope?
6. **Identity token file (PI-2.15).** ADR 0021 row 2.1–2.4 says "a key never goes in a file trellis reads". The #45 amendment allows "the platform's own ambient chain". Workload identity federation reads a token file that the platform maintains. Which phrasing governs?
7. **Telemetry default.** The old inventory says install telemetry is opt-in. settings.md at head lists `enableInstallTelemetry` with default `true` and `enableAnalytics` with default `false`. I did not check whether the default changed or the old entry was wrong.
8. **Classifier models on the local runtime.** Row 2.14 excluded classification as "not a coding-loop concern". pi now scores classifier questions from label log-probabilities on llama.cpp, the runtime TRELLIS already drives, while `ask.rs` spends a full chat call to classify input. Revisit?
9. **Durable subagents (PI-14.3, PI-15.6).** pi-durable resumes interrupted tool calls, where TRELLIS rolls back (ADR 0009). It also adds task-owned subagent conversations. CLAUDE.md rule 5 forbids an agent judging another agent's output, not delegation. Should a subagent tool be weighed against rule 5 and ADR 0005?
10. **Id scheme.** Confirm the sub-ids assigned to the previously unnumbered sections (4, 6, 7, 10, 11, 12, 14, 15), and the split of PI-2.17 and PI-2.18 out of 2.14.
11. **Dating method.** Gap dates come from earliest changelog mentions in a depth-1 clone. A full clone would let the "since 0.87.1" calls be checked against `git log -S`.
