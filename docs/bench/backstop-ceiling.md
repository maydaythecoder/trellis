# Backstop recall ceilings on the audited misses (2026-09-28)

The samples read here were audited by a model, not a person (A-34), so every
figure below inherits that limit. A **ceiling** is the recall a sample would
show if a backstop bound *every* miss of the shapes it can reach, and bound
each one correctly. It is an upper bound on what that backstop could do, not a
recall figure, and it must not be compared to the thresholds in
`BENCHMARK_AND_EVAL.md` §3 as one. No backstop is built or changed here; this
chooses which one is worth measuring properly (#2).

## Method

A **miss** is a site the auditor judged first-party (`is_first_party: true`)
that `audit_score` does not count in the recall numerator: anything but a
`Resolved` verdict naming the true callee under `audit_score.rs`'s `matches`
and `constructor_convention`, which `backstop_ceiling.py` ports literally. Each
miss is classified with `receiver_shapes.classify` (imported, not copied)
against the innermost `def` enclosing the call, with the first-party class set
built as `receiver_shapes.py` builds it. Two populations are outside that
classifier's domain and are labelled rather than forced into a shape: `Name`
sites (a bare `f()`, taken from the sample's own `shape` field), and
attribute calls outside any function body (none occurred in either sample).
The call node is located by start line and exact source text; anything other
than exactly one match aborts the run.

The resolver verdicts are **regenerated**, as the `precision-gate` CI job does,
and joined to the committed human judgements. The committed
`poetry-2.5.0-seed20260919/answers.json` and its `score.txt` (47.8%) are stale
and were not used. The script refuses to run if the regenerated sample's site
ids differ from the committed sample's, or the seeds differ.

The baseline each run prints was checked against `audit_score` over the same
committed `sample.json` and regenerated `answers.json`; both printed 108/159
(seed 20260919) and 72/100 (seed 20260920).

Commit measured: `2c9172e6bdf45f260b0bbdd7e73130bfd24ac9b1` (the tree the
commands ran on; this file and the script were added after it, and no file
under `crates/` differs). Corpus: `.bench/poetry-2.5.0`, the PyPI sdist at
SHA-256 `e73b8738…81336f` (see `README.md`). Python 3.12.10.

Commands, run from the repository root:

```
cargo run --release -p trellis-index --example audit_sample -- .bench/poetry-2.5.0 400 20260919 .bench/regen-20260919
cargo run --release -p trellis-index --example audit_sample -- .bench/poetry-2.5.0 300 20260920 .bench/regen-20260920
py -3 docs/bench/backstop_ceiling.py .bench/poetry-2.5.0 docs/bench/poetry-2.5.0-seed20260919 .bench/regen-20260919
py -3 docs/bench/backstop_ceiling.py .bench/poetry-2.5.0 docs/bench/poetry-2.5.0-seed20260920 .bench/regen-20260920
```

The seeds are reported separately and never pooled: they are different draws
with different denominators.

## Seed 20260919 (400 drawn, 159 first-party)

Baseline recall 108/159 = 67.9%. Misses: 51.

| receiver shape of the miss | misses | ceiling | verdict kinds |
|---|---:|---|---|
| parameter, annotated first-party | 20 | 128/159 = 80.5% | Candidates 18, Unresolved 2 |
| bare name call f() (not an Attribute site) | 11 | 119/159 = 74.8% | Unresolved 11 |
| attribute chain (a.b.m()) | 8 | 116/159 = 73.0% | Unresolved 8 |
| local, type not derivable in-file | 3 | 111/159 = 69.8% | Unresolved 3 |
| call result (f().m()) | 3 | 111/159 = 69.8% | Unresolved 3 |
| self.attr | 2 | 110/159 = 69.2% | Unresolved 2 |
| free name (module-level, import, or builtin) | 2 | 110/159 = 69.2% | Candidates 1, Unresolved 1 |
| self / cls | 1 | 109/159 = 68.6% | Candidates 1 |
| subscript (d[k].m()) | 1 | 109/159 = 68.6% | Unresolved 1 |

## Seed 20260920 (300 drawn, 100 first-party)

Baseline recall 72/100 = 72.0%. Misses: 28.

| receiver shape of the miss | misses | ceiling | verdict kinds |
|---|---:|---|---|
| parameter, annotated first-party | 9 | 81/100 = 81.0% | Candidates 9 |
| bare name call f() (not an Attribute site) | 7 | 79/100 = 79.0% | Unresolved 7 |
| attribute chain (a.b.m()) | 4 | 76/100 = 76.0% | Unresolved 4 |
| local from constructor call, first-party | 3 | 75/100 = 75.0% | Candidates 1, Unresolved 2 |
| call result (f().m()) | 3 | 75/100 = 75.0% | Unresolved 3 |
| local, type not derivable in-file | 2 | 74/100 = 74.0% | Unresolved 2 |

## Candidate backstops, mapped to the shapes they could reach

- **(a) Cross-file propagation of local and attribute types**, reaching
  "attribute chain (a.b.m())" and "local, type not derivable in-file":
  seed 20260919, 11 misses, ceiling 119/159 = 74.8%; seed 20260920, 6 misses,
  ceiling 78/100 = 78.0%. The mapping is the ticket's; it leaves out
  "self.attr" (2 misses on seed 20260919, none on 20260920), which a
  propagation of attribute types might also reach. Adding it would give
  121/159 = 76.1% on seed 20260919. Neither ceiling reaches 85%.
- **(b) Pyright over LSP, invoked only on `Unresolved`/`Candidates` sites**
  (§4's chosen fallback): every miss in both samples carries one of those two
  verdicts (51 of 51, 28 of 28; no miss was `ExternalExcluded` or a wrong
  `Resolved`), so the trigger reaches every miss and the ceiling is every
  miss: 159/159 and 100/100. That is a statement about where the trigger
  fires, not a measurement of Pyright, which was not run.

## What this shows, and what it does not

The largest bucket on both seeds is "parameter, annotated first-party", and
27 of its 29 misses across the two seeds are `Candidates`: the receiver's
declared type is known and the resolver emitted a candidate set rather than
one target, which is what ADR 0012's upper-bound rule does when a
first-party subclass overrides the method (not verified per site here).
Candidate (a) does not reach those sites, since their type is already known;
binding them would need evidence that narrows a declared type, which is a
soundness question rather than a propagation one. The second-largest bucket, bare `f()` calls, is outside
the receiver classifier entirely.

Not checked: whether the misses each shape would reach are bound *correctly*
by any real implementation; the classification of each individual miss by
hand; and any population beyond these two samples. Each ceiling is computed
on a sample of 159 or 100 first-party sites, so a single site moves it by
0.6 or 1.0 percentage points.
