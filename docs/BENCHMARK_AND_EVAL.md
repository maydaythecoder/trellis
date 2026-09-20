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

Measured 2026-09-19 on the pinned corpus. Read the caveat before the numbers.

| | sites | bound | share |
|---|---|---|---|
| **all first-party** | 15,707 | 4,443 | **28.3%** |
| `Name` shape | 4,072 | 3,510 | **86.2%** |
| `Attribute` shape | 11,613 | 933 | **8.0%** |
| `Dynamic` shape | 22 | 0 | 0% |

Unresolved reasons, over the 11,233 unresolved sites: `MethodDispatchUntyped`
10,646 (67.8% of first-party sites), `LocalBinding` 558 (3.6%),
`DynamicAttribute` 22, `DecoratorIndirection` 3, `StarImport` 2,
`UnboundName` 2. Ambiguous: 31.

**This figure is a lower bound on recall and must not be compared to the §3
thresholds.** Recall is defined over call sites whose *true callee* is
first-party. The resolver cannot tell whether `logger.info()` or
`session.query()` ends up in the standard library, so it counts every
unknown-receiver call as a miss — including the many whose true callee §2
excludes from the denominator outright. The 28.3% therefore understates recall
by however large that share turns out to be, and only the audit can say. This
is recorded as A-30.

Two observations that do not depend on the audit:

1. **The loss is almost entirely in `Attribute`.** `Name` binds at 86.2%;
   `Attribute` at 8.0% while carrying 74% of all first-party sites. A pooled
   figure would have hidden this completely, which is why §2 requires the
   split.
2. **The brief's prediction about decorators did not hold.** It expected
   `DecoratorIndirection` to dominate the misses. It is 3 sites out of 11,233.
   The dominant reason is `MethodDispatchUntyped` — unknown receiver types —
   by three orders of magnitude. If a semantic backstop is adopted under S1,
   this says it must be a *type inference* for receivers, not the decorator
   unwrapping the brief anticipated.

Counted directly in the corpus for scale: only 1,052 of the 11,613 first-party
`Attribute` sites are direct `self.x(...)` calls. Following base classes to
resolve inherited `self` dispatch — the obvious next resolver feature — can
therefore address at most about 9% of the `Attribute` gap, and is not the
lever it appears to be.

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
`Candidates`, which by construction is a minority of sites.

**Cheaper intermediate, to try first.** If the `Reason` distribution shows
`DecoratorIndirection` dominating — the prediction, supported by the 20–41%
decorated-definition density across all three candidate repos — then a decorator
unwrapping pass is a far smaller change than adopting Pyright: follow the
decorator to its definition, and where it returns a function defined in the
same first-party module, bind through it. Measure the `Reason` distribution
before committing to Pyright, and record the result in an ADR.

---

## 5. Model floor spike (S7)

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

### Exit criteria for v0.1

A measured recall figure; a stated threshold with reasoning (§3, already
recorded); a known model floor as a configuration; and an ADR on whether a
semantic backstop is needed.
