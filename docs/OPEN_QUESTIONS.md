# OPEN_QUESTIONS

Grouped by who answers. A question here is one that cannot be settled by
reading the brief, the code, or a specification — it needs a measurement, a
decision, or a person.

---

## Answered by measurement (v0.1)

**Q-1 — What is the resolver's actual recall on poetry?**
The number the whole gate depends on. Thresholds are already recorded (85% gate,
60% pivot) so the measurement cannot be argued against after the fact.
→ `BENCHMARK_AND_EVAL.md` §2. Owner: v0.1.

**Q-2 — Does `DecoratorIndirection` dominate the `Reason` distribution?**
Predicted, and the benchmark repos support it: 20–41% of definitions are
decorated. If confirmed, the S1 fallback is a decorator-unwrapping pass, which
is far cheaper than adopting Pyright. If the dominant reason is instead
`MethodDispatchUntyped`, only type inference helps and Pyright becomes hard to
avoid. → Owner: v0.1.

**Q-4 — How stale can a coverage-derived test map be before it misleads?**
Covering tests are derived by inverting a coverage report. That report ages the
moment code changes — which is continuously, mid-run. Options: regenerate per
run (expensive), accept staleness with a warning, or fall back to path-based
heuristics when the report predates the last commit. No basis to choose yet.
→ Owner: v0.2.

**Q-9 — What is the decomposition ratio on real tasks?**
What fraction of real changes partition into more than one group at all. If it
is low, multi-agent mode is not worth its complexity and v1.1 should be dropped
rather than shipped. → Owner: v0.2/v1.1. Blocks the v1.1 decision entirely.

---

## Answered by a decision (needs an ADR)

**Q-3 — Interner: keep the hand-rolled one?**
`lasso` was nominated with "verify current maintenance"; verified 2026-09-18,
last upstream push 2024-08-19, roughly two years stale. A forty-line hand-rolled
interner is in place. Revisit only if profiling shows interning is hot, or a
maintained crate offers a measurably better layout at scale. Low stakes.

**Q-5 — Should factual trail entries be hashed?**
S10 is contained: trails read back as inert data and judgment is fenced. Hashing
would make tampering *detectable* rather than merely contained. Cost 2–3 days.
Open question is what the hash covers and what happens on mismatch — refuse to
resume, or warn and continue. Refusing is safer and also a way to brick a repo
after an unrelated merge.

**Q-6 — Is degradation-warning suppression per-session or persisted?**
Persisting repeat suppression across sessions is more useful and is also a way
to permanently hide a warning the user should keep seeing. This is the S6
tension in its sharpest form. → `docs/ADRs/0003-degradation-policy.md`.

**Q-7 — What happens when trails grow large enough to dominate context?**
Summarization is the obvious answer; the question is whether a summary is
derived state alongside the full trail (safe, more storage) or a replacement
(compact, lossy, and a place for facts to quietly disappear). ADR 0007 currently
says alongside. Untested at volume.

**Q-8 — How does prompt caching change the cost model?**
`COST_MODEL.md` does not model it. On a loop resending context each turn it
could change the input-token column substantially on a metered API. Needs
measurement before the cost model is quoted to anyone.

**Q-10 — Which in-process inference path for tier 3?**
`llama-cpp-2`-style bindings, `mistral.rs`, or `candle`. Real consequences for
build complexity and hardware coverage. ADR 0006 is deliberately left
**Proposed** on this point. Contained: tiers 1, 2 and 4 need no logit access, so
a poor choice degrades rather than blocks. → Owner: v0.2.

**Q-13 — Where does a blast radius get its unresolved count from? — CLOSED
2026-09-19.**
`blast_radius` walked `reverse` adjacency, but an unresolved edge names no
target and therefore never enters `reverse`. The walk structurally could not
see one, so `RadiusReport::coverage.unresolved` was always zero and
`all_edges_certain()`'s unresolved check was vacuous — backwards from what S2
requires, since the radius is exactly where a user reads coverage.

**Resolved:** the count now comes from the **whole file** of anything the
radius touches (`radius::add_touched_file_misses`) — the root symbol's file,
every transitively reached symbol's file, and the file of every call site in
the radius. Of the three candidate scopes, the whole file is the widest that
is still cheap, and the choice is deliberately the noisy one: over-reporting
doubt costs the user a longer list, under-reporting it produces a blast radius
that silently omits the thing that breaks. The enclosing module was rejected
as arbitrary at file granularity, and the transitive import closure as so wide
it would mark nearly every radius uncertain on a real repository.

**What it costs, measured:** on poetry, `trellis radius Factory` reports 123
resolved call sites alongside 3,023 unresolved ones in the touched files. That
ratio is the honest picture today and it is also unusable as a UI — it is the
S6 degradation-UX problem arriving with real numbers attached, and S6 is now
the most urgent design question in the product, not a v1.x nicety.

---

## Answered by a person (product judgment)

**Q-11 — What does the approval screen actually say?**
The `Verified` / `Reviewed` split is structural in the types and counted
separately everywhere. The wording shown to a user is not written. The first
time something breaks after a clean run, this wording decides whether the user
feels informed or lied to (S3). Needs someone to write it and someone to watch a
user read it.

**Q-12 — Does the honesty mechanism survive contact with repetition?**
Designed answer in ADR 0003: aggregate, never suppress; make "add a test" the
cheapest exit. Whether that holds on the tenth partial-coverage run in an hour
is an empirical question about people, not code, and cannot be answered from
this side of a real user.

---

## Flagged as unverifiable from here

**Q-14 — Is a class-scope assignment allowed to shadow a method?**
`resolve_attribute` checks `t.defs` for `self.m()` but not `t.bindings`, so
`class C: run = something` alongside `def run(self)` resolves to the method
rather than reporting ambiguity. The `Name` path already handles the
equivalent case (`ShadowedAmbiguous`). This is a latent unsound edge. It did
not appear in the poetry audit sample and the pattern is rare, so it is
recorded rather than fixed. → Owner: before the v1.0 gate.

**Q-15 — The v0.1 audit is model-read, not human-read.**
`BENCHMARK_AND_EVAL.md` §2 says "a human reads call sites and records what each
one actually calls", and the brief insists ground truth must not be
tool-derived, because a shared blind spot between the tool and its yardstick
reads as agreement. The 2026-09-19 audit followed the protocol exactly —
blind sample, resolver verdicts withheld, written rules — but was performed by
a model reading source, not a person. A model auditing a call graph does not
share the resolver's *implementation*, but it is not the independent evidence
§2 asks for either. Spot-checks of individual entries were verified by hand
against the poetry source and were correct. Recorded as A-34. **The figure
must be re-derived with a human auditor before the v1.0 gate is claimed.**
→ Owner: whoever claims the gate.

**Q-16 — S6 is now the most urgent open design question, not a v1.x one.**
See Q-13's measured cost: a single realistic radius reports 123 resolved call
sites and 3,023 unresolved ones. The brief schedules degradation UX for v1.x.
The numbers say a user meeting that screen once will not meet it twice, so the
question of what they actually see is now load-bearing for whether the product
is usable at all. → Owner: product, before v0.3.

The brief cites several external results as the basis for its two constraints.
They are reproduced in the ADRs as given and are **not independently verified**
in this repository. Anyone relying on them for a decision should read the
primary sources:

- PyCG's ~99.2% precision / ~69.9% recall figures.
- Huang et al. (ICLR 2024) GSM8K self-correction degradation: 95.5% → 91.5% →
  89.0%.
- Chen et al. (ICLR 2024) self-debugging results and the execution ablation.
- Cemri et al. (2025) MAST: 1,642 traces, 41–86.7% failure rates, seven
  frameworks.
- The 17.2× / 4.4× multi-agent error-amplification figures. This one carries the
  most architectural weight of any cited number — it is the stated basis for
  never parallelizing judgment (ADR 0005) — and it is also the one cited with
  the least specificity in the brief. **Worth chasing to its primary source
  before it is repeated in anything public.**
- Yang et al. (MSR '22) prevalence findings over 3M+ Python files.

Verified in this repository, by contrast: llguidance 1.8.0 is MIT and actively
maintained (upstream push 2026-09-04); `github/stack-graphs` is archived;
`Kixiron/lasso`'s last push was 2024-08-19; tree-sitter 0.27.0 pairs correctly
with tree-sitter-python 0.25.0; `bincode` 3.0.0 is a protest release that
refuses to compile; and all benchmark-repo figures in
`BENCHMARK_AND_EVAL.md` §1.
