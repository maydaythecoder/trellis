# ADR 0033 — The smallest supported context window, and a radius sized to fit

**Status:** Accepted.
**Date:** 2026-09-29
**Issue:** #10. Follow-ups: #170 (the bytes-per-token measurement), #228
(the byte budget derived from it).
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
     is recorded here and not acted on. *(Acted on by #228: decision 2.)*

   *Amended 2026-09-30 (#228), replacing "nothing in the code is derived
   from 4,096 tokens or from this ratio: `[model] context_bytes` still has
   no default for a local spec":* **a local spec's default budget is
   `LOCAL_CONTEXT_BYTES` = 14,336 bytes**
   (`crates/trellis-loop/src/adapter.rs`), 4,096 tokens at 3.5 bytes per
   token. It is derived from the densest ratio #170 measured, not from the
   pooled 3.853:
   - *Why not 15,781.* The pooled figure is a size-weighted mean, and a
     prompt denser than the mean packs more tokens into its bytes. The
     first prompt of every run, mostly the file list, ran 3.695; at that
     density 15,781 bytes is about 4,270 tokens, over the window.
   - *Why not 15,132.* That is 4,096 tokens at 3.695
     (`bytes_per_token.py` prints it as `int(4096 × <lowest request>)`),
     and 3.695 is the lowest of 93 requests on one host, not a bound. A
     budget at it has no room for a prompt a little denser than any
     measured.
   - *The margin.* 3.5 bytes per token, about 5% under 3.695, chosen as a
     round figure a reader can recompute (4,096 × 3.5 = 14,336) rather
     than derived from a distribution: 93 requests do not support one. A
     `const` assertion holds the budget under 15,132.
   - *What it bounds, and what it does not.* The budget is in bytes and
     the code holds bytes. That the prompt stays under 4,096 *tokens* is
     inferred through the ratio, on #170's one configuration (Ollama
     0.34.2, `qwen2.5:3b`, `/v1/completions`); another tokenizer or
     template may pack these bytes into more tokens. Re-running #170's
     floor with the budget in force is the check on the inference
     (decision 6).
   - *Where it applies.* `trellis run` with a local runtime spec (`vllm:`,
     `llamacpp:`) and no `[model] context_bytes` (`runtime_context_bytes`,
     `crates/trellis-cli/src/hosted.rs`). An explicit value wins, larger
     or smaller, so a user whose server runs a wider window says so. A
     default rather than unset, because the window a local server was
     started with cannot be read from here, and a prompt over it is not
     refused but cut from the front (decision 2). 4,096 tokens is the
     smallest supported window, so on a wider one the cost is elision a
     run did not need (at worst a cut newest observation or a refused
     step), never a prompt the runtime cuts.
   *Amended 2026-09-29 (#152):* a hosted spec now has a default of 400,000
   bytes (ADR 0024 §5). It is not derived from 4,096 tokens, and its ~4
   bytes-per-token basis is a heuristic, not the measurement #170 makes.
   #170's ratios, above, were measured on a local runtime and are not a
   hosted provider's. #228 changes neither the hosted default nor the
   hosted adapter: a hosted provider refuses an over-window request, and
   the overflow retry (ADR 0024 §5) answers it.

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
   *Amended 2026-09-30 (#228): the revisit.* `RADIUS_OBSERVATION_BYTES`
   stays equal to `OBSERVATION_TAIL`. What outgrew the window was not the
   radius but the prompt around it: a local run had no budget at all, so
   every observation stayed in every prompt. The runtime adapter now holds
   the whole prompt to its budget (`RuntimeModel::fitted_prompt`), part by
   part:
   - *System text, repository context, the skill listing, the file list,
     the task, a prior run, steers, the final notice.* Rendered whole and
     never trimmed. Each has its own cap where the loop sets one (the skill
     listing at `DISCLOSURE_CAP`, the file list at 200 paths, a forced
     skill body at `FORCED_SKILL_HEAD`); the repository's own text and the
     task have none. All of it counts against the budget.
   - *Every call.* Rendered as made, never elided (ADR 0024 §5). Counted.
   - *Older observations.* Elided oldest first to fit (ADR 0024 §5),
     unchanged.
   - *The newest observation.* Never elided. When the prompt is still over
     budget with every older observation elided, it is cut to the room
     left: a skill or note keeps its head with its closing label restated,
     anything else its tail, and a marker says how many of how many bytes
     were shown. With no room even for that marker it becomes one line
     saying none of its bytes were shown.
   - *A radius, and every observation that carries coverage*
     (`blast_radius`, `callers_of`, `callees_of`, `coverage_report`).
     Never cut: rule 3 forbids trimming the coverage lines, and a cut
     listing reads as a complete one.
   - *What still does not fit.* The step fails and nothing is sent. The
     error names the bytes of the fixed part, the calls, the elided notes
     and the newest observation, and says whether the newest carries
     coverage. A runtime does not refuse an over-window prompt: in #170's
     run Ollama kept the last 2,050 tokens of each, so the model answered
     a prompt with its system text, its grounding rule and its task gone.
     Ending the run with the sizes is the lesser failure.
   The largest prompt this can send is therefore at most the budget,
   whatever the repository and the model supply. Held by
   `context_budget_the_largest_prompt_the_loop_can_send_is_within_it`,
   which sweeps the uncapped part from well under the budget to past it
   against a newest observation of every shape at or past its cap, and by
   `context_budget_a_prompt_that_cannot_fit_is_not_sent_or_recorded`. A
   step refused this way records no compaction, since no request was made.
   The hosted adapter is unchanged (decision 1).

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
   *Amended 2026-09-30 (#228):* this is the only place a radius is
   shortened. The prompt's budget (decision 2) never cuts it.

5. **The trail records it.** Each loop `Turn` carries how many of its
   radius's sites were summarised, and `factual.radius.summarised` is the
   sum over the run's radius queries, omitted when zero so a trail with
   nothing summarised keeps its shape and its factual digest. A resumed
   session is told the figure when it is non-zero.

6. **Measured with the budget** *(added 2026-09-30, #228).* Whether any
   prompt still passes 4,096 tokens with the budget in force is a
   measurement: #170's floor, re-run by its method, counting Ollama's
   `truncating input prompt` lines and its per-request token counts.
   **0 of 150 requests passed 4,096 tokens**, against 7 of 20 runs
   before. Ollama logged no `truncating input prompt` line, and its
   largest prompt was 3,750 tokens; the largest prompt trellis sent was
   14,321 bytes. 10 of the 20 runs compacted; none ended on a prompt that
   could not be fitted.
   - *Method.* #170's, on the same host and configuration (Ollama 0.34.2,
     `qwen2.5:3b` Q4_K_M, its runner at `n_ctx_slot = 4096`, spec
     `vllm:http://127.0.0.1:11434#qwen2.5:3b`, one user skill loaded), the
     20 floor tasks by `run_floor.py` over poetry 2.5.0 from the sdist
     (SHA-256 `e73b8738…336f`), mypy 2.3.1,
     `RUST_LOG=trellis=info,trellis_loop::adapter=debug`, no
     `[model] context_bytes`. One deviation: a **debug** binary built at
     6a2c6b9 rather than a release one; prompt bytes do not depend on the
     optimisation level. Trails, logs and the runtime log cut to the run's
     window are in `docs/bench/floor-qwen2.5-3b-q4km-ollama-20260930-budget/`;
     150 requests there are 20 probes and 130 counted prompts.
   - *The ratio, re-measured.* `bytes_per_token.py` over that directory:
     all 20 runs pass both checks; pooled 3.811, first prompts 3.695 again
     (the first prompt is unchanged by the budget), per request 3.636 to
     4.027. **3.636 is denser than the 3.695 the budget was derived from**:
     at it, 15,132 bytes would be about 4,162 tokens, over the window, and
     14,336 bytes is about 3,943. The margin in decision 1 is what kept
     that request inside; the densest ratio is a figure that moves with
     the sample, not a bound.
   - *Outcomes*, for the record and not for this ADR: 0 of 20 repaired
     (16 no-progress, 2 format-failure, 2 budget-exhausted;
     `…-20260930-budget.json`).

## Alternatives rejected

- **Budget a local prompt at the pooled 15,781 bytes** (#228). A pooled
  ratio is not a bound, and the first prompts were measured denser; see
  decision 1.
- **Budget it at 15,132, the densest figure, with no margin** (#228).
  Defensible, and the ticket allowed it; rejected because 3.695 is the
  lowest of 93 requests on one host, and the failure past the line is a
  prompt cut from the front. Revisit if re-runs over a wider task set show
  the margin unused.
- **Cut a radius to fit** (#228). Rule 3; a prompt that cannot hold one
  whole is not sent.
- **Elide calls' arguments as well as observations** (#228). Would let a
  run whose edits outgrow the budget continue, but changes what ADR 0024
  §5 keeps (every call as made). Not built; a run that reaches it ends
  with the bytes of its calls named.

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
- *Since #228:* a local run with no `[model] context_bytes` elides at
  14,336 bytes, so its prompts are no longer byte for byte what they were
  once its transcript passes that. A run whose fixed context and calls
  alone exceed the budget ends with the sizes, rather than sending a
  prompt the runtime would cut.
- The observation is computed from the index and the file bytes at the time
  of the call, before `post_tool` runs, so a line number describes the file
  the index was built from.

## Reversibility

A normal edit. The budget is one constant; the listing and summary are one
function in `trellis-loop`; the trail field is optional and defaulted, so
trails written without it read back under this build as zero, and removing
it again leaves every trail without a summarised radius byte-identical.
Removing the listing returns the observation to the header alone. Changing
the named window or the ratio changes `LOCAL_CONTEXT_BYTES` and its `const`
assertions, this ADR, and ADR 0024 §5's pointer (#228). Removing the default
returns a local run with no setting to no budget, where a prompt of any size
is sent.
