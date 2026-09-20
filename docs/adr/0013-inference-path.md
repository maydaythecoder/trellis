# ADR 0013 — The inference path for harness-side constraint

**Status:** Accepted. Supersedes the open question left in ADR 0006, which is
now fully Accepted.

**Date:** 2026-09-19

## Context

ADR 0006 defined four adapter tiers and accepted all four, but deliberately
left one question open: tier 3 — harness-side constraint, where TRELLIS owns
the sampling loop and applies the token mask itself — requires **logit
access**, and therefore in-process inference rather than an HTTP endpoint.
S17 records this as unresolved with real build-complexity consequences.

The v0.1 exit gate requires this decided.

### What was verified, 2026-09-19

| crate | version | role |
|---|---|---|
| `llguidance` | 1.8.0 | grammar engine, MIT, already in the workspace |
| `llama-cpp-2` | 0.1.156 | bindings to llama.cpp |
| `mistralrs` | 0.8.1 | Rust-native inference engine |
| `candle-core` | 0.11.0 | minimalist ML framework |

All three inference candidates exist and are published. Deeper verification —
licences, maintenance cadence, hardware matrix — could not be completed
because the crates.io API was unreachable from the build machine. Anything
below that depends on a fact not in the table above is reasoning, and is
labelled as such.

### The build machine, which turned out to be evidence

| | |
|---|---|
| GPU | RTX 5060 Ti, 8 GB VRAM |
| RAM | 15.2 GB |
| C++ toolchain | **none** — no cmake, no cl, no gcc, no clang |
| CUDA toolkit | **none** — no nvcc |
| Local weights | none at the time of the decision; the one model directory held git-lfs pointers, not a `.gguf` |

This was probed, not assumed, and it matters more than it looks.
`llama-cpp-2` builds llama.cpp from source, so **on this machine it cannot be
built at all**. That is not a quirk of one laptop: a Rust developer's clean
machine has `cargo` and frequently nothing else, and "runs on your hardware"
is the product's selling point. A tier that cannot compile without first
installing a C++ toolchain and a CUDA toolkit is a tier most users will never
reach.

It cuts against the GGUF argument below rather than settling it, so the two
are weighed explicitly in the decision.

**Probed, not argued:** a throwaway crate depending on `candle-core 0.11`,
`candle-transformers 0.11`, `tokenizers` and `llguidance 1.8` **built clean in
5m30s on this machine**, 164 crates, no C++ toolchain and no CUDA toolkit
present. So a pure-Rust inference path compiles where `llama-cpp-2` cannot
even start. That is one data point about building, not about running a model
well, and it is recorded as such — but it is the difference between a tier a
user can reach and one they cannot.

## Decision

**Two parts, and the ordering is the substance of this ADR.**

### 1. Build tier 2 first, and treat it as the default for local models

Hand the compiled grammar to the serving runtime's guided-decoding parameter
and let its own embedded llguidance do the masking.

The reason is the observation ADR 0006 already records but does not act on:
**llguidance is the grammar engine inside llama.cpp, vLLM and SGLang.** The
grammar TRELLIS compiles from a tool schema is the *same artifact* whether it
masks locally or ships it to the runtime. So for the runtimes local users
actually run, tier 2 delivers exactly what tier 3 delivers, with no inference
code in TRELLIS at all.

Tier 3's marginal value is confined to runtimes that expose logits but *not*
guided decoding. That set is small and shrinking.

### 2. When tier 3 is built, prefer the pure-Rust path (`candle`), with
`llama-cpp-2` as the coverage fallback

This inverts the ordering an earlier draft of this ADR had, and the probe is
why.

The argument for `llama-cpp-2` is real and unchanged: GGUF is the dominant
distribution format for local open weights, and llama.cpp carries the hardware
coverage (CUDA / Metal / ROCm / CPU) that "runs on your machine" has to mean.
A Rust engine that cannot load the file the user already downloaded is simpler
only for us.

What outweighs it is that `llama-cpp-2` requires a C++ toolchain to build, and
`cargo build` on a clean machine is the distribution story. A tier that first
demands cmake, a compiler and a CUDA toolkit is one most users will never
reach — and the measurement above shows that is not hypothetical, because it
is the state of the machine this project is being built on. `candle` reads
GGUF too, which removes most of the format argument; what it gives up is
hardware coverage and kernel maturity, which is the right thing to give up
first because it degrades performance rather than preventing a build.

So: `candle` for the tier-3 default, `llama-cpp-2` behind an off-by-default
feature for users who have the toolchain and want the kernels. Neither is
built until tier 2 is shipped and measured.

## Consequences

- **v0.2 ships tier 2 and tier 4, not tier 3.** That is less work than ADR
  0006 implied and removes the largest unestimated item from the milestone.
- `trellis doctor` must report the active tier accurately, because a user on
  tier 4 (validate-and-retry) has materially worse behaviour and no way to
  tell otherwise. This was already required by S17 and is now load-bearing.
- TRELLIS acquires no C++ toolchain dependency until tier 3 is built, so the
  build stays `cargo build` on a clean machine.
- A user on a runtime with neither native tool calling nor guided decoding
  falls to tier 4 and is told so loudly. Under this decision that is the
  accepted outcome rather than a gap tier 3 would have covered.
- S18 is unaffected and still bites: multi-agent mode needs a continuous-
  batching runtime regardless of which tier is active.

## Alternatives rejected

**Build tier 3 first with `llama-cpp-2`.** The path ADR 0006 implied. Rejected
because it front-loads a C++ build dependency and an FFI surface to obtain,
for llama.cpp users, a capability llama.cpp already has. It is work spent
reaching parity with the runtime rather than beyond it.

**`mistralrs` for tier 3.** Rust-native, no C++ toolchain, and genuinely
attractive on that basis. Rejected as the *first* choice on format coverage
rather than on quality: the decision is driven by what the user already has on
disk. Worth revisiting if its GGUF support and hardware matrix close the gap —
it is the most likely thing to change this ADR.

**`candle` as a rejected option.** An earlier draft rejected it on scope —
model loading, quantization and sampling all becoming ours in a component
whose correctness is not what the product is about. That concern is real and
survives, but `candle-transformers` already ships quantized model loaders, so
the scope is smaller than the draft assumed, and the build probe moved it from
third choice to first. Recorded here because the reversal is the substance of
this ADR, not a tidy-up.

**Skip tier 3 permanently.** Tempting given the above, and rejected because
the tier exists for a real case — a runtime exposing logits without guided
decoding — and because owning the sampling loop is the only configuration in
which TRELLIS can guarantee the constraint rather than request it.

## Reversibility

**High, and deliberately so.** The tiers are behind one adapter interface and
the grammar artifact is identical across them, so adding tier 3 later changes
no call site outside `trellis-model`. That property is the reason this ADR can
defer the expensive half of the decision without cost.

What would change our mind: a measured tier-4 population large enough to
matter, or a serving stack that becomes popular while exposing logits without
guided decoding.

## Note on the model floor (S7)

This ADR unblocks the *decision*; it does not by itself produce the
*measurement*.

Weights are now being fetched — HuggingFace serves this machine at roughly
51 KB/s and supports range requests, so the download is resumable and running
in the background. At that rate a 0.5B Q4 model (491 MB) lands in about three
hours and a 3B Q4 (2.10 GB) in about eleven. Two points is a thin floor but it
is a real one, and it is the difference between an open item and an
unanswerable one.

Until those land, **no figure for the model floor appears anywhere**, and v0.1
exits with S7 open rather than estimated. `01-v0.1-measure.md` §9 records it.
