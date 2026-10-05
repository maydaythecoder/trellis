# ADR 0021 amendment: pi 1.0.2 re-inventory

**Status:** Proposed.
**Date:** 2026-10-04
**Amends:** ADR 0021: the pi section, "Where this stands", the "Owner decisions of 2026-09-28" closing paragraph, Paperclip 72-75, and the itemised cosmetic exclusions.
**Inventory:** pi at upstream commit 200387122ca450d6387f033949423114a270b96c, all 13 packages at 1.0.2. The matrix was written against pi-coding-agent 0.87.1.

The keys PI-01 to PI-11 belong to this plan. When filed, each is written as its issue number.

## Context

The re-inventory lists 136 capabilities:

- 51 unchanged.
- 50 changed.
- 35 new: 23 added upstream after 0.87.1, and 12 that the 0.87.1 inventory missed.

Reading the code against the matrix found three kinds of drift:

- **Merged work still reads "landing".** Rows 1.2, 2.13 and 7 say so although their work is merged.
- **Owner answers absent from the text.** #73 covers consuming MCP. #111 declined user-level `system.md`.
- **Capabilities with no row,** including two old items that never had a disposition (4.11, 4.15).

This amendment changes no accepted disposition. Rows marked "open" wait on the named question ticket; their recommendation is stated, not adopted.

## 1. Corrections: merged work (PI-01)

| Row | Correction |
|---|---|
| 1.2 steering and follow-up | **Amended 2026-10-04:** built in every part named above: delivery between turns (#113), `steer` and `follow_up` over rpc (#114), `tui` (#115), `session` (#116), the trail record (#117, ADR 0034). `clear_queue` is excluded (ADR 0025 §5, the owner's decision). One-at-a-time queue modes are not built: every steer waiting at a model request is delivered in it. |
| 2.13 transports / proxy | proxy built (#150): `HTTPS_PROXY` and `NO_PROXY` in `trellis-model` `transport.rs` (ADR 0024 §1). Websocket stays excluded. |
| 7 slash commands | /name and /copy built (#109). `/new` is not built (ADR 0025 "Not built": each task is already a fresh run). |
| Where this stands, W-C | HTTPS_PROXY built (#150). |
| Where this stands, W-D | /name and /copy merged (#109). User-level `system.md` declined (#111): ADR 0025 §2, system prompt files are repository-level only. |
| Owner decisions of 2026-09-28, closing paragraph | All three are merged and read built (pi 1.2, 2.13, 7). |
| ADR 0034 status | The trail side is built (#117); the loop side that fills it is built (#113). |

## 2. Owner answer missing from the text (PI-01)

**Paperclip 72-75.** #73 (2026-09-28) asked about MCP "exposing or consuming", and the owner kept both excluded. This row therefore covers consuming MCP as well as exposing it (#73). Trusted custom command tools (ADR 0023) are the way to give the model an external tool.

## 3. pi 1.0.2 re-inventory (2026-10-04)

| Items | Disposition |
|---|---|
| 1.7 parallel tool calls (amend) | open: PI-04. Recommended: codemode (17.1) and nested tool calls (4.23) are further routes to several calls in one turn, excluded under the same clause (#63). |
| 1.16 process markers | built (PI-08): `AI_AGENT=trellis` for `bash`, custom tools, hooks and process agents, never added to grounded checks. |
| 2.1 protocols not spoken | The OpenAI Responses, Gemini, Bedrock Converse and Mistral APIs are not spoken. A provider reachable only through one of them is reachable only through an OpenAI-compatible gateway the user runs. A third protocol needs an ADR 0024 amendment. |
| 2.14 pi-ai extras (amend) | Classification and image generation are now rows 2.17 and 2.18. |
| 2.15 Anthropic workload identity federation | open: PI-06. Recommended: deferred with cloud signing, accepted only on #45's terms: credentials from the environment or the platform's own chain, the exchange tested against recorded vectors and a loopback stub, and the access token held in memory only. |
| 2.16 sampling parameters | open: PI-07. Recommended: not built. Local runtimes get temperature 0.0 and hosted models get none; ADR 0028's metric-5 waiver rests on it. |
| 2.17 classifier models | open: PI-05. Recommended: excluded (#66). |
| 2.18 image generation models | open: PI-05. Recommended: excluded (#66). |
| 2.19 virtual models | open: PI-05. Recommended: excluded. ADR 0043's only model switch is triggered by an executed check, never by a model's reading of the request. |
| 2.20 credential commands | built in effect: `trellis doctor` prints whether the key variable is set (`trellis-cli` `hosted.rs`), never its value. Printing a key is not built (ADR 0010). |
| 2.21 list models | built (`trellis model`, session `/models`). |
| 4.11 messages from extensions | deferred: a hook cannot add a message mid-run. The steering channel (ADR 0034) would carry one if it is added. |
| 4.15 event bus | excluded: hooks are separate processes with no shared runtime (ADR 0023). |
| 4.21 provider stream event | excluded under row "4 remaining events"; trellis does not stream hosted replies (#151). |
| 4.22 tool renderer resolver | excluded, TypeScript-runtime specific. |
| 4.23 nested tool calls | open: PI-04. Recommended: excluded under row 1.7 (#63). |
| 4.24 structured tool results | open: PI-04. Recommended: excluded; its only consumer is codemode. |
| 8.9 session picker | excluded, cosmetic (row 11). `trellis log` lists runs, and `trellis resume <run_id>` opens one (ADR 0038). |
| 9.4 RPC commands | open: PI-10, which amends ADR 0025 §5. Recommended: add `get_session_stats` and `get_last_assistant_text`, and record `cycle_model`, `set_steering_mode`, `set_follow_up_mode` and `set_auto_compaction` as not built. |
| 9.7 fork and rebrand | not applicable: a fork renames the crate and binary, and `TRELLIS_CONFIG_DIR` relocates configuration. |
| 10.3 self-update and install channels | excluded: a version check is a networked component (brief Part 1). trellis is built from source with `Cargo.lock` as the pin, and Apex 38-42 already excludes self-update. |
| 11.9 startup header, 11.10 status footer, 11.11 terminal integration, 11.12 pi-tui library | excluded, cosmetic (row 11). `trellis tui`'s status bar shows model, tier and budget. |
| 12.3 offline mode | built in effect: trellis makes no automatic network request, and `--no-default-features` builds a binary without TLS (ADR 0024 §1). |
| 13.5 tool annotations | open: PI-11. Recommended: a `read_only` fact per built-in tool, from one table, in `pre_tool` and `post_tool` call objects. Custom tools are unchanged. |
| 14.3 pi-durable | built in effect through the trail, the queue and `trellis resume` (ADR 0038). An interrupted run rolls back (ADR 0009) rather than resuming a tool call. |
| 14.4 usage and cost | Prompt and completion tokens per model are built (PI-09), as two numbers never summed. Dollars are deferred to v1.x (#67). |
| 15.5 pi-client | excluded with row 15 (#65). |
| 15.6 durable coding agent | excluded: no model-invoked subagent (#130, ADR 0005, ADR 0016). |
| 16.1-16.7 MCP client | open: PI-03. Recommended: excluded (#73). 16.2's OAuth and credential file fall under ADR 0010 (#64), and 16.1's streamable HTTP is a networked component (brief Part 1). A `[[tools.custom]]` command (ADR 0023) is the supported way to reach one stdio server. |
| 17.1-17.6 codemode, tool exposure and tool search | open: PI-04. Recommended: excluded under row 1.7 (#63) and the fixed tool set (`trellis-loop` `registry.rs`). 17.2 also needs the session tree (#50), and 17.3 also falls under row 2.14 (#66). |

## 4. Cosmetic and [ui] exclusions, itemised: additions

From pi §4:

- `registerToolRenderer`: excluded, TypeScript-runtime specific.

From pi §8 and §11:

- Session picker: excluded, cosmetic.
- Startup header and quiet startup: excluded, cosmetic.
- Terminal integration and platform guides: excluded, cosmetic.
- The pi-tui library: excluded; trellis has its own ratatui view in `crates/tui-demo`.

## 5. Changed upstream, disposition unchanged

| Rows | Why the disposition holds |
|---|---|
| 1.4 | `[tools] enable` and `disable` are the + and - lists. |
| 1.5 | The 1 MiB structured result serves codemode. |
| 1.9 | Fixes only. |
| 2.2 | Login stays excluded (#64). |
| 2.4 | Signing stays deferred (#45). |
| 2.6 | Typed entries follow 2.17 and 2.18. |
| 2.7 | Catalog types follow 2.17 and 2.18. |
| 2.8 | One `[model] thinking` per run, so there is no per-message level. |
| 2.10 | The router stays deferred (#45). |
| 2.11 | Radius stays excluded (#65). |
| 2.12 | Code-defined providers need an in-process runtime (ADR 0023). |
| 3.4, 4.1, 4.8, 4.13, 4.16, 4.19, 9.6, 13.2 | The change follows MCP or codemode (PI-03, PI-04). |
| 4.12 | Built (#109). |
| 4.18, 11.1, 11.4, 11.7 | Cosmetic. |
| 4.20 | Documentation only. |
| 6.1 | Built (`templates.rs`). |
| 8.1 | Deferred (#50). |
| 8.4 | `resume <run_id>` and `/name` cover it. |
| 8.5 | Per-model limits are `[[providers]] input_bytes` (#162). |
| 8.8, 9.1 | HTML is excluded (#143). |
| 9.3 | trellis's own vocabulary (ADR 0025 §4). |
| 10.1, 10.2 | ADR 0039 declines code packages. |
| 12.1, 12.2 | Built, field-wise layering. |
| 13.1 | trellis is stricter. |
| 14.1 | No network telemetry. |
| 14.2 | Built. |
| 15.1-15.3 | Excluded (#65). |

## Consequences

- No accepted disposition changes unless the owner picks a non-recommended option on PI-03 to PI-07, PI-10 or PI-11.
- No new CLI command or flag.
- The rpc protocol gains two read-only commands only if PI-10 option A is accepted (ADR 0025 §5). Hook payloads gain `read_only` only if PI-11 option A is accepted (ADR 0023 §5).
- No recommended option adds a crate dependency.
- The grounding contract is untouched:
  - `Verdict` stays in `trellis-checks`.
  - `trellis-index` gains no `trellis-model` dependency.
  - No reach-to-doubt ratio.
  - Budgets divide.
  - Trails are read back as untrusted data.
