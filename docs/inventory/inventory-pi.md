# pi 1.0.2 capability inventory

Upstream is `earendil-works/pi` at commit `200387122ca450d6387f033949423114a270b96c` (2026-10-04, "Add [Unreleased] section for next cycle"), a monorepo of 13 packages, all at 1.0.2: agent, ai, chord, client, codemode, coding-agent, durable, evals, mcp, protocol, server, telemetry, tui. The root `LICENSE` is MIT. This file replaces the pi-coding-agent 0.87.1 inventory, which read the published npm package; that inventory is `git show a8a4b63:docs/inventory/inventory-pi.md`. The re-inventory of 2026-10-04 with TRELLIS status and dispositions is `docs/inventory/inventory-pi-20261004.md`; its tags for 4.12, 4.20 and 8.4, and its totals, predate this file and differ from it. This file records no dispositions; those are ADR 0021's.

136 capabilities: 54 unchanged, 47 changed, 35 new.

Of the 35 new, 23 arrived upstream after 0.87.1, and 12 (tagged `NEW (gap)`) existed before it but were missing from the 0.87.1 inventory.

**How to read this file.**

- **Ids.** Sections 1-3, 5, 8, 9 and 13 keep their 0.87.1 numbers and append new items at the end. Sections 4, 6, 7, 10, 11, 12, 14 and 15 were unnumbered in 0.87.1 and are numbered here in the order that inventory listed them. Sections 16 and 17 are new. 2.17 and 2.18 are split out of 2.14.
- **Tags.** `UNCHANGED`; `NEW` (upstream after 0.87.1); `NEW (gap)` (upstream before 0.87.1, missing from that inventory); `CHANGED: <what changed> (<version>)`. A CHANGED tag rests on a CHANGELOG entry after 0.87.1, cited with its line. Two exceptions, 15.1 and 15.2, say so on their line.
- **Paths.** `ca/` is `packages/coding-agent/`; other paths are relative to the repository root. `ca/CHANGELOG.md:<n>` is a line of the coding-agent CHANGELOG, and `packages/<p>/CHANGELOG.md:<n>` a line of another package's. Every cited line was read at the commit above.
- **What was read.** The CHANGELOG sections from 1.0.2 back to 0.87.1 of the 11 packages that have one (all but chord and evals): coding-agent lines 5-242, and the sections above `## [0.87.1]` in the other ten. The first lines of the READMEs of agent (and lines 54-134), ai (and lines 47, 541), chord, client, codemode, durable, evals, mcp, protocol, server, telemetry and tui, and of `ca/src/experimental/durable/README.md` and `ca/src/experimental/vacation/README.md`. In `ca/docs/`, the lines cited below and the sections around them: usage 7-43, security 5-20, extensions 132-208 and 242-257, mcp 1-34, 100, 189-195, 248-253, sessions 10-30, cli 166-175, 225-236, 281-320, cli-integration 1-12 and 78-96, compaction 27-36, packages 76-95, and json 54-124. `ca/src/core/extensions/types.ts` at the cited lines only.
- **Not read.** The coding-agent README. Any doc beyond the lines and ranges above. Source bodies beyond the cited lines, tests, and examples beyond the cited lines. `packages/durable/docs/spec.md` and `packages/chord/PLANNING.md`. Where a description below goes beyond its cited line, it repeats the 0.87.1 inventory and was not re-checked at 1.0.2.

## 1. Agent loop and tools
- **1.1 Turn loop.** UNCHANGED. A prompt goes to the provider stream, tool calls produce results, and another turn follows until nothing is pending (ca/docs/how-pi-works.md:3)
- **1.2 Steering and follow-up queues.** CHANGED: RPC `prompt`, `steer` and `follow_up` responses, and `AgentSession.steer()`/`followUp()`, report a per-input disposition (0.99.0). `Enter` steers after the current response, `Alt+Enter` follows up after the task, and aborting returns queued messages to the editor; `steeringMode` is `all` or `one-at-a-time` (ca/docs/usage.md:29-40; ca/docs/settings.md:29; ca/docs/rpc-commands.md:40; ca/CHANGELOG.md:185)
- **1.3 Built-in tools.** UNCHANGED. `read`, `bash`, `edit` and `write` by default; `grep`, `find` and `ls` optional; `powershell` on native Windows (ca/docs/cli.md:137)
- **1.4 Tool selection flags and defaultTools.** CHANGED: `defaultTools` takes `+name` and `-name` entries (0.99.0), and `/reload` enables tools newly added to it (0.99.2). `--tools`, `--exclude-tools`, `--no-builtin-tools` and `--no-tools` override it (ca/docs/cli.md:121; ca/docs/settings.md:40, :56; ca/CHANGELOG.md:193, :120)
- **1.5 Output truncation.** CHANGED: `bash` and `powershell` structured results for codemode scripts hold up to 1 MiB, with `truncated` and `full_output_path` (0.99.0). The model-facing limit stays 50KB or 2000 lines (ca/src/core/tools/truncate.ts:6; ca/CHANGELOG.md:208)
- **1.6 Swappable tool backends.** UNCHANGED. `BashOperations` and the other `*Operations` interfaces let tools run remotely, as the ssh example does (ca/examples/extensions/ssh.ts:19)
- **1.7 Parallel tool calls.** CHANGED: codemode scripts call tools in parallel (0.99.0). A tool can still set `executionMode`, and `withFileMutationQueue()` serializes read-modify-write (ca/src/core/extensions/types.ts:624; ca/docs/extensions.md:143; ca/CHANGELOG.md:167)
- **1.8 terminate flag.** UNCHANGED. If every completed tool in a batch returns `terminate: true`, the automatic follow-up request is skipped (ca/docs/extensions.md:140)
- **1.9 Retries and backoff.** CHANGED: "model at capacity" errors are retried (1.0.1), and an unparseable `Retry-After` falls back to exponential backoff (0.99.2). Agent-level retry is `retry.enabled` (ca/docs/settings.md:125; ca/CHANGELOG.md:47, :139)
- **1.10 Overflow recovery.** UNCHANGED. After tools finish, the projected context is checked before the next request, and a length error gets one compact-and-retry (ca/docs/compaction.md:37, :85)
- **1.11 User shell commands.** UNCHANGED. `!cmd` runs a command and sends its output to the model; `!!cmd` runs it without sending (ca/docs/usage.md:76)
- **1.12 Session environment for model shells.** UNCHANGED. Shells the model runs get `PI_SESSION_ID` and the other session variables (ca/docs/environment-variables.md:26)
- **1.13 Shell configuration.** UNCHANGED. `shellPath` picks the shell and `shellCommandPrefix` runs setup before each command (ca/docs/shell-aliases.md:5; ca/docs/settings.md:139)
- **1.14 Images.** UNCHANGED. Images attach to prompts; `images.blockImages` keeps them from models (ca/docs/settings.md:111)
- **1.15 Prompt-cache warming.** UNCHANGED. `cacheWarming` is `off`, `streaming` (default) or `idle` (ca/docs/settings.md:19)
- **1.16 Process markers.** NEW (gap). Child processes see `AI_AGENT=pi` and `PI_CODING_AGENT=true` (ca/docs/environment-variables.md:15-16)

## 2. Providers, models, auth
- **2.1 Built-in providers.** CHANGED: GPT-6.1 Sol added and made the OpenAI Codex default (0.99.1); the TypeSafe provider added and OpenAI Codex renamed "OpenAI Codex (legacy)" (0.99.0). More than 30 API-key providers, each with its own environment variable (ca/docs/providers.md:20-25; ca/CHANGELOG.md:153, :187, :203)
- **2.2 Login.** CHANGED: Sign in with ChatGPT (0.99.0); Anthropic copy-code login and Radius in `/login` (1.0.0). `/login` and `/logout` store credentials in `auth.json` (ca/docs/providers.md:8-10; ca/CHANGELOG.md:179, :76, :81)
- **2.3 Credential values.** UNCHANGED. A key can be a `!command` whose output is the key, so a secret manager is used without writing it to disk (ca/docs/providers.md:71-73)
- **2.4 Cloud platforms.** CHANGED: Bedrock pricing tiers and stale thinking signatures fixed (1.0.1). Azure OpenAI, Amazon Bedrock, Cloudflare and Google Vertex AI (ca/docs/providers.md:116, :129, :153, :178; ca/CHANGELOG.md:50-51)
- **2.5 Credential precedence.** UNCHANGED. A runtime `--api-key`, then `auth.json`, then `models.json`, then the environment (ca/docs/models.md:23)
- **2.6 models.json endpoints and overrides.** CHANGED: `models.json` headers also authenticate image generation (0.99.0). It adds compatible endpoints and holds `modelOverrides` (ca/docs/models.md:45, :66; ca/CHANGELOG.md:186)
- **2.7 Model catalog overlay.** CHANGED: catalog refreshes request chat, image and classifier models (0.99.0). A bundled catalog overlaid by the pi.dev catalog (ca/docs/models.md:15; ca/CHANGELOG.md:188)
- **2.8 Thinking levels.** CHANGED: the agent loop records the requested level on each assistant message (agent 0.99.0). `off` through `max`, with `thinkingBudgets` (ca/docs/settings.md:13, :15; packages/agent/CHANGELOG.md:24)
- **2.9 Scoped model cycling.** UNCHANGED. `--models <patterns>` limits the models that cycling moves through (ca/docs/cli.md:71)
- **2.10 llama.cpp router.** CHANGED: every llama.cpp chat model is also a classifier model (0.99.0). Pi drives the llama.cpp router server, which loads and unloads GGUF models (ca/docs/llama-cpp.md:3, :89-92; ca/CHANGELOG.md:182)
- **2.11 Radius gateway.** CHANGED: Radius sign-in in `/login`, with its MCP server set up in one step (1.0.0) (ca/docs/providers.md:104; ca/CHANGELOG.md:66, :81)
- **2.12 Custom providers.** CHANGED: one registration can add image and classifier models beside chat models (pi-ai 0.99.0). Extensions call `pi.registerProvider()` (ca/docs/custom-provider.md:21, :39-60; packages/ai/CHANGELOG.md:74)
- **2.13 Transports and proxy.** UNCHANGED. `transport` is `auto`, `sse`, `websocket` or `websocket-cached` (ca/docs/settings.md:121)
- **2.14 pi-ai library extras.** CHANGED: image models joined the regular `Provider`/`Models` surface, and classification became `Models.classify()` (pi-ai 0.99.0); both move to 2.17 and 2.18. What stays here: constrained sampling for tools and a faux provider for tests (packages/ai/README.md:47, :541; packages/ai/CHANGELOG.md:68, :78)
- **2.15 Anthropic workload identity federation.** NEW. With no key set, Anthropic authenticates from `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID` and `ANTHROPIC_IDENTITY_TOKEN_FILE` (ca/docs/providers.md:69; ca/CHANGELOG.md:119)
- **2.16 Sampling parameters by thinking level.** NEW. `samplingParamsByThinkingLevel` in `models.json` sets `temperature`, `top_p` and similar per thinking level on OpenAI-compatible APIs (ca/docs/models.md:101; ca/CHANGELOG.md:13)
- **2.17 Classifier models.** CHANGED: split out of 2.14; Jev classifiers are usable from codemode scripts and llama.cpp (0.99.0), plus Cloudflare Clef (1.0.1). Classifier models answer typed choice, yes-or-no and score questions with probabilities (ca/docs/models.md:129-138; ca/CHANGELOG.md:171, :30)
- **2.18 Image generation models.** CHANGED: split out of 2.14; `ModelRuntime.generateImages()` (0.99.0) and codemode's `models.generateImages()` (1.0.0) (ca/docs/models.md:166; ca/docs/codemode.md:165; ca/CHANGELOG.md:186, :75)
- **2.19 Virtual models.** NEW. Experimental: an extension registers one with `pi.registerVirtualModel()` and routes each request to a physical model and thinking level (ca/docs/virtual-models.md:37; ca/CHANGELOG.md:178)
- **2.20 Credential commands.** NEW (gap). `pi auth check`, `pi auth print-api-key` and `pi auth print-bearer-token` (ca/docs/cli.md:281-298)
- **2.21 List models.** NEW (gap). `--list-models [search]` (ca/docs/cli.md:73)

## 3. Context files and system prompt
- **3.1 Context files.** UNCHANGED. `AGENTS.override.md`, `AGENTS.md` and `CLAUDE.md` from the agent directory, the working directory and its ancestors (ca/docs/configuration.md:18)
- **3.2 SYSTEM.md and APPEND_SYSTEM.md.** UNCHANGED. One replaces the system prompt and the other appends to it (ca/docs/configuration.md:20)
- **3.3 System prompt flags.** UNCHANGED. `--system-prompt` and `--append-system-prompt` (ca/docs/cli.md:212)
- **3.4 Sectioned system prompt.** CHANGED: MCP servers appear in a short system prompt section (0.99.2). Sections and the tool loadout are persisted as system messages that patch sections by name (ca/docs/session-format.md:80; ca/CHANGELOG.md:109)

## 4. Extensions API
- **4.1 Extension loading.** CHANGED: mcp, llama.cpp, codemode and tool-search ship as built-in extensions, disabled with `-builtin:<name>` (0.99.0), with a warning when an extension replaces one. TypeScript loads through `jiti` (ca/docs/extensions.md:38; ca/docs/settings.md:160; ca/CHANGELOG.md:175, :177, :192)
- **4.2 Trust and resource events.** UNCHANGED. `project_trust` and `resources_discover` (ca/src/core/extensions/types.ts:678)
- **4.3 Session events.** UNCHANGED. `session_start` and the switch, fork, compact, tree and shutdown events (ca/src/core/extensions/types.ts:733)
- **4.4 Context and provider events.** UNCHANGED. `context` and the provider request and response events (ca/src/core/extensions/types.ts:863)
- **4.5 Agent run events.** UNCHANGED. `before_agent_start` and the agent start, end and settle events (ca/src/core/extensions/types.ts:911)
- **4.6 UI prompt events.** UNCHANGED. `ui_prompt_start` and `ui_prompt_end` (ca/src/core/extensions/types.ts:1004)
- **4.7 Turn and message events.** UNCHANGED. `turn_start`, `turn_end` and the message events (ca/src/core/extensions/types.ts:1020)
- **4.8 Tool events.** CHANGED: nested tool calls emit events with `parentToolCallId` (0.99.0). `tool_call` can change the input or block the call; `tool_result` results compose (ca/src/core/extensions/types.ts:1153; ca/CHANGELOG.md:176)
- **4.9 Selection and input events.** UNCHANGED. `model_select`, `thinking_level_select`, `user_bash` and `input` (ca/src/core/extensions/types.ts:1094)
- **4.10 Registration methods.** UNCHANGED. `registerTool` and the command, shortcut, flag, renderer and provider registrations (ca/src/core/extensions/types.ts:563)
- **4.11 Messages and entries.** UNCHANGED. `sendMessage`, `sendUserMessage` and `appendEntry` (ca/src/core/extensions/types.ts:443)
- **4.12 Session metadata.** UNCHANGED. `setSessionName`, `getSessionName` and `setLabel`; the 0.87.1 inventory missed `getSessionName` (ca/src/core/extensions/types.ts:1713, :1716, :1719)
- **4.13 Tools and commands methods.** CHANGED: `getAllTools()` reports each tool's annotations (0.99.0). `exec`, active tools, `getAllTools` and `getCommands` (ca/src/core/extensions/types.ts:1728; ca/docs/extensions.md:170; ca/CHANGELOG.md:176)
- **4.14 Model methods.** UNCHANGED. `setModel` and the thinking-level getters and setters (ca/src/core/extensions/types.ts:1750)
- **4.15 Event bus.** UNCHANGED. `pi.events` lets extensions talk to each other (ca/docs/extensions.md:86)
- **4.16 Extension context.** CHANGED: `ctx.executeTool()` runs other tools (0.99.0). `ctx` carries `hasUI`, `cwd`, the session manager, the model registry and the abort signal (ca/src/core/extensions/types.ts:325, :367; ca/CHANGELOG.md:176)
- **4.17 Command-only context.** UNCHANGED. `newSession`, `fork`, `navigateTree`, `switchSession` and `reload` (ca/src/core/extensions/types.ts:422)
- **4.18 Extension UI context.** CHANGED: extensions get `theme.style()`, `theme.colors` and `theme.appearance` (0.99.0). `ctx.ui` gives dialogs, notifications, status, widgets and custom components (ca/src/core/extensions/types.ts:149; ca/docs/extensions.md:244; ca/CHANGELOG.md:181)
- **4.19 Tool definition fields.** CHANGED: `exposure`, `namespace`, `annotations`, `outputSchema` and `prepareLoadout()` (0.99.0) (ca/src/core/extensions/types.ts:565; ca/CHANGELOG.md:176)
- **4.20 Mode behaviour.** UNCHANGED. Extensions load in every mode; RPC forwards dialogs, JSON and print have no UI, and `ctx.mode` guards terminal-only behaviour (ca/docs/extensions.md:247-250)
- **4.21 Provider stream event.** NEW. `provider_stream_event` observes parsed provider events before normalization (ca/docs/extensions.md:107; ca/CHANGELOG.md:189)
- **4.22 Tool renderer resolver.** NEW. `pi.registerToolRenderer()` chooses how calls to any tool are drawn, including tools not registered yet (ca/docs/extensions.md:190; ca/CHANGELOG.md:31)
- **4.23 Nested tool calls.** NEW. `ctx.executeTool(name, args)` runs another tool through validation and the `tool_call` handlers (ca/docs/extensions.md:148; ca/CHANGELOG.md:176)
- **4.24 Structured tool results.** NEW. `outputSchema` with matching `structuredContent`, and `isError` results (ca/docs/extensions.md:146; ca/CHANGELOG.md:176)

## 5. Skills
- **5.1 Agent Skills format.** UNCHANGED. The Agent Skills specification, with `disable-model-invocation` (ca/docs/skills.md:7, :53)
- **5.2 Progressive disclosure.** UNCHANGED. Only name and description are advertised; the model reads `SKILL.md` when a task matches (ca/docs/skills.md:3, :45)
- **5.3 Forced skill command.** UNCHANGED. `/skill:name` runs a skill explicitly, under `enableSkillCommands` (ca/docs/skills.md:53)
- **5.4 Skill locations.** UNCHANGED. Pi's own skill directories plus `~/.agents/skills/` and `.agents/skills/` in ancestors (ca/docs/skills.md:61)

## 6. Prompt templates
- **6.1 Prompt templates.** CHANGED: pi-agent-core removed its harness copy of prompt templates, leaving the coding agent's (agent 1.0.0). A Markdown file becomes a `/name` command with `$1`, `$@` and `$ARGUMENTS` substitution (ca/docs/prompt-templates.md:43; packages/agent/CHANGELOG.md:13)

## 7. Slash commands
- **7.1 Model and settings commands.** UNCHANGED. `/settings`, `/model`, `/thinking`, `/scoped-models`, `/login`, `/logout` (ca/docs/slash-commands.md:14)
- **7.2 Session commands.** UNCHANGED. `/new`, `/resume`, `/name`, `/session`, `/tree`, `/fork`, `/clone`, `/compact` (ca/docs/slash-commands.md:27)
- **7.3 Sharing and export commands.** UNCHANGED. `/copy`, `/export`, `/share`, `/bug` (ca/docs/slash-commands.md:39)
- **7.4 Other commands.** UNCHANGED. `/trust`, `/reload`, `/hotkeys`, `/changelog`, `/quit`, `/debug` (ca/docs/slash-commands.md:50)

## 8. Sessions, branching, compaction
- **8.1 Session tree storage.** CHANGED: the session file is created when the first user message is sent (0.99.0). JSONL entries form a tree through `id` and `parentId` (ca/docs/session-format.md:3; ca/CHANGELOG.md:217)
- **8.2 Entry types.** UNCHANGED. Messages, model and thinking changes, compaction, branch summaries, custom entries, labels and session info (ca/docs/session-format.md:154)
- **8.3 Tree navigation and branch summaries.** UNCHANGED. `/tree` moves within one session file, and a branch you leave is summarized (ca/docs/sessions.md:26)
- **8.4 Session flags.** UNCHANGED. `--continue`, `--resume`, `--session-dir`, `-n`/`--name` and the rest; the 0.87.1 inventory missed `--name` (ca/docs/cli.md:78-100)
- **8.5 Auto-compaction.** CHANGED: pi-agent-core removed its harness copy of compaction, leaving the coding agent's (agent 1.0.0). It triggers when context exceeds the window minus `reserveTokens` (16384) (ca/docs/compaction.md:27-36; packages/agent/CHANGELOG.md:13)
- **8.6 Structured summary format.** UNCHANGED. Goal, constraints, progress, decisions, next steps and critical context (ca/docs/compaction.md:236)
- **8.7 Extension-supplied summaries.** UNCHANGED. `session_before_compact` lets an extension supply the summary (ca/docs/compaction.md:94)
- **8.8 Export and share.** CHANGED: HTML exports can show custom messages marked hidden (0.99.0). `/export` writes HTML or JSONL; `/share` uploads (ca/docs/sessions.md:58; ca/CHANGELOG.md:190)
- **8.9 Session picker.** NEW (gap). Search, rename and delete sessions; show paths, sort, and filter to named sessions (ca/docs/sessions.md:18)

## 9. Run modes
- **9.1 Interactive and print modes.** CHANGED: `--tui-mode regular` keeps normal scrollback now that fullscreen is the default, and `--provider` without `--model` fails (1.0.0). Print mode is used when input or output is piped (ca/docs/cli-integration.md:3; ca/docs/cli.md:25; ca/CHANGELOG.md:80, :94)
- **9.2 JSON mode.** UNCHANGED. JSONL events, ending with `agent_settled` (ca/docs/json.md:45, :56)
- **9.3 Event types.** CHANGED: nested tool calls emit events with `parentToolCallId` (0.99.0); the json.md table at :105-107 does not list that field. Agent, turn, message, tool, queue, entry, session-info and thinking-level events (ca/docs/json.md:54-124; ca/CHANGELOG.md:176)
- **9.4 RPC commands.** CHANGED: `prompt`, `steer` and `follow_up` responses carry `disposition` (0.99.0). JSONL commands over stdin and stdout, including `clear_queue` (ca/docs/rpc-commands.md:37-40, :107; ca/CHANGELOG.md:185)
- **9.5 RPC extension-UI subprotocol.** UNCHANGED. Dialog methods emit `extension_ui_request` and block for the response (ca/docs/rpc-extension-ui.md:7)
- **9.6 SDK.** CHANGED: SDK inline extensions opt into built-in status with `builtin: true`, and `AgentSession.steer()`/`followUp()` return a disposition (0.99.0). `createAgentSession` and `AgentSessionRuntime` (ca/docs/sdk.md:8, :58, :112; ca/CHANGELOG.md:192, :185)
- **9.7 Fork and rebrand.** NEW (gap). A source fork sets the CLI name and config directory through `piConfig` in `package.json` (ca/docs/cli-integration.md:82-95)

## 10. Packages
- **10.1 Package commands.** CHANGED: `pi config` has a Built-in section (0.99.0), and `pi update` on a global npm install recommends the managed installer (1.0.1). `pi install`, `remove`, `update`, `list`, `config` (ca/docs/packages.md:12-17, :121; ca/CHANGELOG.md:192, :36)
- **10.2 Package manifest and filters.** CHANGED: pi warns about extension packages that list host-provided modules in `dependencies` (0.99.0). The `pi` key in `package.json` lists resources (ca/docs/packages.md:63, :80-88; ca/CHANGELOG.md:213)
- **10.3 Self-update and install channels.** NEW (gap). The pi.dev installer pins dependencies and updates with `pi update`; npm install; a Nix flake since 1.0.1 (ca/docs/quickstart.md:12-29)

## 11. TUI
- **11.1 Fullscreen mode.** CHANGED: fullscreen is the default (1.0.0); `fullscreenWheelScrollLines` (0.99.0). `tuiMode` is `regular` or `fullscreen` (ca/docs/settings.md:94; ca/CHANGELOG.md:80, :184)
- **11.2 @ file search.** UNCHANGED. Type `@` to search for a file and add it to the prompt (ca/docs/usage.md:17)
- **11.3 External editor.** UNCHANGED. `externalEditor`, defaulting to `$VISUAL` and `$EDITOR` (ca/docs/settings.md:31)
- **11.4 Themes.** CHANGED: the `system` theme, from the terminal's palette, is the default, and themes take `oklch()` and `okhsl()` colors (0.99.0) (ca/docs/themes.md:3; ca/CHANGELOG.md:180-181)
- **11.5 Keybindings.** UNCHANGED. `<agent-dir>/keybindings.json` (ca/docs/keybindings.md:9)
- **11.6 Mermaid diagrams.** UNCHANGED. `markdown.mermaid` is `off`, `final` or `streaming` (ca/docs/settings.md:113)
- **11.7 Inline images.** CHANGED: JPEG, GIF and WebP images are transcoded for the Kitty graphics protocol (1.0.1) (packages/tui/CHANGELOG.md:11; ca/CHANGELOG.md:44)
- **11.8 Games.** UNCHANGED. Example extensions such as `snake.ts` (ca/examples/extensions/README.md:61)
- **11.9 Startup header and quiet startup.** NEW (gap). `quietStartup` hides the startup header and resource listing; the `"header"` value arrived in 1.0.0 (ca/docs/settings.md:93; ca/CHANGELOG.md:74)
- **11.10 Status footer.** NEW (gap). The footer shows folder, session, model, context usage, and accumulated usage and cost (ca/docs/usage.md:9)
- **11.11 Terminal integration and platform guides.** NEW (gap). Terminal setup, tmux, Termux and Windows guides (ca/docs/terminal-setup.md:1; ca/docs/tmux.md; ca/docs/termux.md; ca/docs/windows.md, file names only)
- **11.12 pi-tui library.** NEW (gap). A terminal UI framework with differential rendering and synchronized output (packages/tui/README.md:3)

## 12. Settings
- **12.1 Settings locations and layering.** CHANGED: `mcp.json` is read at user level and, once trusted, per project (0.99.0). User configuration lives in `~/.pi/agent`, project configuration in `.pi/` (ca/docs/configuration.md:3, :15, :31; ca/CHANGELOG.md:175)
- **12.2 Environment variables.** CHANGED: the Anthropic workload identity variables join the provider configuration (0.99.2). `PI_CODING_AGENT_DIR`, `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY` and others (ca/docs/environment-variables.md:81-86, :98; ca/CHANGELOG.md:119)
- **12.3 Offline mode.** NEW (gap). `--offline` or `PI_OFFLINE=1` turns off automatic network activity (ca/docs/cli.md:229-230)

## 13. Security
- **13.1 No sandbox and no per-call approval.** CHANGED: codemode scripts run in a QuickJS sandbox, though the tools they call do not (0.99.0). Safety comes from limiting what the process can reach; trust and review are not a boundary (ca/docs/security.md:5-16; ca/docs/codemode.md:7; ca/CHANGELOG.md:175)
- **13.2 Project trust.** CHANGED: project `.pi/mcp.json` is read only after trust (0.99.0). Without a saved decision, `defaultProjectTrust` (default `"ask"`) decides (ca/docs/security.md:65; ca/docs/mcp.md:32; ca/CHANGELOG.md:175)
- **13.3 Isolation.** UNCHANGED. Containers, sandboxes, or the Gondolin extension (ca/docs/containerization.md:14)
- **13.4 Guard examples.** UNCHANGED. `protected-paths` and the other guard extensions (ca/examples/extensions/README.md:23)
- **13.5 Tool annotations.** NEW. `readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint`, which permission extensions can read (ca/docs/extensions.md:166; ca/docs/mcp.md:252)

## 14. Telemetry and durability
- **14.1 Telemetry.** CHANGED: pi-agent-core no longer carries telemetry schemas or re-exports pi-telemetry (agent 1.0.0). pi-telemetry is a vendor-neutral span contract. `enableInstallTelemetry` defaults to `true` at 1.0.2, against the 0.87.1 inventory's "opt-in"; no CHANGELOG entry dates that (ca/docs/settings.md:167; packages/telemetry/README.md:3; packages/agent/CHANGELOG.md:13)
- **14.2 Agent-core hooks.** CHANGED: the experimental harness left pi-agent-core (agent 1.0.0), and `onProviderStreamEvent` was added (agent 0.99.0). `beforeToolCall`, `afterToolCall`, `prepareRequest` and `transformContext` (packages/agent/README.md:58, :118-126; packages/agent/CHANGELOG.md:13, :23)
- **14.3 pi-durable.** CHANGED: released as a durable agent harness that commits conversations, turns and tool calls before showing them, replacing agent-core's durable runtime (durable 1.0.0) (packages/durable/README.md:5; packages/durable/CHANGELOG.md:17; packages/agent/CHANGELOG.md:13)
- **14.4 Usage and cost accounting.** NEW (gap). `/session` shows token usage and cost; the footer shows accumulated usage and cost (ca/docs/usage.md:9, :62)

## 15. Experimental
- **15.1 chord.** CHANGED: the 0.87.1 inventory said "chord plugins"; at 1.0.2 its README describes an application-composition runtime with facets, services, replicated state and a remote-service boundary. There is no CHANGELOG entry after 0.87.1 behind this tag: chord has no CHANGELOG.md (packages/chord/README.md:3-6)
- **15.2 pi-protocol.** CHANGED: routed envelopes, CBOR encoding and byte-stream framing, at `PROTOCOL_VERSION` 8. There is no CHANGELOG entry after 0.87.1 behind this tag: packages/protocol/CHANGELOG.md has empty sections from 0.99.0 to 1.0.2 (packages/protocol/README.md:3; packages/protocol/src/protocol.ts:5)
- **15.3 pi-server and experimental client and server commands.** CHANGED: pi-server exports `SessionMetadata` and no longer depends on pi-agent-core (server 1.0.0). An experimental local server that routes clients to durable Sessions (packages/server/README.md:3; packages/server/CHANGELOG.md:13)
- **15.4 Docs evals.** UNCHANGED. Behavioural evals; `*.docs.eval.ts` measures documentation lift (packages/evals/README.md:3, :9)
- **15.5 pi-client.** NEW. A transport-neutral client for the experimental service protocol (packages/client/README.md:3)
- **15.6 Durable coding agent with subagents.** NEW. A local coding agent on pi-durable, with a vacation-planner copy (ca/src/experimental/durable/README.md:3-5; ca/src/experimental/vacation/README.md:3)

## 16. MCP client
- **16.1 MCP servers.** NEW. Servers over stdio or streamable HTTP from `mcp.json`, with project overrides since 1.0.1 (ca/docs/mcp.md:3, :34; ca/CHANGELOG.md:175, :29)
- **16.2 MCP OAuth.** NEW. Sign-in with discovery, RFC 9207 issuer checks, step-up scopes and Client ID Metadata Documents (ca/docs/mcp.md:115, :174; ca/CHANGELOG.md:68, :28)
- **16.3 MCP tool exposure.** NEW. Tools are `mcp__<server>__<tool>`, reached by `codemode` (default), `direct` and other exposures (ca/docs/mcp.md:189-195; ca/CHANGELOG.md:124)
- **16.4 MCP resources.** NEW. Resource tools when a server offers resources (ca/docs/mcp.md:234-236)
- **16.5 MCP management.** NEW. `/mcp` in a session, and `pi mcp add|remove|list|login|logout` (ca/docs/mcp.md:28; ca/docs/cli.md:306-320)
- **16.6 MCP from extensions and SDK.** NEW. `pi.registerMcpServer()` and `pi.unregisterMcpServer()` (ca/docs/extensions.md:192-197; ca/docs/mcp.md:254-268)
- **16.7 pi-mcp client library.** NEW. A standalone MCP client with stdio and streamable HTTP transports (packages/mcp/README.md:3-5)

## 17. Codemode, tool search, tool exposure
- **17.1 Codemode tool.** NEW. Model-written JavaScript runs in a QuickJS sandbox and calls tools; only the script's output reaches the model (ca/docs/codemode.md:7; packages/codemode/README.md:3; ca/CHANGELOG.md:175, :84)
- **17.2 Codemode store.** NEW. `store(key, value)` and `load(key)` keep small JSON values across calls (ca/docs/codemode.md:30, :53-57)
- **17.3 Codemode models API.** NEW. `models.classify()` and `models.generateImages()` in scripts (ca/docs/codemode.md:59, :96, :165; ca/CHANGELOG.md:75)
- **17.4 Tool search.** NEW. `tool_search`, off by default, finds undeclared tools and declares the matches (ca/docs/cli.md:170-172; ca/CHANGELOG.md:175)
- **17.5 Tool exposure modes and namespaces.** NEW. `exposure` is `direct`, `model-only`, `codemode`, `deferred` or `hidden`, with a `namespace` (ca/docs/extensions.md:152-164; ca/CHANGELOG.md:176)
- **17.6 Loadout preparation.** NEW. `prepareLoadout(loadout)` adjusts what the model sees while an orchestrating tool is active (ca/docs/extensions.md:180; ca/CHANGELOG.md:176)
