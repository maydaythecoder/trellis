# ADR 0033 — The smallest supported context window, and a radius sized to fit

**Status:** Accepted.
**Date:** 2026-09-29
**Issue:** #10. Follow-up: #170 (the bytes-per-token measurement).
**Amends:** ADR 0024 §5, the "Default budget" bullet.
**Holds:** ADR 0014 and CLAUDE.md rule 3 (a radius ships with its coverage as
two facts, never a ratio).

## Context

The build brief (S5) asks for a context selector sized to a named minimum
window. There was none. The loop had a 2,000-character observation tail
(`OBSERVATION_TAIL`, `crates/trellis-loop/src/adapter.rs`) and elision
(ADR 0024 §5), and the radius observation printed only counts and coverage:
it never said which call sites reach the symbol.

Listing them has a failure mode. `shown_observation` keeps the last 2,000
characters of an observation and cuts from the top, and the top of a radius
is its coverage header. A listing that ran past the cap would reach the model
with its doubt figure removed and nothing saying so, which is the S2 failure
the header exists to prevent.

The owner decided on 2026-09-28 that 4,096 tokens is the smallest supported
window, with the bytes-per-token ratio measured on existing trails. It cannot
be measured on them. Per the issue's split note of 2026-09-29, all 46
committed floor trails under `docs/bench/floor-*` predate the prompt-bytes
field (commit 7e61fbf), so `factual.context` is null in every one, and no
trail records prompt tokens (#105 and #106 add them).

## Decision

1. **The smallest supported context window is 4,096 tokens.** This amends
   ADR 0024 §5's "Default budget" bullet: the window is now named, in tokens.
   *Amended 2026-09-30 (#170), replacing "its byte conversion is pending":*
   the ratio is **3.853 bytes per token, measured on 19 trails**, on one
   configuration only: Ollama 0.34.2 serving `qwen2.5:3b` (Q4_K_M, the
   `ollama pull` tag, run by its llama-server with `-c 4096`) through its
   OpenAI-compatible `/v1/completions`, spec
   `vllm:http://127.0.0.1:11434#qwen2.5:3b`, tier ValidateAndRetry. It is a
   property of that runtime's tokenizer and template on these prompts, and
   is not stated for any other runtime, model or tokenizer.
   **The resulting byte budget for a 4,096-token window is 15,781 bytes**
   (4,096 × 3.853, rounded down).
   - *Trails:* `docs/bench/floor-qwen2.5-3b-q4km-ollama-20260930/`, the
     20-task floor set run by `docs/bench/run_floor.py` over the CRLF
     poetry 2.5.0 corpus with a release binary built at 9fc754d (which has
     #105 and #106), mypy 2.3.1 as the checker. Computed by
     `python docs/bench/bytes_per_token.py <that directory>`.
   - *Fields divided, and per what:* per run, and pooled over runs. The
     numerator is the bytes of every prompt the run sent whose reply was
     read, the denominator the trail's `factual.prompt_tokens`, which is the
     adapter's sum of `usage.prompt_tokens` over those same requests. The
     trail cannot supply the numerator: `factual.context` holds only
     `first_prompt_bytes` and `peak_prompt_bytes`, one request each. So the
     per-request byte counts come from each run's log (the adapter's
     `prompt_bytes` debug event, `RUST_LOG=trellis=info,trellis_loop::adapter=debug`,
     times its retried attempts), and the trail's `first_prompt_bytes`,
     `peak_prompt_bytes` and `turns_measured` are checked against them.
     Neither first nor peak prompt alone is the figure; both are
     cross-checks.
   - *Second check, from the runtime:* `ollama-requests.txt` in the same
     directory is Ollama's server log cut to its per-request lines. In every
     run used, the trail's `prompt_tokens` equals the sum of that run's
     untruncated requests there, over the same number of requests. One run
     (t16) is excluded: four of its counted requests were prompts Ollama
     truncated to 2,050 tokens, so its trail understates its tokens.
   - *Spread:* per run 3.727 to 3.988, median 3.850. Per request (93,
     paired with the runtime's count) 3.695 to 4.104, median 3.857. The
     first prompt of every run is the densest, 3.695 pooled over 19; at that
     density 4,096 tokens hold 15,132 bytes, so 15,781 is a pooled figure
     and not a bound every prompt stays under.
   - *What the tokens include:* Ollama wraps a `/v1/completions` prompt in
     the model's chat template. A one-word prompt was reported as 30 tokens
     and each further word as one, so each request carries about 29 tokens
     that are not trellis's bytes. They are counted, because they occupy
     the window. The prompts also included one skill from the operator's
     own skill directory (`trellis doctor`: "user skills: 1 loaded"), so
     the byte counts are this host's.
   - *What the run showed about the window:* 7 of the 20 runs sent a prompt
     Ollama refused to fit in 4,096 tokens. Its log records those prompts
     at 4,115 to 4,624 tokens untruncated, 16,498 to 19,447 bytes sent. The
     floor tasks already outgrow the minimum window within a few turns, which
     is the condition decision 2 names for revisiting the radius budget; it
     is recorded here and not acted on.

   Nothing in the code is derived from 4,096 tokens or from this ratio:
   `[model] context_bytes` still has no default for a local spec.
   *Amended 2026-09-29 (#152):* a hosted spec now has a default of 400,000
   bytes (ADR 0024 §5). It is not derived from 4,096 tokens, and its ~4
   bytes-per-token basis is a heuristic, not the measurement #170 makes. A
   local spec still has no default. #170's 3.853, above, was measured on a
   local runtime and is not a hosted provider's ratio.

2. **The radius observation has a byte budget,
   `RADIUS_OBSERVATION_BYTES` (`crates/trellis-loop/src/agent.rs`), equal to
   `OBSERVATION_TAIL`.** It bounds the whole observation: coverage header,
   listing and summary. It is tied to the observation cap, not to the token
   window, because the cap is what decides whether the header survives, and
   it is a known figure while the token conversion is not. A `const`
   assertion holds it at or under `OBSERVATION_TAIL`, so the adapter never
   truncates a radius. If #170 shows the 4,096-token window cannot hold the
   cap alongside a prompt, the budget is revisited then, against a measured
   number.
   *Amended 2026-09-30 (#170):* it did. On the one configuration measured
   (decision 1), 7 of 20 floor runs sent a prompt over 4,096 tokens, the
   largest 19,447 bytes against the 15,781-byte conversion. The revisit is
   not made here; this amendment records the measurement it rests on.

3. **What the observation contains.** The coverage text is unchanged, first.
   Then one line per call site: repository-relative path, 1-based line
   (counted from the call's byte offset against the file's bytes; `?` when
   the file cannot be read), the enclosing symbol's name (`<module>` for
   top-level code), and `certain` or `ambiguous`. Certain before ambiguous,
   then path, then line, then byte offset, so the order does not depend on
   how the index walked its edges.

4. **What happens when it does not fit.** The tail of that order is replaced
   by a line stating how many were listed and how many are summarised, then
   one line per file, `<path>: <n> more (<c> certain, <a> ambiguous)`. The
   per-file lines take precedence over individual entries: fewer entries are
   listed until every remaining file has its line. Only when the per-file
   lines cannot all fit with no entry listed are files given lines in path
   order while each line and a count of the rest still fit, and the rest
   collapse into one final line, `and <n> more call sites in <m> files`.
   The room for that line is checked before each file line is added, so the
   summary is never what gets cut. Listed plus summarised always equals the
   radius's site count. A test asserts that the header, the summarised-total
   line and the final line fit together at the widest counts they can hold.

5. **The trail records it.** Each loop `Turn` carries how many of its
   radius's sites were summarised, and `factual.radius.summarised` is the
   sum over the run's radius queries, omitted when zero so a trail with
   nothing summarised keeps its shape and its factual digest. A resumed
   session is told the figure when it is non-zero.

## Alternatives rejected

- **Budget the radius at `4,096 tokens × 4 bytes`.** Four bytes per token is
  a common rule of thumb and has not been measured on these prompts, which
  mix code, paths and prose. Writing it into the code would be an invented
  figure presented as a derived one; "measured, never assumed" rules it out,
  and #170 exists to replace it with a measurement. (#170 measured 3.853
  on one configuration; see decision 1.)
- **List every call site and let the adapter's tail cut it.** The cut is
  from the top, so the part lost is the coverage header, and the model sees
  a list of callers with no statement that it is incomplete. The surviving
  tail is also an arbitrary slice of the order, with no count of what was
  removed.
- **Cut the listing at the budget and append "...".** It keeps the header
  but drops call sites without a count, which rule 3 forbids: a reader
  cannot tell three missing callers from three hundred.
- **Prefer entries over per-file lines** (list entries until the next one
  would not fit, then summarise what room remains). It shows more exact
  locations when a radius spans many files, and fewer files by name. A model
  that knows every file a change reaches can read any of them; one that
  knows a few exact lines and a bare count of the rest cannot tell where to
  look. Revisit if runs show models acting on listed lines and ignoring the
  per-file counts.

## Consequences

- Every `blast_radius` call now shows the model where its callers are, which
  it previously had to find with `grep`.
- A radius observation is at most `OBSERVATION_TAIL` bytes; before this, it
  was only the header. The prompt grows by the listing on each radius turn,
  and elision (ADR 0024 §5) applies to it like any other observation.
- The observation is computed from the index and the file bytes at the time
  of the call, before `post_tool` runs, so a line number describes the file
  the index was built from.

## Reversibility

A normal edit. The budget is one constant; the listing and summary are one
function in `trellis-loop`; the trail field is optional and defaulted, so
trails written without it read back under this build as zero, and removing
it again leaves every trail without a summarised radius byte-identical.
Removing the listing returns the observation to the header alone. Changing
the named window changes only this ADR and ADR 0024 §5's pointer, because no
code reads it, nor the byte conversion #170 measured for it.
