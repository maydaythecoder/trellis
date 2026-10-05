# colibri-runtime track: team analysis

Source read: colibri v1.12.1 (github.com/JustVugg/colibri, commit ce370e8, Apache-2.0) and the TRELLIS tree at 612115d (drain). Nothing was run, so every claim below comes from reading files, with the file and line given.

## Product Manager

**The user and the need.** colibri runs mixture-of-experts models from 7B to 2.8T total parameters on one workstation. It keeps the dense weights resident and streams the routed experts from SSD through RAM and an optional VRAM tier. Six of its nine families have native tool calling. Every configuration TRELLIS has measured against the floor is 3B or smaller, and every one sits below the floor (docs/BENCHMARK_AND_EVAL.md §5.2). colibri is the first way to run large open models on the user's own hardware, which is the BRIEF's premise, at tier 1.

**What the integration is.** TRELLIS acts as a client of a running `coli serve`. It does not reimplement inference (ADR 0013 §1), it does not start, stop, build or tune colibri, and it does not show colibri's dashboard: desktop and web surfaces stay excluded under #70, and the desktop track's ADR 0048 is where they would go.

**What a user hits today if they point TRELLIS at colibri** (traced through the code; not run):
1. `openai-chat:http://127.0.0.1:8000/v1#glm-5.2-colibri` parses, and loopback http is allowed. If `OPENAI_API_KEY` is set, it is sent to colibri (ADR 0024 §3).
2. `trellis doctor` prints Native and "concurrency not reported" without asking the server anything (doctor_cmd.rs:75-84).
3. Hosted specs get no reachability check before the snapshot (run_cmd.rs:659 covers runtime specs only). So a stopped colibri, a model id typo (404 on every POST, openai_server.py:4256-4259), or Inkling/OLMoE/Qwen3.6 (400 on `tools`, :1364, :1423, :1707) each fail after the snapshot, which costs a rollback and leaves a trail file.
4. No `temperature` is sent (hosted.rs:263-275), so colibri samples at 0.7 (:3136-3147). With `[model] thinking = off`, Qwen3.8 still thinks at `xhigh` (:5175-5185).
5. One reply takes minutes: colibri's own figures are a 140 s median for a 128-token request on a 24-core, 61 GiB machine (docs/qwen38.md:139-151) and about 20 s per token warm for GLM-5.3-Flash on an ordinary disk (docs/glm53-flash.md:83-91). The 300 s request timeout is not retried (hosted.rs:689), so the run ends on its first long turn.
6. If a run survives that, its budget is 400,000 bytes (trellis-cli hosted.rs:247), far above colibri's 4,096 or 8,192-token window. Replies near the window are clamped into the room left (:3152-3160), the tool call is cut off, and the format retry resends the same request into the same cut.

**Scope.**
- **In:** identification and doctor; a tools and served-id probe before the snapshot; a request profile (temperature 0, explicit thinking); a local byte budget; a longer timeout; per-turn tokens per second and queue wait in the TUI; an operator guide; one measured floor row.
- **Questions for the owner:** the spec scheme, the form of the timeout, streaming, the weight catalogue, a runtime record in the trail, and brio for `trellis ask`, plus the floor configuration and machine.
- **Out:** colibri's own CLI tools (plan, tune, convert, mirror, bench, cluster), engine internals, GPU backends, the dashboard, and audio.

**Success, observably.**
- doctor's new lines appear against a colibri-shaped stub.
- No post-snapshot failure from a cause the probe can catch.
- Remote provider bodies are byte for byte unchanged.
- §5.2 gains one colibri row with its method.

**Product risks.**
- Turns last minutes, so without tokens per second (and possibly streaming) the product looks hung.
- Some families' weights carry non-Apache licences, such as the Qwen Community License for Qwen3.8.
- The build machine has 15.2 GB of RAM, below what most families need.
- colibri publishes no version, so a contract change upstream shows up only as a failing run.

## Principal System Architect

**Tier mapping.**

| Path | Tier | Notes |
|---|---|---|
| `openai-chat:<base>/v1#<id>` or `provider:` | 1 Native | GLM-5.2/5.3, GLM-5.3-Flash, Kimi K3, DeepSeek V4 and V4.1, Qwen3.8. Tool calls go through colibri's per-family renderers and parsers. |
| `anthropic:<base>#<id>` | 1 Native | Same families. Images are refused. Thinking needs an explicit `{type: "enabled"}`. |
| any spec, `response_format` or `GRAMMAR` | never 2 | The grammar is a draft source whose spans the model verifies. It never constrains sampling (c/grammar.h:10-12; openai_server.py:3097-3101). `guided_json` is ignored. |
| tier 3 | not reopened | ECHO's top-k logprobs are not a mask hook. The segment and edge C ABIs expose greedy selection only and need a C toolchain (ADR 0013). |
| `vllm:<base>#<id>` | 4 | `/v1/completions` takes the prompt with no chat template. The schema probe already drops the run to tier 4 (run_cmd.rs:697-713). |
| Inkling, OLMoE, Qwen3.6 | no tier 1 | Refused at render time. OLMoE and Qwen3.6 accept tools only with `COLI_TOOL_FALLBACK=1`. |

**Spec.** Reuse `openai-chat:` and `provider:`, and identify colibri from `GET /v1/models`, where it answers `owned_by: "colibri"` (:3885-3886). This adds no scheme, no key variable and no CLI surface (brief 4.12). Identification turns on only things that are harmless if a server lies about being colibri, and it never changes where a key goes. A `colibri:` scheme is the alternative put to the owner on COL-01.

**Placement.**
- `trellis-model/src/colibri.rs` (new) holds the response parsing, a `transport::get` (the transport has only `post` today, transport.rs:396) and the tools probe.
- `trellis-cli` holds the policy: when to identify (loopback bases only), what to refuse, what doctor prints.
- `trellis-loop` passes the request profile through and carries the new `turn` event fields.
- No crate dependency is added. trellis-index, trellis-vault and `Verdict` are untouched. The only new configuration surface is the timeout key, which waits on the owner.

**Order before the snapshot.** Build the driver, then identify (for a loopback hosted base this also serves as the reachability check), then the tools and served-id checks, then the profile and budget, then the existing schema probe for runtime specs, then the snapshot. Every refusal costs nothing, which is the rule run_cmd.rs already states for reachability.

**ADR 0024 lines loosened, for an identified colibri endpoint only:**
- §2: Native without a probe. The reason given was spending the user's money, which does not apply on loopback.
- §4: no temperature. colibri accepts 0, and ADR 0028 needs 0.
- §5: the 400,000-byte hosted default. It becomes ADR 0033's 14,336 bytes.

ADR 0047 records all three.

**Context and caching.**
- colibri reuses KV prefixes. ADR 0024 §5 elides oldest observations first, which rewrites the middle of the prompt; elision is monotone, so each observation is rewritten at most once, and each rewrite costs one re-prefill from that point.
- Only the GLM families serve more than one KV slot.
- doctor maps `kv_slots` to `BatchingCapability`, as it does llama.cpp's `total_slots`, and says the count is configured, not measured.

**Timeout and streaming.** The fixed timeout is the blocker. Streaming stays out under #151 until COL-12 measures the turn wait. If it is built, it covers loopback colibri only, with an idle timeout that colibri's 10 s keepalives reset, and the loop still acts only on a complete call.

**Trail.** It keeps recording the spec string. colibri does not expose the settings that change its output (precision, routing, lossy modes), so a format change waits until it does (COL-15).

**Reversibility.** High. With no colibri server present, every code path is today's, and remote endpoints are never identified or probed.

## Senior Full-Stack Engineer

**Where the code changes.**
- `crates/trellis-model/src/transport.rs:396`: add `get` beside `post`, with the same `MAX_RESPONSE` cap, Host header and optional key header.
- `crates/trellis-model/src/hosted.rs`:
  - 263-275: the openai body (no temperature).
  - 325-337: the anthropic body (`adaptive`).
  - 380: the message for a reply with no tool call (COL-07).
  - 689: the 300 s timeout.
- `crates/trellis-cli/src/hosted.rs`:
  - 247 and 288: the byte budget.
  - 338-369: `doctor()`.
- `crates/trellis-cli/src/doctor_cmd.rs:75-84`: Native with no probe.
- `crates/trellis-cli/src/run_cmd.rs`:
  - 656-683: the reachability check (runtime specs only).
  - 697-713: the schema probe.
- `crates/trellis-cli/src/tui_run.rs:93-117`: `tier_from_probe`.
- `crates/trellis-loop/src/adapter.rs:1227`: `HostedModel`, which already has `with_context_bytes`.
- `crates/trellis-loop/src/agent.rs:1017-1029`: the `turn` event.
- `crates/tui-demo/src/tui/app.rs:122-128`: `Budget`.

**colibri wire facts the code must handle.**
- Errors come in the OpenAI envelope `{error: {message, type, param, code}}`.
- 404 `model_not_found` on any POST that names another model id.
- 400 `unsupported_parameter` with `param: "tools"`, raised while rendering, before the engine runs.
- 400 `context_length_exceeded`, which `is_context_overflow` already matches.
- 429 `queue_full` and `queue_timeout`, which are already retried.
- `/v1/models` needs authentication when colibri has a key. `/health` shows only `{status: ok}` to an unauthenticated caller.
- `/health` is at the server root, but an openai-chat base ends in `/v1`, so both reads go to the origin.
- A Host-header guard admits loopback names.
- `usage` is present on every reply. `reasoning_content` is dropped from replay (TRELLIS keeps role, content and tool_calls).
- Unverified: whether colibri accepts the `content: null` that TRELLIS sends on an assistant turn it rebuilt. The contract fixture should include one.

**Tests.** Loopback `TcpListener` stubs, following `tests/hosted_keys.rs`, answering with bodies transcribed from openai_server.py at ce370e8, each body naming its source line. Test names carry fixed prefixes so `scripts/tickets/verify-tests.sh` can count them. CI has no live colibri.

**Sizing.**
- s: COL-07, COL-14, COL-15.
- m: COL-02 to COL-06 and COL-08 to COL-11.
- COL-12 is mostly machine hours plus a write-up.
- COL-13 is m if the owner chooses it.

**Pitfalls.**
- Remote bodies must not change. The existing assertions that no `temperature` is sent and that thinking is `adaptive` must pass unchanged.
- Never identify a non-loopback base. That would be new traffic to a provider.
- Keep colibri logic in `colibri.rs` and `hosted.rs` functions, not inline in run_cmd.rs (3,246 lines).
- Any ticket touching `crates/tui-demo/**` routes to a person, because that path maps to no area in the ticket rules.

## QA / Security Engineer

**Threats and mitigations.**
1. **Spoofed identity.** Any loopback process can answer `owned_by: "colibri"`. The effect is limited to temperature 0, explicit thinking, a one-token probe, the local byte budget and doctor's text. A probe can only lower a tier claim; it never changes where a key is sent. Tests: a non-colibri stub receives byte-identical bodies, and a non-loopback base is never contacted.
2. **Terminal injection.** The model id, the hwinfo CPU and GPU names and error messages are bytes the server controls, so all of them pass through `crate::plain`. Tests use an ESC byte in the model id.
3. **Key exposure.** Under ADR 0024 §3, a set `OPENAI_API_KEY` goes to a loopback endpoint the user named. COL-09's guide recommends a `[[providers]]` entry with `key_env = "COLI_API_KEY"`. The identification GETs follow the same rules, are never logged with the key, and `tests/hosted_keys.rs` and `tests/child_env.rs` must keep passing. A repository-chosen loopback endpoint is sent no key unless the user chose it.
4. **Response bounds.** GETs share the 32 MiB cap. `/health` parsing tolerates missing fields and never panics on bad numbers, which makes it a candidate fuzz target.
5. **Trail.** Unchanged, and read back as untrusted. COL-15 keeps colibri's self-report out of the trail.

**Grounding contract.**
- Nothing colibri returns produces a `Verdict`.
- Tier-1 calls still go through `ToolSet::parse_value`.
- A brio score could only route a code path, never judge a change.
- The new timeout applies per request inside the run's wall-clock budget, so budgets still divide and never multiply.

**Reproducibility hazards.**
- Greedy output is stable within a kernel family, but GPU float paths are not token-identical to CPU (GPU_BACKENDS.md), and batch shape can change results.
- Prefix reuse and `.coli_kv` warmth change wall-clock time.
- MTP and n-gram drafts are verified, so they change only speed.
- `COLI_TOOL_SALVAGE=1` rewrites malformed GLM tool calls on the server, where TRELLIS cannot see it.
- Lossy settings (TOPP/TOPK, CACHE_ROUTE, DEGRADE_ZERO, KV8, KV_TQ, reduced load-time precision) are invisible over HTTP.

So COL-12 records the serve command, environment, backend and cache state, uses `--kv-slots 1` and `KVSAVE=0`, and runs at temperature 0, where ADR 0028 waives repetitions.

**Contract drift.** There is no version endpoint, so the fixtures are pinned to ce370e8. When COL-12 runs, each fixture body should be compared with the live server and any difference noted in the report.

**Not checkable without a live server:** per-family tool-call quality, template behaviour, and real prefill and decode times. COL-12 is the only live check. No claim that colibri is suitable should be written before it.

## Next Steps & Action Plan

1. **Owner:** accept or amend ADR 0047 (COL-01), including the spec question, and answer COL-06's timeout question at the same time, since COL-12 depends on both.
2. **Reader and doctor:** COL-02, then COL-03 (doctor) and COL-04 (pre-snapshot probe and refusals) in parallel.
3. **Sound runs against colibri:** COL-05 (temperature 0 and thinking), COL-08 (byte budget), COL-07 (length-limit message, unblocked now) and COL-06 (timeout, after the owner).
4. **Legible long turns:** COL-10, then COL-11.
5. **Operator guide:** COL-09, once COL-05, COL-06 and COL-08 have landed.
6. **Measurement:** the owner picks COL-12's configuration and machine. Run `docs/bench/run_floor.py` at tier 1 and temperature 0, record the method, per-turn model-call times and bytes per token, and compare the contract fixtures with the live server.
7. **With COL-12's figures:** answer COL-13 (streaming). Independently: COL-14 (catalogue), COL-15 (trail) and COL-16 (measure ask's classifier).
8. **Not planned:** 52 inventory features are listed in uncovered_features, each with its reason (engine internals, colibri-side tooling, GPU backends, desktop surfaces, distribution).
