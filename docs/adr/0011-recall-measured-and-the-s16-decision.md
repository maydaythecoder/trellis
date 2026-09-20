# ADR 0011 — Measured recall, and the S16 decision it forces

**Status: Proposed.** The evidence is settled; the decision is not, and it is
not the implementer's to make. See "Decision" below.

**Date:** 2026-09-19

## Context

ADR 0002 pre-registered three thresholds on 2026-09-18, deliberately before any
measurement existed, on the stated grounds that "a threshold chosen after
seeing the number is a threshold chosen to be met":

| | Value | Consequence |
|---|---|---|
| v1.0 gate | recall ≥ 85% | Ship the blast-radius claim |
| S1 fallback trigger | recall < 85% | Adopt a semantic backstop |
| **S16 pivot trigger** | **recall < 60%** | **The blast-radius guarantee dies** |

The resolver was completed on 2026-09-19 and the measurement run.

## Evidence

Corpus: `poetry-2.5.0.tar.gz` (PyPI sdist), SHA-256
`e73b873890526c03e4d8e20d934292175f55a5823b3bdee2f154f63dae81336f`. 451 files,
81,700 lines, 21,794 call sites.

Method: 400 call sites drawn uniformly, seed 20260919, audited blind against
the written protocol in `BENCHMARK_AND_EVAL.md` §2 — the auditor saw the call
site and its source, never the resolver's verdict. 159 of the 400 had a
first-party true callee and form the denominator.

### The headline

| | |
|---|---|
| **Recall** | **47.8%** (76/159) |
| 95% CI | **[40.0%, 55.6%]** |
| **Precision** | **100.0%** (76/76) |

**The entire confidence interval sits below the 60% pivot trigger.** This is
not a borderline result and no reasonable re-reading of the interval reaches
60%.

### Stratified

| stratum | denominator | recall |
|---|---|---|
| `Name` callees | 72 | **84.7%** |
| `Attribute` callees | 87 | **17.2%** |
| `Dynamic` callees | 0 | n/a |
| `src/` | 36 | 58.3% |
| `tests/` | 123 | 44.7% |

### What passed, and it is not trivial

- **Precision 100%** on every audited `Resolved` edge. Zero unsound edges.
- **4,443 of 4,443** `Resolved` edges on the corpus pass the mechanical
  necessary-condition check (`examples/check_soundness.rs`), and 2,878 of
  2,878 on a second corpus.
- **Zero wrongly excluded**: of 114 sites the resolver dropped as external,
  the auditor confirmed all 114. The Pass 2 first-party/third-party split is
  correct, which is the mechanism ADR 0001 rests on.

So v1.0 gate item 2 — zero unsound edges — is on track. **This is a recall
finding only.** The graph does not lie; it is quiet.

## Diagnosis

Three things the evidence says, none of which change the verdict but all of
which bear on what to do next.

1. **The loss is concentrated in `Attribute` call sites.** `Name` resolution
   at 84.7% is close to the 85% gate on its own. `Attribute` at 17.2% carries
   55% of the denominator and is where essentially all of the deficit lives.

2. **The brief's decorator prediction is falsified.** It expected
   `DecoratorIndirection` to dominate the misses, and S1's recorded patch was
   built on that expectation. Corpus-wide it is **3 sites out of 11,233**
   unresolved; `MethodDispatchUntyped` is **10,646**. Decorated-definition
   density (36% of poetry's definitions) was real and simply did not predict
   the miss distribution, because decorated definitions are rarely called by
   name from first-party code.

3. **The misses have a shape.** Of first-party misses, 51% of auditor
   reasonings cite a pytest fixture, and the recurring pattern is a receiver
   whose type is locally evident — `self._x = SomeClass()` in `__init__`, or a
   parameter with a type annotation. poetry is mypy-strict.

Point 3 is why this ADR is Proposed rather than Accepted: the pre-registered
rule says the guarantee dies, and the diagnosis says the deficit sits in a
specific, unimplemented, and probably tractable capability. **Those are not in
conflict — they are answers to different questions**, and noticing the second
is not grounds for the implementer to set aside the first.

## The rule's verdict

47.8% < 60%. **The S16 pivot trigger has fired.**

`BENCHMARK_AND_EVAL.md` §3 anticipated the temptation to argue past this: "a
backstop that has to find two call sites in five is doing the primary work."
At 47.8%, reaching the 85% gate requires recovering roughly 72% of the
remaining misses. That is the primary work, which is what §3 defined the 60%
line to mean.

## Options

**A. Follow the pre-registered rule.** Pivot per `MISSION.md`: the index is
demoted from guarantee to hint, and the product ships on context continuity,
grounded checks, clean rollback, bounded budgets, and the committed trail.
Note that the hint is a hint with **measured 100% precision** — everything it
says is true, it simply says less than hoped. This is what pre-registration
means and is the default.

**B. Re-audit with a human before deciding.** A-34 and Q-15 record that this
audit was model-read where §2 asks for human-read. A pivot is as consequential
as a gate and deserves the same evidence standard. Cost: one auditor applying
one rule to ~160 first-party sites.

**The sensitivity analysis below is the honest reason this is worth doing
rather than a formality**, and it is a stronger argument for B than the
confidence interval is.

**C. Build one bounded backstop and re-measure.** Annotation- and
assignment-guided receiver typing, confined to the current file plus
`conftest.py`, aimed squarely at the `Attribute` deficit. This **deviates from
the pre-registered rule** and needs its own ADR saying why, or the
pre-registration was theatre. Estimated 6–10 days, unvalidated.

## Decision

**Deferred to the project owner.** The measurement is the implementer's job;
overriding a threshold that was deliberately fixed in advance is not.

## Known limitations of this measurement

- **Model-read, not human-read** (A-34, Q-15). Spot-checks were verified by
  hand and were correct.
- **Auditor disagreement on the denominator.** First-party rates ranged 28% to
  58% across auditors, and of 60 sites whose reasoning cites a pytest fixture,
  39 were judged first-party and 21 were not. Some of that spread is ordinary
  binomial noise at 50 sites; not all of it is. Because `is_first_party` sets
  the denominator, this moves recall in a way the confidence interval does not
  capture — so it is bounded explicitly rather than waved at:

  | scenario | recall |
  |---|---|
  | every fixture site judged *not* first-party was actually a first-party miss | **42.2%** (76/180) |
  | **as measured** | **47.8%** (76/159) |
  | every fixture site judged first-party was actually external | **60.8%** (73/120) |

  Both bounds are deliberately absurd — they assume one entire category was
  misjudged in the same direction, with no offsetting error. **The upper bound
  crosses the 60% trigger, by 0.8 of a percentage point.** That is the most
  favourable reading the data admits under any consistent rule, and it is
  reached only by assuming every auditor who called a fixture first-party was
  wrong and every auditor who did not was right.

  It does not change the verdict: the sampling interval and the disagreement
  would both have to break the same way, each at its maximum. It does mean the
  honest statement is "the result is robust to plausible disagreement", not
  "no disagreement could reach the trigger" — the latter would be false.
- **`dynamic` verdicts were excluded from the denominator** (3 sites). That is
  generous to the resolver.
- **The corpus includes the test suite**, which is 123 of the 159 first-party
  sites. Recall in `src/` alone is 58.3%, but on n=36 that is ±16pp and still
  does not reach 60%.

## What would change our mind

A human re-audit landing materially above 60%. Nothing else: not a better
diagnosis, not a promising backstop, not schedule pressure. Those are inputs to
*which option* is chosen, not to whether the trigger fired.
