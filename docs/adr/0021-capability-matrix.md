# ADR 0021 — Inheriting pi, Apex and Paperclip: the capability matrix

**Status:** Accepted.
**Date:** 2026-09-27
**Inventories:** `docs/inventory/inventory-pi.md` (pi 1.0.2 at upstream commit 200387122ca4, re-inventoried 2026-10-04; the pi rows dated before 2026-10-04 were written against pi 0.87.1),
`docs/inventory/inventory-apex.md` (apex-notes `f449644a`),
`docs/inventory/inventory-paperclip.md` (Paperclip v2026.916.1), with the addendum
`docs/inventory/inventory-paperclip-20261004.md` (PC-101 to PC-144, Paperclip at
upstream commit 1c07b590, 2026-10-04; added 2026-10-06, #335). All three
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
writer". Since ADR 0032 all four run in a throwaway clone of the
repository, and their edits come back through trellis's own write path.
The clone is not an OS sandbox: a writer that reaches the repository by
absolute path can still write there, `.git` included, and against that
writer ADR 0009's guarantee is best-effort; nor are network access or
reads outside the clone contained (ADR 0032 "Limits"). An OS sandbox or
container, row 13.3 below, is still deferred. The rows below for `bash`,
hooks, custom tools and process agents mean "built, with that limit".

**When the waves landed** (2026-09-27):

Every wave has landed on branch `inherit` (PR #1): W-A `2aa7b72`, W-B
`749e5a0`, W-C `69f585b`, W-D `8147c7f`, W-E `773abb0`, and the cross-wave
integration `cf68b00`. A row below marked W-x means **built under that
wave's ADR**, except where the ADR says otherwise. Each of ADRs 0022–0026
has an "As built" or "Not built" section, and it overrides this table
wherever the two disagree. The items those sections name as not built are:

- **W-A (0022):** auto-creating a note for a newly typed link (row 22).
  There is no typing event on a command line; `vault new` is the explicit
  form. Also row 35–37's writing skill as a skill in the binary (ADR 0022
  §6). *Amended 2026-10-06 (#360): built in effect instead: Apex's own
  SKILL.md loads unchanged from a repository or an installed package; row
  35–37 says how.*
- **W-B (0023):** per-item trust (one digest covers hooks, custom tools and
  bash together); tracking what bash writes in the worktree. (A
  configurable custom-tool timeout was listed here; #135 built it, and
  0023 "As built" records how. So was `after_run` on Ctrl-C; #138 built
  it, and 0023 §5 records how.)
- **W-C (0024):** streaming, `HTTPS_PROXY`, a `Retry-After` given as a date,
  summaries written by the model, and OAuth/Bedrock/Vertex/Azure. (The
  `ask` classifier on hosted models was built by #41.) Hosted runs have
  been exercised only against loopback stubs, never a real provider.
  Amended 2026-09-29 (#160): `HTTPS_PROXY` is landing (#150), not yet
  merged; row 2.13 below says so. *Amended 2026-10-05 (#416): built
  (#150, ADR 0024 §1).*
  *Amended 2026-10-04 (#316): HTTPS_PROXY built (#150).*
  *Amended 2026-10-06 (#452): a `Retry-After` given as a date built
  (#149, ADR 0024 §4): `retry_delay` in crates/trellis-model/src/hosted.rs
  parses the IMF-fixdate form; the RFC 850 and asctime forms fall back to
  the fixed backoff (pi row 1.9).*
- **W-D (0025):** `/new`, `/name`, `/copy`, RPC `compact`, and a user-level
  `system.md`. Amended 2026-09-29 (#160): `/name` and `/copy` are landing
  (#109), not yet merged. *RPC `compact` built 2026-09-29 (#110; ADR 0025
  §5).* *Amended 2026-10-05 (#416): `/name` and `/copy` built (#109; ADR
  0025 "Not built", `session.rs`).*
  *Amended 2026-10-04 (#316): /name and /copy merged (#109). A user-level
  `system.md` was declined (#111) by the owner on 2026-09-28: ADR 0025 §2
  stands, and system prompt files are repository-level only.*
- **W-E (0026):** dollar accounting, and webhook or API triggers. Routines
  fire only while `work` runs. *Corrected 2026-10-04: this bullet also
  listed IANA time-zone names, `@profile` mentions and prompt-token
  accounting; they are built (#47, #57, #108; ADR 0026 §7, §1, §5).*

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
| 1.2 steering / follow-up queues | deferred: `session`/`tui` can accept input mid-run only after a reverse channel exists for more than approvals; ADR 0018's channel is the base. **Amended 2026-09-29 (#160):** owner-approved 2026-09-28 to build in parts. Delivery of queued input between loop turns in `trellis-loop` is landing (#113), not yet merged. `steer` and `follow_up` over `trellis rpc` are built (#114, ADR 0025 §5). Still to come: queued input in `tui` (#115) and in `session` (#116), and recording it as user input in the trail (#117). **Amended 2026-10-05 (#416):** built: #113 (delivery between loop turns), #114 (ADR 0025 §5), #115 (`tui`), #116 (`session`) and #117 (the trail) are all merged. **Amended 2026-10-04 (#316):** built in every part named above: delivery between loop turns (#113, `SteeringLog::drain` in `trellis-loop`), `steer` and `follow_up` over `trellis rpc` (#114), queued input in `tui` (#115) and in `session` (#116), and the trail record (#117, #176; ADR 0034). `clear_queue` is excluded (ADR 0025 §5). pi's one-at-a-time queue modes are not built, because every steer waiting at a model request is delivered in it. |
| 1.3 built-in tools read/edit | built; **write, grep, find, ls, bash → W-B** (bash is opt-in, see ADR 0023) |
| 1.4 tool selection flags | W-B (`[tools]` enable/disable) |
| 1.5 output truncation | built (2,000-character tail, and a separate skill window, ADR 0019) |
| 1.6 swappable tool backends (ssh, micro-VM) | deferred: needs the sandbox decision (S-series) first |
| 1.7 parallel tool calls | excluded: one call per turn is the loop shape (ADR 0005). **Amended 2026-09-29 (#63):** the owner kept it excluded on 2026-09-28, and the shape carries weight beyond ADR 0005. ADR 0023's gate log records a denial against the turn number, because the loop makes exactly one `Request::Call` per turn; the trail's actions, the rollback points and the write/check serialization (S5) assume one call per turn too. Building it needs an ADR that first gives the gate log and the trail per-call ids and serializes writes, with a test that two edits to one file never interleave. **Amended 2026-10-04 (#324):** pi 1.0.2 (inventory 1.7, 4.23, 17.1) adds codemode (17.1) and nested tool calls (4.23) as further routes to several calls in one turn: a codemode script calls tools in parallel, and `ctx.executeTool` runs one tool from inside another. Both are excluded under the same clause (#63), and so are the rows the re-inventory section gives them. Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. |
| 1.8 terminate flag | built in effect (`done` ends a run; convergence is decided by checks) |
| 1.9 retry/backoff | built (format retries); provider backoff → W-C |
| 1.10 overflow recovery / compaction | W-C |
| 1.11 `!cmd` user shell | W-D (`session` `!` and `!!`) |
| 1.12 session env for model shells | W-B (`TRELLIS_RUN_ID`, `TRELLIS_MODEL` in the bash tool's environment) |
| 1.13 shell config | W-B (`[tools] shell`) |
| 1.14 images | deferred: no local runtime in the tier matrix takes image input. **Amended 2026-09-29 (#53):** built for hosted tier 1 only, as `trellis run --image <path>` (ADR 0041): PNG, JPEG, GIF or WebP by magic bytes, at most 5 MiB, not resized, its base64 counted against `[model] context_bytes`, and recorded in the trail by name, size and SHA-256. Every other tier refuses the run before the snapshot. |
| 1.15 prompt-cache warming | excluded: a dollar heuristic for hosted APIs; COST_MODEL.md owns cost. **Amended 2026-09-29 (#67):** deferred to v1.x. Its trigger is a dollar estimate, and no dollar figure is shown until COST_MODEL.md has a sourced per-model price table and prompt tokens are counted. |
| 2.1–2.4 30+ providers, OAuth, cloud platforms | **W-C covers the two wire protocols most of them speak** (OpenAI-compatible chat completions, Anthropic Messages) over HTTPS with keys from environment variables. OAuth and device login are excluded because ADR 0010 says a key never goes in a file trellis reads. Bedrock, Vertex and Azure request signing are deferred. **Amended 2026-09-29 (#45):** the owner kept signing deferred on 2026-09-28 and took per-model input limits first (#162). A signing scheme must keep ADR 0010 (credentials come from the environment or the platform's own ambient chain, never a file trellis writes), and is accepted only with its signing tested against recorded vectors and a loopback stub. **Amended 2026-10-04 (2.1 protocols not spoken):** pi 1.0.2's providers include ones reached only through the OpenAI Responses, Gemini, Bedrock Converse or Mistral APIs, which trellis does not speak. Such a provider is reachable only through an OpenAI-compatible gateway the user runs, and a third protocol is an ADR 0024 amendment. Accepted (owner-directed) under the 2026-10-04 delegation. |
| 2.5 credential precedence | W-C, environment only (ADR 0010) |
| 2.6 `models.json` endpoints and overrides | built (`[model] spec`, catalog); overrides → W-C. **Amended 2026-09-29 (#162):** per-model input limits built, as `[[providers]] input_bytes` (ADR 0024 §5); cache lifetimes and cost tables not built. |
| 2.7 catalog overlay | built (`trellis model`) |
| 2.8 thinking levels | W-C, where the provider protocol carries it |
| 2.9 scoped model cycling | W-D (`session` `/model` already switches; add a list) |
| 2.10 llama.cpp router | built in part (`trellis model pull`); router deferred. **Amended 2026-09-29 (#45):** the owner kept the router deferred on 2026-09-28. Listing, loading and unloading models on a running llama.cpp router is not built; `trellis model pull` is the download half. **Amended 2026-09-30 (#216):** `pull` fetches at a pinned commit and verifies SHA-256 before the file is moved into place (ADR 0044). |
| 2.11 Radius gateway | excluded: networked service we would control (ADR 0010) |
| 2.12 custom providers | W-C (a `[[providers]]` table naming a protocol and base URL) |
| 2.13 transports / proxy | W-C (HTTPS; proxy from the environment); websocket excluded. **Amended 2026-09-29 (#160):** W-C did not build the proxy (see "Where this stands"). `HTTPS_PROXY` and `NO_PROXY` are landing (#150, an ADR 0024 §1 amendment), not yet merged. **Amended 2026-10-05 (#416):** built (#150, ADR 0024 §1; `crates/trellis-model/src/transport.rs`, held by `a_proxy_is_used_only_for_https_to_a_remote_host` and `hosted.rs`'s `a_refused_proxy_fails_at_construction_before_any_request`). **Amended 2026-10-04 (#316):** proxy built (#150); this row no longer reads as landing. |
| 2.14 pi-ai extras | constrained sampling built (llguidance, tier 2); image generation and classification excluded (not a coding-loop concern). **Amended 2026-10-04 (#325):** in the pi 1.0.2 inventory classification and image generation are rows 2.17 and 2.18, split out of 2.14; their rows are in the re-inventory section below. |
| 3.1 context files AGENTS.md / CLAUDE.md | W-D |
| 3.2–3.3 SYSTEM.md / APPEND_SYSTEM.md, flags | W-D (`.trellis/system.md`, `.trellis/append_system.md`; the grounding clause cannot be removed) |
| 3.4 sectioned prompt | built (the adapter's fenced sections) |
| 4 extensions: `tool_call`, `tool_result`, `before_agent_start`, `agent_end`, `turn_end`, `input`, `registerTool`, `registerCommand`, `exec`, `get/setActiveTools` | **W-B as executable hooks behind a trust store** (language-agnostic, JSON on stdin/stdout), plus custom command tools. **Amended 2026-10-04 (4.20 mode behaviour, unchanged in pi 1.0.2):** pi's extensions load in every mode, and `ctx.mode` (since 0.78.1) and `ctx.hasUI` let one guard terminal-only behaviour. Hooks here load for any run that has them, whichever of `trellis run`, `session`, `tui` or `rpc` started it (`tooling::prepare`), and a hook draws no UI, so its payload carries no mode and `ctx.mode` has no counterpart; an approval reaches the person through the gate, over rpc as `approve` (ADR 0025 §5). The disposition stays W-B. |
| 4 remaining events (message rendering, provider headers, UI widgets, shortcuts, renderers, themes) | excluded: cosmetic or TypeScript-runtime specific; `before_provider_request` is excluded because a hook could exfiltrate the key |
| 5.1–5.2 skills format, progressive disclosure | built (ADR 0019) |
| 5.3 `/skill:name` force-load | W-D (`run --skill <name>` and session `/skill`) |
| 5.4 extra skill locations | W-D (`~/.agents/skills` read-only, after trust) |
| 6 prompt-template arguments | W-D |
| 7 slash commands | built in part (`session`); `/new`, `/name` and `/copy` → W-D; `/login`, `/logout`, `/share` and `/bug` excluded (ADR 0010, networked). **Amended 2026-09-29 (#160):** W-D did not build `/name` or `/copy`; both are landing in `trellis session` (#109), not yet merged. **Amended 2026-10-05 (#416):** `/name` and `/copy` built in `trellis session` (#109; ADR 0025 "Not built", `session.rs`). **Amended 2026-10-04 (#316):** /name and /copy built (#109); this row no longer reads as landing. |
| 8.1–8.4 session tree, branching, fork | deferred: the trail is the record, and each run is a fresh loop. A branchable session is v2 memory work (v0.3 closed without it). **Amended 2026-10-04 (8.4 session flags, unchanged in pi 1.0.2):** pi's `-n`/`--name` (since 0.78.0) sets a session's display name at startup; the pi 0.87.1 inventory did not list it. trellis has no such flag, and one would grow the CLI surface brief 4.12 fixes, so it stays deferred with this row. The nearest trellis behaviour is `/name` in `trellis session`, which records a label in the trail of each run started after it (#109). |
| 8.5–8.7 compaction | W-C |
| 8.8 export and share | export → W-D (`trellis log --json`); share excluded (networked). **Amended 2026-09-29 (#143):** HTML export excluded. `trellis log --json` is the export, and an HTML view can be rendered from it outside trellis; a renderer inside trellis would have to escape untrusted trail text (ADR 0007) for a browser, a new injection surface for nothing the JSON lacks. |
| 9.1 print mode | built (`trellis run`) |
| 9.2–9.3 JSON event mode | built as `--log-json`; stdout JSONL → W-D (`--json`) |
| 9.4–9.5 RPC | W-D (`trellis rpc`, a subset: prompt, abort, get_state, get_messages, set_model, compact, approve) **Amended 2026-10-04 (#319, written 2026-10-06):** `get_commands` lists 19 commands: prompt, abort, get_state, get_messages, set_model, get_available_models, approve, steer, follow_up, compact, get_codebase, get_radius, get_queue, vault_search, vault_graph, get_session_stats, get_last_assistant_text, get_commands, quit of which 14 are pi's 12 names, approve and quit, and five, get_codebase, get_radius, get_queue, vault_search and vault_graph, are trellis's own (#292–#295). ADR 0025 §5's table gives each of pi 1.0.2's 33 commands as built or not built, with its reason. Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. |
| 9.6 SDK | built in effect (the crates are libraries) |
| 10 packages | deferred: `trellis install` for skills and prompts needs the trust store first; revisit after W-B. **Amended 2026-09-29 (#118, ADR 0039):** built in part: `trellis install <source>@<ref>`, `--list`, `--remove` copy skills and prompt templates into `<user config dir>/packages/`, recorded in `packages.toml`; packages carry no code. **Amended (#119, ADR 0039 §10):** runs read installed skills and templates after the repository's, which wins a name |
| 11 TUI features | excluded [cosmetic]; `@` file search is built in `tui` (paths from `git ls-files`, so ignored files are never offered; #52), and `session` line mode has none. **Amended 2026-10-06 (#500), by ADR 0052, accepted 2026-10-07:** the TUI these features are measured against moves from ratatui (`crates/tui-demo`) to a Go/Bubble Tea client of `trellis rpc` (`tui/`) once it reaches parity; `@` file search moves with it, through an rpc `list_paths` command that keeps ignored files out. The row stays excluded [cosmetic] for everything else. |
| 12 settings layering | W-D (user-level `~/.config/trellis/config.toml` under the repo's) |
| 13.1–13.2 trust | W-B |
| 13.3 containers | first step built: `bash` and custom tools run in a throwaway git clone (ADR 0032); containers, for network and home-directory isolation, still deferred |
| 13.4 guard examples | built in effect: `permission-gate` = ADR 0018, `protected-paths` = `deny_write`, `dirty-repo-guard` = ADR 0009, `git-checkpoint` = snapshot and rollback. **Amended 2026-09-29 (#141):** `plan-mode` = an agent profile whose `tools` list is read-only (for example `read_file`, `grep`, `find`, `ls`; ADR 0026 §4), built in effect |
| 14 telemetry / durability | built (`tracing`, trail, queue) **Amended 2026-10-04 (14.3 pi-durable):** pi-durable was rebuilt in 1.0.0 as a durable harness with checkpointed tasks and subagents. Built in effect through the trail, the queue and `trellis resume` (ADR 0038); an interrupted run rolls back (ADR 0009) rather than resuming a tool call. Its subagents are row 15.6. Accepted (owner-directed) under the 2026-10-04 delegation. |
| 15 experimental (chord, pi-protocol, pi-server) | excluded: networked/multi-attach (brief Part 1) |

### pi 1.0.2 re-inventory (2026-10-04)

Rows for items that `docs/inventory/inventory-pi.md` (pi 1.0.2) tags NEW, and for 4.11 and 4.15, which it tags UNCHANGED but the matrix never gave a disposition. Dispositions in this section are Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation, and the owner can override any row (Context). Rows for 2.15-2.19, 4.23, 4.24, 13.5, 16.1-16.7 and 17.1-17.6 are written by their own tickets: 2.15 by #326, 2.16 by #327, 2.17-2.19 by #325, 4.23, 4.24 and 17.1-17.6 by #324, 13.5 by #328 and 16.1-16.7 by #323. The other rows here were recorded by #322.

| Items | Disposition |
|---|---|
| 1.16 process markers | built (#317): `AI_AGENT=trellis` in the environment of `bash`, custom tools and hooks (crates/trellis-cli/src/tooling.rs) and of process agents (crates/trellis-cli/src/process_agent.rs), and never added to grounded checks, so a Verdict runs in the environment a person's own test run sees (crates/trellis-cli/tests/child_env.rs). pi sets `AI_AGENT=pi` and `PI_CODING_AGENT=true` (inventory 1.16: ca/docs/environment-variables.md:15-16). |
| 2.15 Anthropic workload identity federation | deferred with cloud signing (row 2.1–2.4, #45; recorded by #326). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation, to be revisited once a hosted run has reached a real provider (#8). Since pi 0.99.2, with no Anthropic key set, pi authenticates from `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID` and `ANTHROPIC_IDENTITY_TOKEN_FILE` (inventory 2.15: ca/docs/providers.md:69; ca/CHANGELOG.md:119), so the credential comes from an identity token file the platform provides rather than a key. trellis reads a key from the environment variable the user's configuration names and sends it only to an endpoint the user chose (ADR 0024 §3); if federation were built, trellis would make the exchange with Anthropic directly from the user's machine, so ADR 0010's rule that the key "never touches infrastructure we control" holds either way. A build adds a token exchange and a refresh to trellis-model's hosted path, the one path that carries a credential, before any hosted run has reached a real provider (#8). It is accepted only on #45's terms: credentials from the environment or the platform's own chain, never a file trellis writes; the exchange tested against recorded vectors and a loopback stub; and the access token held in memory only, never in a trail, a log line or a child environment. A hosted run reaching a real provider and the owner wanting federation built reverses this (an ADR 0024 §3 amendment); the owner reading the identity token file as a credential file trellis reads would instead exclude it under row 2.1–2.4's OAuth wording. |
| 2.16 sampling parameters | deferred (recorded by #327). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. pi's `samplingParamsByThinkingLevel` in `models.json` sets `temperature`, `top_p` and similar per thinking level on OpenAI-compatible APIs (inventory 2.16: ca/docs/models.md:101; ca/CHANGELOG.md:13). trellis keeps decoding fixed: local runtimes get `temperature: 0.0` in every dialect (crates/trellis-model/src/runtime.rs:189-212) and hosted models get none, because reasoning models reject a non-default one (crates/trellis-model/src/hosted.rs:263). ADR 0028 waives metric 5 (run-to-run variance) for temperature-0 configurations only, reading the temperature from what the adapter sends; any other setting needs five repetitions per task, and a floor report states its temperature, so a user-set value would have to be recorded in the trail. User-set sampling contradicts no ADR, since ADR 0028 admits other temperatures at five repetitions per task, so it is deferred, not excluded. A task measured at that bar showing a gain at another setting reverses this: `temperature` and `top_p` per model under the user's own `[[providers]]` entries, recorded in the trail beside the model (an ADR 0024 amendment and a trail field under ADR 0007), and this row is amended in place. |
| 2.17 classifier models | excluded (#66, answered 2026-09-28: "Keep all excluded"; recorded by #325). Accepted (owner-directed): #66's answer holds for pi 1.0.2's classifiers, the session's choice under the owner's 2026-10-04 delegation. CHANGED in the inventory, not NEW: split out of 2.14. Classifier models answer typed choice, yes-or-no and score questions with probabilities; Jev classifiers are usable from codemode scripts and llama.cpp (0.99.0), plus Cloudflare Clef (1.0.1) (inventory 2.17: ca/docs/models.md:129-138; ca/CHANGELOG.md:171, :30). The coding-loop use upstream now gives a classifier is choosing the model per request, which row 2.19 excludes. `trellis ask` classifies its input with the chat model itself (crates/trellis-cli/src/ask.rs), a different mechanism that this row does not touch. |
| 2.18 image generation models | excluded (#66, answered 2026-09-28: "Keep all excluded"; recorded by #325). Accepted (owner-directed): #66's answer holds for pi 1.0.2's image models, the session's choice under the owner's 2026-10-04 delegation. CHANGED in the inventory, not NEW: split out of 2.14; `ModelRuntime.generateImages()` (0.99.0) and codemode's `models.generateImages()` (1.0.0) (inventory 2.18: ca/docs/models.md:166; ca/docs/codemode.md:165; ca/CHANGELOG.md:186, :75). A generated image feeds no step an executed check can ground (ADR 0004). Image input to a hosted model is a different row (1.14, #53). |
| 2.19 virtual models | excluded (ADR 0043; recorded by #325). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. Experimental in pi 0.99.0: an extension registers a virtual model with `pi.registerVirtualModel()`, and it routes each request to a physical model and thinking level (inventory 2.19: ca/docs/virtual-models.md:37; ca/CHANGELOG.md:178). Within a run trellis never changes the model. Between runs a person picks it: `trellis run --model`, `/model` in `trellis session` ("set once with `/model`, used by every task", crates/trellis-cli/src/session.rs:13), or `set_model` over `trellis rpc`, which sets the model for later prompts (crates/trellis-cli/src/rpc.rs). The one switch trellis would make without a person choosing it is ADR 0043's frontier escalation, policy only and not built, whose trigger is an executed check: "a failing `Verdict`, never the model's claim" (ADR 0043 decision 3), and whose "Alternatives rejected" refuses a trigger the model chooses. A router that picks the model from a model's reading of the request is that refused trigger. The owner wanting routing between configured models by fixed rules only, with no classifier and recorded in the trail (amending ADR 0043), or reversing #66 so a classifier chooses the route (also amending row 2.14), reverses this and amends rows 2.17-2.19 in place. |
| 2.20 credential commands | built in effect: `trellis doctor` prints whether the configured key variable is set, never its value (`hosted::doctor`, crates/trellis-cli/src/hosted.rs:399-406). Printing a key, pi's `pi auth print-api-key` and `print-bearer-token`, is not built: a key is read from the environment and never written out (ADR 0010) (inventory 2.20: ca/docs/cli.md:281-298). |
| 2.21 list models | built: `trellis model` lists the catalog, and `/models` in `trellis session` lists the catalog and the configured specs (crates/trellis-cli/src/session.rs). pi's is `--list-models [search]` (inventory 2.21: ca/docs/cli.md:73). |
| 4.11 messages from extensions | deferred: a hook cannot add a message to a run while it is going; if one is added, the steering channel (ADR 0034) is its carrier. pi's `sendMessage`, `sendUserMessage` and `appendEntry` (inventory 4.11: ca/src/core/extensions/types.ts:443). |
| 4.15 event bus | excluded: hooks are separate processes with no shared runtime to carry events between them (ADR 0023). pi's `pi.events` (inventory 4.15: ca/docs/extensions.md:86). |
| 4.21 provider stream event | excluded under row "4 remaining events": trellis does not stream hosted replies (#151, declined 2026-09-28), so there are no provider stream events to observe. pi's `provider_stream_event` (inventory 4.21: ca/docs/extensions.md:107). |
| 4.22 tool renderer resolver | excluded, TypeScript-runtime specific, under row "4 remaining events" and in the itemised list. pi's `pi.registerToolRenderer()` (inventory 4.22: ca/docs/extensions.md:190). |
| 4.23 nested tool calls | excluded (row 1.7, #63; recorded by #324). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. pi 0.99.0's `ctx.executeTool(name, args)` lets an extension's tool run another tool through validation and the `tool_call` handlers (inventory 4.23: ca/docs/extensions.md:148; ca/CHANGELOG.md:176). A tool call that makes tool calls is several calls in one turn, which row 1.7 excludes: ADR 0023's gate log, the trail's actions, the rollback points and S5's write/check serialization assume the loop's one `Request::Call` per turn (ADR 0005). A custom tool is a command that returns text (`ToolResult::Output`, crates/trellis-loop/src/tools.rs:31-35), and the tool set is fixed at run start with a call outside it refused (crates/trellis-loop/src/registry.rs:1-11), so a custom tool has no route back into the registry. The owner reversing #63 reverses this, as row 1.7 sets out. |
| 4.24 structured tool results | excluded (row 1.7, #63; recorded by #324). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. pi 0.99.0 lets a tool declare an `outputSchema` and return matching `structuredContent`, and return `isError` results (inventory 4.24: ca/docs/extensions.md:146; ca/CHANGELOG.md:176, the same entry that adds exposure modes beside codemode, 17.5). A structured result is for a caller that is a program, which in pi is a codemode script (17.1; inventory 1.5 gives `bash` its structured results "for codemode scripts") and here would be one, excluded under row 1.7. trellis's tool set is fixed at run start (crates/trellis-loop/src/registry.rs:1-11), and every command's result, a custom tool's included, is text for the model to read (`ToolResult::Output`, crates/trellis-loop/src/tools.rs:31-35). The owner reversing #63, and codemode with it, reverses this. |
| 8.9 session picker | excluded, cosmetic (row 11): `trellis log` lists runs and `trellis resume <run_id>` opens one (ADR 0038). pi's picker searches, renames and deletes sessions (inventory 8.9: ca/docs/sessions.md:18). |
| 9.7 fork and rebrand | built in effect: a fork names the executable in `[[bin]] name` (crates/trellis-cli/Cargo.toml), and `TRELLIS_CONFIG_DIR` relocates configuration without a fork (crates/trellis-cli/src/user_config.rs:36-47). The `TRELLIS_` environment variable names are written out in the code, not derived from the product name as pi's are from `piConfig` (inventory 9.7: ca/docs/cli-integration.md:82-95). |
| 10.3 self-update and install channels | excluded: a version check is a networked component (brief Part 1); trellis is built from source with `Cargo.lock` as the pin, and row Apex 38–42 already excludes self-update. pi's installer, `pi update`, npm and a Nix flake (inventory 10.3: ca/docs/quickstart.md:12-29). |
| 11.9 startup header | excluded, cosmetic (row 11), in the itemised list. pi's `quietStartup` (inventory 11.9: ca/docs/settings.md:93). |
| 11.10 status footer | excluded, cosmetic (row 11), in the itemised list; `trellis tui` shows the model and tier in its status line and the run's budget beneath it (crates/tui-demo/src/tui/view.rs). Usage totals are row 14.4 (inventory 11.10: ca/docs/usage.md:9). |
| 11.11 terminal integration | excluded, cosmetic (row 11), in the itemised list: pi's terminal setup, tmux, Termux and Windows guides (inventory 11.11: ca/docs/terminal-setup.md:1). |
| 11.12 pi-tui library | excluded, cosmetic (row 11), in the itemised list; `trellis tui` is its own ratatui view (crates/tui-demo) (inventory 11.12: packages/tui/README.md:3). |
| 12.3 offline mode | built in effect: trellis makes no automatic network request to a remote host; the only connection a person did not ask for is bare `trellis`'s first-run picker probing runtimes on this machine's loopback (crates/trellis-cli/src/first_run.rs, ADR 0050 §2), and `trellis doctor` reads a loopback `/v1/models` only when run. `--no-default-features` builds a binary without TLS (crates/trellis-cli/Cargo.toml `default = ["hosted"]`; ADR 0024 §1). pi's `--offline` and `PI_OFFLINE=1` (inventory 12.3: ca/docs/cli.md:229-230). **Amended 2026-10-07 (#557), owner-directed (owner, 2026-10-07):** the overview's board may start the user's own `gh` CLI, read-only (`gh pr list`, `gh run list`), through `secrets::scrub` with a bounded wait, for PR and CI state, only when it is installed and logged in; trellis makes no network request of its own and passes `gh` no token. Accepted; built 2026-10-07 (#558). **As built 2026-10-07 (#558):** `trellis overview` (crates/trellis-cli/src/overview.rs) starts the first `gh` on PATH (absolute entries only) through `secrets::scrub` with no stdin, as `gh pr list --state open` and `gh run list -L 10`, each bounded to 10 seconds, after which it and what it started are killed; missing, not logged in, timed out or failing, the board says so and the command still exits 0. |
| 13.5 tool annotations | built (#328): a `read_only` fact for built-in tools only. Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. pi 0.99.0 gives every tool `readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint`, so a permission gate can treat read-only calls apart without naming each tool. trellis's `pre_tool` input and `post_tool`'s `call` object now carry `"read_only": true` or `false` for every built-in call, from one table in crates/trellis-loop/src/gate.rs (`READ_ONLY_TOOLS`), inserted in hooks.rs after `call_json` returns (ADR 0023, amendment 2026-10-04). A custom tool's call object gets no key, and `call_json`, which also builds the hosted transcript's tool arguments, is unchanged. Destructive, idempotent and open-world hints are not built. Custom tools declaring `read_only` under trust would reverse the built-ins-only choice. |
| 14.4 usage and cost | prompt and completion tokens per model built in `trellis session` `/status` (#318, `status_to` in crates/trellis-cli/src/session.rs), as two numbers never summed; over `trellis rpc`, `get_session_stats` answers the same per-model totals for the runs that process started (#319, `session_stats` in crates/trellis-cli/src/rpc.rs; row 9.4–9.5). Dollars deferred to v1.x (#67). pi's `/session` and footer show usage and cost (inventory 14.4: ca/docs/usage.md:9, :62). |
| 15.5 pi-client | excluded with row 15 (#65): a client for the experimental service protocol (inventory 15.5: packages/client/README.md:3). |
| 15.6 durable coding agent | excluded: no model-invoked subagent (#130, ADR 0005, ADR 0016). pi's local coding agent on pi-durable, with subagents (inventory 15.6: ca/src/experimental/durable/README.md:3-5). |
| 16.1-16.7 MCP client | excluded (#73, answered 2026-09-28: "Keep all excluded"; recorded by #323). Accepted (owner-directed): #73's answer holds for pi 1.0.2's client, the session's choice under the owner's 2026-10-04 delegation. pi 0.99.0 made an MCP client part of pi: stdio and streamable HTTP servers from `~/.pi/agent/mcp.json` and, after project trust, `.pi/mcp.json`; OAuth; resources; `/mcp` and `pi mcp`; exposure through codemode by default; and the standalone pi-mcp package. The reasons for keeping MCP out are about trellis's trust model and do not change because pi ships a client: a long-lived connection, remote tool schemas the model sees unreviewed, and credentials per server. 16.2 (OAuth and its credential file, `mcp-auth.json`) is excluded under ADR 0010 and #64, and 16.1's streamable HTTP servers are a networked component (brief Part 1). The supported way to reach one stdio MCP server is a `[[tools.custom]]` command (ADR 0023) declared under `.trellis/`: trusted by digest, gated by `[control]`, run in the throwaway clone with keys scrubbed (ADR 0032), returning text (crates/trellis-loop/src/registry.rs:176-191). The owner reopening #73 reverses this: stdio servers only, or streamable HTTP as well, needs a v2.0 brief line and an ADR amending Paperclip 72–75 and ADR 0023, and amends this row in place. |
| 17.1-17.6 codemode, tool exposure and tool search | excluded (row 1.7, #63; recorded by #324). Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation. pi 0.99.0 added a codemode tool whose model-written JavaScript runs in a QuickJS sandbox and calls tools, with only the script's output reaching the model (17.1); a store that keeps small JSON values across calls (17.2); `models.classify()` and `models.generateImages()` in scripts (17.3); `tool_search`, which finds undeclared tools and declares the matches (17.4); exposure modes `direct`, `model-only`, `codemode`, `deferred` and `hidden` with namespaces (17.5); and `prepareLoadout` (17.6) (inventory 17.1-17.6: ca/docs/codemode.md:7, :30, :59; ca/docs/cli.md:170-172; ca/docs/extensions.md:152-164, :180; ca/CHANGELOG.md:175-176). A script that calls several tools is several calls in one turn, which row 1.7 excludes (#63), and codemode would add a JavaScript engine dependency. trellis's tool set is fixed: built once at run start, and a call outside it is refused (crates/trellis-loop/src/registry.rs:1-11), so 17.4-17.6, which change what the model is offered while a run is going, have no place to act. They keep a large tool list, mostly MCP servers, out of the prompt; with MCP excluded (row 16.1-16.7, #73) trellis's list is its 16 approvable built-ins (crates/trellis-loop/src/gate.rs:178) plus a repository's custom tools. 17.2 also needs state carried across calls on a session branch, and the session tree stays deferred (#50, declined 2026-09-28; rows 8.1–8.4). 17.3 is classification and image generation, excluded under row 2.14 (#66). The owner reversing #63 reverses this (an ADR superseding ADR 0005's one call per turn, as row 1.7 sets out), and so does reversing #73 so that a repository's tool list outgrows the prompt (deferred exposure with `tool_search` for custom tools, amending ADR 0023 and the registry's fixed set). |

#### pi 1.0.2: changed upstream, disposition unchanged (2026-10-04)

Added 2026-10-06 (#329). This section adds no disposition and changes no row. The pi 1.0.2 inventory tags 47 items CHANGED since 0.87.1; eight take a new or amended row from another ticket (1.2, 1.7, 2.1, 2.14, 2.17, 2.18, 9.4, 14.3); on 2026-10-06 those for 1.7, 2.14, 2.17 and 2.18 are written, and those for 1.2, 2.1, 9.4 and 14.3 are not yet. **Amended 2026-10-06 (#322):** all eight are now written: 1.2 in row 1.2 (#316), 9.4 in row 9.4–9.5 (#319), and 2.1 and 14.3 as amendments in rows 2.1–2.4 and 14 (#322). For each of the other 39 the upstream change leaves the pi row's reason standing, on the reason given here, so a disposition that was re-read against pi 1.0.2 can be told from one that was not. A row named here is as written in the table above; PI-03 is #323, PI-04 is #324, PI-05 is #325 and PI-11 is #328.

| Item (`inventory-pi.md`) | Why the disposition stands |
|---|---|
| 1.4 Tool selection flags and defaultTools | `[tools] enable` and `disable` (`ToolsSection`, crates/trellis-cli/src/config.rs) do what pi's +name and -name entries do; `[tools]` comes from the repository alone and is not layered over the user file (ADR 0025 §7; `layer()` in crates/trellis-cli/src/user_config.rs), so pi's project-over-user merge of those entries has no counterpart; each run builds its tool set at start (crates/trellis-loop/src/registry.rs), so nothing needs `/reload` |
| 1.5 Output truncation | the model-facing cap is unchanged (2,000-character tail, ADR 0019); the new 1 MiB structured result serves codemode (row 17.1-17.6, PI-04, #324) |
| 1.9 Retries and backoff | upstream fixes only (0.99.2, 1.0.1); `retry_delay` (crates/trellis-model/src/hosted.rs) honours `Retry-After` in seconds or as an IMF-fixdate HTTP date (#149, ADR 0024 §4), and an unparsed value, the RFC 850 and asctime forms included, falls back to the 1, 2, 4 s backoff, as upstream's 0.99.2 fix does; the W-C bullet under "Where this stands" that lists a dated `Retry-After` as not built predates #149 |
| 2.2 Login | stays excluded (ADR 0010, #64); the new login flows still store credentials |
| 2.4 Cloud platforms | upstream fixes only; request signing stays deferred (#45) |
| 2.6 models.json endpoints and overrides | `[[providers]] input_bytes` built (#162); typed classifier and image entries follow rows 2.17 and 2.18 (PI-05, #325); cache lifetimes and cost tables stay not built (row 2.6) |
| 2.7 Model catalog overlay | built (crates/trellis-model/src/catalog.rs, `trellis model`); the new type filter serves classifier and image models, rows 2.17 and 2.18 (PI-05, #325) |
| 2.8 Thinking levels | built for the levels the two spoken protocols carry (crates/trellis-model/src/hosted.rs); a run uses one `[model] thinking` throughout, so there is no per-message level to record |
| 2.10 llama.cpp router | `trellis model pull` built (ADR 0044); router management stays deferred (#45); its new classifier listing follows row 2.17 (PI-05, #325) |
| 2.11 Radius gateway | stays excluded, a networked service (ADR 0010, #65); its reach into login, MCP and sharing falls under the same clauses and #73 |
| 2.12 Custom providers | `[[providers]]` built (ADR 0024 §2); code-defined providers need an in-process runtime trellis does not have (ADR 0023) |
| 3.4 Sectioned system prompt | built (fenced sections, crates/trellis-loop/src/adapter.rs); the new mcp_servers section follows the MCP client (row 16.1-16.7, PI-03, #323) |
| 4.1 Extension loading | processes exchanging JSON in place of an in-process runtime (row "4 extensions"); the new built-in extensions are MCP, llama.cpp, codemode and tool search (PI-03 #323, #45, PI-04 #324) |
| 4.8 Tool events | built (`pre_tool` gates, `post_tool` observes; ADR 0023 §5); nested calls and MCP calls through them follow rows 4.23 (PI-04, #324) and 16.1-16.7 (PI-03, #323) |
| 4.13 Tools and commands methods | the tool set is fixed at run start (crates/trellis-loop/src/registry.rs); exposure reporting follows row 17.1-17.6 (PI-04, #324) |
| 4.16 Extension context | hooks get JSON on stdin (ADR 0023 §5); `executeTool` and `tools` follow row 4.23 (PI-04, #324) |
| 4.18 Extension UI context | excluded, cosmetic, per the itemised list; desktop and web surfaces stay excluded (#70) |
| 4.19 Tool definition fields | the new fields are items 4.24, 13.5, 17.5 and 17.6, recorded in rows "4.24 structured tool results" and "17.1-17.6 codemode, tool exposure and tool search" (PI-04, #324) and "13.5 tool annotations" (PI-11, #328) |
| 6.1 Prompt templates | built, every substitution form including `${@:-d}` and `${@:N}` (crates/trellis-cli/src/templates.rs); `trellis run --prompt <name>` covers `--prompt-template` |
| 8.1 Session tree storage | stays deferred (#50) |
| 8.5 Auto-compaction | built, elision keyed to `[model] context_bytes` (ADR 0024 §5, ADR 0033); per-model limits are `[[providers]] input_bytes` (#162); routed-model windows follow virtual models (row 2.19, PI-05, #325) |
| 8.8 Export and share | built, `trellis log --json` (crates/trellis-cli/src/log_cmd.rs); HTML stays excluded (#143) and share excluded (#65) |
| 9.1 Interactive and print modes | built (`trellis run`, `trellis session`, `trellis tui`); `--export` renders a session as HTML, excluded (#143) |
| 9.3 Event types | `--json` carries trellis's own tracing events and framing records, not pi's vocabulary (ADR 0025 §4) |
| 9.6 SDK | built in effect (the crates are libraries); the new extension factories serve MCP, codemode and tool search (PI-03 #323, PI-04 #324) |
| 10.1 Package commands | `trellis install` copies skills and prompt templates (ADR 0039), which declines packages that carry code |
| 10.2 Package manifest and filters | only `skills/<name>/SKILL.md` and `prompts/<name>.md` are copied (crates/trellis-cli/src/install.rs; ADR 0039) |
| 11.1 Fullscreen mode | excluded, cosmetic, per the itemised list; `trellis tui` is its own full-screen view |
| 11.4 Themes | excluded, cosmetic, per the itemised list |
| 11.7 Inline images | excluded, cosmetic, per the itemised list |
| 12.1 Settings locations and layering | built (crates/trellis-cli/src/user_config.rs; ADR 0025 §7); the settings pi made user-only (MCP, trust, cache warming) each have their own row |
| 12.2 Environment variables | `TRELLIS_CONFIG_DIR` and the platform fallbacks (crates/trellis-cli/src/user_config.rs); the pi variables added name features excluded or absent here |
| 13.1 No sandbox and no per-call approval | trellis is stricter: the control dial (ADR 0018), `bash` off by default (ADR 0023 §3), the throwaway clone (ADR 0032) |
| 13.2 Project trust | built (crates/trellis-cli/src/trust_cmd.rs, crates/trellis-loop/src/trust.rs; ADR 0023 §4); trust-gating `mcp.json` follows the MCP client (row 16.1-16.7, PI-03, #323) |
| 14.1 Telemetry | `tracing` is the one telemetry path; no network telemetry (brief Part 1) |
| 14.2 Agent-core hooks | built (crates/trellis-loop/src/orchestrator.rs and gate.rs); upstream's removal of the agent-core harness changes nothing here |
| 15.1 chord | stays excluded with row 15 (#65) |
| 15.2 pi-protocol | stays excluded with row 15 (#65) |
| 15.3 pi-server and experimental client and server commands | stays excluded with row 15 (#65) |

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
| 15–19 layout, canvas, labels, annotations | excluded [ui]. **Amended 2026-10-10 (#315):** the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) draws no vault graph, so it has no layered layout or grid mode (15), no large-graph mode (16), no label tiers (17) and no vault canvas (18). Its vault view (`desktop/ui/vault.js`, from the `vault_search` and `vault_graph` answers `desktop/src/state/vault.rs` types) shows a note's lineage, children and references in and out as text lists, read-only. Its two SVG pictures are the run's action graph (`desktop/ui/graph.js`) and the codebase view, whose drill-down, pan, zoom and cap of six labels per package (laid out in `desktop/src/codebase.rs`, drawn by `desktop/ui/codebase.js`) work over trellis's module graph from `get_codebase`, not the vault; which labels show is fixed by the core and does not change with zoom. Annotation drawing (19) also stays excluded under row 12: it writes `annotations.json`, which trellis never opens (`crates/trellis-vault/src/edit.rs`), and the desktop draws none. The disposition stays excluded [ui]. |
| 20, 22–26 create, auto-create linked, retitle with link rewrite, reparent, link, delete | W-A as `trellis vault` subcommands; delete is a file removal (git is the undo; no OS trash). **Amended 2026-09-29 (#61):** 22, auto-create linked, is excluded, for ADR 0022 §7's reason: Apex's trigger is an editor autosave seeing a newly typed `[[X]]`, a command line has no such event, and doing it on `check --fix` would create a note for every unresolved link. `trellis vault new` is the explicit form. |
| 21, 27–29 editor, copy/paste, drop import, undo | excluded [ui] |
| 30 BM25 search | W-A (`trellis vault search`) and W-B (a `search_notes` tool) |
| 31 workspaces / project create | W-A (`trellis vault init`); tabs excluded [ui]. **Amended 2026-10-06 (#359, re-inventory 2026-10-04):** Apex's project create (src-tauri/src/lib.rs:290-313) stays W-A as `trellis vault init`; tabs, recent projects, the folder rename and the location chooser are rows 44 to 47. |
| 32–34 validation status, live sync, size warnings | W-A (`trellis vault check` with the Apex report line and size warnings); live sync excluded (no long-lived process) |
| 35–37 writing skill, hierarchy prompt, AGENTS.md | W-A ships the writing contract as a built-in skill; the prompt is excluded [ui]. **Amended 2026-10-06 (#359, re-inventory 2026-10-04):** the AGENTS.md item (Apex's AGENTS.md:1-60) is not W-A's: AGENTS.md reaches the model as a context file under ADR 0025, and trellis ships no Apex rules of its own (crates/trellis-cli/src/context_files.rs). The writing skill is built in effect (owner-directed, 2026-10-04): Apex's SKILL.md (skills/apex-notes-writing/SKILL.md at `f449644a`) loads unchanged when copied into `.trellis/skills/` or read from an installed package's `skills/` directory (ADR 0039 §10), held by the tests apex_s_writing_skill_loads_unchanged_from_the_repository and apex_s_writing_skill_loads_unchanged_from_an_installed_package in crates/trellis-loop/src/skills.rs (#360); as ADR 0022 §6 says, trellis carries no skill in its binary, and the row was settled as the session's choice under the owner's 2026-10-04 delegation. |
| 38–42 build, packaging, native, self-update | excluded [ui/desktop]. **Amended 2026-10-10 (#315):** the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) has no bundler and no npm: its page is static HTML, CSS and ES modules in `desktop/ui/` (38; ADR 0048 section 5, held by `no_dev_url_and_no_build_hooks` in `desktop/src/lockdown.rs`). It builds from source with cargo for x86_64 Windows and Linux, the two targets `desktop/deny.toml` names, with `bundle.active` false in `desktop/tauri.conf.json`, so there is no installer, signing or notarisation, and no macOS target (39). The `desktop` job in `.github/workflows/ci.yml` builds, lints and tests it on Windows and Linux, and `release.yml` builds and ships only `trellis-cli` (40). It has no Tauri plugin (the `desktop-plugin` rule in `scripts/checks/desktop-guards.sh`), so no folder picker, native dialog or opener: a repository is opened by its typed path, `--root`, the current directory or the overview's recent list; the approval dialog is an HTML `dialog`; the page has no link; and `desktop/src/lockdown.rs` refuses navigation off the app's own origin and every new window (41). It has no updater: it is rebuilt with cargo, as `desktop/README.md` says (42). The disposition stays excluded [ui/desktop]. |
| 43 path safety for workspace files | built (W-A), shown by the a_symlinked tests in crates/trellis-vault (tests/parity.rs): a symlinked notes/, manifest.json, layout.json or dates.json is refused, as Apex refuses a symlinked notes/ (src-tauri/src/lib.rs:236-243) and its workspace metadata; see the ADR 0022 §1 and §3 amendments (#353). (re-inventory 2026-10-04) |
| 44–47 workspace tabs, recent projects, project folder rename, project location chooser | excluded (#70): a command line opens one repository's vault, and row 31's `trellis vault init` is the built part (web/appShell.ts:1102-1222, 1346-1380, 1058-1131, 1382-1537). (re-inventory 2026-10-04) **Amended 2026-10-07 (#557), owner-directed (owner, 2026-10-07):** the recent list with each repository's last outcome, which `trellis overview` prints and the desktop shows, is a launcher list, not cross-repo analysis; nothing is compared or analysed across repositories. Workspace tabs, the folder rename and the location chooser stay excluded (#70). Accepted; built 2026-10-07 (#558). **As built 2026-10-07 (#558):** `trellis overview` prints the start screen's recent list, each repository with its newest trail's outcome (ADR 0050 section 3, as built); nothing is compared across repositories. The desktop reads it (#296); drawing it is #562's, not built. |
| 48–49 paste plain text as a note, editor link rendering | excluded [ui], as rows 21, 27–29 (web/notes/noteComposer.ts:67-96, web/appShell.ts:179-298). (re-inventory 2026-10-04) |
| 50 per-line date labels and the note date header | excluded [ui]; trellis vault dates prints each note's created day and matching line count (web/ui/dateDecorations.ts:1-281). (re-inventory 2026-10-04) |
| 51 date tracking that never blocks Markdown | built (W-A), shown by the after_edit tests in crates/trellis-vault (src/dates.rs): a vault edit lands and exits 0 when dates.json cannot follow it; see the ADR 0022 §5 amendment (#354). (re-inventory 2026-10-04) |
| 52 search result ranking layer | not built, per ADR 0022 §4, and porting it is ticketed: the library port and the switch of `trellis vault search` to it, which amends this row when it lands (web/searchResults.ts:1-255). (re-inventory 2026-10-04) **Amended 2026-10-06 (#355, #362):** built: trellis vault search ranks with the ported layer, trellis_vault::search's search_results, shown by the vault_search tests in crates/trellis-cli (src/vault_cmd.rs) and search_results_match_apex_vectors; see the ADR 0022 §4 amendment. search_notes keeps the index ranking (ADR 0024 §6), and the porting question was answered under the owner's 2026-10-04 delegation. (re-inventory 2026-10-04) |
| 53–55 rope connect gesture, hub and PageRank scoring, workspace chrome | excluded [ui], as rows 15–19 (web/appShell.ts:5883-5920, web/labelVisibility.ts:19-234, web/appShell.ts:443-637). (re-inventory 2026-10-04) |

## Paperclip (`inventory-paperclip.md`)

| Items | Disposition |
|---|---|
| 1–3 multi-company, company lifecycle, settings | excluded [db][multi-user]: one repository is one trellis "company" |
| 4 reporting tree | W-E (`reports_to` on agent profiles; no cycles) |
| 5–6 roles, members | excluded [multi-user] |
| 7 agent permissions | W-E (per-profile `[control]`, tool enable list, `deny_write`). Corrected 2026-10-04: a profile also has its own write scope, `allow_write`, which only narrows what the repository allows (#182; ADR 0026 §4). |
| 8–9 team catalog, built-in agents | deferred. Corrected 2026-10-04: a starter catalog is built as copy-in profiles under `examples/agents/` (`implementer`, `test-writer`, `docs-writer`; #55, #183), which a person copies into `.trellis/agents/`; nothing is built in. |
| 10–11 16 external adapters, ACP | W-E adds one `process` profile: runs any command in the worktree, then trellis's checks decide. The adapters themselves are excluded because each one wraps a third-party agent. **Amended 2026-10-06 (#350), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** A process profile may speak the Agent Client Protocol (ACP), one JSON-RPC protocol over stdio that many agents speak, with trellis as the client, held to the bounds ADR 0026 §11's note of 2026-10-06 records; the per-agent adapters stay excluded. Not built: the build, with its own ADR mapping the protocol's messages inside those bounds, is a separate ticket. |
| 12, 17–18 external adapters as plugins, sandboxes | excluded [server]; sandbox deferred |
| 13 lifecycle pause/resume/terminate | W-E (profile `paused`) |
| 14 instruction bundle | W-E (profile `instructions` file) |
| 15 config revisions | built in effect (profiles are files in git) |
| 16 environments | deferred |
| 19–20 task fields and statuses | W-E (priority, labels, goal = vault stem, parent task, assignee profile, blockers, status set) |
| 21–22 atomic checkout, execution lock | built (ADR 0020 hard-link claim) |
| 23–24 blockers, sub-issues waking parents | W-E |
| 25 comments | W-E (a notes log per task; `@profile` mentions are deferred). Corrected 2026-10-04: an `@profile` mention in a person's note wakes that profile's task on the next `work` tick (#57; ADR 0026 §1). |
| 26–28 documents, attachments, work products | deferred; the trail and the diff are the work product. **Amended 2026-09-29 (#56):** excluded, for the same reason: the trail and the diff are the work product. A task's `focus` note (ADR 0017) and files in the repository cover input documents, and copies stored in the git dir would be lost with a linked worktree (ADR 0020 §1) and not versioned by git. |
| 29 labels | W-E |
| 30, 34 interactions, decisions | built in part (ADR 0026 §1, #140): a queued task's run may end on an `ask_human` question with up to 8 options; the task waits `blocked` (reason `question`) and a person answers with `queue note` and `queue set --status todo`. A structured decision record is deferred |
| 31, 37–38 work modes, cases, feedback training | deferred. **Amended 2026-09-29:** 31 work modes excluded (#141): a proposed breakdown is model text, and the human files tasks with `queue add --parent`; a mode whose output filed tasks would let the model create its own work queue with no human step. 37 cases and pipelines excluded (#58): a case is a parent task with blockers (rows 23–24), and a second hierarchy beside parent tasks would be a second source of truth for the same ordering. 38 feedback votes excluded until something reads them (#142): a vote nothing consumes is a record without a use, and feeding votes into prompts would make human approval read as evidence of correctness (ADR 0004). |
| 32 execution policy (review stage) | W-E (`needs_approval` on a task: `work` does not claim it until `trellis queue approve`) |
| 33 watchdogs | built in part (stale claims listed); deferred beyond that. Corrected 2026-10-04: a heartbeat can also release a stale claim, opt in with `[work] stale_claims = "release"` (#59; ADR 0020). |
| 35–36 attention feed, inbox policy | excluded [web-ui][multi-user]. **Amended 2026-10-10 (#315):** the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) has no attention feed and no inbox. Its board (`desktop/ui/queue.js`, from the `get_queue` answer `desktop/src/queue.rs` types) lists each task in the order `get_queue` gives, with its status, priority, text, labels, assignee, parent, blockers and waiting reason, read-only, under one count per status and no sum of any; a task blocked on its run's `ask_human` question shows that question, cut short, in its waiting reason, which names `trellis queue note` and `trellis queue set` as the way to answer (`Blocked::describe` in `crates/trellis-queue/src/lib.rs`). Its approval dialog (`desktop/ui/dialogs.js`) shows the open `approval_request` of the run in flight. Ranking, dismiss, snooze, unread state and tabs are not built, and the [multi-user] clause stands for an inbox policy per person (36). The disposition stays excluded [web-ui][multi-user]. |
| 39–40 projects, goals with ancestry | goals: built (vault lineage, ADR 0020 §6); projects excluded (one repository) |
| 41 wake triggers | W-E: timer (`work --every`), routine, blockers resolved, approval; mention deferred. Corrected 2026-10-04: a mention wakes too (#57; ADR 0026 §1). |
| 42–46 DB queue, scheduler, runs, protocol | built locally (ADR 0020); coalescing → W-E (a routine never enqueues a duplicate while one is pending) |
| 47–49 routines | W-E (5-field cron with timezone, the coalesce/skip/always and skip/catch-up policies, `{{var}}` templates); webhook and API triggers excluded [server]. Corrected 2026-10-04: the timezone may be an IANA zone name as well as UTC or a fixed offset (#47; ADR 0026 §7). |
| 50–54 costs and budgets | W-E, **in tokens** from the trail, with monthly and lifetime windows, an 80% warning and a hard stop that pauses the profile. Dollars are excluded until COST_MODEL.md prices them. **Amended 2026-09-29 (#67):** dollars deferred to v1.x. Runs stay token-only through v1.0. No dollar figure is shown until COST_MODEL.md has a sourced per-model price table and prompt tokens are counted; when dollars arrive they sit beside tokens and never replace them. Corrected 2026-10-04: prompt tokens are counted, in the ledger as a second number beside the completion tokens and never added to them (#108; ADR 0026 §5). Dollars stay deferred. |
| 55–57 approvals, board | W-E (task approval and budget override via `trellis queue approve`); hire and strategy approvals excluded [multi-user] |
| 58 trust presets | deferred |
| 59–63 execution workspaces, dev servers, previews | deferred to v1.1 (parallel worktrees are ADR 0016 territory); previews excluded [server]. Corrected 2026-10-04: a profile's `[[services]]`, such as a dev server, start beside each task it runs (#60; ADR 0026 §12); a per-task worktree is still not built. **Amended 2026-10-06 (#332), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** Paperclip has since added `enableIsolatedWorkspacesByDefault` (packages/shared/src/validators/instance.ts:46), which makes an isolated git worktree a task's default workspace, and `enableWorktreeRunExecution` (:88). A worktree per queued task is v1.1 scope, and is not built: `work` still runs every task in the one worktree it was started from and stops at the first converged change (ADR 0020 §5), and none of #98–#101 changes that (the note above). The build is a ticket of its own, not yet filed, blocked by #98–#101 (#101 is still open), under its own ADR, which follows these rules. Each claimed task runs in a linked worktree of its own on branch `trellis/task/<id>`, created with the mechanics ADR 0037 uses for a partition's (a `trellis_vcs::Worktree` branched from the base commit, created one at a time). An unconverged task's worktree is removed after its rollback; a converged task's stays, with its change, for a person. `work` goes on to the next task past a converged one. Merging is the person's, and trellis never pushes; row 63's previews stay excluded [server]. The queue stays in the git directory of the worktree `work` was started from (ADR 0020 §1), and a task's linked worktree has no queue of its own. Each task's worktree is clean before its run (ADR 0009). For a task run in its own worktree, the build's ADR replaces ADR 0020 §5's rule that `work` stops at the first converged change. Reversed if #98–#101 do not land, or the build's ADR finds that a run in a task's own worktree cannot keep ADR 0009's snapshot and rollback: then a worktree per task is deferred to v1.1 with no tracking issue. **Amended 2026-10-12 (#624):** the build's ADR is ADR 0053, which records the rules above and settles the rest: the worktree at `.trellis/work/task/<id>/`, a dirty task worktree, an id that is not a branch name, or an existing `trellis/task/<id>` refused after the claim with the task put back untouched and `work` exiting 1; an unconverged task's branch kept when it holds the trail commit; trust looked up under the starting worktree; `--max-tasks` unchanged. Built in #709 (`trellis-vcs`) and #710 (`work`), filed from ADR 0053. |
| 64–67 skills import, catalog, injection modes, policy | built (ADR 0019); per-profile skill allow/deny → W-E |
| 68 Skill Studio | excluded [web-ui]. **Amended 2026-10-10 (#315):** the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) has no skill view: its composer's starters are the prompt templates `get_commands` lists, not its skills, and the composer sends no `skill` field. A skill is test-run with `trellis run --skill <name>`, `/skill` in `trellis session`, or the `skill` field of rpc's `prompt` (ADR 0025 §5), and a repository skill's versions are its git history; an installed one is pinned to a ref and a commit (ADR 0039). The disposition stays excluded [web-ui]. |
| 69–71 secrets | excluded: ADR 0010 |
| 72–75 connectors, tool gateway, MCP | excluded: brief v2.0 ("MCP exposure") and [server]. **Amended 2026-10-04 (#316):** the owner's answer of 2026-09-28 keeps this excluded for consuming MCP as well as exposing it (#73), until a v2.0 brief; trusted custom command tools (W-B) give the model external tools. |
| 76–82 plugins | excluded [server][web-ui]; the executable hooks of W-B are the local extension point |
| 83–84 activity log, export | W-E (JSONL activity log in the git dir; `trellis queue log`) |
| 85–87 portability | W-E (`trellis queue export/import` of profiles, routines and tasks to a folder; nothing secret exists to scrub) |
| 88–93 identity, deployment, database, storage | excluded [server][db][multi-user] |
| 94–96 CLI | built in effect: the local subcommands; `--json` → W-D |
| 97–98 REST API | excluded [server] |
| 99–100 UI | excluded [web-ui]. **Amended 2026-10-10 (#315):** the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) is one local Tauri window over one `trellis rpc` child per repository, with the six sections its rail lists: Overview, Run, Trails, Codebase, Board and Vault (`desktop/ui/index.html`). Paperclip's views are not built: its dashboard (the Overview section shows recent repositories, the last run, a codebase summary and the board, from `trellis --json overview`), command palette, org chart, work timeline, inbox (rows 35–36), artifacts (rows 26–28), costs in dollars (rows 50–54; the desktop shows token counts only), search across tasks (PC-124; the desktop's search boxes filter trails by task text and search module names and vault notes), agent chat (PC-101), Skill Studio (row 68), mobile layouts (the window's minimum is 860 by 600, `desktop/tauri.conf.json`) and the experimental flags of row 100; nor are locales, since the page is in English only. There is no web UI: `trellis --web` (ADR 0050 section 5) is refused as not built (`crates/trellis-cli/src/main.rs`), and #375, open, builds it. No test yet checks that the page draws every pane the core's view carries (#513, open). The disposition stays excluded [web-ui]. |

### Paperclip re-inventory (2026-10-04)

**Status:** Accepted (owner-directed), 2026-10-04: the session's choice under the owner's 2026-10-04 delegation. Added 2026-10-06 (#335). The owner can override any row (Context).

Rows for PC-101 to PC-144, the capabilities `docs/inventory/inventory-paperclip-20261004.md` lists at Paperclip commit 1c07b590 (2026-10-04, #15100) that the v2026.916.1 inventory has no row for. Eleven arrived upstream after v2026.916.1 (PC-105, 107, 108, 117, 118, 119, 121, 122, 138, 140, 142); the other 33 were missed by that inventory or folded into another of its rows (the native runner, PC-109, is split out of row 10). The last column is the upstream paths the addendum gives for each row, relative to the Paperclip repository at 1c07b590. A sentence beginning `S17 question:` names a question another ticket of story S17 answers; that ticket replaces the sentence. No row adds CLI surface, a dependency, or a profile or routine field.

| ID | Capability | Disposition | Clause | Upstream paths (inventory-paperclip-20261004.md) |
|---|---|---|---|---|
| PC-101 | Agent Chat | built in part | `trellis session` is the conversation (ADR 0025); a persistent conversation per agent is deferred with pi rows 8.1–8.4; a chat that files tasks for the model is excluded by row 31 (#141). **Amended 2026-10-06 (#339), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** PC-101: a persistent conversation per agent profile is excluded, where this row deferred it with pi rows 8.1–8.4. Each task is a fresh run with a fresh budget and snapshot so that it can roll back to the state it started from (crates/trellis-cli/src/session.rs), and a chat that files tasks is the model creating its own work, which row 31 excluded (#141). In its place: the interactive session is the conversation (ADR 0025), `trellis queue add` is the hand-off a person makes, and `trellis resume <run_id>` carries a finished run's context forward (ADR 0038). Reversed if pi's session tree (rows 8.1–8.4) is built as v2 memory work: a per-profile transcript then belongs on a branchable session, still with no task filing. | server/src/services/agent-conversations.ts; server/src/services/chat-completion-delivery.ts; packages/db/src/migrations/0288_glorious_jamie_braddock.sql |
| PC-102 | Board concierge chat, Conference Room | excluded [web-ui] | Brief Part 1, "any surface beyond the CLI" (#70). | server/src/routes/board-chat.ts; packages/shared/src/feature-catalog.ts:165 |
| PC-103 | Chat channel connectors | excluded | Brief Part 1, "any networked component"; as rows 72–75 (#73). | server/src/routes/chat-channels.ts; server/src/services/chat-channels.ts; doc/connections/CHAT-CONNECTOR-UX.md |
| PC-104 | Email channels | excluded | Brief Part 1, networked. | server/src/routes/email.ts; server/src/services/email-channels.ts |
| PC-105 | GitHub review bots | excluded | Brief Part 1, networked; rule 5 (ADR 0005) where the reviewed change is agent-written. | doc/connections/GITHUB-REVIEW-BOT.md; server/src/services/chat-github-reviews.ts; server/src/services/chat-github-review-policy.ts; packages/db/src/migrations/0283_jittery_psynapse.sql |
| PC-106 | AI connections | excluded | ADR 0010 (the user's key never touches infrastructure trellis controls) and ADR 0024 §3 (only the user chooses where a key is sent); OAuth and device login are excluded (pi rows 2.1–2.4). | doc/connections/AI-CONNECTIONS.md; server/src/routes/ai-connections.ts; server/src/services/ai-connections.ts; server/src/services/device-login-service.ts |
| PC-107 | Browser Use Cloud | excluded | Brief Part 1, networked. | doc/connections/BROWSER-USE.md; server/src/routes/browser-use.ts; server/src/services/browser-use.ts; packages/db/src/migrations/0290_browser_use_cloud.sql |
| PC-108 | Memory connectors | excluded | Brief Part 1, networked; trellis's memory is the trail (ADR 0007) and the vault (ADR 0017). | doc/connections/MEMORY.md; doc/memory-landscape.md; packages/shared/src/validators/instance.ts:56 (`enableMemoryConnectors`); server/src/services/cognee-connection.ts |
| PC-109 | Paperclip native runner | built in part | Trellis runs its own loop natively and drives another agent as a process profile (ADR 0026 §11); Paperclip's runner protocol (PRP) belongs to its control plane and is excluded with it [server]. **Answered 2026-10-06 (#350; rows 10–11 and ADR 0026 §11), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** A process profile may speak the Agent Client Protocol with trellis as the client, inside ADR 0026 §11's bounds (throwaway clone, scrubbed environment, no `fs` or `terminal` client capability, permission requests to `[control]`, carry-back under trellis's refusals, only executed checks converge); not built. PRP stays excluded. | doc/architecture/paperclip-runner.md; packages/paperclip-runner/; server/src/services/native-runtime/; server/src/realtime/runner-prp-ws.ts; doc/grok-native-runner.md |
| PC-110 | Durable session goals | excluded | A trellis run ends when an executed check converges it or a budget stops it (ADR 0004, ADR 0008); a goal held by the provider is not consulted, and Paperclip's goals are part of PRP (PC-109). **Amended 2026-10-06 (#340), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** Upstream an active goal keeps a session taking turns after the model would have stopped (SPEC §11.7, `runner-goals.ts`). Here a `done` with no passing executed check ends the run not converged (`StoppedWithoutPassingChecks`, crates/trellis-loop/src/agent.rs; ADR 0004) and the run is rolled back (ADR 0009); keeping a run working after such a `done` is not built. A person carries the work on by putting the task back with `queue requeue` (ADR 0020) or by starting a new run with `trellis run --resume` (ADR 0038). ADR 0043's frontier escalation, accepted and not built, is triggered by a failing check and never by the model's claim, so a `done` before any check ran does not qualify for it. A goal that an agent driven as a process profile keeps for itself is that agent's own state, which trellis neither reads nor counts toward convergence (ADR 0026 §11). Reversed if the benchmark protocol (docs/BENCHMARK_AND_EVAL.md) shows runs ending `StoppedWithoutPassingChecks` with budget left often enough that going on would converge them: then answering a premature `done` and going on within the budget, as its own ticket with an ADR 0005 amendment. | server/src/services/runner-goals.ts; server/src/services/runner-goal-control-broker.ts; packages/db/src/schema/agent_task_sessions.ts |
| PC-111 | Completion contracts and status arbitration | built, stricter | Only an executed check converges a run; `Verdict` is sealed in `trellis-checks`; `ToolResult` has no pass variant (ADR 0004). | doc/architecture/native-status-arbitration.md; doc/execution-semantics.md:1397; doc/execution-semantics.md:1492 |
| PC-112 | Durable continuations and scheduled retries | built in part | Attempts are counted per task and `queue requeue` puts a task back by hand; a stale claim returns to `pending/` on its own only under `[work] stale_claims = "release"` (#59, ADR 0020 §5); nothing retries on a schedule. Amended 2026-10-06 (ADR 0020 §5): after 3 stale-claim releases (`MAX_STALE_RELEASES`), a dead holder blocks the task in `pending/` for a person instead of releasing it again, logged as `stale_claim_blocked` (#352). | doc/architecture/durable-continuation-scheduler.md; server/src/services/routable-blocked.ts; server/src/services/run-continuations.ts; releases/beta/v2026.1002.0-beta.0.md:29 (#14046, #13888) |
| PC-113 | Issue tree control | built in part | A subtree hold would be a second state beside `status`. Amended 2026-10-06 (ADR 0026 §3): `queue cancel --children` cancels a task and its pending descendants in one command, best-effort against a claim made while it runs. **Amended 2026-10-06 (#452):** status was `deferred`; built in part: `queue cancel --children` is built (#336), the subtree hold stays deferred. | packages/shared/src/constants.ts:429-436 (`ISSUE_TREE_CONTROL_MODES`); server/src/routes/issue-tree-control.ts; server/src/services/issue-tree-control.ts; doc/composer-stop.md |
| PC-114 | Execution monitors | excluded | Re-checking an external service needs a networked component (brief Part 1); a monitor the agent sets is the model scheduling its own work, which row 31 (#141) keeps with a person; a person's recurring re-check is a routine or `work --every` (ADR 0026 §7). **Amended 2026-10-06 (ADR 0026 §1):** a person may block a task until an instant with `queue set --reason until --at`, after which the first `Queue::reconcile` sets it to `todo` (#341; not built, #342 builds it); a monitor the agent sets stays excluded (#141). | packages/shared/src/constants.ts:527-543 (`ISSUE_EXECUTION_MONITOR_KINDS`); server/src/services/issue-execution-policy.ts; doc/execution-semantics.md:351 |
| PC-115 | Run diagnostics and failure reports | built | The trail (ADR 0007), `trellis log` and `trellis resume <run_id>` (ADR 0038), and the waiting reasons `queue list` prints (ADR 0026 §1). | server/src/services/run-failure-diagnostics.ts; server/src/services/run-failure-report.ts |
| PC-116 | Live events | built | Loop events to the TUI (crates/trellis-cli/src/tui_bridge.rs); JSONL on stdout and rpc events (ADR 0025). | server/src/services/live-events.ts; server/src/realtime/live-events-ws.ts; doc/run-log-events.md |
| PC-117 | Persistent agent files | deferred | Agent-writable files under `.trellis/` are excluded by the rule that model writes never land there (ADR 0023). Amended 2026-10-06 (ADR 0026 §4): a profile may set `memory = "<vault stem>"`, a vault note compiled as standing context for each of its runs and edited only through the normal write path, recorded as the rule a later ticket builds to (#337; not built). | doc/agent-files.md; server/src/services/agent-file-store.ts; packages/db/src/migrations/0287_serious_tinkerer.sql; skills/para-memory-files/SKILL.md |
| PC-118 | Skills created from work | excluded | Model writes never land in `.trellis/` (ADR 0023), where skills live (ADR 0019); a run may write a draft skill elsewhere in the repository for a person to move. | doc/runner-created-skills.md; packages/paperclip-runner/src/protocol-actions/create-skill.ts |
| PC-119 | Skill sources from GitHub | built in part | `trellis install <source>@<ref>`, `--list` and `--remove` import skills from a git source at a pinned ref (ADR 0039); refreshing in place is not built, since reinstalling a name is refused until `--remove` (ADR 0039, "Not built"), so nothing matches Paperclip's refresh that keeps the last good version when it fails. | server/src/services/skill-sources.ts; server/src/services/github-skill-source.ts; server/src/services/skill-source-git-snapshot.ts; packages/db/src/migrations/0291_conscious_secret_warriors.sql; packages/db/src/migrations/0292_powerful_devos.sql |
| PC-120 | Core Paperclip agent skills | built in effect | The loop's prompt and tools carry what Paperclip's skills teach its agents (ADR 0005); trellis ships no built-in skill, and for Apex's writing contract `trellis vault check` prints the report line its SKILL.md asks for instead (ADR 0022 §6). | skills/paperclip/SKILL.md; skills/paperclip-board/SKILL.md; skills/paperclip-converting-plans-to-tasks/SKILL.md; skills/paperclip-create-agent/SKILL.md |
| PC-121 | Runner API escape hatch | excluded [server] | There is no REST API to call (rows 97–98). | doc/runner-api-tools.md; packages/paperclip-runner/src/protocol-actions/search-api.ts; packages/paperclip-runner/src/protocol-actions/call-api.ts; server/src/services/native-runtime/runner-api-rollout.ts |
| PC-122 | Agent personas | excluded, cosmetic | "Cosmetic and [ui] exclusions". | doc/agent-personas.md; packages/shared/src/agent-appearance.ts; packages/db/src/migrations/0280_unique_genesis.sql |
| PC-123 | Onboarding and first task | excluded [web-ui] | Brief Part 1; `trellis doctor` probes the setup. | server/src/services/onboarding-first-task-assets.ts; server/src/services/onboarding-seed.ts; server/src/routes/onboarding-seed.ts |
| PC-124 | Company and task search | built in part | `queue list` filters by status, label, assignee and priority, and `trellis vault search` searches notes (ADR 0022 §4). Amended 2026-10-06 (ADR 0026 §3): `queue list --text` filters tasks by a case-insensitive substring of their id, text, labels or notes. | doc/SEARCH.md; server/src/services/task-search.ts; server/src/services/company-search.ts; server/src/routes/issues.ts:7814 |
| PC-125 | Document annotations | excluded | Documents are excluded (rows 26–28, #56), and annotation editing is [ui] (Apex row 12). | server/src/services/document-annotations.ts; packages/shared/src/document-anchors.ts |
| PC-126 | External objects | excluded | Brief Part 1, networked: status is fetched from remote services. **Amended 2026-10-07 (#557), owner-directed (owner, 2026-10-07):** still excluded as a trellis service; the overview's board may start the user's own `gh` read-only for PR and CI state, and trellis makes no network request of its own (row 12.3). Accepted; built 2026-10-07 (#558; row 12.3's as-built line). | server/src/services/external-objects.ts; server/src/services/github-external-object-provider.ts; packages/shared/src/external-objects.ts |
| PC-127 | Folders | excluded [web-ui] | Labels group tasks (row 29). **Amended 2026-10-06 (#343), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** PC-127: desktop-only; the desktop groups routines by their `[task] labels`, and trellis's routine files (`.trellis/routines/<name>.toml`, read from one flat directory by `load_dir` in crates/trellis-queue/src/routine.rs), `trellis routines list` and the CLI do not change. The view needs a read-only routines listing over `trellis rpc`, a protocol addition with its own ADR, and a desktop ticket; neither exists yet, and until both land routines are grouped nowhere. Routine names and labels are repository text, shown by the desktop as text and never rendered as HTML. Reversed if the desktop's ADR is not accepted or the desktop plan declines a routines view: then excluded [web-ui] beside rows 99–100. | server/src/routes/folders.ts; server/src/services/folders.ts |
| PC-128 | In-app announcements | excluded | Brief Part 1: a networked fetch shown in a UI card. | doc/ANNOUNCEMENTS.md; server/src/services/announcement-feed.ts; server/src/routes/announcements.ts; announcements/current.json |
| PC-129 | Smoke Lab | excluded | It exercises connector paths, which rows 72–75 exclude (#73). | doc/connections/SMOKE-LAB-TUTORIAL.md; server/src/routes/smoke-lab.ts; server/src/services/smoke-lab.ts |
| PC-130 | Decision queues and triage | excluded [web-ui][multi-user] | As rows 35–36. | server/src/routes/decision-queues.ts; server/src/services/decision-queues.ts; packages/shared/src/validators/decision-queue.ts |
| PC-131 | Per-user surfaces | excluded [multi-user][web-ui] | Brief Part 1. | server/src/routes/user-profiles.ts; server/src/routes/sidebar-preferences.ts; server/src/routes/inbox-dismissals.ts |
| PC-132 | Workspace file resources | built in part | `@` file search in `tui` over `git ls-files` (#52) and the `read_file` tool; a file browser is [web-ui]. | server/src/routes/file-resources.ts; server/src/services/workspace-file-resources.ts |
| PC-133 | Hosted agent profiles | excluded | The provider runs the loop on its own infrastructure, so trellis's gate (ADR 0018), write path and rollback never see its tool calls; a provider's CLI can run as a process profile (ADR 0026 §11), with its writes judged at carry-back (ADR 0032). **Amended 2026-10-06 (#340), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** A hosted agent harness (Claude Managed Agents, AWS Bedrock AgentCore) is reached only through a process profile whose command runs in the throwaway clone (ADR 0026 §11, ADR 0032). The command's environment is scrubbed of every hosted key, and any key the harness needs reaches it only through the profile's `pass_env`, which `trellis trust` prints as `receives:` (ADR 0024 §3). The clone contains neither the harness's network calls nor the tool calls it makes on the provider's side, so trellis judges only the files that come back from the clone (ADR 0032, "Limits"). A client in trellis for a provider-side loop stays excluded, since trellis's gate would never see that loop's tool calls. | server/src/routes/managed-agent-profiles.ts; server/src/services/managed-agent-profiles.ts; server/src/routes/remote-agent-profiles.ts; server/src/services/remote-agent-profiles.ts |
| PC-134 | Cloud-managed instances and execution allowlist | excluded | Brief Part 1 (networked, multi-user); ADR 0010. | server/src/routes/cloud.ts; server/src/services/cloud-instance.ts; server/src/services/execution-allowlist.ts |
| PC-135 | Task drain and hot restart | excluded | No daemon (ADR 0020; ADR 0026 §7, #68); Ctrl-C rolls back (ADR 0009). | packages/shared/src/validators/instance.ts:123-144 (`startTaskDrainRequestSchema`); server/src/routes/instance-settings.ts; server/src/services/hot-restart.ts |
| PC-136 | Evals | built in effect | Trellis measures through its benchmark protocol (docs/BENCHMARK_AND_EVAL.md); Paperclip's evals test its own control plane and runner, and a model-graded oracle would be a model verdict (ADR 0004). | doc/evals.md; evals/; packages/paperclip-eval-kernel/ |
| PC-137 | First-party telemetry | excluded | Brief Part 1, networked; `tracing` is the one telemetry path (CLAUDE.md, Style). | server/src/telemetry.ts; doc/TELEMETRY_WORKFLOW.md |
| PC-138 | Per-message run settings, prompt-only task creation | built in part | A model per run (`--model`, session `/model`), thinking levels where the protocol carries them (pi row 2.8, ADR 0024), a profile's model for queued tasks; a model-written task title is excluded: a task is the text a person typed. **Amended 2026-10-06 (ADR 0026 §1):** a queued task may name its own model with `queue add --model <spec>`, used over its profile's model and `work --model` and held to ADR 0024 §3's key rule for a profile's model (#345; not built, #346 builds it). | ui/src/components/task-chat/ComposerRunSettingsPicker.tsx; server/src/services/issue-title.ts; packages/db/src/migrations/0293_broad_rattler.sql |
| PC-139 | Routine activity gate | built | **Answered 2026-10-06 (#348; ADR 0026 §7 amendment of 2026-10-06), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** As Paperclip's `require_external_activity`, a routine may set `activity_gate = "commits"` to skip a scheduled fire when no commit since it last enqueued a task touches a path outside `.trellis/runs/`; any git error fires, `routines run` is not gated, and a routine without the key fires as today (not built; CLIP-05 builds it). **Amended 2026-10-06 (#349):** status was `deferred`; built: a routine's `activity_gate = "commits"` (crates/trellis-queue/src/routine.rs; work.rs reads HEAD and the changed paths through trellis_vcs), shown by the routine_activity_gate_ tests in crates/trellis-queue. | packages/shared/src/constants.ts:642-646 (`ROUTINE_ACTIVITY_GATE_POLICIES`); packages/shared/src/validators/routine.ts; server/src/services/routines.ts |
| PC-140 | Harness permission defaults | built | The control dial, auto or approve with an approve list (ADR 0018); a process agent runs unattended in a throwaway clone (ADR 0032). | server/src/services/native-runtime/native-execution-input.ts:265; releases/v2026.1001.0.md (Breaking Changes, #13686, #13693) |
| PC-141 | Responsible-user attribution, per-person GitHub identity | excluded [multi-user] | Trellis runs as the one local user, with that user's git identity. | doc/execution-github-identity.md; server/src/services/responsible-user-denial-run-outcomes.ts; packages/shared/src/validators/company.ts |
| PC-142 | Queued responses and steering into a running turn | built | `steer` and `follow_up` between loop turns in rpc, tui and session, recorded in the trail (ADR 0034); `ask_human` ends the run and the answer reaches the next one (ADR 0026 §1). **Answered 2026-10-06 (#347; ADR 0026 §1 amendment of 2026-10-06), Accepted (owner-directed): the session's choice under the owner's 2026-10-04 delegation.** #57's mention wakes stay, and every run of a queued task is to be given the notes a person added since its last run, marked by a new task field `notes_seen` (not built; CLIP-03 builds it). Paperclip #14577 stopped mentions starting runs; here a mention counts only in a person's `queue note`, which refuses inside a run (#189), so the agent-to-agent dispatch it withdrew does not arise, and rows 25 and 41 do not change. **Amended 2026-10-06 (#351):** `notes_seen` built: a run of a queued task is given the notes a person added since its last run (Task::input_since_last_run in crates/trellis-queue), shown by the input_since_last_run_ tests in crates/trellis-queue and rerun_input_reaches_a_task_set_back_to_todo_with_queue_set in crates/trellis-cli. | server/src/services/queued-interaction-response.ts; server/src/services/issue-comment-wakeup.ts; server/src/services/issue-queued-comment-queue.ts; doc/execution-semantics.md:425 |
| PC-143 | Agents hiring agents | excluded [multi-user] | Hire approvals are excluded (rows 55–57); a profile is a committed file a person writes (ADR 0026 §4). | skills/paperclip-create-agent/SKILL.md; server/src/services/hire-hook.ts; packages/shared/src/constants.ts:690 (`hire_agent`) |
| PC-144 | Agent delegation | excluded | Row 31 (#141): a model does not file its own tasks without a human step; agents parallelize work, never judgment (ADR 0005); trellis's own parallelism is partitioning by blast radius (ADR 0016, ADR 0037). | doc/execution-semantics.md:273 (`create_task`, `set_dependencies`); skills/paperclip-converting-plans-to-tasks/SKILL.md |

#### Rows 1–100 whose upstream changed

Between v2026.916.1 and 1c07b590 the upstream of these rows changed; each disposition above stands, on the clause given here.

| Rows | Upstream change | The disposition still rests on |
|---|---|---|
| 1–3, 5–6 | Resolver-policy cap, default responsible user, board and bootstrap claim, Owner Instance Admin. | [db][multi-user]: one repository is one company, and one person answers. |
| 10–11 | paperclip_runner on by default for self-hosted instances, ACP engines on more adapters, a shared rich-ACP transport. | The named adapters still each wrap a third-party agent; the S17 question on ACP for process profiles is recorded at rows 10–11. |
| 12, 16–18 | Plugin environment driver, lease policies, kubernetes mode, CreateOS, Daytona hardening. | [server], and the order the owner set on #20: the throwaway clone first (ADR 0032), containers later. |
| 26–28 | Text-attachment tabs, artifact cards, `register_deliverable`. | #56: the trail and the diff are the work product. |
| 31 | `skill_test` mode. | #141; a skill test from the command line is `trellis run --skill <name> --prompt <template> --args` (ADR 0025). |
| 32 | `review_policy`, native review handoffs. | A person reviews `in_review` work; a reviewer-agent stage stays out by rule 5 (ADR 0005). |
| 35–37 | Combined Inbox + Task List, summary slots written by the Summarizer. | [web-ui][multi-user]; #58. |
| 47 | `app_webhook` and `fireflies_hmac` signing modes, a trigger wizard. | A webhook trigger needs a listening server (brief Part 1). |
| 64–65 | GitHub skill sources, more bundled skills. | `trellis install <source>@<ref>` (ADR 0039); most catalog skills drive Paperclip's own API, and a generic one can be copied into a package as `skills/<name>/SKILL.md`. |
| 69–71 | gcp and vault provider stubs, class-3 static leases, remote import. | ADR 0010. |
| 72–75 | Composio broker retired, MCP aggregators on by default, 82 app definitions, new gateway application types. | The owner's answer of 2026-09-28 on #73, "Keep all excluded": MCP and connectors stay excluded until a v2.0 "MCP exposure" brief, and trusted custom command tools (ADR 0023) are how the model gets an external tool; none of the changes touches that. |
| 76–82 | `approvals.respond` and other new plugin capabilities, distribution image catalogs, new UI slots. | [server][web-ui]; a plugin answering approvals is further still from the rule that only a person answers (#189). |
| 88–100 | Bind presets, backup retention, release channels, rewritten experimental flags, Chat-first navigation. | [server][db][multi-user][web-ui]; #70. |

#### Claims the source does not bear out

These claims, in Paperclip's docs or in the v2026.916.1 inventory, are not borne out by Paperclip's source at 1c07b590, so nothing is ported from them. Each is as the addendum's section `Claims the source does not bear out` gives it.

- **The 30-second minimum heartbeat interval.** Stated only in doc/SPEC-implementation.md:1350 (§11.6).
- **Secret folders.** They exist only as the Secrets page's grouping of `/`-separated secret names (ui/src/pages/secrets/secret-path.ts); the server stores no folder.
- **The Task Chat and Auto-Create Recovery Tasks experimental flags** (row 100 of `docs/inventory/inventory-paperclip.md`). No flag in packages/shared/src/feature-catalog.ts or packages/shared/src/validators/instance.ts names either.
- **Routine signing modes.** docs/api/routines.md:129 lists four; packages/shared/src/constants.ts:651 has six, `app_webhook` and `fireflies_hmac` besides those four.
- **Plugin approval decisions.** doc/plugins/PLUGIN_SPEC.md:878 (§15.2, forbidden capabilities) lists approval decisions, against the `approvals.respond` plugin capability in packages/shared/src/constants.ts:1372 and server/src/services/plugin-capability-validator.ts.

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
- `registerToolRenderer`: excluded, TypeScript-runtime specific. (row 4.22, #322)

From pi §8 and §11, under the row "11 TUI features":

- Keybindings: excluded, cosmetic.
- Fullscreen mode: excluded, cosmetic.
- External editor hook: excluded, cosmetic.
- Mermaid rendering: excluded, cosmetic.
- Inline images: excluded, cosmetic.
- Games: excluded, cosmetic.
- Session picker: excluded, cosmetic. (row 8.9, #322)
- Startup header and quiet startup: excluded, cosmetic. (row 11.9, #322)
- Status footer: excluded, cosmetic. (row 11.10, #322)
- Terminal integration and platform guides: excluded, cosmetic. (row 11.11, #322)
- The pi-tui library: excluded, cosmetic; `trellis tui` is its own ratatui view (crates/tui-demo). (row 11.12, #322)

`@` file search is not on this list. Row 11 records it as built in `tui`
(#52).

From Apex §12, under the row "12 `annotations.json`":

- Annotation editing: excluded, [ui].

From Apex §15–19, under the row "15–19 layout, canvas, labels, annotations":

- Layered-layout canvas (the SVG graph view): excluded, [ui].
- Large-graph mode: excluded, [ui].
- Label tiers: excluded, [ui].
- Drawing freehand annotations on the canvas: excluded, [ui].
- *Amended 2026-10-10 (#315):* the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) builds none of these four for the vault: its vault view (`desktop/ui/vault.js`) shows a note's relations as text, and its codebase view's drill-down, pan, zoom and six-label cap (`desktop/ui/codebase.js`) work over trellis's module graph; row 15–19 says what it has and lacks.

From the Apex re-inventory of 2026-10-04 (#359), rows 44–50 and 53–55:

- Workspace tabs (row 44–47): excluded (#70).
- Recent projects (row 44–47): excluded (#70). *Amended 2026-10-07 (#557),
  owner-directed:* the recent list with each repository's last outcome is a
  launcher list, not cross-repo analysis (owner, 2026-10-07); accepted, and
  built 2026-10-07 (#558). *As built 2026-10-07 (#558):* `trellis overview`
  prints the list with each repository's newest outcome; the desktop's
  drawing of it is #562's, not built.
- Project folder rename (row 44–47): excluded (#70).
- Project location chooser (row 44–47): excluded (#70).
- Paste plain text as a note (row 48–49): excluded, [ui].
- Editor link rendering (row 48–49): excluded, [ui].
- Per-line date labels and the note date header (row 50): excluded, [ui].
- Rope connect gesture (row 53–55): excluded, [ui].
- Hub and PageRank scoring (row 53–55): excluded, [ui].
- Workspace chrome (row 53–55): excluded, [ui].

A desktop surface adopted by a later ADR re-dispositions these rows. *Amended 2026-10-10 (#315):* the desktop (`docs/adr/0048-a-tauri-desktop-interface.md`) opens one repository at a time, from a typed path, `--root`, the current directory or the overview's recent list (`desktop/ui/overview.js`), so it has no workspace tabs, folder rename or location chooser, and its one list of repositories is the launcher list that row 44–47's amendment of 2026-10-07 accepts (rows 44–47); it edits no note, shows no note's body or dates, and makes no link of a note's text (48–50); it draws no vault graph, so no rope gesture and no hub scoring, and has no pane resize, fullscreen control or help page, its status bar (`desktop/ui/status.js`) showing trellis's state (53–55); and those rows keep their disposition.

Building any item listed here needs an ADR amending ADR 0021.

## Owner decisions of 2026-09-28

Added 2026-09-29 (#160). The owner answered a set of parked tickets on
2026-09-28. Each row above carries its change as a dated amendment after
the disposition it replaces, so the earlier disposition stays readable. In
summary:

- Kept excluded, with a further reason: parallel tool calls, pi 1.7 (#63).
- Excluded, from deferred or W-A: Paperclip 26–28 attachments (#56),
  31 work modes (#141), 37 cases and pipelines (#58) and 38 feedback
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
**Amended 2026-10-04 (#285):** ADR 0048 places a Tauri desktop interface in
this repository as `desktop/`, its own Cargo workspace and a `trellis rpc`
client, owner-directed: the session's choice under the owner's 2026-10-04
delegation of the upstream plan's open questions. Every write it causes
still goes through the orchestrator (ADR 0048 section 10), and the TUI stays
the product interface (Q-19).
**Amended 2026-10-04 (#369):** ADR 0050 adds a web view, `trellis --web`: the
desktop's page served on 127.0.0.1 only, behind a per-launch token and a Host
guard, with every write still through the orchestrator. Owner-directed.
**Amended 2026-10-07 (#557):** owner-directed, the owner's decision of
2026-10-07: bare `trellis` opens the desktop and prints the overview (ADR
0050 sections 1 and 3, amended). Checked against this paragraph: the
desktop's four views (graph, run, cross-repo, board; ADR 0048 section 10,
amended) are read-only, and every write the desktop causes still goes
through `prompt`, and so through the orchestrator. Accepted; built in part.
*As built 2026-10-07 (#558, #561):* `trellis overview` is built (#558), and
bare `trellis` at a terminal starts the desktop detached and prints the
overview (#561; ADR 0050 section 1, as built). Not built: rpc's `path`,
`blockers`, `parent` and the refusal (#559) and the drawn views (#562).

Three rows named work that was approved but not merged into `drain` when
this section was written: pi 1.2 (#113–#117), pi 2.13 (#150) and pi 7
(#109). All three are merged and read built (corrected 2026-10-04, #316;
also recorded by #416). A row moves to built only once its work is merged.

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
