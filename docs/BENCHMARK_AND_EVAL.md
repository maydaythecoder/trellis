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

- **Sample size: 400 call sites**, drawn uniformly at random from poetry's
  10,421, with a fixed seed recorded in the run artefact.
- At 400 with a true recall near 0.85, the 95% Wald interval is roughly ±3.5
  percentage points. That is tight enough to place a measurement against the
  thresholds in §3, which are separated by 25 points.
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
