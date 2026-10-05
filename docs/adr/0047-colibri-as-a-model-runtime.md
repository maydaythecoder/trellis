# ADR 0047 — colibri as a TRELLIS model runtime

**Status:** Proposed.
**Date:** 2026-10-04
**Decided by:** pending; the owner, on COL-01 (#252).
**Reference:** colibri (github.com/JustVugg/colibri) v1.12.1 at ce370e8 (ce370e87d7b623d7759b52ec2007d75fc5b0e87e, 2026-09-25), Apache-2.0, inventoried in docs/inventory/inventory-colibri.md as COL-ENG-01 to COL-ENG-107. The code was read, not run: no colibri server was started for this ADR.
**Amends:** ADR 0024 §2 (tier 1 claimed with no probe), §4 (no `temperature`) and §5 (the hosted default byte budget), each only for a loopback endpoint identified as colibri; then ADR 0028 (its statement that the hosted adapter sends no temperature), for that endpoint only.
**Holds:** ADR 0013 (no inference code in TRELLIS; tier 3 not built). ADR 0024 §3 (where a key is sent) is unchanged. ADR 0004 (only trellis-checks produces a `Verdict`). The ten rules in CLAUDE.md are unchanged. No crate dependency is added.

## Context

colibri is a C inference engine for very large mixture-of-experts models. It keeps dense weights resident and streams routed experts from disk through RAM and an optional VRAM tier. It serves nine families. Its Python gateway, `c/openai_server.py`, serves:
- `POST /v1/chat/completions` (OpenAI chat, tools, SSE)
- `POST /v1/messages` (Anthropic Messages)
- `POST /v1/completions` (raw text, no chat template)
- `GET /v1/models`, `GET /health` and `GET /metrics`
- `/v1/brio` (closed-set scoring), `/experts`, `/profile` and a dashboard

| family | tools (tier 1) | default / max context, tokens | KV slots | output budget |
|---|---|---|---|---|
| GLM-5.2/5.3 | yes | 4,096 / 1,048,576 | 16 | 16,384 |
| GLM-5.3-Flash | yes | 8,192 / 1,048,576 | 16 | 16,384 |
| Kimi K3 | yes | 8,192 / 1,048,576 | 1 | 16,384 |
| DeepSeek V4 Flash | yes | 4,096 / 1,048,576 | 1 | 16,384 |
| DeepSeek V4.1 Flash | yes | 4,096 / 1,048,576 | 1 | 16,384 |
| Qwen3.8-Flash-Next | yes | 8,192 / 262,144 | 1 | 8,192 |
| Qwen3.6-35B-A3B | only with `COLI_TOOL_FALLBACK=1` | 8,192 / 262,144 | 1 | 8,192 |
| Inkling | no | 8,192 / 1,048,576 | 1 | 16,384 |
| OLMoE | only with `COLI_TOOL_FALLBACK=1` | 4,096 / 4,096 | 1 | 1,024 |

The table comes from `c/family_registry.py:1085-1432` (`FamilyCapabilities`, `FamilyLimits`). The output budget is the family's `interactive_max_output`, which `coli serve` uses.

TRELLIS reaches a runtime through a spec (`crates/trellis-model/src/spec.rs`) and an adapter tier (ADR 0006, 0013, 0024). It has no colibri-specific code. Reading both trees found:

| # | Finding | Where |
|---|---|---|
| 1 | colibri's grammar never constrains sampling. `response_format` and `GRAMMAR` feed a draft source whose spans the model verifies. Unknown fields such as `guided_json` are ignored. colibri cannot be a tier-2 runtime, and TRELLIS's schema probe sees the schema ignored and falls to tier 4. | c/grammar.h:10-12; c/openai_server.py:3097-3101; crates/trellis-cli/src/run_cmd.rs:717-732 |
| 2 | Inkling, OLMoE and Qwen3.6 answer `tools` with 400 `unsupported_parameter`, param `tools`, while rendering the prompt, before the engine runs. TRELLIS claims Native from the protocol without asking. | c/openai_server.py:1364-1367, 1423-1426, 1707-1708; crates/trellis-cli/src/doctor_cmd.rs:75-84 |
| 3 | Every POST naming a model other than the served id gets 404 `model_not_found`. | c/openai_server.py:4256-4259 |
| 4 | With no `temperature`, colibri samples at `COLI_TEMP`, else 0.7. Tier 1 sends none. | c/openai_server.py:3136-3147; crates/trellis-model/src/hosted.rs:263-275 |
| 5 | With neither `reasoning_effort` nor `enable_thinking`, thinking is off except on Qwen3.8 (`xhigh`) and on servers started with `COLI_THINK=1`. The Anthropic path thinks only on `thinking.type == "enabled"`, and TRELLIS sends `"adaptive"`. | c/openai_server.py:5175-5185, 5251-5254; crates/trellis-model/src/hosted.rs:335-337 |
| 6 | Each reply is clamped to the room the prompt leaves in the context window (`maxctx - len - 1`), and the context window is not reported over HTTP. Separately, a requested `max_tokens` above the server's `--max-tokens` cap is lowered to that cap. A loopback openai-chat spec gets TRELLIS's 400,000-byte hosted default. | c/colibri.c:9387-9413 (the reply clamped to the room the prompt leaves); c/openai_server.py:3152-3160 (the `--max-tokens` cap); crates/trellis-cli/src/hosted.rs:247, 288 |
| 7 | `/v1/models` returns the served id with `owned_by: "colibri"`. `/health` returns `{status: ok}`; to an authenticated caller, or when the server has no key, it adds scheduler counters, `kv_slots`, `tiers` (expert counts and GB per tier) and `hwinfo`. Nothing reports colibri's version (the HTTP server name is `colibri` alone), its family or its context window. | c/openai_server.py:3885-3886, 4049, 4318-4331, 4357-4362, 3608-3637 |
| 8 | TRELLIS gives each request a fixed 300 s timeout, does not retry it, and reads replies whole. colibri's own figures put a 128-token request at a median 140 s on a 24-core, 61 GiB machine (Qwen3.8), and a decode token at about 20 s warm on an ordinary disk (GLM-5.3-Flash). A request that is not streamed gets no keepalive. | crates/trellis-model/src/hosted.rs:687; runtime.rs:141; colibri docs/qwen38.md:139-151, docs/glm53-flash.md:83-91, c/openai_server.py:4940-5150 |
| 9 | colibri's default policy keeps precision and router semantics, and every lossy setting is opt-in. None of these settings is visible over HTTP. | colibri README.md; docs/tuning.md; c/resource_plan.py:959-962 |

## Decision

1. **TRELLIS is a client of colibri's HTTP gateway and nothing more.** It does not start, stop, build or configure colibri, link its segment or edge C ABIs, or speak its engine protocol. The operator runs `coli serve`. TRELLIS gains no inference code.

2. **A colibri server is reached through specs that already exist.**
   - Tier 1: `openai-chat:http://127.0.0.1:8000/v1#<model-id>`, or `provider:<name>#<model-id>` with a `[[providers]]` entry in the user's own config. Setting `key_env = "COLI_API_KEY"` there means a user's `OPENAI_API_KEY` is not sent to colibri; ADR 0024 §3 sends a set variable to a loopback endpoint the user named.
   - `anthropic:http://127.0.0.1:8000#<model-id>` also works, without image input.
   - `vllm:http://127.0.0.1:8000#<model-id>` gives tier 4 only, with an untemplated prompt, and doctor says so.
   - No `colibri:` scheme is added, subject to COL-01's question to the owner.

3. **colibri is identified, not assumed.** For a loopback base only, TRELLIS reads `GET /v1/models` at the endpoint's origin, and `owned_by: "colibri"` identifies the server. This is the server's own claim, so:
   - it turns on only behaviour that is harmless if the claim is false (decisions 4 to 6);
   - it never changes where a key is sent;
   - every string from it is sanitised before it reaches a terminal and treated as untrusted data wherever it is kept.

   A refused connection, no answer within 10 s, or a 401 or 403 stops a run before its snapshot. A 404, a 405, a 5xx, a body that is not a JSON model list, or a list not owned by colibri means the server is not colibri, and the run continues as it does today.

4. **One tools probe before the snapshot (amends ADR 0024 §2).** For an identified colibri endpoint, `trellis run`, `trellis doctor` and the TUI status bar send one chat request carrying one tool and `max_tokens: 1`, bounded at 10 s, at most once per origin and served id in a process. colibri raises the tools refusal while rendering the prompt, before generation, so a refusal arrives inside the bound.
   - A 400 whose `param` is `tools` is a refusal. The run stops before its snapshot naming the `vllm:` tier-4 spec, and names `COLI_TOOL_FALLBACK=1` only when colibri's own 400 message names it (OLMoE's and Qwen3.6's do). Inkling has no fallback. doctor reports tier 1 as unavailable.
   - A 200 is reported as Native (probed).
   - Anything else, no answer within 10 s included, is reported as Native (assumed), with the reason.
   - A served id that differs from the spec's is refused before the snapshot.
   - The identification read doubles as the reachability check that hosted specs lacked.

   ADR 0024 §2 declined a probe because it would spend the user's money. On loopback it spends a moment of the user's machine. Remote endpoints are never identified or probed.

5. **A request profile for colibri (amends ADR 0024 §4, and ADR 0028's statement that the hosted adapter sends no temperature, for colibri only).** Bodies sent to an identified colibri endpoint carry `temperature: 0` and state thinking explicitly:
   - openai-chat: `enable_thinking: false`, or the configured `reasoning_effort`;
   - Anthropic: `thinking: {type: "disabled"}` or `{type: "enabled"}`.

   Every other endpoint's body is unchanged byte for byte. Temperature 0 is what tiers 2 and 4 already send, and ADR 0028 waives repetitions only at 0.

6. **The byte budget leaves room for the reply (amends ADR 0024 §5).** An identified colibri endpoint with no `[model] context_bytes` gets a default below the hosted one (`HOSTED_CONTEXT_BYTES`), sized so that the prompt leaves the reply room in colibri's window: colibri clamps each reply to the room the prompt leaves (c/colibri.c:9387-9413), so a prompt that fills the window leaves a turn no room for its tool call. The figure is the owner's answer on COL-08, recorded by that ticket. An explicit value wins, larger or smaller. doctor states that colibri does not report its window.

7. **doctor reports what colibri reports, labelled as colibri's report:**
   - runtime colibri, the served id, and whether the spec matches it;
   - `version: not reported over HTTP`;
   - the tier from decision 4;
   - batching from `kv_slots`: Continuous with that count when it is above one (colibri's configured slots, said to be configured), SingleStream otherwise;
   - expert counts and resident GB per memory tier, and the host's cores, RAM and GPUs;
   - without authentication, a line saying colibri shows these only to an authenticated request.

8. **Accounting uses per-request figures.**
   - Output and prompt tokens are already read from `usage`.
   - The reply's output tokens, the model call's wall time and its `x-colibri-queue-wait-ms` header become fields on the loop's `turn` event (tracing is the one telemetry path), and the TUI shows tokens per second.
   - `/metrics` is not read: its counters are server-wide and cannot be charged to one run on a shared server.
   - Nothing here is a cost in money.

9. **The per-request timeout becomes configurable**, for the tier-1 and runtime clients alike and still bounded by the run's wall-clock budget, in the form COL-06 asks the owner about (proposed: `[model] timeout_s`, default 300 s).

10. **Streaming is not built by this ADR.** #151 stands until the floor run (decision 12) reports the model-call time per turn; COL-13 asks the owner. If streaming is built after that, it covers loopback colibri only, with an idle timeout reset by colibri's 10 s keepalives, and the loop still acts only on a complete tool call.

11. **`trellis model pull` does not fetch colibri containers.** The catalogue stays one pinned GGUF file per entry (ADR 0044); colibri downloads, converts and verifies its own shards.

12. **One colibri configuration is measured against the floor** (BENCHMARK_AND_EVAL.md §5) at tier 1 and temperature 0. It is reported as model × container precision × colibri version × backend, with the serve command, machine and cache state beside it. COL-12 asks the owner which configuration.

13. **Not adopted now:**
    - `/v1/brio` for `trellis ask`: the current classifier is measured on colibri first, and a score would only choose a code path, never become a `Verdict`.
    - A runtime record in the trail: colibri does not report the settings that would make one worth a format change.

    Both are put to the owner on COL-15.

## Alternatives rejected

- **A `colibri:` scheme.** It would be explicit in the trail's spec string, but it grows the spec surface and needs a key variable of its own. Identification gives the same request profile with no new surface. It is put to the owner on COL-01.
- **A static table of colibri's families**, keyed by default model id. The operator's `--model-id` breaks the key, and the table would claim tool support and a context window the server never stated. The probe asks the server.
- **A probe that costs nothing**, sending `tools` together with `n: 2` (rendering refuses `tools` before `generation_options` refuses `n`). It depends on the order of colibri's validation, which can change without notice, and such a change would become a false Native claim on every family.
- **Tier 2 through `response_format`.** colibri's grammar never constrains sampling (finding 1). Asking upstream for a constraining mode buys little where native tools already work.
- **Tier 3 through colibri's segment or edge ABI.** It needs a C toolchain, version 1 exposes greedy selection only, and ADR 0013 keeps tier 3 unbuilt.
- **Temperature 0 for every loopback tier-1 endpoint.** A loopback proxy in front of a hosted reasoning model would forward it and draw the 400 that ADR 0024 §4 avoids.
- **Supervising `coli serve` from TRELLIS.** TRELLIS starts no runtime for any dialect, and a model load of minutes inside `trellis run` would turn a refused connection into a hung one.
- **The local byte budget (`LOCAL_CONTEXT_BYTES`, 14,336 bytes) as colibri's default.** It is sized to the whole 4,096-token window, so a prompt held to it can leave a turn no room for its tool call (COL-08).

## Consequences

- A tier-1 run against colibri is repeatable at temperature 0. It refuses before its snapshot on a family without tools, on a wrong model id or on a stopped server, and its default budget leaves the reply room unless told otherwise.
- doctor tells a user which colibri model is loaded, how many slots it serves and where its experts live, and says what colibri does not report.
- Until the timeout is configurable, any turn longer than 300 s ends the run, and on most machines colibri targets, turns are minutes long.
- Eliding old observations (ADR 0024 §5) rewrites the middle of the prompt, which costs colibri a re-prefill from the first changed token. Elision is monotone, so each observation is rewritten at most once.

## What is not built

- Everything above: this ADR is Proposed, and COL-01 is the owner's acceptance.
- Streaming (decision 10), a `/v1/brio` client, a runtime record in the trail, and colibri weights in the catalogue.
- Version, family and context window: colibri does not expose them, so TRELLIS does not report them.
- Agents pinned to colibri KV slots (`cache_slot`). Unpinned conversations already hash to a stable slot; pinning waits on a measured multi-agent run on a 16-slot family.
- No colibri server has been run against TRELLIS. Every behaviour above was read from colibri's source at ce370e8, and the contract tests use bodies transcribed from that source.

## What would change our mind

- If colibri reports its version, family, context window and lossy settings on `/health`, doctor would print them, the byte budget would follow the reported window, and the trail question would reopen.
- If colibri adds a mode in which `response_format` constrains sampling, tier 2 would apply to colibri.
- If the floor run's model-call times stay under the default timeout, decision 9 would matter less and streaming would stay out.
