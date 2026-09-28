# ADR 0024 — Hosted providers over HTTPS with native tool calls, and deterministic compaction

**Status:** Accepted. Written before the code; amended in place where the
code disagreed or the text left a decision open, each amendment marked
*Amended at implementation* (§2, §3, §4, §5, §6), and "Measurements" filled in.
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

*Amended at implementation.* Decisions the text above left open:

- **History is replayed, not rebuilt.** Each reply's assistant message is
  kept (Anthropic: the whole `content` array, thinking blocks and their
  signatures included; OpenAI: `role`, `content`, `tool_calls` only) and sent
  back unchanged with that call's result. A turn the provider never produced
  (none today; the governance audit turn is appended after the last request)
  gets a synthesised id, `call_trellis_<n>` or `toolu_trellis_<n>`.
- **Native tool definitions** take the branch's `description` (the schemars
  doc comment) and `{type: object, properties, required,
  additionalProperties: false}` without `tool`. `read_skill` is left out when
  there is no skill, matching the tool line.
- **Reply ceiling** 16,000 tokens (`max_completion_tokens` on OpenAI,
  `max_tokens` on Anthropic), against 4,096 locally: a hosted reasoning
  model spends thinking against it. An OpenAI-compatible server that accepts
  only `max_tokens` is not accommodated.
- **A format retry resends the same request**, as the completion tiers do;
  the model is not told why its previous reply was refused.
- **`trellis ask`'s intent classifier** treats a hosted spec as it treats a
  scripted one (everything is a task); the TUI labels it "tier 1, native
  tool calls".

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

*Amended at implementation.* Over loopback `http://` the key is sent when
its variable is set and not required when it is not (a local server usually
wants none). `[[providers]]` refuses unknown fields, so `api_key = "..."`
fails to load rather than sitting unread in a committed file. The evidence
is `crates/trellis-cli/tests/hosted_keys.rs`: one run per protocol, each
asserting first that the stub received `Authorization: Bearer <key>` or
`x-api-key: <key>`, that the run converged, and that the JSONL log holds the
`request as sent` trace (so the widest log path was exercised); then that
the key and `<header>sk-` appear in none of stdout, stderr (the fmt layer),
the `--log-json` file, or any file in the repository outside `.git`, the
trail included. A third test runs with the variable unset against an https
provider and asserts a refusal naming it, no `snapshot at` line and no
trail. Unit tests cover `Debug` of the key and of the client, scrubbing,
and the 401 body that echoes a masked key.

*Amended at implementation: a path this section does not close.* The key is
in trellis's own process environment, and every child process inherits it:
`[checks]` commands, and, in a repository the user has trusted (ADR 0023),
`bash`, custom tools and hooks. A command that prints the variable
(`bash: echo $ANTHROPIC_API_KEY`) returns it as an observation, which the
loop's `turn` event logs and the trail's action summary records. trellis
writes nothing of the key itself; a command it runs can. Removing the
variable from each child's environment is not built: the process-wide
`std::env::remove_var` is `unsafe` in edition 2024 and the workspace forbids
`unsafe`, so it would be a per-spawn `env_remove` in `trellis-loop` (bash,
custom tools, hooks) and in `trellis-checks` (the check runner). Until then:
a hosted key is readable by any command a run executes, which for `bash`,
custom tools and hooks requires `trellis trust`, and for `[checks]` does not
(ADR 0023 §4 records that gap).

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

  *Amended at implementation.* A timeout is not retried (300 s per request;
  retrying one would multiply a stall), and neither is a refused connection.
  The retry waits are real sleeps and are not charged separately to the
  budget; the wall-clock ceiling covers them.

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
- *Amended at implementation.* **What is measured.** On the completion
  tiers, the prompt's bytes. On a hosted protocol, the serialized request
  body's bytes, tools included, because that is what the provider counts
  against its window. A compaction is recorded each time a request elides
  more turns than the one before, so a run held at its budget records one
  per turn. The notes name the call with the same label the gate uses:
  `elided: run_checks Typecheck on a.py, 4000 bytes`.
- *Amended at implementation.* **A caveat not measured.** Anthropic models
  that check replayed history for edits (thinking blocks tied to earlier
  turns) may reject a request whose older `tool_result` changed. Elision is
  monotone so each result changes at most once, but no run against such a
  model has been made; whether it is refused is pending the first hosted
  measurement run.
- *Amended at implementation.* **Trail placement.** A compaction is filed
  under turn *n + 1* for a request made with *n* turns. For the request that
  produced `done` there is no such model turn; if the governance audit then
  appends one, the compaction sits just before the audit's action, which is
  also the order they happened in. Otherwise it is the trail's last action.
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

*Amended at implementation.* `cmd_run` attaches the configured vault to the
tool set before the driver is built and before the snapshot; a note tool in
`enable` with no `[context] vault` refuses there. `read_note` resolves its
argument the way `trellis vault` does (path, path without `.md`, alias, slug)
and an ambiguous or unknown name is an error observation pointing at
`search_notes`. Both read the vault afresh on each call. End-to-end tests:
`crates/trellis-cli/tests/note_tools.rs`.

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

Taken 2026-09-27 on the build machine (Windows 11, `x86_64-pc-windows-msvc`,
rustc 1.93.0), release profile as the workspace defines it (`lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`).

| build | `trellis.exe` bytes | crates in the binary's graph |
|---|---|---|
| before this ADR (`3234bee`) | 7,084,032 | 168 |
| `29528e2`, `--no-default-features` (local only) | 7,357,440 | 168 |
| `29528e2`, default features (`hosted` on) | 8,685,568 | 178 |

- **Size:** `cargo build --release -p trellis-cli [--no-default-features]`,
  then `ls -l target/release/trellis.exe`. The local-only build grows
  273,408 bytes over `3234bee` (the protocol mapping, transport, compaction
  and note tools, all of which compile without TLS). TLS adds 1,328,128 bytes
  (18.1%) on top.
- **Crates:** `cargo tree -p trellis-cli -e normal --prefix none
  [--no-default-features]`, `(*)` markers stripped, unique lines counted.
  Workspace crates are included in both counts. `Cargo.lock` is not a
  measure of this: it records every optional crate whether or not a feature
  turns it on, and went from 365 to 385 entries with or without `hosted`.
  The ten crates `hosted` adds are `rustls`, `rustls-pki-types`,
  `rustls-webpki`, `ring`, `untrusted`, `subtle`, `zeroize`, `tokio-rustls`,
  `webpki-roots`, and a second `getrandom` (0.2, through `ring`).
- **Duplicates:** `cargo deny check` now also warns of two `getrandom` and
  two `windows-sys` versions, both through `ring`. Licences, bans,
  advisories and sources pass.

**Decision: `hosted` is a default feature of `trellis-cli`.** 1.3 MB buys
tier 1, which BRIEF §4.7 says to use when present, and a local-only binary
is one flag away and still builds and passes its tests (`cargo test -p
trellis-cli --no-default-features`). `release.yml` passes no feature flags,
so it is left as it is and **the released binary includes TLS**. CI does not
build the local-only configuration; it was run by hand for this ADR.

## Not built

- Streaming.
- A proxy from `HTTPS_PROXY` / `HTTP_PROXY` (row 2.13). A run behind a proxy
  cannot reach a hosted provider.
- `Retry-After` given as an HTTP date; it falls back to the fixed backoff.
- OAuth, Bedrock, Vertex and Azure request signing.
- Model-written summaries (§5).
- Parallel tool calls (ADR 0005).
- A byte default for `[model] context_bytes` (§5).
- The intent classifier in `trellis ask` for a hosted model (§2).
- A hosted run has not been made against a real provider: every test uses
  a loopback stub. The first measured hosted run is pending.
- A CI job for the local-only build.
- Model overrides beyond `[model] context_bytes` and `thinking` (row 2.6):
  per-model input limits, cache lifetimes and cost tables are not read.
- Stripping the key from child processes' environments (§3).
