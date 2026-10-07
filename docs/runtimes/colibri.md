# colibri as a TRELLIS runtime

How to start colibri so that `trellis run` can use it, which spec to pass, and
what TRELLIS reads from the server and when. TRELLIS is a client of colibri's
HTTP gateway and nothing more: it does not start, stop, build or configure
colibri (ADR 0047 decision 1). You run `coli serve`.

Upstream facts below are colibri v1.12.1 at ce370e8 as read for
docs/inventory/inventory-colibri.md (rows `COL-ENG-nn`) and ADR 0047. Two
were read for #274 and are not in the inventory; they say so where they
appear. No colibri server has been run against TRELLIS (ADR 0047, What is not
built): every TRELLIS behaviour here is tested against stubs that answer with
bodies transcribed from colibri's source.

## Weights

TRELLIS fetches no colibri weights. Its catalogue is one pinned GGUF file per
entry (ADR 0044), and a colibri container is a set of shards of 20 GB to
1.6 TB (inventory, open question 6) that colibri downloads, converts and
verifies itself (ADR 0047 decision 11). `trellis model` says so under its
listing, and `trellis model pull` given one of the nine family names below
(as ADR 0047's table spells them, any case) refuses with a line pointing here.
You run colibri's own commands:

```text
coli convert
coli mirror verify
coli doctor --deep
coli info
```

- **Convert: `coli convert`** (COL-ENG-14). A resumable FP8 to int4
  conversion, taking `--repo`, `--ebits`, `--io-bits`, `--xbits`,
  `--group-size` and `--no-mtp`, with per-family converters and repackers
  behind it; colibri's family registry names each family's converter
  (COL-ENG-55). Two families start from other forms: Kimi K3 runs native
  MXFP4 from HF shards (COL-ENG-49), and OLMoE is imported from GGUF
  (COL-ENG-54, COL-ENG-60).
- **Verify: `coli mirror verify`** (COL-ENG-13). `coli mirror` plans, stages
  and verifies a usage-ranked partial mirror, checking SHA-256 and writing a
  receipt; its flags are `--mirror`, `--source-dir`, `--usage`, `--budget-gib`
  and `--reserve-gib`.
- **Check: `coli doctor --deep`** (COL-ENG-11) reads headers, shards, tensors,
  the index and the mirror; `coli info` (COL-ENG-08) reports a container's
  geometry, shards, RAM and disk.
- **What a container is:** safetensors read by range, with format codes, stamps
  and a container declaration (COL-ENG-58; colibri's docs/FORMATS.md).

`coli serve --model <model>` then loads it (below).

Not recorded in the inventory or ADR 0047, so not stated here: the `--repo`
value or a full command line for any family; which command does the download
(ADR 0047 says colibri downloads, not which subcommand); which flags each
`coli mirror` step takes; and what the SHA-256 is checked against. Weight
licences per family were not verified, except that colibri's README says Z.ai
releases GLM-5.2 under MIT (inventory, Licence). None of these commands was
run for TRELLIS.

## Start the server

```sh
export COLI_API_KEY="<a key you choose>"
coli serve --model <model> --model-id qwen3.8-flash-next \
  --host 127.0.0.1 --port 8000 \
  --ctx 8192 --kv-slots 1 --temp 0 --api-key "$COLI_API_KEY"
```

What each setting is for:

- `--model-id`: the id colibri serves. Every request naming another id gets
  404 `model_not_found` (ADR 0047 finding 3), so the `#<id>` in every spec
  below must equal it. TRELLIS checks this before a run's snapshot and refuses
  on a mismatch (`colibri_tier`, crates/trellis-cli/src/hosted.rs).
- `--host 127.0.0.1`, `--port 8000`: colibri has no TLS (inventory finding 4),
  and TRELLIS sends plain `http://` to a hosted protocol only on this machine.
  8000 is colibri's default port (inventory, open question 9). TRELLIS identifies and
  probes a loopback base only; a remote colibri is used as a plain hosted
  endpoint with none of what follows.
- `--ctx 8192` or more: colibri does not report its context window over HTTP
  (ADR 0047 finding 7) and clamps each reply to the room the prompt leaves
  (finding 6). TRELLIS therefore holds an identified colibri's prompt to
  25,088 bytes unless told otherwise: (8,192 - 1,024) x 3.5, an 8,192-token
  window less a 1,024-token reply reserve at 3.5 bytes per token
  (`COLIBRI_CONTEXT_BYTES`, crates/trellis-cli/src/hosted.rs; ADR 0047
  decision 6, #271). A server left at a 4,096-token window, the default of
  GLM-5.2/5.3, DeepSeek V4 and DeepSeek V4.1 (ADR 0047 table), cannot hold that
  prompt. **OLMoE's maximum context is 4,096 tokens** (COL-ENG-54), so it
  cannot be raised to 8,192; set `[model] context_bytes` for it (below).
  The 3.5 ratio was measured on no colibri tokenizer; COL-12 (#277) may
  recompute it.
- `--kv-slots`: 1 to 16, and above 1 only on GLM-5.2/5.3 and GLM-5.3-Flash
  (COL-ENG-29). TRELLIS reads the count from `/health` as the server's
  batching (below).
- `--temp 0`: with no `temperature` in a request colibri samples at
  `COLI_TEMP`, else 0.7 (ADR 0047 finding 4; inventory finding 7). TRELLIS
  sends `temperature: 0` to an identified colibri itself (below), but a server
  other clients share should not depend on every client doing so.
- `--api-key`: a key colibri requires on every request. `--api-key` defaults
  to `COLI_API_KEY` in colibri's environment (c/coli:2641 at ce370e8, read for
  #274), so a colibri started with that variable set requires a key even
  without the flag. colibri refuses a non-loopback bind without one
  (inventory finding 4). Where TRELLIS sends a key is below.
- `--model <model>`: what colibri loads (COL-ENG-01, COL-ENG-05). TRELLIS never
  sees it.

To see the prompt colibri rendered from TRELLIS's messages, start it with
`COLI_DEBUG=2` in its environment, which tees the rendered prompt and the
output; `COLI_DEBUG=1` tees the output only (COL-ENG-44). TRELLIS does not log
colibri's rendering.

## The specs

Pass one of these with `--model`, or name it in `[model] spec`. The `#` part is
the `--model-id` above.

Tier 1, native tool calls, through colibri's OpenAI chat route:

```text
openai-chat:http://127.0.0.1:8000/v1#qwen3.8-flash-next
```

Tier 1 through the Anthropic Messages route, which takes no image
(ADR 0047 decision 2; COL-ENG-20):

```text
anthropic:http://127.0.0.1:8000#qwen3.8-flash-next
```

Tier 1 through a `[[providers]]` entry in your own config (next section):

```text
provider:colibri#qwen3.8-flash-next
```

Tier 4, for a model whose chat template refuses tools: colibri's
`/v1/completions`, which receives the prompt without a chat template, with the
reply validated after (inventory finding 3; ADR 0047 decision 2):

```text
vllm:http://127.0.0.1:8000#qwen3.8-flash-next
```

The other way past a tools refusal is to restart colibri with
`COLI_TOOL_FALLBACK=1`, which only OLMoE and Qwen3.6 accept (table below).
colibri's grammar never constrains sampling, so colibri is never tier 2
(ADR 0047 finding 1).

## Keys

A key goes only where you chose to send it (ADR 0024 §3), and identifying
colibri never changes that (ADR 0047 decision 3). The rule as
`hosted::resolve` applies it (crates/trellis-cli/src/hosted.rs):

- **`openai-chat:` passed with `--model`.** TRELLIS sends `OPENAI_API_KEY` when
  it is set, and no key when it is not. So a user with `OPENAI_API_KEY` set
  sends that key to colibri. Against a colibri started with a key, an unset
  `OPENAI_API_KEY` means no key is sent: `/v1/models` answers 401 and the run
  stops before its snapshot, naming the unset variable. `anthropic:` works the
  same way with `ANTHROPIC_API_KEY` (`Protocol::default_key_env`,
  crates/trellis-model/src/hosted.rs).
- **The same spec from the repository's `[model] spec`.** A repository-chosen
  endpoint on this machine is used only while its key variable is unset, and
  then gets no key. With `OPENAI_API_KEY` set it is refused before any request,
  since any port the repository's own code leaves listening is this machine
  too.
- **`provider:` with an entry in your own config.** The entry's `key_env` is
  the variable sent, so colibri gets its own key and `OPENAI_API_KEY` is never
  read. Only your file may set `key_env`; a repository's entry that sets it is
  refused (crates/trellis-cli/src/config.rs, `ProviderSection`). Your file is
  `config.toml` in `TRELLIS_CONFIG_DIR`, else `%APPDATA%\trellis` on Windows,
  else `$XDG_CONFIG_HOME/trellis`, else `$HOME/.config/trellis`
  (crates/trellis-cli/src/user_config.rs).

```toml
[[providers]]
name = "colibri"
protocol = "openai"
base_url = "http://127.0.0.1:8000/v1"
key_env = "COLI_API_KEY"
```

- **`vllm:`.** The runtime client sends no `Authorization` header at all
  (crates/trellis-model/src/runtime.rs, `post`). The vllm: spec sends no key, so tier 4 needs a colibri started without a key.
  That means neither `--api-key` nor `COLI_API_KEY` in colibri's environment;
  otherwise every tier-4 request gets 401. When a run refuses on a tools
  refusal having sent a key, its message says this too (`preflight`).

## The nine families

The tier each gives TRELLIS, from ADR 0047's table (`c/family_registry.py`,
`FamilyCapabilities` and `FamilyLimits`) and the tools refusals
(COL-ENG-48, COL-ENG-53, COL-ENG-54). Contexts are colibri's default and
maximum, in tokens.

| Family | Tier for TRELLIS | Context, default / max | Notes |
|---|---|---|---|
| GLM-5.2/5.3 | 1 | 4,096 / 1,048,576 | raise `--ctx` to 8,192 or more; up to 16 KV slots; GLM-5.3 thinks (below) |
| GLM-5.3-Flash | 1 | 8,192 / 1,048,576 | up to 16 KV slots; vision; thinks (below) |
| Kimi K3 | 1 | 8,192 / 1,048,576 | |
| DeepSeek V4 Flash | 1 | 4,096 / 1,048,576 | raise `--ctx` to 8,192 or more |
| DeepSeek V4.1 Flash | 1 | 4,096 / 1,048,576 | raise `--ctx` to 8,192 or more; vision |
| Qwen3.8-Flash-Next | 1 | 8,192 / 262,144 | vision; the configuration COL-12 (#277) measures |
| Qwen3.6-35B-A3B | tools refused; tier 1 only with `COLI_TOOL_FALLBACK=1`, else tier 4 (`vllm:`) | 8,192 / 262,144 | |
| Inkling | tools refused, with no fallback: tier 4 (`vllm:`) only | 8,192 / 1,048,576 | |
| OLMoE | tools refused; tier 1 only with `COLI_TOOL_FALLBACK=1`, else tier 4 (`vllm:`) | 4,096 / 4,096 | its maximum is 4,096 tokens: set `[model] context_bytes` |

GLM-5.3 and GLM-5.3-Flash think at `Reasoning Effort: Low` even with
`[model] thinking = "off"`: their template opens a thinking block whatever the
request says (c/openai_server.py:2204-2226 at ce370e8, read for #274). TRELLIS
sends `enable_thinking: false` for `off`, which these two do not honour.

## Images

`trellis run --image` attaches each image to every request, its base64 counted
against the run's byte budget (`image_input::fits`,
crates/trellis-cli/src/image_input.rs). colibri takes one image per request,
on its vision families only (COL-ENG-82), so pass `--image` once. TRELLIS does
not enforce the one-image limit itself. Once a run has identified colibri it
checks the images again against colibri's smaller default budget, before the
snapshot (#527). The `anthropic:` route takes no image.

## What TRELLIS reads, and when

All of it only for a loopback base, each read bounded at 10 s
(`READ_TIMEOUT`, crates/trellis-model/src/colibri.rs), and each carrying the
key the tier-1 client would send, or none (`colibri_key`). Every string the
server returns is escaped before it reaches a terminal.

1. **Identification: `GET /v1/models`** at the base's origin. `owned_by:
   "colibri"` identifies the server, and its id is the served id
   (`colibri::identify`). A refused connection, no answer, or a 401 or 403
   stops `trellis run` before its snapshot; a 404, another owner, or a body
   that is not a model list means "not colibri", and the run goes on as for any
   hosted endpoint (`colibri_tier`). A served id other than the spec's is
   refused there too.
2. **The one tools probe:** `POST /v1/chat/completions` carrying one tool
   (`noop`) and `max_tokens: 1` (`colibri::probe_tools`), sent at most once per
   origin and served id in a process, so the run, `doctor` and the TUI's status
   bar share one answer (`tools_probe`). A 400 whose `param` is `tools` is a
   refusal: the run stops before its snapshot naming the `vllm:` spec, and
   names `COLI_TOOL_FALLBACK=1` only when colibri's own message does
   (`preflight`, `fallback_sentence`). A 200 is tier 1, probed. Anything else
   is tier 1, assumed, with the reason.
3. **The self-report: `GET /health`**, once per run that went ahead, then one
   `runtime identified` event with `runtime = "colibri"`, `served`, and
   `kv_slots`, `tiers_vram`, `tiers_ram` and `tiers_disk` where colibri
   reported them (`self_report`). It is logged through `tracing`, which by
   default shows `trellis` targets at INFO (`RUST_LOG` changes that,
   crates/trellis-cli/src/main.rs), and never written to the trail
   (ADR 0047 decision 13). A `/health` that fails stops nothing.

`trellis run` prints `runtime:  colibri, model <id>, tools probed` (or `tools
not verified: <reason>`) under its `model:` line
(crates/trellis-cli/src/run_cmd.rs). The TUI's status bar shows `1, colibri
<id>, tools probed`, `1, colibri <id>, tools assumed` or `1, unavailable:
colibri <id> refuses tools` (`ColibriTier::label`). `trellis ask` reads
`/v1/models` only, and sends no probe (crates/trellis-cli/src/ask.rs).

Every request to an identified colibri carries `temperature: 0` and states
thinking: `enable_thinking: false`, or the configured `reasoning_effort`, on
`openai-chat:`; `thinking: {type: "disabled"}` or `{type: "enabled"}` on
`anthropic:` (ADR 0047 decision 5; `openai_body` and `anthropic_body`,
crates/trellis-model/src/hosted.rs). Every other endpoint's body is unchanged.

## `[model] timeout_s` and `[model] context_bytes`

```toml
[model]
timeout_s = 1800
context_bytes = 53760
```

- **`timeout_s`**: the seconds one model request may take, for every runtime;
  300 when unset. When both your file and the repository's set it, the lower
  wins, and 0 is refused at load (crates/trellis-cli/src/config.rs,
  `model_timeout`; user_config.rs; ADR 0047 decision 9). colibri's own figures
  put a 128-token request at a median 140 s on a 24-core machine, and a decode
  token at about 20 s on an ordinary disk (ADR 0047 finding 8), so raise it
  on such a machine. The run's wall-clock budget still bounds the whole run.
  Replies are read whole; streaming, with an idle timeout that colibri's
  keepalives reset, is not built (#280, #281).
- **`context_bytes`**: the byte budget a run holds its prompt to, eliding the
  oldest observations past it. Unset, an identified colibri gets 25,088 bytes
  (above). A value set here wins, larger or smaller; so does a
  `[[providers]] input_bytes` for the model, and with both set the smaller
  wins (`hosted_context_bytes`, crates/trellis-cli/src/hosted.rs). For a larger
  `--ctx`, set it to (ctx - 1,024) x 3.5: 53,760 for `--ctx 16384`. For OLMoE
  at 4,096 tokens, (4,096 - 1,024) x 3.5 is 10,752.

## What `trellis doctor` prints

`trellis doctor --model <spec>` for an identified colibri
(crates/trellis-cli/src/doctor_cmd.rs). Each `tier:` line is headed by the
tier number, the status bar's vocabulary (`Probe::report`,
crates/trellis-model/src/tier.rs, #523):

```text
runtime:    colibri at http://127.0.0.1:8000/v1, serving `qwen3.8-flash-next`; the spec's model matches it
tier:       1, Native (probed) — none: the model's own tool-calling API is in use
batching:   single-stream; multi-agent mode runs agents sequentially
version:    not reported over HTTP
context:    colibri does not report its window; runs use 25,088 bytes, which fits a window of 8,192 tokens with 1,024 left for the reply at 3.5 bytes per token. Set [model] context_bytes to fit the --ctx colibri was started with
experts:    <n> in VRAM (<gb> GB resident), <n> in RAM (<gb> GB resident), <n> on disk; as reported by colibri
host:       <cores> cores, <gb> GB RAM (<gb> GB available), CPU <cpu>; <n> GPU(s), <gpu>, <gb> GB VRAM; as reported by colibri
```

The other forms:

- When the probe settled nothing, or colibri refused tools, with a note
  naming the `vllm:` spec (`colibri_tier_report`):

  ```text
  tier:       1, Native (assumed: <reason>) — none: the model's own tool-calling API is in use
  tier:       1, unavailable: colibri refuses tools for `<id>` (HTTP 400, param `tools`); tier 4 is `vllm:<origin>#<id>`
  ```

- `runtime:` says the spec's id `does NOT match` the served one when it does
  not.
- `batching:` is `continuous, measured concurrency <n>` when `/health`
  reports `kv_slots` above 1, with a note that the count is colibri's
  configured slots, not a measurement under load (`colibri_batching`).
- `health:     liveness only. ...` in place of `experts:` and `host:` when
  colibri answered `/health` with liveness only, as it does to a request
  without its key.
- For a `vllm:` spec on colibri, doctor says the tier is 4
  (`ValidateAndRetry`) without a schema probe and names the tier-1 spec
  (`colibri_vllm_note`). `trellis run` reaches tier 4 there through its schema
  probe, which colibri's grammar ignores.

## Not built

- Streaming replies from colibri (#280, #281).
- The floor measurement of Qwen3.8-Flash-Next at tier 1 (COL-12, #277), which
  may recompute the 3.5 bytes per token.
- `/v1/brio` for `trellis ask` (#276).
- Reporting colibri's version, family or context window: colibri does not
  expose them over HTTP (ADR 0047, What is not built).
