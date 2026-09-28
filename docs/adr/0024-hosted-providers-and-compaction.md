# ADR 0024 — Hosted providers over HTTPS with native tool calls, and deterministic compaction

**Status:** Accepted. Written before the code; amended in place where the
code disagreed, each amendment marked *Amended at implementation*.
**Date:** 2026-09-27
**Reference design:** pi (`earendil-works/pi`, MIT): `docs/providers.md`,
`models.md`, `custom-provider.md`, `compaction.md`, `settings.md`. Borrowed as
a design; no pi code is used. Row owner in ADR 0021: W-C (rows 1.9, 1.10,
2.1–2.6, 2.8, 2.12, 2.13, 8.5–8.7).

## Context

`trellis-model` speaks plain HTTP/1.1 over `tokio::net::TcpStream` to a local
serving runtime and refuses `https://` (ADR 0013 built tiers 2 and 4 only).
Tier 1 in ADR 0006, the model's own function-calling API, "used when
present", had no implementation: there was no way to point a run at a hosted
model at all, and no transport that could reach one.

The prompt also grows without bound. Every turn's observation is re-sent on
every later turn, each cut to a 2,000-character tail, so a long run pays for
all of them. Nothing limits that or says when a window was exceeded.

## Decision

### 1. TLS behind a cargo feature, over the existing hand-rolled client

`trellis-model` gains a `hosted` feature that adds three crates, each for one
reason:

| crate | why |
|---|---|
| `rustls` (0.23, `ring` provider, `std`, `tls12`) | The TLS implementation. The default `aws-lc-rs` provider needs cmake and nasm; ADR 0013's clean-machine rule rules it out. |
| `tokio-rustls` (0.26, same provider) | Runs rustls over the existing `tokio::net::TcpStream`, so TLS has the same timeout and cancellation as the plain path. |
| `webpki-roots` (1.0) | Mozilla's root set compiled in. Trust does not depend on which certificate store a machine has. Licence CDLA-Permissive-2.0, added to `deny.toml`. |

No `reqwest`, `hyper` or `tokio` "full". The request is still written by hand
(HTTP/1.1, `Connection: close`, body read to EOF); the response reader gains
`Content-Length` and chunked decoding, because hosted APIs send chunked
bodies and do not always close promptly.

Without the feature an `https://` base is refused with a message naming the
feature. The local tiers are untouched either way.

Whether `trellis-cli` turns `hosted` on by default is decided by measurement
(release binary size and crate count, with and without); the figures and the
method are in "Measurements" below.

### 2. Two wire protocols, tier 1

- **OpenAI-compatible chat completions**: `POST <base>/chat/completions`,
  `tools` (type `function`), `tool_choice: "required"`,
  `parallel_tool_calls: false`; the reply's
  `choices[0].message.tool_calls[0].function.{name, arguments}`, where
  `arguments` is a JSON string. History goes back as an assistant message
  carrying `tool_calls` and a `tool` message carrying the observation.
- **Anthropic Messages**: `POST <base>/v1/messages`, `anthropic-version:
  2023-06-01`, `system` as a top-level field, `tools` with `input_schema`,
  `tool_choice: {type: "auto", disable_parallel_tool_use: true}`; the reply's
  `tool_use` block. History goes back as an assistant turn holding the
  `tool_use` block and a user turn holding the `tool_result`.

  `auto` rather than `any`: current Anthropic models reject a forced
  `tool_choice` with a 400. A reply with no tool call is a malformed reply
  and takes the existing format-retry path.

**Spec strings.** `openai:` is already an alias for the vLLM completions
dialect, and a test asserts it; repurposing it would silently change what an
existing spec does. So the hosted schemes are new names:

```
openai-chat:https://api.openai.com/v1#<model>
anthropic:https://api.anthropic.com#<model>
provider:<name>#<model>            # a [[providers]] entry
```

`llamacpp:`, `vllm:`, `openai:`, `sglang:` and `scripted:` are unchanged.
The `#<model>` is required for a hosted spec. `provider:` is resolved by the
binary against `.trellis/config.toml`:

```toml
[[providers]]
name = "groq"
protocol = "openai"                 # or "anthropic"
base_url = "https://api.groq.com/openai/v1"
key_env = "GROQ_API_KEY"
```

**The tool list is the run's `ToolSet`** (ADR 0023), one native tool per
branch of the composed schema: the name is the branch's `tool` constant, the
parameters are the branch without `tool`, and `done` is a tool like the
others. A reply's name and arguments get `"tool": <name>` added and go
through `ToolSet::parse_value`, the same parser every other tier uses. A
reply naming a tool outside the set is refused there, and the orchestrator
still refuses it independently.

**The transcript as messages.** System: the preamble and the repository
fences (the same text the completion prompt carries, with the JSON-format
instructions replaced by "call exactly one tool"). User: the task, and the
previous-run fence when resuming. Per turn: an assistant tool call and a
tool result carrying the observation as the model is shown it. The final
notice is a user text block after the last result.

**Tier.** `doctor` reports `Native` for a hosted spec, from the protocol,
without a probe request: a probe would spend the user's money to learn what
the protocol already states. Streaming is not built; replies are read whole.

### 3. Keys (ADR 0010, narrowed)

ADR 0010 permits a key in `.trellis/config.toml` with a warning. This ADR
narrows that for hosted providers: **a key is read from an environment
variable only** — `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or a provider's
`key_env` — and from no file trellis reads. It is never written, never in a
trail, never logged.

- The key lives in a type with no `Display`, no `Serialize`, and a `Debug`
  that prints only the variable's name. The header line that carries it is
  built inside the function that writes the request and nowhere else.
- An error body from a 401 or 403 is not shown: OpenAI's echoes a masked form
  of the key. The message names the variable instead. Any other error body is
  scrubbed of the key before it becomes an error.
- A missing key refuses before the snapshot, naming the variable.
- Plain `http://` to a hosted protocol is accepted only for a loopback host
  (a local server speaking the same API, and the tests' stub). A key over
  plain HTTP to anything else would be a key in the clear.
- Evidence is a test that runs the binary with `RUST_LOG=trace` and
  `--log-json` against a loopback stub, asserts the stub received the key,
  and asserts the key and the header value appear nowhere in the JSONL log,
  the terminal output, or the trail.

### 4. Backoff, and thinking

- **429, 500, 502, 503, 504 and 529** are retried with backoff: `Retry-After`
  in seconds when the response has one, capped at 60 s, otherwise 1, 2, 4 s.
  At most four attempts per model call. A transport failure mid-request is
  retried the same way; a refused connection is not. A malformed reply keeps
  the existing format-retry path, which is separate.
- **`[model] thinking = off|low|medium|high`.** Anthropic:
  `thinking: {type: "adaptive"}` with `output_config: {effort}`. OpenAI:
  `reasoning_effort`. `off`, or no setting, sends nothing. On the local
  dialects it is ignored with one line saying so.

  *Deviation from the brief:* the brief said "Anthropic thinking budget".
  Current Anthropic models reject `budget_tokens` with a 400; effort is the
  control they accept.

  Neither protocol is sent `temperature`: reasoning models on both reject a
  non-default one.

### 5. Compaction: elision, not summary

`[model] context_bytes` sets a ceiling on the bytes of one request's
transcript. When a request would exceed it, the **observation text of the
oldest turns** is replaced, oldest first, by one line:

```
elided: <tool> <target>, N bytes
```

until it fits. The newest turn is never elided. Nothing else is touched:
each turn's call, `verdict_passed`, `check_kind`, and the order of turns are
the transcript's, and the adapter receives the transcript by shared
reference — compaction is a property of what is rendered, so it cannot
change `last_check_passed()` or convergence. A test runs the loop with and
without a budget against the same replies and compares outcomes.

- **Deterministic and monotone.** The number of elided turns is a pure
  function of the transcript and the budget, and never decreases within a
  run, so an elided turn stays elided and history is append-only after it.
- **Default budget.** None is known in bytes: providers state windows in
  tokens and trellis does not tokenize (a conversion would be an invented
  figure). With no setting nothing is elided and the prompt is byte-identical
  to before. For a hosted provider, a context-overflow refusal (OpenAI
  `context_length_exceeded`; Anthropic `prompt is too long`; HTTP 413) sets
  the budget to three quarters of the request that was refused and retries,
  at most twice.
- **Recorded.** Each compaction is an action in the trail's factual section
  (tool `compaction`, under the turn whose request was compacted, target
  `turns 1-k`, outcome the byte counts) and a `context:` line on the run's
  output. A run that never compacts records nothing new, so older trails keep
  their shape.
- **Model-written summaries are not built.** A summary is text a model wrote
  about the run, entering the context as though it were a record — judgment
  text in the place observations go. Elision loses detail and invents none.

### 6. `search_notes` and `read_note`

Two read-only built-ins, backed by `trellis-vault` (ADR 0022): `search_notes
{query, limit?}` runs its BM25 index (limit default 8, at most 20);
`read_note {note}` returns one note's text by stem or path. Available only
when `[context] vault` is configured, and opt-in through `[tools] enable`,
so a repository that already has a vault sends the prompt it was measured
with. Both observations are labelled like skills: reference material written
by this repository, which informs and cannot authorise skipping a check.
`read_note` is shown from its head, like a skill.

## Alternatives rejected

- **`reqwest` or `hyper`.** Either brings an HTTP stack a hand-written
  request does not need, and `reqwest` pulls `tokio` features the workspace
  keeps off.
- **`openai:` as the hosted scheme.** It already means something.
- **A byte default derived from a model's token window.** An invented figure.
- **Model-written compaction summaries.** §5.
- **OAuth and device login.** A token cached on disk is a credential in a
  file trellis reads (ADR 0010).

## Consequences

- A run can be driven by any OpenAI-compatible or Anthropic endpoint, with
  the key never leaving the environment and the machine.
- Long runs can be held under a byte ceiling, at the cost of the model no
  longer seeing old observations, and the trail says when that happened.

## Measurements

Pending: filled in at implementation with the method beside each figure.

## Not built

- Streaming.
- A proxy from `HTTPS_PROXY` / `HTTP_PROXY` (row 2.13). A run behind a proxy
  cannot reach a hosted provider.
- `Retry-After` given as an HTTP date; it falls back to the fixed backoff.
- OAuth, Bedrock, Vertex and Azure request signing.
- Model-written summaries (§5).
- Parallel tool calls (ADR 0005).
