# colibri engine and serving side: capability inventory (v1.12.1, Apache-2.0)

**Upstream:** `JustVugg/colibri` at commit `ce370e87d7b623d7759b52ec2007d75fc5b0e87e` (2026-09-25, "Merge pull request #1663 from JustVugg/dev"), version 1.12.1 (`c/version.py`). Read from the clone at `C:/Users/mayda/.claude/jobs/366105fd/tmp/upstream/colibri`.
**Scope:** the engine and serving side: `c/`, `colibri/`, `docs/`, `docker/`, `Makefile`, `flake.nix`, `pyproject.toml`, `README*`, `CHANGELOG.md`, `GPU_BACKENDS.md`, `.github/workflows/`. `web/`, `desktop/` and `site/` are out of scope. The one exception is the data the Brain and Profiling pages read, which comes from server endpoints and is covered here.

## Method

- **Sources read.** README.md in full, and `docs/api.md`, `brio.md`, `serve_protocol.md`, `SETTINGS.md`, `tuning.md`, `grammar-draft.md`, `corpus-draft.md`, `CACHE_ROUTE.md` and `segment-runtime.md`/`edge-runtime.md` (opening sections). From `quickstart.md`, `benchmarking.md` and `routing-telemetry.md`, the sections in use. `ENVIRONMENT.md` was read by section: all 277 variables are covered here grouped by mechanism, not one row each. For the per-model docs, `cuda.md`/`vulkan.md`/`metal.md`/`windows.md`, `FORMATS.md` and `int4-rans256-g0.md`, headings plus targeted reads. `GPU_BACKENDS.md`; `docker/*`; `flake.nix`; `pyproject.toml`; `colibri/*.py`; `release.yml` and the `ci.yml` job list. The CHANGELOG entries for 1.12.1, 1.12.0, 1.11.0 and the 1.7 "Interfaces" section. In code: the `c/coli` argparse definitions and the handlers for run, chat, serve, stop, info, build and bench. In `c/openai_server.py`: routes, `generation_options`, the Anthropic translation, brio, systemone, the stdout dispatcher, the scheduler and `serve()`/`main()`. `c/family_registry.py` (dataclasses and the nine descriptors), the `c/grammar.h` header, the grammar call sites and the TOPK/TOPP readers in `c/colibri.c`, and the docstrings of `resource_plan.py`, `doctor.py`, `autotune.py`, `cluster.py`, `tools/README.md` and `tools/expert_atlas/README.md`.
- **TRELLIS side read.** `docs/BRIEF.md`, `docs/MISSION.md`, ADR 0021 and the three inventories, and ADRs 0006, 0013, 0024, 0033 and 0041. In code: `crates/trellis-model/src/{lib,spec,runtime,tier,hosted,catalog,pull}.rs`; in `crates/trellis-cli/src/`, `config.rs` (ModelSection), `doctor_cmd.rs` (schema_report, timeouts) and `run_cmd.rs` (the constraint probe at run start); and `crates/tui-demo/src/tui/app.rs` (AppState).
- **Granularity.** There is one row per `coli` subcommand, with every flag listed, plus one row for the shared flag set. There is one row per HTTP route and one per cross-cutting API behaviour. There is one row per model family, and one row per mechanism for engine features, with their environment variables listed.
- **Status (TRELLIS).** For serving-side rows, the status describes TRELLIS as a *client* of the feature. TRELLIS serving an API of its own is excluded by BRIEF Part 1 ("Any networked component"). `excluded` is used only with a clause:
  - BRIEF Part 1 "Any networked component", for colibri's server, cluster and metrics endpoint;
  - BRIEF Part 1 "Any surface beyond the CLI" plus ADR 0021 "Owner decisions of 2026-09-28" (#70), for the dashboard and its data.
- **Engine internals are `missing`, not `excluded`.** This covers kernels, tiers, quantisation, speculation and formats. ADR 0013 §1 builds tier 2 "with no inference code in TRELLIS at all" and `trellis-model/src/lib.rs` says tier 3 is "deliberately not built". But ADR 0013 rejects "skip tier 3 permanently", so no exclusion clause exists for them.
- **Disposition.**
  - `drive-via-runtime`: the capability stays in colibri, and TRELLIS reaches it by pointing a model spec at a running `coli serve`. Any small TRELLIS-side change needed to use it is named in the notes.
  - `port`: a client-side reader the harness itself should own.
  - `desktop-ui`: belongs to a front end outside the CLI, currently excluded by ADR 0021 #70.
  - `already-built`: TRELLIS already has the equivalent and nothing new is needed.
  - `owner-decision`: an ADR or product choice is needed before anything is built.
- **Nothing was run.** Every claim comes from reading files. Upstream performance figures are upstream's own measurements.

## Licence

Apache-2.0, Copyright 2026 Vincenzo Fornaro (`LICENSE`, `NOTICE`). `THIRD_PARTY_NOTICES.md`:
- parts of `c/backend_cuda_dsv4.cu` are adapted from ds4 (MIT);
- the DeepGEMM sm120 headers (MIT) and CUTLASS (BSD-3-Clause) are *fetched* at build time by `c/tools/fetch_deepgemm.sh`, not vendored;
- vLLM is run as an oracle and not vendored.

TRELLIS is also Apache-2.0 (`LICENSE`, `Cargo.toml`, `deny.toml`). Porting code would only require keeping the NOTICE; driving colibri as a runtime imports no code. Weight licences are per model and were not verified here, except that README says Z.ai releases GLM-5.2 under MIT.

## Findings that decide most dispositions

1. **colibri is never a tier-2 runtime.**
   - `response_format` (`json_object`/`json_schema`/`gbnf`) and the `GRAMMAR`/`SCHEMA` variables are a *speculative-draft source*. They never constrain sampling (`c/grammar.h:1-35`; `c/openai_server.py:3097-3101`; `c/colibri.c:9528-9532`, greedy only), and only the GLM-5.2/5.3 engine accepts the payload at all.
   - TRELLIS's `vllm:` dialect sends `guided_json`, which colibri ignores. `trellis run` probes the schema before the run and rebuilds the driver unconstrained when it is ignored (`crates/trellis-cli/src/run_cmd.rs:689-715`, `doctor_cmd.rs:13-30`).
   - colibri therefore runs as tier 1 (native tools) or tier 4, and TRELLIS reports tier 4 truthfully.
2. **Tier 1 works on six of nine families.** Per the registry's `FamilyCapabilities` (`family_registry.py:1085-1440`), tools are supported on GLM-5.2/5.3, GLM-5.3-Flash, Kimi K3, Qwen3.8, DeepSeek V4 and V4.1. Inkling, OLMoE and Qwen3.6 answer 400 to `tools` unless `COLI_TOOL_FALLBACK=1` (OLMoE, Qwen3.6). TRELLIS `doctor` reports `Native` from the protocol without a probe (ADR 0024 §2), so it would claim tier 1 against those three families.
3. **Spec strings.**
   - `openai-chat:http://127.0.0.1:8000/v1#<model-id>`
   - `anthropic:http://127.0.0.1:8000#<model-id>`
   - tier 4: `vllm:http://127.0.0.1:8000#<model-id>`
   - The `#<model-id>` is mandatory: `check_model` answers 404 to any other name (`openai_server.py:4256`), and `vllm:` sends `"default"` without it (`runtime.rs:203`).
   - `/v1/completions` passes the prompt to the engine **without a chat template** (`openai_server.py:5531-5538`). Ollama, the one measured tier-4 runtime (ADR 0033), applies one.
4. **Loopback on both sides.** colibri has no TLS and refuses a non-loopback bind without `COLI_API_KEY` (`openai_server.py:5540-5562`). TRELLIS accepts plain `http://` for hosted protocols only on loopback (ADR 0024 §3), so a remote colibri needs a TLS terminator in front.
5. **Context.**
   - Default context is 4,096 tokens for GLM, OLMoE (also its maximum), DeepSeek V4 and V4.1, and 8,192 for the others (`family_registry.py` limits). TRELLIS's minimum window is 4,096 tokens (ADR 0033).
   - colibri clamps `max_tokens` to whatever room the prompt leaves. A near-full prompt therefore gets a short reply ending in `finish_reason: "length"`, which reaches TRELLIS as a malformed tool call.
   - `--ctx` has to be raised. The 3.853 bytes/token ratio is Ollama-only and should not be reused for colibri.
6. **Timeouts against prefill.**
   - TRELLIS uses a fixed 300 s timeout per request (`runtime.rs:141`, `hosted.rs:689`) and reads replies whole; streaming is not built (ADR 0021, W-C).
   - colibri sends keepalives only on SSE. Upstream documents CPU-streaming prefill at a few tokens per second (`docs/api.md`) and about 0.9 s/token on Qwen3.6 (`docs/brio.md`). At that rate, a 4,096-token first request would far exceed 300 s on a disk-streaming host (derived estimate, not measured).
   - Later turns reuse the KV prefix. TRELLIS's elision of the oldest observations (ADR 0024 §5) rewrites that prefix and loses the reuse.
7. **Sampling.** TRELLIS's tier-1 body sends no `temperature` (`hosted.rs:263`), so colibri samples at its `COLI_TEMP` default of 0.7. For greedy runs, start `coli serve --temp 0`.
8. **Thinking.** TRELLIS's `openai-chat:` path sends `reasoning_effort`, which colibri accepts. Its `anthropic:` path sends `thinking: {type: "adaptive"}` plus `output_config` (`hosted.rs:335-337`), and colibri enables thinking only for `type: "enabled"` (`openai_server.py:5251-5254`), so thinking is silently off on that path.
9. **Concurrency.** `max_kv_slots` is 16 for GLM and GLM-5.3-Flash and 1 elsewhere. `/health`'s authed scheduler snapshot carries `capacity` and `kv_slots` (`openai_server.py:211-218,4318-4331`), but TRELLIS has no reader for it, so `vllm:` yields `ContinuousUnmeasured` (S18).
10. **The precision guarantee is invisible to TRELLIS.** colibri's default policy never silently changes precision or routing. But lossy knobs (`--topp`/`--topk` expert pruning, `DEGRADE_ZERO`, `CACHE_ROUTE`, load-time `*_BITS`) are operator-side, not exposed over HTTP, and not recorded in the TRELLIS trail, which records only the spec string.
11. **Brio fits `trellis ask`.** Brio mode (closed-set scoring with an entropy) matches the shape of `trellis ask`'s intent classification. It must never become a `Verdict` (ADR 0004).

## Features

| ID | Feature | What | Upstream evidence | TRELLIS status (evidence) | Disposition |
|---|---|---|---|---|---|
| COL-ENG-01 | coli common flags | `--model --ram --auto-tier --no-tune-profile --ctx --gpu --vram --policy --repin --cap --ngen --topp --topk --effort --think/--no-think --temp --cluster-workers --cluster-coordinator --version`, mapped to engine env | c/coli:2557-2594; docs/SETTINGS.md | missing (ADR 0013 §1; trellis-model/src/lib.rs) | drive-via-runtime |
| COL-ENG-02 | Launcher dispatch | engine and template picked from config.json via the registry; COLI_ENGINE; source vs libexec layout; RLIMIT_NOFILE; UTF-8 console; readline | c/coli:1-125,388-430; c/family_registry.py | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-03 | coli chat | REPL; auto-attach to :8000 (`--attach --no-attach --api-key --stats`); /brio /reset /help /quit, TAB; thinking shown; pasted image paths as data: URIs; .coli_kv resume notice; COLI_CHAT_STATS, COLI_SHOW_THINK, COLI_COLOR, COLI_RAW | c/coli:1442-1460,1490-1733,1735-1854,1856-2087 | partial (session.rs, tui_run.rs, tui_bridge.rs, tui-demo app.rs; images only on `run --image`, ADR 0041) | already-built |
| COL-ENG-04 | coli run | one-shot prompt per family; unwired for Qwen3.6, Qwen3.8, V4.1 | c/coli:1378-1440 | missing (`trellis run` is the agent loop, BRIEF 4.12) | drive-via-runtime |
| COL-ENG-05 | coli serve | `--host --port --model-id --api-key --cors-origin --max-queue --queue-timeout --kv-slots --allowed-host`; pidfile; cluster discovery | c/coli:2088-2121,2637-2647 | excluded as a TRELLIS server (BRIEF Part 1 "Any networked component") | drive-via-runtime |
| COL-ENG-06 | coli web | serve plus browser (`--no-browser`), same flags, serves web/dist | c/coli:2300-2350,2664-2676 | excluded (BRIEF Part 1 "Any surface beyond the CLI"; ADR 0021 #70) | desktop-ui |
| COL-ENG-07 | coli stop | stop a serve by port (`--port --dry-run`): pidfile, /proc scan, Windows liveness | c/coli:2122-2157,2246-2299 | missing (TRELLIS runs no runtime; runtime.rs `reachable`) | drive-via-runtime |
| COL-ENG-08 | coli info | geometry, shards, RAM, disk, engine built, config.json guidance | c/coli:1175-1235 | missing (tier.rs Probe::report is harness-side) | drive-via-runtime |
| COL-ENG-09 | coli build, setup.sh, make targets | per-family build; self-test 30-32/32; test/check/asan/fuzz/segment/gpu targets | c/coli:1166-1173; c/setup.sh; Makefile; c/Makefile | missing (ADR 0013 clean-machine rule) | drive-via-runtime |
| COL-ENG-10 | coli plan and planner | VRAM/RAM/disk plan (`--json`), reasons, bottleneck, next_actions, GPU discovery, `--auto-tier`, .coli_analysis.json cache | c/coli:1236-1254; c/resource_plan.py | missing (tier.rs BatchingCapability only) | drive-via-runtime |
| COL-ENG-11 | coli doctor | readiness (`--json`; `--deep` headers, shards, tensors, index, mirror) | c/coli:1255-1296; c/doctor.py | partial (doctor_cmd.rs probes tier, schema, batching, checkers) | drive-via-runtime |
| COL-ENG-12 | coli tune | measured profile (`--prompt --rotate-prompt --tokens --repeats --timeout --min-gain`); quality gates; fingerprinted profiles | c/coli:1297-1377; c/autotune.py; docs/tuning.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-13 | coli mirror | plan/stage/verify a usage-ranked partial mirror (`--mirror --source-dir --usage --budget-gib --reserve-gib`); SHA-256; receipt | c/coli:2537-2556; c/tools/mirror_plan.py | partial (`model pull` SHA-256 at pinned revision, ADR 0044, pull.rs) | drive-via-runtime |
| COL-ENG-14 | coli convert and converters | resumable FP8→int4 (`--repo --ebits --io-bits --xbits --group-size --no-mtp`); per-family converters and repackers | c/coli:2458-2536; c/tools/convert_*.py, k3_repack.py, repack_*.py; c/scripts/ | missing (catalog.rs, pull.rs: one GGUF file) | owner-decision |
| COL-ENG-15 | coli bench | GLM-only hellaswag/arc/mmlu (`--limit --data`); quant ablation; PPL meter | c/coli:2351-2457; c/tools/eval_glm.py, quant_ablation.py | missing (BENCHMARK_AND_EVAL.md measures the harness) | drive-via-runtime |
| COL-ENG-16 | Local cluster mode | coordinator (`--host --port --stale-after --allowed-host`), workers (`--port --coordinator --node-id --advertise-host --layers --ebits --dbits`), TCP batch-union per layer | c/coli:2158-2245,2648-2661; c/cluster.py | excluded (BRIEF Part 1 networked) | drive-via-runtime |
| COL-ENG-17 | openai_server.py standalone | `--model --engine --arch --host --port --model-id --api-key --cors-origin --cap --max-tokens --max-queue --queue-timeout --kv-slots --allowed-host` | c/openai_server.py:5612-5659 | excluded as a TRELLIS server (BRIEF Part 1) | drive-via-runtime |
| COL-ENG-18 | POST /v1/chat/completions | messages, stream, max_tokens/max_completion_tokens, temperature, top_p, ≤4 stop, n=1, usage, reasoning_content, queue-wait header; refuses logprobs, penalties, n≠1; seed ignored | openai_server.py:3044-3166,4839-5160,5169-5241 | built (client: spec.rs `openai-chat:`, hosted.rs:263-276) | drive-via-runtime |
| COL-ENG-19 | OpenAI tool calling | tools/functions, tool_choice; native GLM, DSML, K3 XTML (TOOL sideband), Qwen3.8, GLM-5.3-Flash formats; COLI_TOOL_SALVAGE; COLI_TOOL_FALLBACK | openai_server.py:307-376,485-720,1131-1890 | partial (tier 1 client; doctor reports Native unprobed, ADR 0024 §2) | drive-via-runtime |
| COL-ENG-20 | POST /v1/messages | Anthropic: system, tool_use/tool_result, signed thinking blocks, tool_choice, named-event streaming with ping; refuses stop_sequences, top_k, images | openai_server.py:2713-2888,5243-5530 | built (client: `anthropic:`, hosted.rs:278-338) | drive-via-runtime |
| COL-ENG-21 | POST /v1/completions | legacy string prompt, no chat template | openai_server.py:5531-5538 | partial (`vllm:` dialect, runtime.rs:59-95,183-221; guided_json ignored, run drops to tier 4) | drive-via-runtime |
| COL-ENG-22 | SSE streaming | content/reasoning/tool deltas; include_usage; 10 s keepalives; headers after ACCEPT; Anthropic events; disconnect cancels | openai_server.py:4940-5150,5383-5530 | missing (ADR 0021 W-C: streaming not built; 300 s timeouts) | owner-decision |
| COL-ENG-23 | Reasoning controls | enable_thinking; reasoning_effort none..xhigh; COLI_THINK; Qwen3.8 xhigh default; OLMoE off; Anthropic `type: enabled` | openai_server.py:2623-2712,5169-5195,5251-5262 | partial (config.rs `[model] thinking`; hosted.rs:273-274,335-337) | drive-via-runtime |
| COL-ENG-24 | Sampling controls | COLI_TEMP (0 greedy), NUCLEUS 0.90, SEED, REP_PEN; gateway default 0.7/0.9 | docs/ENVIRONMENT.md; c/colibri.c:11453; openai_server.py:3133-3150 | partial (temperature 0 on tiers 2/4; none on tier 1) | drive-via-runtime |
| COL-ENG-25 | Stop sequences | ≤4 client stops filtered in JSON and SSE; implicit GLM role stops; x_colibri_ignore_leading_stop; COLI_SERVE_ALL_STOPS | openai_server.py:2889-3042 | missing (TRELLIS sends no stop) | drive-via-runtime |
| COL-ENG-26 | Assistant-turn continuation | trailing assistant turn continues; refused with tools; COLI_CONTINUE_ASSISTANT; advertised on /health | openai_server.py:2503-2570; docs/api.md | missing (transcripts end on user or tool turns) | drive-via-runtime |
| COL-ENG-27 | Output clamp and context refusal | max_tokens a ceiling (length finish); non-fitting prompt → context_length_exceeded before SSE | openai_server.py:3152-3160; docs/api.md | built (hosted.rs:476-486 is_context_overflow) | already-built |
| COL-ENG-28 | response_format and grammar drafts | json_object/json_schema/gbnf; GRAMMAR/SCHEMA/GRAMMAR_DRAFT; forced spans verified, never constraining; greedy only; GLM-5.2 only | c/grammar.h:1-35; c/schema_gbnf.h; c/colibri.c:7737-7815,9203-9240,9528-9532; openai_server.py:3097-3129,4855-4859 | missing (tier 2 sends json_schema/guided_json, runtime.rs:183-221) | owner-decision |
| COL-ENG-29 | KV slots and cache_slot | `--kv-slots` 1-16 (max 16 GLM/GLM-5.3-Flash, 1 others); cache_slot; conversation-hash slot; .coli_kv.N | openai_server.py:2910-2940,4866-4881 | missing (no cache_slot sent) | drive-via-runtime |
| COL-ENG-30 | Admission scheduler | bounded FIFO, 429 queue_full/queue_timeout, slot priority, cancellation, queue-wait header | openai_server.py:110-270 | built (hosted.rs:488-533 retry with Retry-After) | already-built |
| COL-ENG-31 | Continuous batching across slots | mux: serial prefill, batched decode across slots | docs/serve_protocol.md | partial (tier.rs:69-115 S18; runtime.rs:317-337) | drive-via-runtime |
| COL-ENG-32 | Auth and network guards | Bearer or x-api-key; bind refusal; Host guard and allowed hosts; CORS defaults; 4 MiB body; COLI_IMAGE_ROOT; no TLS | openai_server.py:47-55,1894-1958,4163-4255,5540-5562 | built (client keys, ADR 0024 §3) | already-built |
| COL-ENG-33 | GET /v1/models | served id list and lookup; mismatch → 404 model_not_found | openai_server.py:3885-3887,4256-4258,4360-4366 | missing (`#model` typed by hand, spec.rs) | port |
| COL-ENG-34 | GET /health | public liveness; authed scheduler snapshot (incl. capacity), kv_slots, continue_assistant, tiers, hwinfo | openai_server.py:211-218,4318-4331 | missing (only llama.cpp /props probed, runtime.rs:317-337) | port |
| COL-ENG-35 | GET /metrics | Prometheus colibri_scheduler_* gauges, counters, histograms; authed | openai_server.py:232-266,4308-4317 | excluded as a TRELLIS endpoint (BRIEF Part 1; tracing, BRIEF 4.11) | drive-via-runtime |
| COL-ENG-36 | GET /experts | EMAP tier and heat map plus HITS bitmap with seq; authed | openai_server.py:4333-4341 | excluded (ADR 0021 #70) | desktop-ui |
| COL-ENG-37 | GET /profile | ≤120 per-turn phase records; authed | openai_server.py:3617-3630,4342-4357 | excluded (ADR 0021 #70) | desktop-ui |
| COL-ENG-38 | Static dashboard hosting | web/dist, SPA fallback, traversal-safe, experts.json | openai_server.py:4263-4301 | excluded (BRIEF Part 1; ADR 0021 #70) | desktop-ui |
| COL-ENG-39 | POST /v1/brio | options, questions or schema; state or messages; normalize; entropy; nested pin snapshots | openai_server.py:4421-4700; c/pin_pool.h; c/decode_batch.h; docs/brio.md | missing (ask.rs generation classifier, ADR 0024 §2 #41; ADR 0004) | owner-decision |
| COL-ENG-40 | POST /v1/systemone | Jev API over brio: noul/choice/score, confidence formula, 422s | openai_server.py:4686-4822; docs/brio.md | missing (no scoring client in trellis-model) | drive-via-runtime |
| COL-ENG-41 | Mux serve protocol | SUBMIT/IMAGE/STOP/CANCEL; DATA/TOOL/ECHO/ACCEPT/DONE/ERROR; logprobs=k, pin=1; telemetry; serve_codec.h | docs/serve_protocol.md; c/serve_codec.h; openai_server.py:3411-3884 | missing (HTTP only: runtime.rs, transport.rs) | drive-via-runtime |
| COL-ENG-42 | Legacy SERVE protocol | interactive lines, \x02RESET/MORE/PROMPT, END sentinel | docs/serve_protocol.md | missing (HTTP only) | drive-via-runtime |
| COL-ENG-43 | Gateway engine lifecycle | persistent engine; bind before load; READY; Windows kill-on-close job; SIGTERM/SIGBREAK; drain saves .coli_usage | openai_server.py:3294-3410,3842-3884,5540-5610 | missing (no runtime supervision) | drive-via-runtime |
| COL-ENG-44 | COLI_DEBUG tee | 1 output, 2 rendered prompt and output | openai_server.py:4842-4851 | missing (TRELLIS traces its own request, ADR 0024 §3) | drive-via-runtime |
| COL-ENG-45 | OpenAPI and docs site | openapi.json (8 paths), Mintlify nav, doc regeneration rules | docs/api-reference/openapi.json; docs/docs.json; docs/MAINTAINING-DOCS.md | missing (stubs in crates/trellis-cli/tests/hosted_keys.rs) | port |
| COL-ENG-46 | GLM-5.2/5.3 engine | 744B/40B; MLA+DSA; int8 MTP; int4-gs64; tools; grammar payload; 16 slots; ctx 4096 | c/colibri.c; family_registry.py | missing (ADR 0013; not in catalog.rs) | drive-via-runtime |
| COL-ENG-47 | GLM-5.3-Flash engine | 321B; int4 experts, BF16 dense with load-time bits; vision; tools; Metal; 16 slots; ctx 8192 | c/glm53.c; docs/glm53-flash.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-48 | Inkling engine | 975B; int4 experts; BF16 or int4 dense; audio; no tools | c/inkling.c; docs/inkling.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-49 | Kimi K3 engine | 2.8T; native MXFP4 from HF shards; KDA+MLA; XTML tools; recurrent checkpoints; Vulkan | c/kimi_k3.c; docs/kimi_k3.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-50 | DeepSeek V4 Flash engine | 284B (REAP-150B); fp4/fp8 native; DSML tools; greedy, 1 slot; prefix checkpoints; CUDA tier; drafts off | c/deepseek_v4.c; docs/deepseek-v4.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-51 | DeepSeek V4.1 Flash engine | 552B; Engram on disk; DSA; vision; DSpark; DSML tools | c/deepseek_v41.c; docs/deepseek-v41.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-52 | Qwen3.8-Flash-Next engine | 125B + 51B n-gram; block-FP8; int8 trunk; CUDA tier; vision; tools | c/qwen38.c; docs/qwen38.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-53 | Qwen3.6-35B-A3B engine | DeltaNet hybrid; gs64/per-row/mixed containers; VRAM tier with COLI_PLACE probe; tools only via fallback | c/qwen36.c; c/qwen36_tier.c; docs/qwen36*.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-54 | OLMoE engine | 7B int8; GGUF import; PPL; no tools or thinking; ctx max 4096 | c/olmoe.c | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-55 | Family registry | per family: model_types, engine, model id, capabilities, limits, planner geometry, variants, converter | c/family_registry.py:28-147,1085-1440 | missing (ADR 0024 §2 Native from protocol) | port |
| COL-ENG-56 | Per-family chat templates | render_chat_* per family, pinned to vendored templates; open-turn shapes | openai_server.py:1131-1890,2172-2502,2571-2622 | missing (tier 1 sends messages; tiers 2/4 raw prompts) | drive-via-runtime |
| COL-ENG-57 | Tokenizers | tiktoken-exact BPE, GPT-2, o200k, Qwen NFC; measured against HF | c/tok.h; c/tok_unicode*.h; c/qwen38_nfc*.h | missing (ADR 0033 byte budget, Ollama-only ratio) | drive-via-runtime |
| COL-ENG-58 | Containers and format registry | safetensors range reads; fmt codes, stamps, container declaration | c/st.h; c/quant.h; docs/FORMATS.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-59 | Entropy-coded expert tiers | int4-rans256-g0; rANS SIMD paths, RANS_PATH fails loudly; DietGPU ANS tier | c/rans.h; c/tools/repack_rans.py, rans_verify.py; docs/int4-rans256-g0.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-60 | GGUF reader and OLMoE converter | stdlib GGUF reader, dequant, profile, convert | c/tools/gguf_*.py, convert_gguf_to_olmoe.py | missing (catalog fetches GGUF, no conversion) | drive-via-runtime |
| COL-ENG-61 | Three-tier residency and expert cache | dense resident; streamed per-layer LRU; RAM_GB, cap, CAP_RAISE, RSS_GUARD_GB, overcommit guard, MLOCK, COLI_MMAP, COLI_MAP_EXPERTS, TRUNK_RESIDENT_LAYERS | README; docs/ENVIRONMENT.md; c/expert_store.h; c/expert_ffn.h | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-62 | Learning cache and pinning | .coli_usage history (engine-id, decay, USAGE_SAVE); PIN, PIN_GB, PIN=auto, AUTOPIN, PIN_FILL; warmup.ps1 | c/route_trace.h; docs/routing-telemetry.md; c/warmup.ps1 | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-63 | Live re-pin | REPIN=N LFRU, hysteresis, ≤4 swaps; balanced policy | docs/tuning.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-64 | Lookahead prefetch and coupling | PILOT*, COUPLE*, PREFETCH, LOOKA | docs/tuning.md; docs/ENVIRONMENT.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-65 | Expert I/O pipeline | one pread per expert; batch-union; PIPE*, DIRECT, URING; .coli_ssd probe | c/uring.h; docs/ENVIRONMENT.md; README | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-66 | Multi-disk mirror and split | COLI_MODEL_MIRROR, COLI_DISK_WEIGHTS, COLI_MIR_STRIPE, COLI_MODEL_DIRS, MIRROR counters | docs/multidisk.md; docs/ENVIRONMENT.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-67 | NUMA and OpenMP tuning | COLI_NUMA mbind; physical-core cap; COLI_NO_OMP_TUNE, COLI_OMP_TUNED; XEXP | c/omp_tune.h; docs/tuning.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-68 | Cache-aware routing | CACHE_ROUTE with ROUTE_J/M/P/ALPHA/AGREE; Qwen3.6 two levels | docs/CACHE_ROUTE.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-69 | Opt-in lossy modes | `--topp/--topk` (TOPP/TOPK) expert pruning warns; experimental-fast; DEGRADE_ZERO/TAU; EXPERT_BUDGET quarantined | c/colibri.c:1332,11312-11313; docs/tuning.md; c/resource_plan.py:959-962 | missing (trail records spec only) | drive-via-runtime |
| COL-ENG-70 | KV persistence | .coli_kv per slot, crash-safe warm reopen; KVSAVE | docs/tuning.md; c/kv_persist.h | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-71 | KV prefix reuse and checkpoints | truncate-and-extend; COLI_KV_SHARE; previous-turn reuse; V4 and K3 checkpoints; prefix logs | c/kv_prefix.h; docs/api.md; docs/deepseek-v4.md; docs/kimi_k3.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-72 | KV compression | MLA latent 57× smaller; KV8; KV_TQ; refuses mismatched files | c/kv_fp8.h; c/kv_tq.h; docs/ENVIRONMENT.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-73 | Long-context mechanics | CTX and per-family MAXT; COLI_PREFILL_CHUNK; KVB_FLASH*; DSA*; ABSORB | docs/ENVIRONMENT.md; c/sparse_attn.h; c/sparse_index.h | missing (ADR 0033 4,096-token minimum) | drive-via-runtime |
| COL-ENG-74 | Precision and router guarantee | default never silently changes precision or routing; policies; tune never sweeps them; fail-loud backends | README; docs/tuning.md; c/resource_plan.py:959-962; c/autotune.py:1-8 | missing (trail records spec only, ADR 0007; spec.rs) | owner-decision |
| COL-ENG-75 | Load-time precision choices | GLM53_BITS, K3_*_BITS, COLI_DENSE_*, INK_DENSE_Q4, Q38_*, QWEN_EXPERT_ACT, COLI_V4_ROWS16 | docs/ENVIRONMENT.md; CHANGELOG 1.12.1 | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-76 | Integer kernels and numerics | IDOT, I4S, IDOT_GS, AMX, I4_ACC512, I3_AVX512, SSE4.1, fused pair | c/idot.h; c/exact_dot.h; c/sse41_kernels.h; GPU_BACKENDS.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-77 | Exact verify and reproducibility | COLI_EXACT_VERIFY; SPEC_PIN; DRAFT=0 IDOT=0 COLI_CUDA=0; CONSIST=1 | README; docs/tuning.md | missing (temperature 0, runtime.rs) | drive-via-runtime |
| COL-ENG-78 | Correctness oracles | TF, REF, ORACLE_STRICT, REPLAY, SCORE, ABLATE, logit dumps, PPL; tiny fixtures; reference runners; CI | docs/ENVIRONMENT.md; c/tools/make_*; c/tests/ | missing (verdicts from repo checks, ADR 0004) | drive-via-runtime |
| COL-ENG-79 | MTP drafts | int8 MTP head; DRAFT, MTP, SPEC; guard; CUDA MTP | README; docs/ENVIRONMENT.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-80 | n-gram and corpus drafts | live n-gram; COLI_DRAFT_CORPUS, CORPUS_K, CORPUS_MINACC | docs/corpus-draft.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-81 | DSpark drafter | V4/V4.1 draft head and markov drafter with rollback; off on V4 | docs/deepseek-v41.md; c/deepseek_v4_dspark.inc | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-82 | Vision input | image_url data: URI; GLM-5.3-Flash, V4.1, Qwen3.8; one image per request; token ceilings | openai_server.py:1894-2117,5196-5228; c/vision_tower.h; c/tools/*_image.py | built (client: `run --image`, ADR 0041; hosted.rs:223) | drive-via-runtime |
| COL-ENG-83 | Audio input | Inkling input_audio WAV 16 kHz → DMel | openai_server.py:846-1000; docs/inkling.md | missing (no audio input in TRELLIS) | drive-via-runtime |
| COL-ENG-84 | Routing telemetry lines | EMAP, HITS, TIERS, HWINFO, PROF cached by the gateway | docs/serve_protocol.md; openai_server.py:3607-3640; c/telemetry.h | excluded (ADR 0021 #70) | desktop-ui |
| COL-ENG-85 | Expert atlas | topic probes; affinity, specialisation, replication gate; experts.json for Brain Explore | c/tools/expert_atlas/ | excluded (ADR 0021 #70) | desktop-ui |
| COL-ENG-86 | Routing and placement research tools | ROUTE_TRACE, STATS, coupling/pairs reports, residency_sim, placement tools, CNRE simulator | c/tools/route_*.py, residency_sim.py, placement_*.py | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-87 | Dashboard pages | Chat, Brio, Brain Explore/Live routing, Profiling, themes | docs/api.md#web-dashboard; README; web/ (not read) | excluded (BRIEF Part 1; ADR 0021 #70) | desktop-ui |
| COL-ENG-88 | PROF profile | PROF=1 percentiles, I/O totals, phase shares, knob verdict; backend profilers | docs/ENVIRONMENT.md; c/telemetry.h | missing (trail wall clock, MISSION goal 7) | drive-via-runtime |
| COL-ENG-89 | Per-turn accounting | STAT: tokens, tok/s, hit %, RSS, prompt tokens, length flag; usage; chat footer | openai_server.py:3168-3195,5163-5167; c/coli:1470-1500 | partial (reported tokens; TUI Budget, app.rs) | drive-via-runtime |
| COL-ENG-90 | Benchmark protocol and datapoint | five rules; minimum report; datapoint.py; engine_evidence.py; manifest | docs/benchmarking.md; c/tools/datapoint.py; c/experiment_manifest.py | missing (docs/BENCHMARK_AND_EVAL.md) | drive-via-runtime |
| COL-ENG-91 | HTTP serving benchmark and baselines | closed-loop or paced, warmup, SLO goodput; three-engine baseline; batch/prefix/interference | c/tools/benchmark_*.py; docs/baselines/ | missing (docs/BENCHMARK_AND_EVAL.md) | drive-via-runtime |
| COL-ENG-92 | Micro and system benchmarks | iobench, fp4, split CUDA, blocksweep, efficiency, break-even | c/iobench.c; c/tools/bench_*, efficiency.py | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-93 | Published tables and experiment logs | reference/community rows; quality benchmark; raw logs | docs/benchmarks.md; docs/experiments/ | missing (docs/bench holds TRELLIS's own) | drive-via-runtime |
| COL-ENG-94 | CUDA backend (Linux) | VRAM tier, multi-GPU, release host, pipeline, attention, TC W4A16, fallback, fault injection | c/backend_cuda.cu; docs/cuda.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-95 | Windows CUDA DLL and V4 CUDA tier | runtime-loaded DLL; portable arch; DeepGEMM sm120; DSV4_HYBRID; MoE batch/double | c/backend_loader*.c; c/backend_cuda_dsv4.cu; docs/deepseek-v4.md; docs/windows.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-96 | HIP/ROCm backend | one .cu via compat shim; HIP=1; HIP_DLL fail-closed runtime | GPU_BACKENDS.md; c/backend_gpu_compat.h | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-97 | Metal backend | METAL=1, residency sets, platform cap; GLM-5.3-Flash and K3 paths | c/backend_metal.mm; docs/metal*.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-98 | Vulkan backend | VK=1 expert tier, dense, attention, second device; RADV; K3 tier | c/backend_vulkan.c; c/shaders/; docs/vulkan.md | missing (ADR 0013) | drive-via-runtime |
| COL-ENG-99 | Segment runtime ABI | layer-range sessions, snapshots, numeric_class; seven CPU adapters | c/segment_runtime.*; docs/segment-runtime.md | missing (ADR 0013) | owner-decision |
| COL-ENG-100 | Edge runtime ABI | tokenizer/embedding/head; greedy select only; static library | c/edge_runtime.*; docs/edge-runtime.md | missing (ADR 0013) | owner-decision |
| COL-ENG-101 | GitHub release archives | three platform archives with dashboard; smoke test; SHA256SUMS | .github/workflows/release.yml | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-102 | Docker images and compose | clone image; slim multi-stage non-root; compose | docker/ | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-103 | Nix flake | package, apps, devShell, installCheck | flake.nix | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-104 | pip package | colibri-engine, extras, runpy `coli`, single version | pyproject.toml; colibri/ | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-105 | make install layout | bin/coli plus libexec engines; uninstall; COLI_ENGINE | c/Makefile; c/coli:85-106 | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-106 | Windows-native distribution | coli.cmd, MSYS2/MSVC, Smart App Control, HIP DLL walkthrough | c/coli.cmd; docs/windows.md | missing (TRELLIS ships itself only) | drive-via-runtime |
| COL-ENG-107 | CI matrix | engine, sanitizers, musl, GPU syntax, per-family oracles, cluster parity, web, python, gguf, ARM | .github/workflows/*.yml | missing (TRELLIS CI covers itself) | drive-via-runtime |

**Totals:** 107 features.
- Status: built 6, partial 9, missing 81, excluded 11.
- Disposition: drive-via-runtime 85, port 4, desktop-ui 7, already-built 4, owner-decision 7.

## Open questions

1. **Runtime naming.** Should colibri be a named runtime in `trellis-model` (a `colibri:` dialect or a `[[providers]]` preset) that reads `/health`, `/v1/models` and the family limits? Or should it stay reachable only through the generic `openai-chat:`/`anthropic:`/`vllm:` specs?
2. **Timeouts and streaming.** Request timeouts are fixed at 300 s (`runtime.rs:141`, `hosted.rs:689`), and doctor uses 30 s (`doctor_cmd.rs:100`). Should they become configurable, or should streaming be built (deferred under W-C) so colibri's SSE keepalives can carry a long prefill?
3. **Tier 2 from colibri.** Should TRELLIS accept tier 1/tier 4 against colibri, or ask upstream for a constraining grammar mode? Such a mode would be new upstream: colibri's grammar is deliberately draft-only.
4. **Brio for `trellis ask`.** Should Brio replace `ask`'s generation classifier? That needs an ADR, and Brio output must never become a `Verdict` (ADR 0004).
5. **Runtime identity in the trail.** Should the trail record colibri's version, family, policy and lossy knobs? colibri does not expose policy or quantisation over HTTP.
6. **Model fetch.** Multi-shard containers (20 GB to 1.6 TB) do not fit the single-file, pinned-SHA-256 catalog (ADR 0044). The catalog's Apache/MIT-only rule would also need each family's weight licence checked.
7. **Context fit.** OLMoE's maximum context equals TRELLIS's minimum window, so it is effectively unusable for TRELLIS. Bytes per token for colibri's tokenizers is unmeasured.
8. **Upstream doc drift (code wins).**
   - The `docs/api.md` tool matrix disagrees with the registry: Qwen3.8 is "no" there and True in the registry; GLM-5.3-Flash, V4.1 and Qwen3.6 are absent; `SETTINGS.md` says Kimi K3 rejects tools.
   - `ENVIRONMENT.md` says GRAMMAR/SCHEMA "constrain generation"; `grammar.h` and `grammar-draft.md` say they never do.
   - `ENVIRONMENT.md` calls TOPK/TOPP sampling filters; in `colibri.c` they prune experts (`colibri.c:1332,11312-11313`), and sampling uses NUCLEUS.
   - `ENVIRONMENT.md` says "seven engine binaries"; there are nine families.
   - `serve_protocol.md` lists `PERF`/`ENTROPY`/`GPUS`/`TOPK`/`REPIN` lines and a `colibri` stats object. The engine emits `PROF` (`colibri.c:9148`), the gateway emits no `colibri` object, and its dispatcher raises on unknown line kinds (`openai_server.py:3523-3650`), contrary to the documented forward-compatibility rule.
   - The `api.md` intro says images return an error, but the vision families accept them.
9. **docker-compose.** As written it appears to refuse to start: it has no `--port` (so the server listens on 8000) against `5000:5000`/`EXPOSE 5000`, and binds `0.0.0.0` with no `COLI_API_KEY`. Not run.
10. **Qwen3.6 telemetry.** Qwen3.6 is not on `route_trace.h` (`routing-telemetry.md`), so it has no `.coli_usage` learning cache and no `PIN=auto`.
11. **desktop-ui rows.** These would need an ADR amending ADR 0021 #70. colibri's dashboard data endpoints are auth-gated and loopback by default.

**Left unread:**
- `c/tests/` (379 files);
- engine `.c`/`.cu`/`.mm` bodies beyond targeted greps;
- `ci.yml` job bodies;
- `docs/experiments/*`;
- per-model docs beyond headings and targeted reads;
- most of `c/tools/*.py`;
- `web/`, `desktop/` and `site/`.
