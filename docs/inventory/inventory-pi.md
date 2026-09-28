# pi 0.87.1 capability inventory (@earendil-works/pi-coding-agent, MIT)

Source is the published package, unpacked at `scratchpad/package`: `docs/*.md`, `examples/`, and `dist/core/extensions/types.d.ts`. Items marked [cosmetic] are appearance-only.

## 1. Agent loop and tools
1. **Turn loop.** A prompt goes to the provider stream; tool calls produce results; another turn follows until nothing is pending. (how-pi-works)
2. **Steering and follow-up queues.** Steering is delivered after the current turn, follow-up after the run. Each queue has an `all` or `one-at-a-time` mode. Aborting puts the queued text back in the editor. (usage, settings)
3. **Built-in tools.** `read`, `bash`, `edit` and `write` are on by default. `grep`, `find` and `ls` are optional. `powershell` exists on native Windows. (cli)
4. **Tool selection flags.** `--tools`, `--exclude-tools`, `--no-builtin-tools`, `--no-tools`, and the `defaultTools` setting. (cli, settings)
5. **Output truncation.** 50KB or 2000 lines by default. (examples/extensions/README, dist/core/tools/truncate.js)
6. **Swappable tool backends.** `Read/Bash/Edit/Write/Grep/Find/LsOperations` interfaces, so tools can run remotely (ssh, gondolin).
7. **Parallel tool calls.** A tool can set `executionMode: "sequential"`. `withFileMutationQueue` serializes read-modify-write. (extensions)
8. **`terminate: true`.** If every tool in a batch returns it, the follow-up model request is skipped. (extensions)
9. **Retries.** Agent-level exponential backoff, plus provider retry settings. (settings, json)
10. **Overflow recovery.** On a length error, one compact-and-retry, recorded as `context_edit` omissions. (compaction)
11. **User shell commands.** `!cmd` runs a command and sends its output to the model; `!!cmd` runs it without sending. (usage)
12. **Environment for shells the model calls.** `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL`. (environment-variables)
13. **Shell configuration.** `shellPath`, `shellCommandPrefix`, aliases. (shell-aliases)
14. **Images.** `@file`, paste, auto-resize to 2000px, `blockImages`. (usage)
15. **Prompt-cache warming.** `off`, `streaming` or `idle`; warms only when the estimated saving is at least $0.05. (settings)

## 2. Providers, models, auth
1. **Built-in providers.** More than 30 API-key providers, each with its own environment variable. (providers)
2. **Login.** OAuth and device-flow `/login` and `/logout`; credentials are stored in `auth.json`. (providers)
3. **Credential values.** A credential can be a `!command` whose output is the key, can use `$VAR` interpolation, and can carry its own `env` object. (providers, custom-provider)
4. **Cloud platforms.** Azure OpenAI, Bedrock, Vertex, Cloudflare. (providers)
5. **Credential precedence.** `--api-key`, then `auth.json`, then `models.json`, then environment variables or ambient credentials. (models)
6. **`models.json`.** Adds compatible endpoints (Ollama, vLLM, LM Studio) and holds `modelOverrides`, `inputLimits` and `promptCache`. (models)
7. **Model catalog.** An overlay catalog with an offline cache, refreshed by `pi update --models`. (models, cli)
8. **Thinking levels.** `off` through `max`, with `thinkingBudgets` and `modelThinkingLevels`. (settings)
9. **Scoped model cycling.** `--models`, `enabledModels`, `/scoped-models`. (cli)
10. **`/llama`.** A llama.cpp router that loads, unloads and downloads models from Hugging Face. (llama-cpp)
11. **Radius gateway catalog.** (providers)
12. **Custom providers.** `registerProvider`, `refreshModels`, OAuth, `streamSimple`, `onPayload`/`onResponse` hooks. (custom-provider)
13. **Transports.** `sse`, `websocket`, proxy settings, timeouts. (settings)
14. **pi-ai library.** Constrained sampling for tools, image generation, classification, a faux provider for tests, and cross-provider handoff. (pi-ai README)

## 3. Context files and system prompt
1. **Context files.** `AGENTS.md`, `CLAUDE.md` and `AGENTS.override.md` are read from the agent directory, the working directory and its ancestors. They need no trust; `--no-context-files` turns them off. (configuration)
2. **`SYSTEM.md` and `APPEND_SYSTEM.md`.** At user or project level, one replaces the system prompt and the other appends to it; the project file wins. (configuration)
3. **Flags.** `--system-prompt`, `--append-system-prompt`. (cli)
4. **Sectioned system prompt.** Sections for tool snippets, guidelines, skills and context files. Changes are persisted as deltas. (system-prompt.d.ts)

## 4. Extensions API
TypeScript loaded via jiti, from the user directory, the project directory, `-e`, or packages. (extensions)

**Events** (`pi.on`, which returns an unsubscribe function):
- **Trust and resources:** `project_trust`, `resources_discover`.
- **Session:** `session_start`, `session_info_changed`, `session_before_switch`, `session_before_fork`, `session_before_compact`, `session_compact`, `session_compact_failed`, `session_shutdown`, `session_before_tree`, `session_tree`.
- **Context and provider:** `context`, `context_with_system`, `cache_warming_decision`, `before_provider_request`, `before_provider_headers`, `after_provider_response`.
- **Agent run:** `before_agent_start`, `agent_start`, `agent_end`, `agent_before_settle` (can append and continue once), `agent_settled`.
- **UI prompts:** `ui_prompt_start`, `ui_prompt_end`.
- **Turns and messages:** `turn_start`, `turn_end` (can continue), `message_start`, `message_update`, `message_end` (can replace the message).
- **Tools:** `tool_execution_start`, `tool_execution_update`, `tool_execution_end`, `tool_call` (can change the input or block the call), `tool_result` (results compose).
- **Selection and input:** `model_select`, `thinking_level_select`, `user_bash`, `input`.

**`ExtensionAPI` methods:**
- **Registration:** `registerTool`, `registerCommand`, `registerShortcut`, `registerFlag`/`getFlag`, `registerMessageRenderer`, `registerMarkdownTransformer`, `registerEntryRenderer`, `registerProvider`/`unregisterProvider`.
- **Messages and entries:** `sendMessage` (as steer, followUp or nextTurn), `sendUserMessage`, `appendEntry`.
- **Session metadata:** `setSessionName`, `setLabel`.
- **Tools and commands:** `exec`, `get/setActiveTools`, `getAllTools`, `getCommands`.
- **Model:** `setModel`, `get/setThinkingLevel`.
- **Other:** an `events` bus.

**`ctx` (passed to handlers):**
- **Fields:** `hasUI`, `cwd`, `sessionManager`, `modelRegistry`, `signal`.
- **Methods:** `abort`, `isIdle`, `isProjectTrusted`, `getContextUsage`, `compact`, `getSystemPrompt`, `shutdown`.
- **Commands only:** `waitForIdle`, `newSession`, `fork`, `navigateTree`, `switchSession`, `reload`.
- **`ctx.ui`:** `select`, `confirm`, `input`, `editor`, `notify`, status, widgets, header and footer, overlays, editor text, autocomplete [mostly cosmetic].

**Tool definition fields:** `promptSnippet`, `promptGuidelines`, `constrainedSampling`, `prepareArguments`, `executionMode`, renderers.

**Mode behaviour:** extensions load in every mode. RPC mode gets dialogs; JSON and print modes get no UI.

## 5. Skills
1. **Agent Skills format.** Frontmatter: name, description, license, compatibility, metadata, `allowed-tools`, `disable-model-invocation`.
2. **Progressive disclosure.** The prompt carries only name, description and path; the model reads the full skill on demand.
3. **`/skill:name args`** forces a skill; the `enableSkillCommands` setting controls this.
4. **Locations.** `~/.pi/agent/skills`, `.pi/skills`, `~/.agents/skills`, and `.agents/skills` in any ancestor; plus the `--skill` and `--no-skills` flags.

## 6. Prompt templates
A Markdown file becomes a `/name` command. Frontmatter may set `description` and `argument-hint`. Arguments are substituted with `$1`, `$@`, `$ARGUMENTS`, `${1:-default}` and `${@:N:L}`, using shell-style quoting.

## 7. Slash commands
- **Model and settings:** `/settings`, `/model`, `/thinking`, `/scoped-models`, `/login`, `/logout`, `/llama`.
- **Sessions:** `/new`, `/resume`, `/name`, `/session`, `/tree`, `/fork`, `/clone`, `/compact`, `/import`.
- **Sharing and export:** `/copy`, `/export`, `/share`, `/bug`.
- **Other:** `/trust`, `/reload`, `/hotkeys`, `/changelog`, `/quit`, `/debug`.

## 8. Sessions, branching, compaction
1. **Storage.** A JSONL tree using `id`/`parentId`; format v3 with auto-migration.
2. **Entry types:** `session`, `message`, `model_change`, `thinking_level_change`, `usage`, `compaction`, `context_edit`, `branch_summary`, `custom`, `custom_message`, `label`, `session_info`.
3. **`/tree`.** Branching inside one file, with filters and labels. When you leave a branch it is summarized.
4. **Session flags.** `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, `--no-session`, `--session-dir`.
5. **Auto-compaction.** Triggers when the context exceeds the window minus `reserveTokens` (16384); keeps `keepRecentTokens` (20000).
6. **Summary format.** Goal, Progress, Decisions, Next Steps and Critical Context, plus the files read and modified.
7. **Extension summaries.** An extension can supply the compaction or tree summary.
8. **Export and share.** Export to HTML or JSONL; share as a gist or through Radius.

## 9. Run modes
1. **Interactive and print.** Print mode (`-p`) is the default when stdin or stdout is not a TTY, and exits non-zero on error.
2. **JSON mode.** A session header, then JSONL events. `message_update` carries deltas only; `agent_settled` marks the end.
3. **Event types:** agent, turn, message, tool, `queue_update`, `entry_appended`, compaction, `auto_retry_*`, `bash_execution_update`, `extension_error`.
4. **RPC over stdin/stdout JSONL, 33 commands:**
   - Prompting: `prompt`, `steer`, `follow_up`, `abort`, `clear_queue`.
   - Model and thinking: `set_model`, `cycle_model`, `get_available_models`, and set/cycle/list for thinking.
   - Queue modes: `set_steering_mode`, `set_follow_up_mode`.
   - Compaction and retry: `compact`, `set_auto_compaction`, `set_auto_retry`, `abort_retry`.
   - Shell: `bash`, `abort_bash`.
   - Session: `new_session`, `get_state`, `get_messages`, `get_session_stats`, `export_html`, `switch_session`, `fork`, `clone`, `get_fork_messages`, `get_entries`, `get_tree`, `get_last_assistant_text`, `set_session_name`.
   - Discovery: `get_commands`.
5. **RPC extension-UI subprotocol.** Dialog requests wait for a response.
6. **SDK.** `createAgentSession`, an in-memory `SessionManager`, custom resource loaders.

## 10. Packages
- **Commands.** `pi install`, `remove`, `update`, `list` and `config`, from npm, git, a URL or a local path; `--local` scope; pinned refs.
- **Manifest.** A `pi` manifest lists extensions, skills, prompts and themes, with resource filters.

## 11. TUI [mostly cosmetic]
Fullscreen mode, `@` file search, an external editor, themes, keybindings, Mermaid diagrams, images, games.

## 12. Settings
- **Locations.** The agent directory `~/.pi/agent` (settings, models, auth, trust, sessions) and a project `.pi/`. Project settings override user settings; resource lists merge.
- **Environment variables.** `PI_CODING_AGENT_DIR`, `PI_OFFLINE`, among others.

## 13. Security
1. **No sandbox.** pi has no built-in sandbox and no per-call approval.
2. **Project trust.** Trust gates `.pi` resources and `.agents/skills`; it does not gate context files. Decision order: the CLI `--approve`/`--no-approve` flags, then a `project_trust` extension, then the saved decision, then `defaultProjectTrust`.
3. **Isolation.** Docker, Docker Sandboxes, OpenShell or the Gondolin micro-VM.
4. **Guard examples.** `permission-gate`, `protected-paths`, `dirty-repo-guard`, `sandbox`, `plan-mode`, `git-checkpoint`, `subagent`.

## 14. Telemetry and durability
- **Telemetry.** Install telemetry (opt-in) and a vendor-neutral span contract.
- **Agent-core hooks.** `beforeToolCall`, `afterToolCall`, `prepareRequest`, `finishTurn`, `transformContext`.
- **pi-durable.** Conversation, task and document records stored in memory, JSONL or SQLite.

## 15. Experimental
chord plugins, pi-protocol (CBOR), pi-server with multi-attachment sessions, docs evals.
