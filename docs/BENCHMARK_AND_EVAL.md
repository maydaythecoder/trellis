# BENCHMARK_AND_EVAL

Milestone v0.1 in full. No product ships from v0.1; its output is numbers.

Everything in this document that is a measurement is labelled with how it was
obtained. Everything that is a threshold is labelled as a choice. The two are
never mixed.

---

## 1. Benchmark repository (resolves S14)

### Selection criteria

1. **Mid-size**: 20k–100k lines of Python. Small enough to hand-audit a
   ground-truth sample, large enough that resolution cost is visible.
2. **First-party-import-heavy**: a substantial fraction of imports resolve
   inside the repository. A repo that mostly calls its dependencies exercises
   nothing — Pass 2 discards third-party names.
3. **Real test coverage**: a test suite that runs, so `run_tests` verdicts mean
   something and covering-test derivation has something to derive from.
4. **Idiomatic, not exotic**: heavy decorator and context-manager use, which
   Yang et al. (MSR '22, 3M+ Python files) found widespread, and little `eval`,
   which they found rare. Misses should be boring and common.
5. **Permissively licensed and stable**, so the ground-truth audit stays valid.

### Candidates, measured

Measured 2026-09-18 against shallow clones of each project's default branch.
Line counts are total lines in `.py` files including blanks and comments — not
SLOC. Import, decorator and dynamic-construct counts are `grep` proxies and
overcount slightly; the `parsed by TRELLIS` block beneath is exact, produced by
`trellis index` (Pass 1) at commit of this document.

| | flask | poetry | scrapy |
|---|---|---|---|
| `.py` files | 83 | 279 | 504 |
| total `.py` lines | 18,345 | 44,822 | 93,650 |
| test files (`test_*.py`) | 27 | 54 | 157 |
| import statements | 696 | 2,908 | 4,989 |
| first-party imports | 223 | 1,093 | 1,752 |
| — absolute / relative | 47 / 176 | 1,093 / 0 | 1,729 / 23 |
| first-party share | 32.0% | 37.6% | 35.1% |
| `getattr` calls | 21 | 22 | 102 |
| `eval`/`exec` calls | 2 | 0 | 9 |
| star imports | 0 | 2 | 0 |

Parsed by TRELLIS (Pass 1, exact):

| | flask | poetry | scrapy |
|---|---|---|---|
| definitions | 1,622 | 2,163 | 7,824 |
| — decorated | 658 (40.6%) | 784 (36.2%) | 1,576 (20.1%) |
| imports | 648 | 2,654 | 4,588 |
| call sites | 3,963 | 10,421 | 24,461 |
| — syntactically dynamic | 57 (1.4%) | 22 (0.2%) | 12 (0.05%) |

### Choice

**Primary: `python-poetry/poetry`.** It sits in the middle of the size range,
has the highest first-party import share of the three, a real test suite, and
the cleanest import style (entirely absolute, so relative-import resolution
does not confound the first recall measurement). Its dynamic-construct density
is the lowest of the three (22 `getattr`, no `eval`), which means a low recall
figure against it would be a resolver problem rather than a repo problem — that
is the property that makes it a good first measurement.

**Secondary: `pallets/flask`.** Small enough to hand-audit generously, and
import style is almost entirely *relative* (176 relative against 47 absolute).
Relative-import resolution is a distinct code path with distinct failure modes,
and poetry would never exercise it. Highest decorator density of the three at
40.6% of definitions, so it is also the sharpest test of the
`DecoratorIndirection` prediction.

**Scale case: `scrapy/scrapy`.** Nearly 94k lines and 24k call sites, the top
of the v1.0 supported range (S9). Also the most dynamic dispatch by absolute
count (102 `getattr`). Used for build time, memory, and incremental latency —
not for the recall gate, because hand-auditing a sample of 24,461 call sites to
the required confidence is not affordable.

### The pinned corpus actually measured (2026-09-19)

The 2026-09-18 figures above were taken against shallow clones of each
project's default branch. Those clones no longer exist, and on 2026-09-19 this
machine could not reach GitHub: `raw.githubusercontent.com` failed TLS
revocation checking (`CRYPT_E_REVOCATION_OFFLINE`) and `git clone` stalled at
roughly 140 bytes/second.

The corpus was therefore obtained from PyPI instead, which is **more**
reproducible than a branch tip, not less — a released sdist is immutable and
hash-addressable, where `main` moves:

| | |
|---|---|
| Artefact | `poetry-2.5.0.tar.gz` (PyPI sdist) |
| SHA-256 | `e73b873890526c03e4d8e20d934292175f55a5823b3bdee2f154f63dae81336f` |
| Obtained | 2026-09-19, `pip download poetry --no-deps --no-binary :all:` |

It contains both `src/` and `tests/`, so the test-suite criterion is met.

**It is not the same corpus as the row above and the numbers differ, because
the sdist includes the test suite.** Measured by `trellis index` at commit of
this section:

| | poetry 2.5.0 sdist |
|---|---|
| `.py` files | 451 (193 under `src/`, 258 under `tests/`) |
| total `.py` lines | 81,700 |
| test files (`test_*.py`) | 129 |
| definitions | 3,735 |
| call sites | 21,794 |
| — first-party (recall denominator) | 15,707 |
| — external, excluded | 6,087 |

Build time, release build, warm page cache, three runs: **116.9 / 118.9 /
119.2 ms**. That is roughly 690 lines/ms wall-clock for parse *and* resolve,
against the ~1,400 lines/ms measured for Pass 1 alone — so resolution roughly
doubles the cost of parsing, and neither is close to being the constraint at
this size (S9). Peak working set: **37.9 MB**.

`.trellis/index.bin` (postcard): **589.7 KiB** for 81,700 lines of Python,
roughly 7 bytes per line. That size is the reason A-24 gitignores it rather
than committing a blob that changes on nearly every edit. A `trellis radius`
query against a warm cache returns in **48 ms** total process time, against
~120 ms to rebuild.

### What would change this choice

If poetry's first recall figure lands high enough to look suspicious — above
the ~70% PyCG whole-program figure despite a simpler algorithm — check first
whether the denominator is wrong (see §2) before believing it.

---

## 2. Recall measurement

### Ground truth is hand-audited, not tool-derived

Comparing TRELLIS's graph to Pyright's, or to PyCG's, measures **agreement**,
not correctness, and the two share blind spots — both are static analyses of
the same dynamic language. A shared blind spot reads as agreement, which is the
worst possible failure of a ground-truth method: it inflates the number in
exactly the cases that matter.

So: a human reads call sites and records what each one actually calls.

### Sampling

- **Sample size: 400 call sites**, drawn uniformly at random from the pinned
  corpus's **21,794** call sites, seed **20260919**, by
  `examples/audit_sample.rs`. The frame is every call site, not the
  first-party ones: whether a callee is first-party is exactly what the
  auditor decides, so pre-filtering by the resolver's own opinion would let it
  choose its own denominator.
- **Correction (2026-09-19).** The ±3.5pp figure previously stated here was
  wrong. It assumed the recall denominator equals the sample size. It does
  not: recall is measured over sites whose *true callee is first-party*, and
  the audit found those to be roughly 38% of all call sites. 400 draws put
  about 150 sites in the denominator, and the 95% Wald interval near p = 0.47
  is therefore roughly **±8 percentage points**, not ±3.5.
- The thresholds in §3 are 25 points apart, so ±8pp still places a measurement
  against them — but only when it does not land near a boundary. A future
  audit must size its sample on the **expected first-party count**: roughly
  1,000 draws to put 400 sites in the denominator.
- **Stratification**: the sample is additionally reported broken down by callee
  shape (`Name` / `Attribute` / `Dynamic`), because a single pooled figure hides
  the case that matters. A resolver that is 95% on `Name` and 20% on
  `Attribute` has a very different fallback decision from one that is uniformly
  75%.

### Denominator, stated precisely

Recall is measured over **call sites whose true callee is first-party**.

A call site whose callee is genuinely `os.path.join` is not a miss — Pass 2
discards third-party names by design, and counting those in the denominator
would understate recall by roughly the third-party share of calls, which is
large. The auditor therefore records for each sampled site: the true callee,
and whether it is first-party. Sites with a third-party callee are excluded
from the denominator and counted separately, so the exclusion is auditable
rather than assumed.

**The resolver applies the same definition, and until 2026-09-20 it did not.**
`typed_receiver` recorded a receiver whose stated type was not a first-party
class as `MethodDispatchUntyped` — a first-party miss. So `def go(val: str):
val.lower()` counted against recall, although the source states the receiver is
a `str` and the callee is a standard-library method. ADR 0015 corrected it: a
site leaves the denominator when its type is *demonstrably* not first-party (a
builtin nothing shadows, or a name whose every binding resolves outside the
root), and a type the resolver merely fails to resolve stays a miss.

That asymmetry is what keeps this figure falsifiable. If failure to resolve
were an exclusion, every resolver gap would remove itself from its own
denominator and recall could only rise.

**A wrong exclusion is invisible to everything except this audit.** An excluded
site emits no edge, so `examples/check_soundness.rs` cannot see one, and the
repository-wide report cannot count what it has excluded from itself. That is
why `audit_score` reports **wrongly excluded** as a first-class figure: sites
dropped as external that the auditor found first-party. Recorded as S21.

### Definitions

- **Recall** = (sites where `Resolved` names the true callee) / (sites whose
  true callee is first-party).
- A `Candidates` result **containing** the true callee counts as **not
  resolved** for recall. It is reported separately as *ambiguous-but-covered*.
  This is the conservative reading and the right one: the product's claim is
  that it names the call sites, and naming four possibilities is not naming one.
- **Precision** = (sites where `Resolved` names the true callee) / (sites where
  anything was `Resolved`). An unsound edge — `Resolved` naming the *wrong*
  symbol — is a precision failure, and by the v1.0 gate there must be zero of
  them.

### What the resolver bound, which is **not** recall

Measured 2026-09-20 on the pinned corpus, after ADR 0012 (receiver typing) and
ADR 0015 (the denominator correction). Read the caveat before the numbers.

| | sites | bound | share |
|---|---|---|---|
| **all first-party** | 13,510 | 5,908 | **43.7%** |
| `Name` shape | 4,072 | 3,472 | **85.3%** |
| `Attribute` shape | 9,416 | 2,436 | **25.9%** |
| `Dynamic` shape | 22 | 0 | 0% |

Unresolved reasons, over the 6,737 unresolved sites: `MethodDispatchUntyped`
6,090, `LocalBinding` 558, `DecoratorIndirection` 63, `DynamicAttribute` 22,
`StarImport` 2, `UnboundName` 2. Ambiguous: 865. Excluded as external: 8,284.

**Two corrections are folded into that table, and neither is a recall
improvement in the same sense.**

ADR 0012's receiver typing genuinely resolved more sites: `Attribute` binding
rose because annotations and constructor assignments now type receivers.

ADR 0015 changed the *denominator*, not the numerator. It moved 15,707 to
13,510 while `resolved` stayed at 5,908 — **not one additional call site became
resolvable**. Quoting 37.6% → 43.7% without that sentence describes an
improvement that did not happen.

| | before ADR 0015 | after |
|---|---|---|
| Denominator | 15,707 | 13,510 |
| Excluded as external | 6,087 | 8,284 |
| **Resolved** | **5,908** | **5,908** |
| Bound | 37.6% | 43.7% |

**This figure is a lower bound on recall and must not be compared to the §3
thresholds.** Recall is defined over call sites whose *true callee* is
first-party. The resolver cannot tell whether `logger.info()` or
`session.query()` ends up in the standard library, so it counts every
unknown-receiver call as a miss whenever it cannot show otherwise. ADR 0015
removed the cases where it *can* show otherwise; what remains is every call on
a receiver whose type is simply unknown, and some unknown share of those are
third-party. The 43.7% therefore still understates recall, and only the audit
can say by how much. This is recorded as A-30.

Two observations that do not depend on the audit:

1. **The loss is almost entirely in `Attribute`.** `Name` binds at 85.3%;
   `Attribute` at 25.9% while carrying 70% of all first-party sites. A pooled
   figure would have hidden this completely, which is why §2 requires the
   split.
2. **The brief's prediction about decorators did not hold.** It expected
   `DecoratorIndirection` to dominate the misses. It is 63 sites out of 6,737.
   The dominant reason is `MethodDispatchUntyped` — unknown receiver types —
   by two orders of magnitude. If a semantic backstop is adopted under S1,
   this says it must be a *type inference* for receivers, not the decorator
   unwrapping the brief anticipated.

Counted directly in the corpus for scale: only 1,052 of the first-party
`Attribute` sites are direct `self.x(...)` calls. Following base classes to
resolve inherited `self` dispatch — the obvious next resolver feature — can
therefore address a small share of the `Attribute` gap, and is not the lever
it appears to be.

**Two candidate backstops measured 2026-09-20 and rejected**, both of which the
roadmap had expected to be the answer (`docs/bench/`):

- **pytest fixture return types: already closed.** Parameter annotations have
  been collected as type facts since ADR 0012. poetry annotates its fixture
  parameters, so exactly **10** call sites in the corpus have an *unannotated*
  test-function parameter as receiver, and none of those names a fixture.
- **Return-type propagation: 67 sites.** One hop, both `f().m()` and
  `x = f(); x.m()`, requiring the callee first-party and return-annotated, the
  return type a first-party class, and the method present on it. Sixty-seven
  sites is **0.4 percentage points**, against a rule whose every application is
  a fresh chance to emit an unsound edge.

Classifying every attribute-shaped call site by receiver shape
(`docs/bench/receiver_shapes.py`) is what found ADR 0015's denominator error
instead. The remaining bulk sits in `attribute chain (a.b.m())` (1,969 sites)
and locals whose type is not derivable within one file (1,883) — both requiring
cross-file type propagation, which is a larger commitment than anything
attempted so far and has not been costed.

> **These are the FIRST-measurement figures.** They fired the S16 pivot
> trigger, which was then averted by the ADR 0012 backstop. For the
> final-state numbers — recall 72.0% on a fresh sample, precision 100% — see
> `01-v0.1-measure.md` §7. Do not quote 47.8% as the current recall.

### RESULT — the hand audit, 2026-09-19

400 call sites, seed 20260919, audited blind against the protocol above. 159
had a first-party true callee and form the denominator. Artefacts, including
every judgement and its one-line reasoning, are committed under
`docs/bench/poetry-2.5.0-seed20260919/`.

| | |
|---|---|
| **Recall** | **47.8%** (76/159) |
| 95% CI | **[40.0%, 55.6%]** |
| **Precision** | **100.0%** (76/76) |
| Ambiguous, true callee covered | 2 |
| Wrongly excluded as external | **0** of 114 exclusions |
| Auditor could not determine | 3 |

Stratified:

| stratum | denominator | recall |
|---|---|---|
| `Name` callees | 72 | **84.7%** |
| `Attribute` callees | 87 | **17.2%** |
| `Dynamic` callees | 0 | n/a |
| `src/` | 36 | 58.3% |
| `tests/` | 123 | 44.7% |

**Against §3: 47.8% is below the 60% pivot trigger, and the whole confidence
interval is below it.** See ADR 0011. The decision that follows is the project
owner's, not the measurement's.

What passed and should not be lost in that: precision is 100%, every one of
the 4,443 `Resolved` edges passes the mechanical check below, and all 114
third-party exclusions were confirmed by the auditor. The graph does not lie.
It is quiet.

#### Limitations of this measurement

- **Model-read, not human-read** — §2 above asks for a human. A-34, Q-15.
  Spot-checks were verified by hand and were correct.
- **Auditors disagreed about the denominator.** First-party rates ranged 28%
  to 58% across auditors; of 60 sites whose reasoning cites a pytest fixture,
  39 were judged first-party and 21 were not. Some of that is binomial noise
  at 50 sites per auditor; not all of it is. `is_first_party` sets the
  denominator, so this moves recall in a way the CI does not capture.
- **`dynamic` verdicts are excluded from the denominator** (3 sites), which is
  generous to the resolver.
- Four sites were counted correct under the constructor convention (the
  resolver named the class, the auditor its `__init__`). Each is listed in
  `score.txt` rather than folded in silently.

### Soundness, checked over every edge rather than sampled

`examples/check_soundness.rs` verifies a necessary condition on **every**
`Resolved` edge: the symbol named must be the final identifier of the call
expression that produced it. Result, 2026-09-19:

| corpus | Resolved edges | violations |
|---|---|---|
| poetry 2.5.0 sdist | 4,443 | **0** |
| second corpus (371 files, 13,276 call sites) | 2,878 | **0** |

This matters for v1.0 gate item 2, which asks for zero unsound edges across
the corpus. The 400-site audit cannot establish that — it bounds the *rate*,
and a rate low enough to go unobserved at 400 sites still leaves dozens across
15,000 edges. The mechanical check is necessary but **not sufficient**: an
edge to the wrong `foo` in a different module satisfies it, and only the audit
catches that. The two are complementary and neither substitutes for the other.

### Incremental update latency — not implemented

The brief (4.3) specifies `Tree::edit` plus reparse of only the changed file,
then re-resolving the invalidated edges. **That is not built.** The honest
current figure for "latency on single-file save" is therefore the full rebuild
cost: **~120 ms** on poetry, since nothing smaller exists. Recorded as a
measurement of the current state rather than left blank, and it is a real cost
because the agent loop edits files mid-run — this sits in the inner loop.

### Also measured in v0.1

| Metric | Why |
|---|---|
| `Reason` distribution over unresolved sites | Input to the S1 fallback decision. If `DecoratorIndirection` dominates as predicted, the fallback is a decorator-unwrapping pass, not a whole semantic engine. |
| Full index build time | Baseline for S9. |
| Incremental update latency on single-file save | The loop edits files mid-run; this is in the inner loop. |
| Peak RSS | S9: no published numbers exist for any Python tool at 1M lines. |
| `.trellis/index.bin` size, postcard | Derived state committed to the repo; size is a real cost to the user. |

### Baseline already measured (Pass 1 only)

Release build, 16 logical cores, warm page cache, three runs:

- poetry: 279 files parsed in 31.8 / 32.0 / 33.5 ms.
- That is roughly 1,400 total lines/ms wall-clock across 16 cores, or about
  **88 lines/ms/core** — consistent with the ~90 lines/ms figure the brief
  cites for tree-sitter.

Parsing is not the problem. Resolution, memory, and storage are unmeasured.

---

## 3. Thresholds (resolves S16)

**These are choices, made before any measurement exists, deliberately. They are
not predictions of what the number will be.**

Recall is measured as defined in §2 — first-party denominator, `Candidates`
counted as unresolved.

| Threshold | Value | What happens |
|---|---|---|
| **v1.0 gate** | **recall ≥ 85%** | Ship the blast-radius claim as specified. |
| **S1 fallback trigger** | **recall < 85%** | The import-scoped resolver is insufficient alone. Adopt a semantic backstop before v1.0 (see §4). |
| **S16 pivot trigger** | **recall < 60%** | The blast-radius guarantee dies. Pivot per `MISSION.md`: index demoted to hint, ship context continuity, checks, and the trail. |

### OUTCOME, 2026-09-19

Measured recall **47.8%**, 95% CI **[40.0%, 55.6%]**. **The S16 pivot trigger
fired.** The thresholds below were set on 2026-09-18, before the resolver
existed, and are reproduced here unchanged. ADR 0011 records the evidence, the
diagnosis, and the options; the decision is the project owner's.

### Reasoning for 85%

The threshold is set by what the *unresolved list* costs the user, because that
list is the product's honesty mechanism and also its main friction.

At 85% recall on a function with 50 first-party callers, roughly 7 are
unresolved. Seven items is a list a person reads. The degradation UX in S6 —
"proceed with gaps recorded / inspect candidates / add a test" — is tractable at
that size and becomes theatre well before it doubles.

85% is also deliberately *above* PyCG's ~69.9%. That looks wrong for a simpler
algorithm and is not, because the problems differ: PyCG resolves whole-program
call graphs including flows through third-party code, while TRELLIS resolves
only first-party names and discards everything else at Pass 2. The easier
problem should produce the better number. If it does not, the resolver is
underperforming its own design rather than hitting a language limit — which is
precisely the finding that should trigger the fallback.

### Reasoning for 60%

Below 60%, the majority of first-party call sites are still named, but the
unresolved list on a 50-caller function runs to 20 items. At that size the user
must re-derive the scope themselves, which is the task the product exists to
remove. Goal state 2 is then not delivered in any meaningful sense, and
continuing to ship a blast-radius claim would be the "confidently reports a
blast radius missing the thing that breaks" failure the brief names as worse
than no tool at all.

The 25-point gap between the two thresholds is the band where a semantic
backstop is worth its cost. Below it, no backstop rescues the claim, because a
backstop that has to find two call sites in five is doing the primary work.

---

## 4. S1 fallback, with its trigger recorded

Fires when recall < 85%. Options, both assessed 2026-09-18:

| | Pyright over LSP | GitHub stack-graphs |
|---|---|---|
| Language | TypeScript, on Node | Rust, native |
| License | MIT | MIT / Apache-2.0 |
| Type-aware | Yes | No |
| Incremental | Per-file | Per-file, by design |
| Call-graph dump | No batch API; must be driven query by query | Not its output either |
| Memory | Multi-GB on large repos | Modest |
| **Maintenance** | Active | **Repository archived** (verified 2026-09-18: `github/stack-graphs`, `isArchived: true`, last push 2025-09-09) |

**Chosen fallback: Pyright over LSP**, and the choice is now forced rather than
weighed. The brief assessed stack-graphs as "recently inactive"; it is, as of
this writing, formally archived. Adopting an archived dependency for the
load-bearing correctness path is not a trade worth making, and its Python
ruleset was already pinned to old grammars with open cross-file resolution
bugs.

Pyright's costs are real and accepted: a Node process, multi-GB memory on large
repos, and no batch call-graph dump, so it is driven query by query. That last
property is what makes it viable as a *backstop* rather than a replacement — it
is invoked only for call sites the primary resolver left `Unresolved` or
`Candidates`, which by construction is a minority of sites. How far that
trigger, and cross-file type propagation, could move recall on the two audited
samples is bounded in [`bench/backstop-ceiling.md`](bench/backstop-ceiling.md)
(ceilings, not recall figures).

**Cheaper intermediate, to try first.** If the `Reason` distribution shows
`DecoratorIndirection` dominating — the prediction, supported by the 20–41%
decorated-definition density across all three candidate repos — then a decorator
unwrapping pass is a far smaller change than adopting Pyright: follow the
decorator to its definition, and where it returns a function defined in the
same first-party module, bind through it. Measure the `Reason` distribution
before committing to Pyright, and record the result in an ADR.

---

## 5. Model floor spike (S7)

**Measured, one configuration, 2026-09-20.** The design below is unchanged;
what follows it is what was actually run, what it establishes, and what it
does not.

Run under grammar constraint throughout, so the floor measured is about
**judgment, not format**. Constrained decoding guarantees a well-formed call,
not a sensible one: a 3B model will reliably emit valid JSON calling the wrong
function.

- **Tasks**: 20–30 real changes drawn from poetry's own git history, each with a
  known-good diff as the gold trace.
- **Repetitions**: 5 runs per task per configuration. A floor set on a mean is a
  lie; the floor is set on the **worst case or p25**, stated explicitly.
- **Configurations**: the floor is reported as a **configuration —
  model × quantization × runtime** — never as a parameter count. A
  well-quantized 14B on a good runtime beats a badly-quantized 32B, and "7B
  minimum" tells a user nothing.

### Metrics, in priority order

1. **Check-recovery rate — primary.** Given a failing type check or test, does
   the next iteration reduce the failure count? Report failures resolved per
   iteration, *and* the rate of iterations that increase failures. The whole
   architecture rests on the model acting sensibly on check output; a model that
   cannot read a type error and make progress is unusable here regardless of
   everything below.
2. **Task completion rate.** The headline number, but lagging.
3. **Argument correctness.** The grammar guarantees a well-formed path string,
   not an existing file. Graded against the gold trace.
4. **Termination discipline, both directions.** Stopping early with the job half
   done, and looping past the point of progress. Different failures, different
   fixes; never averaged together.
5. **Run-to-run variance.** Sets the floor, per above.
6. **Context degradation.** Same task at 25 / 50 / 75% window fill. Output feeds
   the context budget in S5.
7. **Wall-clock per task**, including constraint overhead.

### 5.1 What was run

| | |
|---|---|
| Configuration | Qwen2.5-0.5B-Instruct, **Q4_K_M**, candle on CPU, tier 2 (llguidance) |
| Corpus | poetry 2.5.0, SHA-256 in `01-v0.1-measure.md` §2 |
| Tasks | 20, `docs/bench/floor_tasks.json` |
| Checker | `mypy --ignore-missing-imports --no-error-summary --no-color-output --hide-error-context --follow-imports=silent --no-incremental` |
| Budget | 8 iterations, 600 s wall clock, plus the one escalation turn |
| Harness | `docs/bench/run_floor.py`; report `docs/bench/floor-qwen2.5-0.5b-q4km.json` |

The serving runtime is a throwaway probe (`.bench/probe`) speaking llama.cpp's
`/completion`, so the **real** `RuntimeModel` adapter drove it unchanged. A
probe with its own model path would have measured a code path no user runs,
and the adapter's behaviour under a weak model is precisely what is in
question.

### 5.2 Result

| Configuration | Harness | Repaired | no-progress | budget-exhausted | format-failure | Median s/task |
|---|---|---|---|---|---|---|
| Qwen2.5-0.5B-Instruct Q4_K_M, candle CPU, tier 2 | as at `24bba3d` | **0 / 20** | — | 16 | 3 | 209 |
| the same | with the two §5.3 fixes | **0 / 9** | — | 9 | 0 | — |
| Qwen2.5-Coder-3B-Instruct Q4_K_M, candle CPU, tier 2 | with the no-progress stop | **0 / 17** | 14 | 3 | 0 | ~150 |

The 3B row is 17 tasks, not 20: the host stopped the job for memory pressure
with three outstanding. See §5.6. `no-progress` did not exist when the first
two rows were measured, which is why they show none — the cost columns are
comparable only within a harness version, the `repaired` column across all of
them.

`errors_after == errors_before` on all 20 tasks of the first row.

The second row is **9 tasks, not 20**: the run was stopped by the host at task
9 for system memory pressure, which is unrelated to the run and says nothing
about it. It is reported at the size it reached rather than extrapolated, and
its purpose — the per-task A/B in §5.3 — does not need the full set.

**Nothing was repaired, and nothing was made worse.** The second half matters:
the model never increased a file's error count, so this is not a configuration
that damages a repository. It is one that does not move it.

The tasks are not uniform — each defect raises its file's error count by a
verified amount between one and five — so repair is scored as a return to the
count measured on the clean tree immediately before the defect was applied, by
re-running the checker in the scoring harness. Never by reading the run's own
verdict.

### 5.3 What this establishes, and what it does not

**It does not locate the floor.** One configuration measured below it gives a
lower bound: the floor is somewhere above Qwen2.5-0.5B-Instruct Q4_K_M. A floor
is an interval, and finding it needs a configuration that passes. The second
(Qwen2.5-Coder-3B Q4_K_M) was still downloading when this was written.

Three findings do not depend on the remaining configurations.

**1. Constraint eliminates the format-*shape* failure, and not termination.**
In one unconstrained probe on the demo repository — N=1, on a different corpus
from the floor tasks — the same model emitted a fenced ` ```json ` block
containing `repo.edit`, an invented tool wrapped in prose. Under tier 2, across
the 20 floor tasks, every *token* was schema-valid and no generation was
off-schema.

Note the unit. Every token being valid is not every generation being a complete
object, and the difference is the next paragraph.

Three tasks still failed on format, and the mechanism is worth stating because
it is not a broken constraint. The model emitted

```
{"path":"...","old":"<error>Error: <error>...<warning>Warning: ...
```

and kept going until `max_tokens` ran out. Every token was schema-valid: a JSON
string may be arbitrarily long, so no grammar can stop it, and the
`whitespace_flexible: false` setting that solved an earlier stall does not reach
inside a string value. **Constraint guarantees prefix validity, not
termination.** Any claim that tier 2 makes output "well-formed by construction"
has to carry that qualification.

The adapter reported this as "no JSON object in output", which is true and
misdiagnoses it — that is the message an *ignored* schema produces, and it
sends a reader to check whether their runtime supports guided decoding when the
answer is that it does. The two are now told apart on a local discriminator: an
object that opened and never closed was truncated; no brace at all means the
schema was ignored.

**2. Constraint guarantees nothing whatever about content.** Among the valid
calls this configuration produced:

```
edit{path: "src/…/config_source.py", old: "src/…/config_source.py", new: "src/…/config_source.py"}
edit{path: "…/file_config_source.py", old: "<old value>", new: "<new value>"}
edit{path: "…/dict_config_source.py", old: "...", new: "..."}
edit{path: "tests\u{1}installation\u{1}test_chef.py", old: "156", new: "156"}
```

Schema placeholders echoed back, the file path filled into all three fields,
line numbers used as edit anchors, control characters as path separators. This
is the distinction the spike exists to measure, and it is now demonstrated
rather than asserted.

**3. The run found two harness defects, both of which flattered the model.**
Neither was visible from reading the code.

- `run_checks` with no `paths` reached mypy as `.`, so it checked the whole
  repository and returned poetry's **pre-existing** errors. The observation gave
  no hint this had happened, so it read as a report about the model's own
  change — and the model edited a file it saw in that output. Passing no path is
  the model's error; being unable to tell is the harness's. There is now a
  `SCOPE:` line when and only when `paths` is empty.
- An edit whose `old` equals its `new` wrote identical bytes back and reported
  `applied: true`. The model was told it had acted, the file entered the
  rollback set and the trail's proposed list, and an iteration was spent.
  Refused now.

Both are fixed, and the same configuration is re-run against the fixed harness
as the second row of §5.2. That A/B is the only thing separating "this model is
below the floor" from "this harness withheld what it needed", and the
conclusion is not entitled to the first reading without it.

Under greedy decoding the readout is **per-task divergence, not the tally**.
Each task's first move must be identical to the first run, because the prompt
is identical; trajectories can only diverge after the first observation the
fixes changed. If the model reads `SCOPE:` and then passes paths, the harness
mattered. If it reads it and emits the same edit, the model is the constraint.

The truncation-message change shipped in the same commit does not contaminate
this comparison: adapter retries are internal, and that text never enters the
model's prompt.

**Result of the A/B, over the 9 tasks it reached: the harness was not the
constraint.**

| | |
|---|---|
| Tasks with a byte-identical trajectory to run 1 | **8 of 9** |
| Tasks repaired, either run | 0 |
| `run_checks` calls that passed a path, after the `SCOPE:` line | **0 of 10** |
| `read_file` calls, across 81 turns | 1 |

The last two lines carry it. The `SCOPE:` line states in plain words that no
paths were given, that the whole repository was checked, that failures already
present are not the model's, and that `paths` should name the files the task
names. The task names its file in its first sentence. Across every call in
every task, the model passed no path — and in 81 turns it read a file once.

The single divergent task is t04, and it diverged into more of the same. Its
first move in both runs was an edit whose `old` equalled its `new`, so the new
refusal fired and the observation changed. Run 1 ended at three iterations on
a format failure; run 2 spent all nine, produced two more `old == new` edits
with control characters for path separators, ran the checks twice more with no
paths, and repaired nothing. The fix made the failure longer, not better —
which is what a legible failure looks like when the thing failing is judgment.

This is the evidence for reading the §5.2 row as a model result rather than a
harness artefact. It is 9 tasks of one configuration, so it supports that
reading for *this* configuration and does not generalise to a stronger one.

### 5.4 Deviations from the design above, each with its reason

| Design | What was done | Why |
|---|---|---|
| 20–30 changes from poetry's git history, gold diff per task | 20 synthetic mutations on the poetry tree, each verified to move the checker | The corpus is a hash-pinned sdist carrying no `.git`, and the network could not reach GitHub. Grading is on check-recovery, which `floor_tasks.json`'s own protocol specifies, so no gold diff is required |
| 5 repetitions per task per configuration | 1 | Decoding is greedy at temperature 0, so repetitions are byte-identical — and were observed to be, across four tasks that share a file and therefore share a prompt. A configuration sampled at non-zero temperature still needs the 5 |
| Floor as a configuration | One configuration, below the floor | The second was still downloading. Reported as a lower bound, not a floor |
| Metric 3, argument correctness against a gold trace | Not measured | No gold trace, per the first row. Partly substituted by the verbatim calls in §5.3 |
| Metric 5, run-to-run variance | Not measured | Nothing varies at temperature 0 |
| Metric 6, context degradation at 25/50/75% fill | Not measured | Needs a configuration that succeeds at low fill, so degradation has something to degrade from |

### 5.5 Cost, and what the figure is a property of

Median 209 s per task, 9 turns each, on CPU. Per-turn cost grows through a run
because the probe re-prefills the whole transcript on every call — it holds no
KV cache between requests. **That growth is a property of the probe rig, not of
TRELLIS**, and the figure must not be quoted as the harness's overhead. What it
bounds honestly is this measurement: a 20-task sweep costs about 70 minutes of
wall clock per configuration on this machine.

### 5.6 Second configuration: Qwen2.5-Coder-3B-Instruct Q4_K_M

Measured 2026-09-20, same corpus, same task set, same checker, same 8-iteration
budget, tier 2 confirmed by `trellis doctor` before the run.

**17 of 20 tasks.** The host stopped the background job for system memory
pressure with three tasks outstanding — the 3B probe alone holds 3.5 GB. That
is a property of this machine, not of the run, and the figures below are
reported at the size they reached rather than extrapolated. The composition is
uniform across all 17, so the remaining three would have had to differ from
every previous one to change the reading.

```
repaired            0 / 17
no-progress        14      stopped after 3 identical calls
budget-exhausted    3      varied enough to reach the ceiling
iterations         69 total, 4.1 mean
```

**The 3B is much better at judgement than the 0.5B, and fails anyway.** It
varies its edit per task, targets the file the task names, and proposes
plausible Python:

```
t00  old: "def validate(self, data):"   new: "def validate(self, data, errors):"
t04  old: "def load(self, path):"       new: "def load(self, path, encoding='utf-8'):"
t13  old: "def build(self, project: Project, options: BuildOptions) -> None:"
```

Against the 0.5B's schema placeholders and line numbers, that is a different
class of output.

**And all 17 fail on the same thing: `src/poeetry/`.** A doubled `e` in a
repository name the model only ever had to copy from the task string. Not one
task in 17 spelled it correctly.

That looked far more like a decode defect than like judgement, so the rig was
ruled out before the result was recorded. `tokenize` then `decode` over the
probe's own trie returns `src/poetry/config/config.py` byte-identical, and the
prompt is built from the task string, which spells it correctly. The model sees
`poetry` and emits `poeetry`. The test is in the probe crate.

The brief predicts that *quantization degrades format reliability faster than
it degrades reasoning*. This is that claim with a finer edge: under Q4_K_M the
**reasoning survived and verbatim copy fidelity did not**. Constraint cannot
help, and this is worth being precise about — the grammar admits any string, so
a misspelled path is perfectly valid JSON. Tier 2 guarantees the call parses,
never that its arguments refer to anything real.

**Every one of the 17 opened with `edit`.** Never `read_file`, never
`run_checks`, never `blast_radius`. The 0.5B opened every task with a pathless
`run_checks`; the 3B edits before looking. Two distinct failure signatures,
neither of which is the behaviour the loop is built around — read, change,
verify.

### 5.7 What two configurations establish

**The floor is still not located.** Two points, both below it:

| Configuration | Repaired | Characteristic failure |
|---|---|---|
| Qwen2.5-0.5B-Instruct Q4_K_M | 0 / 20 | Schema-valid nonsense: placeholders as anchors, the path in every field |
| Qwen2.5-Coder-3B-Instruct Q4_K_M | 0 / 17 | Sound edits aimed at a path it cannot spell |

Two failing points bound the floor from below more tightly than one and still
do not say where it is. A configuration that *passes* is the only thing that
turns this into an interval, and neither of these is it.

What the pair does establish is that the two failures are **different in kind**,
which a single measurement could not have shown. The 0.5B cannot form a useful
call; the 3B forms one and cannot address it. A harness improvement that fixed
the first would do nothing for the second.

### 5.8 A limitation the run exposed in the harness

The no-progress stop takes the largest share of the credit for this sweep
finishing in 45 minutes rather than hours: 69 iterations against the 153 the
same runs would have spent at the ceiling.

It also missed three of them. `t12`, `t13` and `t16` reached the budget because
their calls were not byte-identical — they differed **only in indentation**,
four spaces against five, while the observation was the same "path does not
exist" error nine times over.

So the stronger signal is the repeated *observation*, not the repeated call: a
model that varies one field while re-issuing the same doomed action is not
making progress, and the tool result says so even when the call does not.
Deliberately not changed while the sweep was running, so this result comes from
one harness version.

### Exit criteria for v0.1

A measured recall figure — **done**, §2. A stated threshold with reasoning —
**done**, §3. An ADR on whether a semantic backstop is needed — **done**,
ADR 0012. A known model floor as a configuration — **partial**: two
configurations are measured and both are below the floor, which bounds it from
below without locating it. The two fail differently — one cannot form a useful
call, the other forms one and cannot address it — which a single measurement
could not have shown.
