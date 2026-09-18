# ADR 0006 — Adapter tiers, and the inference path

**Status:** Partially accepted. Tiers **Accepted**; inference path **Proposed**,
pending v0.2 measurement (S17).
**Date:** 2026-09-18

## Context

TRELLIS must run on models the user hosts, including models with no native
tool-calling support. Quantization degrades format reliability faster than it
degrades reasoning: a 14B quant may reason adequately about code and still emit
malformed tool calls.

## Decision — grammar authority

**llguidance is the single grammar authority.** Rust-native, so it embeds with
no FFI to a Python stack. It accepts JSON Schema, regular expressions, and a
Lark-like CFG syntax, computing a token mask via an Earley parser over a
derivative-based lexer with lazily-built automata.

Tool schemas are never hand-written BNF: an argument struct derives
`schemars::JsonSchema`, schemars emits JSON Schema, llguidance compiles it.
Implemented and tested in `trellis-model/src/grammar.rs`.

Verified 2026-09-18: llguidance **1.8.0**, **MIT** licensed, upstream last
pushed 2026-09-04.

**API note.** The brief's three-call sketch is illustrative; the real Rust
surface is `Constraint::compute_mask() -> Result<&StepResult>` and
`Constraint::commit_token(Option<TokenId>) -> Result<CommitResult>`, where
`CommitResult { stop, backtrack, ff_tokens }`. `backtrack` and `ff_tokens` are
not optional to handle — fast-forward tokens are a real throughput win and
backtracking must be honoured. `is_stopped` is exposed on the FFI surface;
in Rust the stop signal arrives on `CommitResult::stop`.

## Decision — four tiers

| Tier | Mechanism | Degradation |
|---|---|---|
| 1 `Native` | The model's own function-calling API | none |
| 2 `RuntimeConstrained` | Compiled grammar handed to the runtime's guided-decoding parameter | none for correctness |
| 3 `HarnessConstrained` | TRELLIS owns the sampling loop and applies the mask | ~50us/token mask computation |
| 4 `ValidateAndRetry` | Opaque API: validate returned text, retry on failure | malformed calls retried, not prevented; a run can fail on format alone |

Selected by probing model and runtime at startup, never assumed. `trellis
doctor` reports the active tier and what degrades on it, because the difference
between tier 3 and tier 4 is otherwise invisible to a user.

Because llguidance is already the grammar engine inside llama.cpp, vLLM,
SGLang and Guidance, the grammar compiled from a tool schema is the *same
artefact* whether TRELLIS masks locally or ships it to the serving runtime. One
engine, two delivery paths, no divergence.

## Alternatives considered — grammar engine

**llama.cpp's own grammar sampler.** Slower: no lexer, backtracking parser.

**Outlines.** Pre-computes masks for every automaton state, capping constraint
complexity and adding startup cost and memory. Also Python.

**XGrammar, or a hand-rolled DFA masker.** Both viable fallbacks if llguidance
becomes unmaintained. Both worse today.

## Open — the inference path (S17)

Tier 3 needs logit access, which means **in-process inference, not an HTTP
endpoint**. Candidates: `llama-cpp-2`-style bindings, `mistral.rs`, `candle`.
Each has real consequences for build complexity and hardware coverage.

Deliberately unresolved. To be evaluated in v0.2 against: GPU/accelerator
coverage, build complexity on Windows/macOS/Linux, quantization format support,
and maintenance cadence.

**Containment.** Tiers 1, 2 and 4 need no logit access, so a poor tier-3 choice
degrades the product rather than blocking it.

## What changes our mind

- llguidance becoming unmaintained or relicensed → XGrammar.
- A serving runtime exposing logits over a stable API → tier 3 without
  in-process inference, which would be strictly better.
