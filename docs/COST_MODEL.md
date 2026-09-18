# COST_MODEL

Cost and wall-clock per change, per configuration, at three sizes.

**Every figure in the projection tables below is an estimate derived from
stated assumptions, not a measurement.** Nothing here has been observed. The
document exists so the assumptions are written down and falsifiable before
v0.2 produces real numbers; the instrumentation that will replace them is
already in place (`tracing` spans, per ADR 0008).

Measured figures appear in §1 and are labelled as such.

---

## 1. What has actually been measured

Release build, Windows 11, 16 logical cores, warm page cache, 2026-09-18.

| | poetry (44.8k lines, 279 files) | scrapy (93.7k lines, 504 files) |
|---|---|---|
| Pass 1 parse, wall-clock | 31.8 / 32.0 / 33.5 ms | not timed in release |
| Throughput | ~1,400 lines/ms across 16 cores | — |
| Per core | ~88 lines/ms | — |
| Call sites found | 10,421 | 24,461 |

This is consistent with the ~90 lines/ms figure commonly cited for tree-sitter.

**Parsing is not a cost driver.** Resolution, model inference, and check
execution are. None of the three is measured yet.

---

## 2. Cost drivers, in order

1. **Check execution.** A test suite run dominates everything else on a real
   repo. poetry's suite is 54 test files; a full run is seconds to minutes, and
   the loop may run it many times.
2. **Model inference.** Tokens on a metered API, wall-clock on local weights.
3. **Resolution.** Once per run, plus incremental updates per edit.
4. **Parsing.** Measured above. Negligible.

The ordering matters because it says where optimization pays: scoping tests to
the covering set is worth far more than any parser tuning.

---

## 3. Assumptions behind the projections

| # | Assumption | Basis |
|---|---|---|
| C-1 | An iteration is one model turn plus one tool call | Design |
| C-2 | A converging change takes 6–12 iterations | **Guess.** S7 spike measures it |
| C-3 | Context per iteration: 4k–16k tokens depending on radius size | **Guess** |
| C-4 | Scoped test run: 5–30 s; full suite: 60–180 s | Typical for a repo of poetry's size; unverified |
| C-5 | Checks run on roughly half of iterations | **Guess** |
| C-6 | Local 14B quantized generation: 20–40 tok/s single stream | Typical consumer GPU; unverified |
| C-7 | Frontier API: ~1–2 s latency per turn, generation-bound | Typical; unverified |

If C-2 or C-5 is badly wrong, every figure below moves proportionally. They are
the two worth measuring first.

---

## 4. Projections

"Call sites" is the size of the blast radius, which drives context size and the
number of files touched.

### Configuration A — frontier API (metered; tokens are the scarce resource)

| Radius | Iterations | Tokens in / out | Est. cost | Est. wall-clock |
|---|---|---|---|---|
| 10 | 6–8 | 40k / 6k | low single-digit cents to ~$0.20 | 1–3 min |
| 50 | 8–12 | 120k / 15k | ~$0.15–$0.80 | 3–8 min |
| 400 | 12–20+ | 500k–1M / 40k | ~$1–$5 | 15–45 min |

Cost ranges are wide because per-token prices differ by an order of magnitude
across models; they are not quoted per model here because a quoted price ages
badly and this document should not carry a figure that silently goes stale.
Compute from the token columns and the current price list.

Prompt caching materially changes the input column on a loop that resends
context each turn, and is not modelled. → `OPEN_QUESTIONS.md` Q-8.

### Configuration B — local open weights (wall-clock is the scarce resource)

Marginal token cost approaches zero. Electricity is not modelled.

| Radius | Iterations | Est. generation | Est. checks | Est. wall-clock |
|---|---|---|---|---|
| 10 | 6–8 | 2–5 min | 0.5–2 min | **3–7 min** |
| 50 | 8–12 | 5–12 min | 1–4 min | **6–16 min** |
| 400 | 12–20+ | 15–40 min | 3–10 min | **20–50 min** |

The 400-site row is where the default 20-minute wall-clock budget (A-7) will
fire. That is a deliberate signal, not an oversight: a wide change on local
weights should require the user to raise the ceiling explicitly, because the
alternative is discovering the cost after paying it.

### Configuration C — local weights, multi-agent (N=4)

**Only meaningful on a continuous-batching backend** (vLLM, SGLang). On
llama.cpp or Ollama, four agents queue on one GPU and the result is no faster
than sequential, often slower (S18). `trellis doctor` reports this and
`--agents auto` yields 1.

Assuming continuous batching and a change that genuinely decomposes into 4
disjoint partitions:

| Radius | Partitions | Est. wall-clock | vs. single agent |
|---|---|---|---|
| 400 | 4 | 8–20 min plus 3–10 min integration | ~1.7–2.5× faster |

Not 4×, for three reasons, all structural:

1. Integration is serial by design and runs the **full** check suite.
2. Batched throughput is sublinear in concurrency.
3. The decomposition ratio is unknown — most changes may not partition at all
   (S5). A change that yields one partition sees zero speedup and pays the
   coordination overhead.

**The decomposition ratio is the number that decides whether v1.1 is worth
shipping**, and it cannot be measured before v0.2.

---

## 5. What would falsify this model

- C-2 (iterations to converge) off by 2× in either direction.
- The decomposition ratio below ~30%, which would make Configuration C not
  worth its complexity.
- Check execution turning out *not* to dominate on real repos, which would
  redirect optimization effort entirely.
