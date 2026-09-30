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

*Amended after the eighth review of PR #1:*
- **One header parser.** The framer (which decides when a response read in
  pieces is whole) looked for the substring `transfer-encoding: chunked`,
  and the parser for a field named `transfer-encoding` whose value
  contained `chunked`. `Transfer-Encoding:chunked` (no space) was
  therefore read to end of stream and then dechunked, and
  `x-transfer-encoding: chunked` framed as chunked and parsed by its
  length. Both now use `parse_head`: field names compared
  case-insensitively, values trimmed, every `Transfer-Encoding` field's
  comma-separated codings taken in order; the body is chunked if and only
  if the last coding is `chunked`, any `Transfer-Encoding` overrides
  `Content-Length`, and a last coding that is not `chunked` is read to end
  of stream (RFC 9112 §6.3). Evidence:
  `transport::tests::the_framer_and_the_parser_read_transfer_encoding_the_same_way`,
  which failed before on `Transfer-Encoding:chunked`.
- **A response is at most 32 MiB**, header and body. Past it the read
  stops with an error naming the limit, and a `Content-Length` above it is
  refused on the header, before the body it promises. A model reply is
  kilobytes; before this, a server streaming an unframed body filled
  memory until the request's timeout. The local runtime client
  (`runtime.rs`), which read to end of stream the same way, has the same
  cap, and its "not JSON" errors quote the first 400 characters of the
  body rather than all of it. Evidence:
  `a_response_past_the_cap_is_refused_rather_than_buffered`,
  `a_declared_length_past_the_cap_is_refused_before_the_body` and
  `runtime::tests::a_runtime_reply_past_the_cap_is_refused_rather_than_buffered`,
  each of which failed before (the first two ran into their timeouts; the
  third returned a 40 MiB body as a JSON error).

Without the feature an `https://` base is refused with a message naming the
feature. The local tiers are untouched either way.

*Amended 2026-09-29 (#150): `HTTPS_PROXY` as a CONNECT tunnel.* The owner's
decision on #150 fixes the scope, and this is all of it:
- **Only `https://` is proxied, only through `CONNECT`.** For an https
  endpoint trellis reads `HTTPS_PROXY`, then `https_proxy` (an empty value
  is unset), connects to that proxy, sends `CONNECT host:port`, requires a
  200, and runs the TLS handshake with the provider over the tunnel. The
  proxy is told the provider's host and port and nothing else: the key,
  the transcript and the reply are inside TLS end to end, so ADR 0010's
  "no proxy in the key's path" still holds in the sense that matters,
  which is that no proxy can read it. The proxy is the user's, named in
  the user's environment. A refusal (anything but 200) is a connection
  error naming the proxy and the status, and is not retried.
- **Plain `http://` is never proxied**, and `HTTP_PROXY` and `ALL_PROXY`
  are not read: a proxy forwarding a plain request reads it, headers
  included, and plain http goes only to this machine (above).
- **Loopback is always direct**, whatever the variables say.
- **`NO_PROXY` / `no_proxy`** is a comma-separated list: `*` for every
  host, `.example.com` for that domain and every name under it, and a bare
  `example.com` the same (curl's and Go's reading). Case is ignored and an
  IPv6 literal may be bracketed. An entry with a `:port`, or a CIDR range,
  is compared as written and so matches nothing; the request then goes
  through the proxy, which is the failure a user can see and correct.
- **No proxy credentials.** A `HTTPS_PROXY` with `user:pass@` is refused
  when the client is built, before a run's snapshot, with a message that
  does not repeat the value; a proxy answering 407 is reported as asking
  for credentials. A `https://` or `socks5://` proxy URL is refused the
  same way: only an http proxy that accepts `CONNECT` is spoken to.
- The variables are read only for an endpoint that would be proxied, so a
  proxy URL trellis refuses does not break a run against a local server.

Evidence: the four `https_proxy_*` tests in
`crates/trellis-cli/tests/hosted_keys.rs`, through the real binary against
a loopback CONNECT proxy: the CONNECT line names the provider, the first
bytes in the tunnel are a TLS ClientHello naming it, and neither the
CONNECT head nor the output holds the key; nothing reaches the proxy with
only `HTTP_PROXY`/`ALL_PROXY` set or with the host in `NO_PROXY`; and a
credentialed URL is refused before the snapshot without printing the
password. There is no TLS server in those tests, because the client trusts
only the compiled-in roots, so a completed handshake through the tunnel is
not exercised; `transport::tests` covers the tunnel carrying bytes after
the 200 and the matching rules. No run through a real proxy has been made.

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
binary against `.trellis/config.toml` and the user's own `config.toml`
(ADR 0025 §7):

```toml
[[providers]]
name = "groq"
protocol = "openai"                 # or "anthropic"
base_url = "https://api.groq.com/openai/v1"
key_env = "GROQ_API_KEY"            # the user's file only; see §3, amended
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
- **Spend is the reply's reported output tokens** (`usage.completion_tokens`,
  `usage.output_tokens`). A reply without one has an unknown count, not 0:
  the trail records `None`, the in-run token ceiling charges it the reply
  ceiling, and a budgeted profile's run is refused (ADR 0026 §5). Amended
  after the second review of PR #1.
- **A format retry resends the same request**, as the completion tiers do;
  the model is not told why its previous reply was refused.
- **`trellis ask`'s intent classifier** sends one classification request
  per input on a hosted spec, as it does on a local runtime; the TUI labels
  the spec "tier 1, native tool calls". *Amended 2026-09-29 (#41), by the
  owner's decision:* it previously treated a hosted spec as a scripted one
  (everything is a task), so every question snapshotted the worktree, ran
  the loop and rolled back. The spec is resolved as a run resolves it, so
  the §3 refusal of a repository-chosen endpoint applies before the
  request, and the key is read from the user's environment as for a run.
  The request carries no tool list and no `tool_choice` (the loop's
  `"required"` would have nothing to choose from) and no system prompt, is sent with
  thinking off whatever `[model] thinking` says, and its reply is read as
  text. A provider failure is reported under the same context as the local
  path's ("asking the model what this input is"). Tested against a loopback
  stub only.

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

  *Amended after the fourth review of PR #1: IPv6 and the `Host` header.*
  The base URL's authority was split at its last `:`. That split `[::1]`
  itself, so `http://[::1]/v1` was refused with "`1]` is not a port
  number". A bracketed host is now split at `]`, and the host is kept
  without its brackets, which is the form a TLS server name and an IP
  parse both take. `Host:` carried the name alone. Under RFC 9110 §7.2 a
  missing port means the scheme's default, so a server on `:8080` was
  named as if it were on `:80`. The header now adds the port when it is
  not the default, and brackets an IPv6 literal. The local runtime client
  (`RuntimeClient`, tier 2) had the same parser and the same header, and
  now shares both. Evidence, in `crates/trellis-model/src/transport.rs`:
  `a_bracketed_ipv6_host_parses_with_and_without_a_port` and
  `the_host_header_names_a_port_that_is_not_the_scheme_default`, both of
  which failed before the change.
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

*Amended at implementation: the key reaches child processes. Recorded as a
gap at first; closed in wave K as below.* The key is in trellis's own
process environment, and a child inherits that environment unless the spawn
removes the variable: `[checks]` commands, and, in a repository the user has
trusted (ADR 0023), `bash`, custom tools and hooks. A command that prints
the variable (`bash: echo $ANTHROPIC_API_KEY`) would return it as an
observation, which the loop's `turn` event logs and the trail's action
summary records.

*Closed.* Every child process trellis starts has the key variables removed
from its environment, per spawn, with `Command::env_remove`. The
process-wide `std::env::remove_var` is not used: it is `unsafe` in edition
2024, the workspace forbids `unsafe`, and trellis itself still needs the key
for its own requests.

- **One list, in `trellis-core`** (`trellis_core::secrets`), because every
  crate that spawns may depend on it. `scrub(&mut Command)` removes
  `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` always, whether or not anything
  was registered, so a path where registration was missed still removes
  the defaults. It also removes every name registered with
  `register_key_vars`. A tokio command is scrubbed through `as_std_mut`.
- **Registration.** `main` loads the effective configuration (the
  repository's file over the user's, ADR 0025 §7) as soon as the root is
  known and registers every `[[providers]] key_env` in it, before any
  command runs. `hosted::build` registers the `key_env` it resolved as well.
  A configuration that fails to load registers nothing, and the command
  that reads it then fails on the same error; the two defaults are still
  removed.
- **Where it is applied.** Every `Command::new` in shipped code: the check
  runner in `trellis-checks`; `trellis-loop`'s `exec::run`, which is the one
  spawn behind `bash`, custom tools, hooks and the git-backed listings,
  scrubbed after `spec.env` so nothing set there can put a key back; the
  `write_file` ignore probe; `trellis-vcs`'s git runner, because `git
  commit` runs the repository's own hooks; and in the binary the process
  agent, `doctor`'s `--version` and `--help` probes of the configured
  checkers, `model`'s `curl`, and the session's `!` shell line.
- **One exception, by configuration.** A process agent's profile may name
  variables in `pass_env`, which are put back after the scrub (ADR 0026
  §11). The default is none.
- **The `chat` crate** depends on nothing in the workspace by design, so it
  removes the two default names itself (`chat::scrub_keys`) at its four
  spawns. It reads no `[[providers]]`, so it has no other names to remove.
  *[2026-09-29, #16: `crates/chat` was deleted (Q-19); `chat::scrub_keys`
  went with it and is recoverable from git history.]*

Evidence is `crates/trellis-cli/tests/child_env.rs`. The binary is run with
four canary keys in its environment: both defaults, a repository provider's
`key_env`, and a user-level provider's `key_env` from a user configuration
file. A marker variable is set alongside them. A check, a tests command,
`bash`, a custom tool and a `pre_tool` hook each write their whole
environment to a file outside the repository. Each file must hold the
marker, so a dump that recorded nothing cannot pass, and none may hold a
canary. Terminal output and the trail are checked for the canaries too. A
process agent is run the same way, with and without `pass_env =
["ANTHROPIC_API_KEY"]`: without it no canary arrives, and with it exactly
that one does. A last test scans every non-test `Command::new` under
`crates/*/src` and fails on one with no scrub within the next 16 lines.
With the scrub removed from the check runner, `exec::run` and the process
agent, the first two tests fail on `OPENAI_API_KEY`.

Not covered: a process a child starts after reading the key from somewhere
other than its environment (a file, a keychain) is outside what trellis
passes to it.

*Amended 2026-09-27: a key goes only where the user chose to send it.*
Review of PR #1 found that a cloned repository's `.trellis/config.toml`
could choose both halves of a key's journey: `[model] spec` named a hosted
base URL, and `[[providers]]` named a `base_url` **and** a `key_env`, with
the repository's entry winning a name over the user's. A plain `trellis run`
then sent the user's key, or any variable the repository named
(`AWS_SECRET_ACCESS_KEY`), to a host the repository chose. Now:

- **`key_env` comes only from the user's file** (ADR 0025 §7). A
  repository `[[providers]]` entry that sets it refuses the load, naming
  the file and the entry. An entry without it uses the protocol's own
  variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`). This makes `key_env`
  optional in the schema (`#[serde(default)]`, empty meaning the protocol's
  variable), a non-append change to `config.rs`.
- **On a provider name both files define, the user's entry wins.**
- **A hosted endpoint the repository chose, not on this machine, is refused
  before any snapshot or request** unless the pair (origin, key variable) is
  one the user already sends that key to: `(https://api.openai.com,
  OPENAI_API_KEY)`, `(https://api.anthropic.com, ANTHROPIC_API_KEY)`, or the
  origin and variable of an entry in the user's own `[[providers]]`. The
  pair, not the origin alone, so a repository cannot point OPENAI_API_KEY
  at the origin of the user's groq entry. Origin is scheme, lowercased host
  and port with the default filled in; an authority the transport might
  read differently (userinfo, `?`, `#`, `\`, whitespace) is refused rather
  than interpreted.

  *Amended after the fifth review of PR #1:* the host and port are split by
  the transport's own parser, `trellis_model::split_authority`, now public
  for this, and loopback is the transport's `is_loopback`. The allow list
  had kept a split at the last `:` after the transport stopped using one,
  so `http://[::1]/v1` was refused as loopback and a user's own provider on
  a bracketed IPv6 literal never matched. An IPv6 origin is written with
  its brackets (`https://[2001:db8::1]:8443`). Evidence:
  `hosted::tests::a_bracketed_ipv6_authority_is_read_as_the_transport_reads_it`,
  which failed before the change.
- **"The repository chose"** means: the repository's `[model] spec`, an
  agent profile's `model` (ADR 0026), or any `[[providers]]` entry the
  repository defined, *whoever named the entry*: `--model provider:x#m`
  names the entry, not its host. **The user chose** `--model` (on `run`,
  `tui`, `doctor`, `work`), `/model` typed in a session, the model in an
  RPC request or `set_model` (the operator's program), and a spec written
  in the user's own file. ~~Loopback endpoints are allowed from either side:
  a key is sent there only when its variable is set.~~
- *Amended after the seventh review of PR #1:* **a repository-chosen
  loopback endpoint is allowed only when no key would be sent.** If the
  key's variable is unset (or blank, the test `HostedClient` applies when
  it reads the key), a keyless local server works from the repository's
  spec as before. If it is set, a repository-chosen loopback endpoint gets
  the same pair test as any other host: it is allowed only when the user's
  own `[[providers]]` names that origin with that variable, and otherwise
  refused before the snapshot; `--model` (or any other choice by the user,
  as above) sends the key where it says. The old reason, "a key is sent
  there only when its variable is set", was the problem stated as its
  defence: "this machine" includes any port the repository's own test
  suite or dev server leaves listening, so a committed
  `[model] spec = "openai-chat:http://127.0.0.1:<port>/v1#m"` received the
  user's OPENAI_API_KEY on a plain `trellis run`. Evidence:
  `hosted::tests::a_repository_loopback_endpoint_gets_no_key_unless_the_user_chose_it`
  (failed before the change: the keyed repository-chosen spec resolved),
  and `tests/hosted_keys.rs::a_plain_run_sends_no_key_to_a_loopback_port_the_repository_chose`,
  which binds a port, points a hostile repository's spec at it, and
  asserts the run refuses before its snapshot and nothing connects; with
  the old rule the run started with "key from OPENAI_API_KEY" and
  connected.
- `doctor` applies the same rule and refuses with the same message.

Evidence: `hosted::tests` (the allowlist, the pairing, the user's own
entries, `--model`, loopback, and the ambiguous authorities),
`user_config::tests::a_repository_file_may_not_name_the_variable_a_key_is_read_from`
and `..._the_user_s_entry_wins`, and two binary tests in
`tests/hosted_keys.rs` that run a plain `trellis run` in a hostile
repository with the variables set to a canary and assert a refusal before
the snapshot with the canary nowhere in the output. The existing key-leak
tests now define their provider in a user config directory, since the
repository may no longer name `key_env`.

### 4. Backoff, and thinking

- **429, 500, 502, 503, 504 and 529** are retried with backoff: `Retry-After`
  when the response has one, capped at 60 s, otherwise 1, 2, 4 s. The
  header is honoured in seconds (the cap applied to the number before it
  becomes a duration, so `1e20` waits 60 s rather than panicking) or as an
  IMF-fixdate HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`, the form RFC 9110
  §5.6.7 says senders must use), parsed by hand rather than with a new
  dependency: a future date waits until then, a date at or before now
  waits zero. The obsolete RFC 850 and asctime date forms, a zone other
  than `GMT`, and a date that does not exist (`31 Feb`) are not honoured
  and fall back to the fixed backoff.
  At most four attempts per model call. A transport failure mid-request is
  retried the same way, and so is a connect the OS timed out (amended by
  #210, below); a refused connection is not. A malformed reply keeps
  the existing format-retry path, which is separate.
- **`[model] thinking = off|low|medium|high`.** Anthropic:
  `thinking: {type: "adaptive"}` with `output_config: {effort}`. OpenAI:
  `reasoning_effort`. `off`, or no setting, sends nothing. On the local
  dialects it is ignored with one line saying so.

  *Deviation from the brief:* the brief said "Anthropic thinking budget".
  Current Anthropic models reject `budget_tokens` with a 400; effort is the
  control they accept.

  *Amended after the third review of PR #1: a turn with no recorded reply
  withholds thinking.* History is replayed from each reply the provider
  sent (§2). A turn the provider did not produce has nothing to replay, so
  it is rebuilt as a bare `tool_use`. Examples are a turn carried over from
  another model or session, and the governance audit's turn. With thinking
  on, Anthropic refuses such a request with a 400 ("Expected `thinking` or
  `redacted_thinking`, but found `tool_use`"), and the run ended on a
  request trellis had built wrong. Two fixes were considered:
  - send the request without thinking
  - render the whole history as plain user and assistant text instead of
    tool blocks
  Plain text would keep thinking on, but it changes what every other
  request looks like to handle a rare case, and the model then sees its own
  calls as prose. So on Anthropic, a request whose history has any turn
  without a recorded reply is sent without `thinking` and `output_config`,
  and one `thinking: withheld for the rest of this run` line says so. It
  stays withheld for the rest of that transcript. The unreplayed turn is
  still in the history, and the replies made without thinking have no
  thinking block to send back, so turning it on again would draw the same
  400. OpenAI's `reasoning_effort` has no such rule and is unchanged.
  `trellis_model::thinking_sent` is the rule. Evidence, against a stub that
  applies Anthropic's check:
  `with_thinking_on_a_turn_with_no_recorded_reply_does_not_draw_a_400` and
  `with_thinking_on_a_history_the_provider_produced_keeps_thinking` in
  `crates/trellis-loop/src/adapter.rs`.

  *Amended after the fourth review of PR #1.* The rule's input, "every turn
  replayed", was computed twice. The adapter matched issued calls to turns
  for the warning, and `request_body` checked each exchange for a replay.
  The two agreed, but only because they were written the same way. Both now
  call `trellis_model::every_turn_replayed` on the same `Conversation`, so
  the warning and `thinking_withheld()` cannot disagree with the body that
  is sent. The two tests above are unchanged and still pass. They guard
  this change and did not fail before it.

  *Amended after the fifth review of PR #1.* That built a whole
  `Conversation` (system prompt, every observation) on each call only to
  ask which turns carried a reply. The decider is now one per-turn
  predicate in the adapter, `issued_for`, which `conversation()` uses to
  set each exchange's `replay` and `every_turn_replayed(t, issued)` uses to
  answer without building one. There is still one decider: the
  `Conversation` the body is built from has a replay exactly where
  `issued_for` found one. Guarded by
  `the_replay_decision_without_a_conversation_agrees_with_the_body_s`, which
  compares the two on seven history shapes under both protocols; a cost
  change, so no test failed before it. The two thinking tests still pass.

  Neither protocol is sent `temperature`: reasoning models on both reject a
  non-default one.

  *Amended at implementation.* A timeout is not retried (300 s per request;
  retrying one would multiply a stall), and neither is a refused connection.
  The retry waits are real sleeps and are not charged separately to the
  budget; the wall-clock ceiling covers them.

  *Amended by #210.* A connect the operating system timed out (`TimedOut`;
  on Windows `os error 10060`) is retried like a failure mid-request: at
  most four attempts, 1, 2, 4 s apart. A run against a loopback stub on a
  loaded host ended on one, because every connect failure was treated as a
  refusal. The connect has no timeout of its own; it runs inside the
  request's 300 s timeout, so each attempt waits as long as the OS allows
  the handshake. What a user sees: an endpoint whose connect times out now
  fails after up to four OS connect timeouts and 7 s of waits instead of
  one, and the error says `after 4 attempts`. A refused connection, a name
  that does not resolve, a proxy's refusal of `CONNECT` and the 300 s
  request timeout are still not retried. Each retry goes to the same
  endpoint, and its log line carries the address and the OS error, never
  the key or the request body. Evidence: the `connect_retry_` tests in
  `crates/trellis-model/src/hosted.rs` and `transport.rs`.

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
  at most twice. *Amended after the third review of PR #1:* a retry whose
  body is byte for byte the one just refused is not sent. That happens once
  nothing is left to elide: the newest observation, the system prompt and
  the tools are never elided. The run ends with "the prompt without any
  elidable observation still exceeds the provider's window" rather than
  paying for the same refusal again
  (`an_overflow_that_elision_cannot_shrink_is_not_resent_unchanged`).
  *Amended by ADR 0033:* the smallest supported window is now named, as
  4,096 tokens; its byte conversion, measured by #170 on one local
  configuration (Ollama 0.34.2, `qwen2.5:3b`), is 3.853 bytes per token,
  15,781 bytes (ADR 0033 §1 has the method). *Amended 2026-09-30 (#228):*
  a **local** spec with no `[model] context_bytes` now uses **14,336
  bytes** (`LOCAL_CONTEXT_BYTES`: 4,096 tokens at 3.5 bytes per token, a
  stated margin under 3.695, the densest ratio #170 measured; ADR 0033
  §1). "With no setting nothing is elided", above, no longer holds for a
  local spec. The runtime adapter holds the whole prompt to its budget,
  not only the transcript: with every older observation elided, the
  newest is cut, and a prompt still over is not sent (ADR 0033 §2). An
  explicit value wins, larger or smaller.
  *Amended 2026-09-29 (#152), on the owner's decision of 2026-09-28:* a
  **hosted** spec with no `[model] context_bytes` uses **400,000 bytes**
  (`HOSTED_CONTEXT_BYTES`, `crates/trellis-cli/src/hosted.rs`), applied when
  the driver is built. The figure is about 100,000 tokens at roughly 4
  bytes per token, and that ratio is a **heuristic, not a measurement**:
  #170 measures it on trails carrying both prompt bytes and prompt tokens,
  and per-model input limits (split from #45) replace this default when
  they exist. An explicit value always wins over the default, larger or
  smaller. A local
  spec is unchanged: unset still means nothing is elided and its prompt is
  byte for byte what it was. Config parsing is unchanged; `[model]
  context_bytes` still reads as unset when it is absent. The overflow retry
  above still applies, now shrinking from 400,000 bytes or less. Held by
  `hosted_context_default_elides_an_unconfigured_run_below_400000_bytes` and
  `hosted_context_default_yields_to_an_explicit_value_above_or_below_it`.
  *Amended 2026-09-29 (#162), on the owner's decision of 2026-09-28:* a
  `[[providers]]` entry may name per-model input limits in bytes, as
  `input_bytes = { "<model>" = <bytes> }`, keyed by the model name the spec
  writes after `#`. For a `provider:<name>#<model>` spec the budget is: with
  both `[model] context_bytes` and that model's limit set, **the smaller**;
  with one, that one, larger or smaller than 400,000 (a limit replaces the
  default for its model); with neither, 400,000. The smaller of two, not
  `context_bytes` first, because both are ceilings and the endpoint refuses
  a request over the model's own limit however large the run's budget is.
  A spec that names no entry (`openai:<url>#<model>`) has no per-model
  limit. A repository cannot raise a limit the user set: on a provider name
  both files define, the user's entry wins whole (§3's layering, unchanged),
  and under the minimum the repository's `context_bytes` can only lower it.
  The field is optional, so an entry without it parses as before. Held by
  `model_input_limit_smaller_than_context_bytes_wins`,
  `model_input_limit_absent_is_today_s_behaviour`,
  `model_input_limit_replaces_the_hosted_default_even_above_it` and
  `model_input_limit_a_repository_cannot_raise_the_user_s`.
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
- *Amended 2026-09-27.* **How the fit is found.** It used to render the
  whole request once per candidate number of elided turns, which for a
  hosted body is one JSON serialization of the conversation per extra
  elided turn. The answer is unchanged (the smallest count at or above the
  current one that fits, else all but the newest), but it is now computed
  from per-turn sizes: eliding a turn changes the request by the size of
  its note minus the size of its observation (plain length on the
  completion tiers, JSON-escaped length for a hosted body). The chosen
  count is rendered and its size checked against the prediction; on a
  mismatch the old scan runs. Not a binary search, because the size is not
  monotone: a note can be longer than a short observation. Tests assert
  the same answer as the scan on a non-monotone transcript, and a bounded
  number of renders.
- *Amended 2026-09-29 (#110).* **Compaction on request.** `compact` over
  `trellis rpc` (ADR 0025 §5) elides, at the run's next model request,
  every observation but the newest: the most this section ever elides, so
  what a requested compaction produces is a function of the transcript
  alone. "Deterministic and monotone" therefore reads: the number of
  elided turns is a function of the transcript, the budget and the turns
  at which a client asked, and never decreases. With no budget set, a run
  keeps what a request elided, so its later prompts are no longer
  byte-identical to an uncompacted run's; a run nobody asks is unchanged.
  Recorded as above, with the outcome naming the request instead of a
  budget. The owner declined model-written summaries for this command as
  well (#153).
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

*Amended 2026-09-28 (#62):* the `local-only` job in `.github/workflows/ci.yml`
now builds and tests this configuration (`cargo build` and `cargo test -p
trellis-cli --no-default-features`) on Linux and Windows on every push and
pull request.

## Not built

- Streaming. *Amended 2026-09-29 (#151), by the owner's decision of
  2026-09-28:* it stays out, for two reasons. The loop acts on a complete
  tool call and never on a partial one, so streaming a hosted reply would
  buy only a progress display. And it would put two SSE event grammars
  (OpenAI's `data:` chunks and Anthropic's typed events) into the
  hand-rolled client that §1 hardened against unframed bodies, adding a
  class of partial-reply failure on the one path that carries the key. The
  chat interface already streams from local runtimes
  (`qa/REPORT-2026-09-21.md`). Revisit when a hosted turn's wait is
  measured as a problem.
- `Retry-After` given as an obsolete HTTP date (RFC 850 or asctime form);
  it falls back to the fixed backoff. IMF-fixdate is honoured (§4).
- OAuth, Bedrock, Vertex and Azure request signing.
- Model-written summaries (§5).
- Parallel tool calls (ADR 0005).
- *Built since (#228):* a byte default for `[model] context_bytes` on a
  local runtime (§5, ADR 0033 §1). A hosted spec has one since #152; it
  rests on a heuristic bytes-per-token ratio, and no hosted provider's
  ratio has been measured.
- A hosted run has not been made against a real provider: every test uses
  a loopback stub. The first measured hosted run is pending.
- Model overrides beyond `[model] context_bytes`, `thinking` and
  `[[providers]] input_bytes` (row 2.6): cache lifetimes and cost tables
  are not read. Per-model input limits are read since #162 (§5).
- Stripping the key from child processes' environments (§3).
