# Gap review, 2026-10-11: nine repositories against TRELLIS

What each repository has that TRELLIS lacks and would take, read from each
one's own source and docs at the commit named, and compared against ADR 0021,
the inventories under `docs/inventory/`, the ADRs and the code. Every
candidate became a GitHub issue with `## Done when`, a fenced `## Verify`,
the dimension labels `trellis-tickets new` computes (docs/TICKETING.md) and
a `from-<repo>` label. Dispositions are the owner's: this file records what
was found and why each item fits or does not; ADR 0021 stays the map once
the owner decides.

## Repositories read

| Repository | Commit read | Licence | Last read by TRELLIS | What changed since |
|---|---|---|---|---|
| JustVugg/colibri | bf244291, 2026-10-06, v2.0.0 | Apache-2.0 | ce370e87 (v1.12.1, `inventory-colibri.md`) | 169 PRs: `/v1/brio` removed, `/v1/systemone` the decision API; `--kv-slots` to 16 on every text family; Qwen3.6 tools; images on `/v1/messages`; `/v1/models` carries `capabilities` and `input_modalities`; new families (MiMo-V2.6, Qwen3-Coder-30B-A3B, Qwen3.8-27B, image and decision engines); `coli setup` serves on :8000 with no key |
| boriemannetje/apex-notes | f449644a, 2026-09-09 | MIT | the same commit (`inventory-apex*.md`) | nothing on `main`; the two `codex/*` branches are ancestors or pre-rebase copies of it |
| earendil-works/pi | 96abe9a5, 2026-10-11, 1.1.0 | MIT | 20038712 (1.0.2, `inventory-pi.md`) | 129 commits: `durationMs` on tool results and assistant messages, `startedAt`/`endedAt` on durable tasks; colour codes stripped from bash output; `server_busy` retried; `--tools +name/-name`, `--no-mcp`; a Rust SSH env daemon (`packages/env`); published config schemas; durable nested calls and structured output |
| paperclipai/paperclip | e733bddc, 2026-10-11 | MIT | 1c07b590 (`inventory-paperclip*.md`) | 228 commits, release v2026.1009.0: public MCP, chat-first navigation, capacity errors retried after 1 then 2 minutes, isolated worktrees per task by default, OS-backed SQLite restore locks, 39 migrations (identity keys, commentary, billing reservations, decision models, voice) |
| Jakubantalik/thinking-orbs | de85557c, 2026-08-16 | MIT | never | a React package of canvas "thinking" indicators keyed by nine agent states; a pure geometry engine; reduced-motion and hidden-tab handling |
| odysseus-dev/odysseus | a8c147b2, 2026-10-07 | AGPL-3.0-or-later | never | a FastAPI AI workspace: agent loop with tool policy and approvals, sensitive-path refusal on file tools, background tool jobs, hardware-fit ranking, a model-quirks evidence map (code not portable; ideas only) |
| crynta/terax-ai | 54606c46, 2026-10-09 | Apache-2.0 | never | a Tauri 2 terminal workspace with a BYOK agent panel: secret-path deny-list on read and write, approval-gated tools with read-before-edit, sub-agent tool whitelists, a loopback TCP control plane with a token |
| openclaw/clawhub | d044664a, 2026-10-08 | MIT | never | a skill registry and CLI: per-install lock and origin records with a content fingerprint, update that refuses over local edits, pin with a reason, inspect and publish dry runs, artifact digests, requirement declarations, scans, trusted publishing |
| alvinreal/awesome-opensource-ai | 5cc09c37, 2026-10-11 | CC0-1.0 | never | a curated list; eight listed projects were read: vLLM (c059bee8, Apache-2.0), guidelint (9c282a30, MIT), anthropic-experimental/sandbox-runtime (5507fb0e, Apache-2.0), OrcaReplay (a9178ca2, Apache-2.0), mini-swe-agent (04d809ce, MIT), Serena (1de556f7, GPL app), Ollama (eab97e9f, MIT), oh-my-pi (0affc77e, MIT) |

## Candidates, by impact then effort

Impact: H/M/L as judged against the mission and the open stories; effort per
docs/TICKETING.md (s under 2 h, m one session, l split); risk per §5. Each
issue's body carries the upstream `path:line`, the TRELLIS `file:line` or ADR
row that shows the gap, the rules it touches and how they stay intact.

| Impact | Effort | Risk | Issue | P | Milestone | Source | Blocked by | Title |
|---|---|---|---|---|---|---|---|---|
| H | s | low | #610 | P1 | v1.0 | awesome list → vLLM, guidelint | — | Fix the vLLM constraint request: send structured_outputs.json, which replaced guided_json in vLLM 0.12 |
| H | m | low | #611 | P1 | v1.x | terax-ai (and odysseus) | — | Refuse read_file on a path git ignores and leave ignored entries out of ls, as find, grep and write_file already do |
| H | m | medium | #612 | P1 | v1.x | colibri | — | Record colibri 2.0.0 in the runtime guide, ADR 0047 and trellis model pull's family list |
| H | m | low | #616 | P1 | v1.0 | colibri | — | Measure Qwen3-Coder-30B-A3B on colibri 2.0.0 against the S7 floor at tier 1 |
| H | m | high | #620 | P2 | v1.x | awesome list → OrcaReplay, mini-swe-agent | — | Write a replayable step recording per run and accept it through scripted: so a run re-executes offline against real files and checks |
| H | l | high | #621 | P2 | v2 | awesome list → sandbox-runtime | — | Decide and spike an OS sandbox around the throwaway clone's command: network off and home unreadable (bubblewrap on Linux, sandbox-exec on macOS) |
| H | l | high | #624 | P2 | v2 | Paperclip | — | Record the worktree-per-queued-task decision (ADR 0021 row 59-63) now that #98-#101 are closed, and split its build |
| M-H | m | high | #625 | P2 | v1.x | colibri | — | Offer a tier-1 provider: spec for a colibri found on 127.0.0.1:8000 in the first-run list, never a bare openai-chat: spec |
| M | s | medium | #613 | P2 | v1.x | Paperclip, pi | — | Wait longer, bounded, on a 529 or 503 with no Retry-After before failing a hosted run |
| M | s | medium | #627 | P2 | v1.x | colibri | — | List the `runtime identified` event in the client contract so a client may rely on it |
| M | s | low | #630 | P2 | v1.x | apex-notes | — | Open a note's lineage, children and links from the desktop vault view with buttons, never links |
| M | m | medium | #614 | P2 | v1.x | thinking-orbs | — | Emit a `model call` event before each next_step so a client can tell waiting-on-the-model from running-a-tool |
| M | m | low | #615 | P2 | v1.x | thinking-orbs | #614 | Show the run's phase in the desktop status bar: waiting on the model, running a tool, or checks |
| M | m | high | #617 | P2 | v1.x | ClawHub | — | Record a content digest for each installed package and report drift in trellis install --list |
| M | m | high | #618 | P2 | v1.x | ClawHub | #617 | Add trellis install --upgrade <name>: re-resolve the recorded ref, refuse over local edits, replace atomically |
| M | m | high | #622 | P2 | v1.x | pi | — | Record ended_at and per-action, per-model-call and per-check durations in the trail |
| M | m | medium | #626 | P2 | v1.x | colibri | — | Read colibri 2.0.0's model card and refuse, before the snapshot, a model that does not chat or a run image the model cannot take |
| M | m | low | #628 | P2 | v1.x | colibri | #627 | Show an identified colibri's self-report in the desktop's run section: three separate counts, unreported shown as such |
| M | m | medium | #629 | P2 | v1.x | apex-notes | — | List the vault's notes in the desktop vault view before a query is typed, through vault_search with an empty query |
| M | m | medium | #631 | P2 | v1.x | apex-notes | — | Add a read-only vault_check rpc command that answers what trellis vault check prints, with no fix path |
| M | m | low | #632 | P2 | v1.x | apex-notes | #631 | Show trellis vault check's report and run-gate answer in the desktop vault view, never as a check row |
| L-M | s | medium | #619 | P2 | v1.x | ClawHub | — | Add trellis install --dry-run: print the copy and skip plan and the bounds verdict, write nothing |
| L-M | m | medium | #633 | P2 | v1.x | odysseus | — | Report in doctor where the configured Ollama model sits, as Ollama reports it (size, size_vram, quantization from /api/ps) |
| L | s | medium | #623 | P2 | maintenance | pi | — | Keep terminal colour codes out of tool observations captured from bash and custom tools |

## What the review changed on existing tickets

- #276 (classifier on colibri) is written around `/v1/brio`, removed in 2.0.0; commented, and #612 carries the edit to `/v1/systemone`.
- #277 says colibri 1.12.1; commented. Its 24 GB requirement stands; #616 measures a model that fits the 15 GB host.
- #40, #46, #48, #49, #51 (pi and Paperclip split parents) defer rows that are now built (#150/#149, #108, #109/#110, #113–#117, ADR 0039); each is commented with the row and the closing ticket, for the owner to close (TICKETING §11). #43's two built halves are noted; its third item is unverified.

## Considered and not filed

- colibri: a `/v1/systemone` client for `trellis ask` (owned by #276 once re-pointed; a score is never a Verdict, ADR 0004); `strict_tool_calls` (a failed strict parse is a 502 the adapter already retries); `logprobs`/`echo` (report, never constrain; tier 3 not built, ADR 0013 §2); `response_format` (still draft-only upstream, `c/grammar.h:10-12`); speculative decoding, Vulkan, second GPU, int4 experts (engine internals, ADR 0047 decision 1); image generation (ADR 0021 row 2.14); the MCP server (rows 16.1–16.7); `coli setup` and archives (TRELLIS never builds or fetches colibri, decisions 1 and 11); cache slots per agent (waits on a measured multi-agent run); System One, Brain and Profiling pages (server-wide telemetry, decision 8); the `data: blob:` CSP (no new CSP source without amending ADR 0048); the per-address connection cap on `--agents auto` (low impact: needs nine or more partitions; left for the cache-slot decision).
- apex-notes: per-line date labels, the vault canvas, label tiers, hub ranking (ADR 0021 rows 15–19 and 50 as amended by #315); editor, paste, drop import, undo, auto-create (#70, ADR 0049; rpc has no vault write); readable external links (the page has no links, lockdown).
- pi: `aborted` on `agent_settled` (built in effect: rpc `run_end` carries `interrupted`); `--tools` patterns and `--no-mcp` (row 1.4; the CLI surface is fixed by brief 4.12; MCP excluded by #73); published config schemas (built in effect: `deny_unknown_fields` and `doctor`); OSC 7501 program status (row 11 cosmetic; the owner could reverse it for the Go TUI); output files 0600 (nothing to apply); `.env` loading removed (never read); the SSH env daemon (row 1.6/13.3, behind the sandbox decision, #621); durable nested calls, codemode sandboxes (rows 1.7, 4.23, 4.24, 17.x); classifier models (rows 2.17–2.19); the 3.5 chars/token estimate (budgets are bytes).
- Paperclip: public MCP, voice, fast responses, review bots, connection pools (brief Part 1 networked; #73; rows 72–75); tool grants enforced as written (already fail-closed); SQLite restore locks (ADR 0020's hard-link claim is deliberate, #352); silent-run flags (bounded already by the timeouts); agent commentary, `update_skill`, default grants, delegation handoff (rows 38, PC-118, PC-143/144, rule 5); private tasks, lifecycle, identity keys, decision models (multi-user/db/#66); `accessRequest` and `initialText` on questions (ADR 0026 §4, ADR 0018, row 30); Codex runs surviving restarts (no daemon; ADR 0038); `estimated` cost status and reservations (dollars deferred, #67); shorter default instructions (prompt text measured by their evals, not ported).
- thinking-orbs: the orb animation and React component (cosmetic; no npm in `desktop/`).
- odysseus: the repeated-call loop breaker, output truncation, untrusted wrapping, taint escalation, turn contract, background jobs, threat model, quirks map, `HARNESS_VERSION`, MCP, A/B UI (each built, excluded or served by an existing document; detail in #633's source notes).
- terax-ai: read-before-edit (the edit's unique-anchor rule already fails a guessed `old`), per-hunk accept (a gate may not mutate, ADR 0005), the loopback control plane (trellis opens no socket for control; the web view is a page, ADR 0050 §5), OS keychain (a new dependency; keys stay in user-named variables), SSRF pinning, sub-agent whitelists (built), symlink re-check (built, #441), workspace registry, PTY/OSC detection, auto-updater (a Tauri plugin, refused).
- ClawHub: the registry, search, `latest` tags, soft delete, rename and merge, moderation, telemetry, trusted publishing (a networked service, brief Part 1; #65); `pin --reason` (the required ref is the pin, ADR 0039 §2; folded into #618); scans and requirement frontmatter (packages carry no code, #120; a skill informs and never binds, ADR 0019); Nix pointers and `install` specs (#120).
- awesome list: Serena (MCP excluded; an LSP-fed index contradicts ADR 0001; GPL); oh-my-pi hashline edits (would change `ReadFile` output and every measured prompt); mini-swe-agent's SWE-bench runner (per-instance docker and network; one pinned corpus by choice); OpenLLMetry/OpenLIT (no second telemetry path); Microsandbox and other microVM sandboxes (heavier than the clone-per-call design needs; #621 covers the gap with OS primitives); Outlines/XGrammar/Guidance (llguidance already validates; whether vLLM's xgrammar honours every keyword in TRELLIS's schema is unverified and worth a measurement note); vLLM's `structural_tag` and Lark grammar fields (JSON Schema is the one format every runtime takes).
