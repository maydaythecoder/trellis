# ADR 0047 — colibri as a TRELLIS model runtime

**Status:** Accepted (owner-directed), under the owner's delegation of 2026-10-04.
**Date:** 2026-10-04; accepted 2026-10-06.
**Decided by:** the owner, who accepted it on COL-01 (#252) on 2026-10-06 as that ticket specifies. Each answer below that was once a question to the owner is the session's choice under the owner's 2026-10-04 delegation, recorded by the ticket the decision names.
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
   - No `colibri:` scheme is added.

   **Delegated answer (spec form):** (a), recorded by COL-01 (#252) on 2026-10-06. A colibri server is reached by the existing `openai-chat:<base>/v1#<model-id>` spec or a `provider:` entry, and identified from `GET /v1/models` (`owned_by: "colibri"`, decision 3); no `colibri:` scheme is added. It adds no spec scheme, no key variable and no CLI surface, and what identification turns on never changes where a key is sent. It would be reversed by a reported run in which a server that is not colibri answered `owned_by: "colibri"` and the profile it then received changed that run's outcome; a `colibri:` scheme the user writes would then replace identification.

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

6. **The byte budget leaves room for the reply (amends ADR 0024 §5).** An identified colibri endpoint with no `[model] context_bytes` gets a default below the hosted one (`HOSTED_CONTEXT_BYTES`), sized so that the prompt leaves the reply room in colibri's window: colibri clamps each reply to the room the prompt leaves (c/colibri.c:9387-9413), so a prompt that fills the window leaves a turn no room for its tool call. COL-08 records the figure, the session's choice under the owner's 2026-10-04 delegation. An explicit value wins, larger or smaller. doctor states that colibri does not report its window.

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

9. **The per-request timeout becomes configurable**, for the tier-1 and runtime clients alike and still bounded by the run's wall-clock budget, in the form COL-06 records (proposed: `[model] timeout_s`, default 300 s), the session's choice under the owner's 2026-10-04 delegation.

   **Delegated answer (timeout):** (a), 2026-10-04, recorded by COL-06 (#264) on 2026-10-06. The per-request timeout is `[model] timeout_s`, one key for every runtime, in seconds, default 300 s; when both the repository and the user set it, the lower of the two values wins, as for `[checks] timeout_s` (ADR 0004), so a repository can only shorten what the user set; 0 is refused at load. The run's wall-clock budget (ADR 0008) still bounds the whole run, and the probe clients keep their own short bounds (the 10 s of decisions 3 and 4), which this key does not change. (b), a fixed figure for colibri, would be a guess at a machine's disk speed; (c), streaming in place of a timeout change, is left to COL-18 (decision 10), and a stream's idle timeout would sit beside this key rather than replace it. Built with #265, which reads the key from config (`RepoConfig::model_timeout`, crates/trellis-cli/src/config.rs), and #266, which applies it to the run and ask clients (`Driver::build` in crates/trellis-cli/src/run_cmd.rs, `hosted::build` in crates/trellis-cli/src/hosted.rs, both clients in crates/trellis-cli/src/ask.rs); evidence: the `model_timeout_applies_` tests in crates/trellis-cli/tests/model_timeout.rs. It would be reversed by COL-18 recording streaming and its idle timeout being shown to end a hung request on every runtime this key serves, or by a repository-set value shown to hold a run past the wall-clock budget the user set.

10. **Streaming is not built by this ADR.** #151 stands until the floor run (decision 12) reports the model-call time per turn; COL-18 records the answer, the session's choice under the owner's 2026-10-04 delegation. If streaming is built after that, it covers loopback colibri only, with an idle timeout reset by colibri's 10 s keepalives, and the loop still acts only on a complete tool call.

11. **`trellis model pull` does not fetch colibri containers.** The catalogue stays one pinned GGUF file per entry (ADR 0044); colibri downloads, converts and verifies its own shards.

12. **One colibri configuration is measured against the floor** (BENCHMARK_AND_EVAL.md §5) at tier 1 and temperature 0. It is reported as model × container precision × colibri version × backend, with the serve command, machine and cache state beside it. The model is Qwen3.8-Flash-Next; COL-12 measures it and records the configuration, the session's choice under the owner's 2026-10-04 delegation.

13. **Not adopted now:**
    - `/v1/brio` for `trellis ask`: the current classifier is measured on colibri first, and a score would only choose a code path, never become a `Verdict`.
    - A runtime record in the trail: colibri does not report the settings that would make one worth a format change.

    COL-15 and COL-16 record the answers, the session's choice under the owner's 2026-10-04 delegation.

## Alternatives rejected

- **A `colibri:` scheme.** It would be explicit in the trail's spec string, but it grows the spec surface and needs a key variable of its own. Identification gives the same request profile with no new surface. Rejected by decision 2's delegated answer (a) (#252).
- **A static table of colibri's families**, keyed by default model id. The operator's `--model-id` breaks the key, and the table would claim tool support and a context window the server never stated. The probe asks the server.
- **A probe that costs nothing**, sending `tools` together with `n: 2` (rendering refuses `tools` before `generation_options` refuses `n`). It depends on the order of colibri's validation, which can change without notice, and such a change would become a false Native claim on every family.
- **Tier 2 through `response_format`.** colibri's grammar never constrains sampling (finding 1). Asking upstream for a constraining mode buys little where native tools already work.
- **Tier 3 through colibri's segment or edge ABI** (why decision 1 links neither). Linking a C ABI needs a C toolchain. The build machine has none (docs/adr/0013-inference-path.md:39), TRELLIS takes on no C++ toolchain dependency until tier 3 is built (docs/adr/0013-inference-path.md:113), neither reopen trigger for tier 3 has fired (:123), and when tier 3 is built ADR 0013 §2 prefers candle (:81). Logits are not what is missing. The edge ABI is version 2 (`COLI_EDGE_ABI_VERSION 2u`, c/edge_runtime.h:26), and `coli_edge_logits` (:192) returns the complete row-major vocabulary logits (docs/edge-runtime.md:77-82), the access a tier-3 mask needs. colibri is the case ADR 0013 keeps tier 3 for, a runtime exposing logits without guided decoding (:155-158), since its grammar never constrains sampling (c/grammar.h:10-12). So the two ABIs are a candidate tier-3 substrate if a reopen trigger fires.
- **Temperature 0 for every loopback tier-1 endpoint.** A loopback proxy in front of a hosted reasoning model would forward it and draw the 400 that ADR 0024 §4 avoids.
- **Supervising `coli serve` from TRELLIS.** TRELLIS starts no runtime for any dialect, and a model load of minutes inside `trellis run` would turn a refused connection into a hung one.
- **ADR 0033's local byte budget (14,336 bytes) as colibri's default.** It is sized to the whole 4,096-token window, so a prompt held to it can leave a turn no room for its tool call (COL-08).

## Consequences

- A tier-1 run against colibri is repeatable at temperature 0. It refuses before its snapshot on a family without tools, on a wrong model id or on a stopped server, and its default budget leaves the reply room unless told otherwise.
- doctor tells a user which colibri model is loaded, how many slots it serves and where its experts live, and says what colibri does not report.
- Until the timeout is configurable, any turn longer than 300 s ends the run, and on most machines colibri targets, turns are minutes long. **Amended 2026-10-06 (#484):** it is configurable. `[model] timeout_s` (decision 9's delegated answer) is read by #265 and applied by #266 to the hosted and runtime clients alike. It defaults to 300 s, the lower of the repository's and the user's value wins, and 0 is refused at load. A turn longer than 300 s ends the run only where the key is unset, and the run's wall-clock budget still bounds the run.
- Eliding old observations (ADR 0024 §5) rewrites the middle of the prompt, which costs colibri a re-prefill from the first changed token. Elision is monotone, so each observation is rewritten at most once.

## What is not built

- Everything above: this ADR is accepted (COL-01, #252) and not built; each behaviour waits for its own ticket in story S14.
- Streaming (decision 10), a `/v1/brio` client, a runtime record in the trail, and colibri weights in the catalogue.
- Version, family and context window: colibri does not expose them, so TRELLIS does not report them.
- **A `cache_slot` on requests to colibri** (COL-ENG-29, COL-UI-15). TRELLIS sends none (crates/trellis-model/src/hosted.rs:194-277, `request_body` and `openai_body`), and colibri pins such a request anyway. When `kv_slots` > 1 it hashes the leading system messages and the first user message to a slot (`conversation_cache_slot`, c/openai_server.py:2910-2938, applied to chat and Anthropic requests at :4870-4876 and :5297-5303), so every turn of one conversation lands on one slot and reuses that slot's cached prefix. colibri's docs/api.md:380-382 still says an omitted `cache_slot` keeps slot 0; that is out of date against the code, which has hashed since colibri's #634. TRELLIS runs one agent per run (`--agents` other than 1 is refused, crates/trellis-cli/src/main.rs:658-661; ADR 0045 is accepted and not built, #101). A run therefore sends one conversation, the hash already keeps it on one slot, and a `cache_slot` would change nothing. **Amended 2026-10-06 (#482):** that parenthetical is out of date. `--agents N --target ...` is built (#465, #466; ADR 0045's amendment of this date): above width 1 a run sends one conversation per partition, up to N at once, each hashed to a slot as above, so two can share one slot and wait on each other (the collision cost below). TRELLIS still sends no `cache_slot`; a run with no target still runs one agent and sends one conversation.
  - What a collision costs, once `--agents N` exists. A request with a slot, hashed or sent, waits for that slot only, even while others are free (`_available_slot`, c/openai_server.py:199-209), so two partitions hashed to one slot run their turns one after another. TRELLIS's 300 s request timeout (crates/trellis-model/src/hosted.rs:687, finding 8) covers that wait and is not retried (hosted.rs:859). colibri's `--queue-timeout` (300 s by default, c/openai_server.py:5630-5631) starts later, at admission, so its 429 `queue_timeout`, which TRELLIS retries up to `MAX_ATTEMPTS` (4) attempts in all (hosted.rs:490, :803), can arrive first only once the request timeout is raised (decision 9). Each turn also replaces the other partition's state on that slot: on GLM-5.2/5.3 the next turn re-prefills everything after the prefix the two prompts share (c/colibri.c:9685-9691), and on GLM-5.3-Flash a prompt that does not extend the slot's cache starts over (c/glm53.c:3240-3252).
  - How likely a collision is. With N partitions hashed to independent, uniform slots out of 16, the chance that at least two share one is `1 - 16!/((16-N)! * 16^N)`: 6.25% for 2, 33.3% for 4 and 87.9% for 8. Partitions whose system prompt and first user message are byte-identical always share a slot; whether they are depends on the prompt #101 gives each partition. On every family except GLM-5.2/5.3 and GLM-5.3-Flash, `max_kv_slots` is 1 (c/family_registry.py:1125-1432), so every request is on slot 0 and no `cache_slot` can separate partitions.
  - Sending one would replace the hash with a partition index. That waits on `--agents N` existing (#101) and on a measured multi-agent run on a 16-slot family (see What would change our mind).
  - **Amended 2026-10-06 (#484):** three things above are out of date. First, `--agents N` exists (#465, #466, #482), so "once `--agents N` exists" and the first of the two conditions just above are met. The second is not: no multi-agent run has been measured on a 16-slot family, so a `cache_slot` still waits. Second, #482's "up to N at once" holds only for some families. Since #483 an explicit N, like `auto`, keeps N only when `doctor_cmd::probe_batching` reports continuous batching; otherwise the partitions run one after another (width 1), and the `agents:` line says why. For colibri that answer comes from `/health`'s `kv_slots` (decision 7, built by #267, `colibri_batching`). Above 1 it is continuous batching with that count, so on GLM-5.2/5.3 and GLM-5.3-Flash a run sends up to N conversations at once (fewer when the plan has fewer parallel partitions, ADR 0045 §4). An explicit N is not capped at the slot count, and `auto` takes the count. A count of 1, which every other family reports, is single-stream. A `/health` without `kv_slots`, or one that could not be read, is unknown. Both of those run at width 1. The count is colibri's configured slots, which TRELLIS takes as the measurement; nothing has measured what the slots sustain under load. Third, the 300 s request timeout in the collision bullet is now `[model] timeout_s`'s default (see the Consequences amendment of this date), so the request timeout can now be raised.
- No colibri server has been run against TRELLIS. Every behaviour above was read from colibri's source at ce370e8, and the contract tests use bodies transcribed from that source.

## What would change our mind

- If colibri reports its version, family, context window and lossy settings on `/health`, doctor would print them, the byte budget would follow the reported window, and the trail question would reopen.
- If colibri adds a mode in which `response_format` constrains sampling, tier 2 would apply to colibri.
- If the floor run's model-call times stay under the default timeout, decision 9 would matter less and streaming would stay out.
- If a multi-agent run (#101) on a 16-slot family shows a nonzero queue wait while /health shows a free slot, in the `x-colibri-queue-wait-ms` figure that COL-10a (#255) reads, partitions are colliding on a hashed slot, and TRELLIS would send each partition's index as `cache_slot`.
