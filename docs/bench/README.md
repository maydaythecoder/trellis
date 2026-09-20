# Benchmark artefacts

The evidence behind the numbers in `BENCHMARK_AND_EVAL.md`. Committed because
a measurement whose inputs are not inspectable is an assertion.

## What is here

```
poetry-2.5.0-seed20260919/
  sample.json     the 400 sampled call sites, with the auditor's judgements
  answers.json    the resolver's verdict per sampled site
  score.txt       the output of examples/audit_score.rs over the two
  chunks/         the sample as it was split for auditing, and each filled part
```

## Reproducing

The corpus is the PyPI sdist `poetry-2.5.0.tar.gz`, SHA-256
`e73b873890526c03e4d8e20d934292175f55a5823b3bdee2f154f63dae81336f`. It is not
vendored here — `.bench/` is gitignored — so fetch it:

```
pip download poetry==2.5.0 --no-deps --no-binary :all: -d .bench
tar xzf .bench/poetry-2.5.0.tar.gz -C .bench
```

Then:

```
cargo run --release -p trellis-index --example audit_sample -- \
    .bench/poetry-2.5.0 400 20260919 <out>
cargo run --release -p trellis-index --example audit_score -- <out>
```

**The seed reproduces the draw, not the judgements.** Re-running
`audit_sample` against the same corpus yields the same 400 call sites;
`true_callee` and `is_first_party` are human (here, model) judgements and are
reproduced only by reading them out of `sample.json`. That is why the filled
file is committed rather than regenerated.

## Provenance of the judgements

Recorded plainly because it bears on how much the figure is worth (A-34,
OPEN_QUESTIONS Q-15):

- The audit followed the protocol in `BENCHMARK_AND_EVAL.md` §2 — blind
  sample, resolver verdicts withheld in a separate file, written rules stated
  up front in `sample.json`'s `instructions` field.
- **It was performed by a model reading the poetry source, not by a person.**
  §2 asks for a human. A model does not share the resolver's implementation,
  so it is not the circular tool-versus-tool comparison §2 warns against — but
  it is not independent human evidence either.
- Individual entries were spot-checked by hand against the poetry source and
  were correct.
- **The figure must be re-derived with a human auditor before the v1.0 gate is
  claimed.**

## Licence

`sample.json` embeds short excerpts of poetry's source for context. poetry is
MIT licensed, copyright Sébastien Eustace and contributors. The excerpts are
here as measurement evidence.

## The pre-gate audit (A-34, Q-15)

No v1.0 gate claim may rest on the two audits already scored. Both were drawn
at 300–400 sites, and `BENCHMARK_AND_EVAL.md` §2 records why that is too few:
the recall denominator is not the sample size but the subset whose true callee
is first-party, which has run at roughly a third. 400 draws put ~150 sites in
the denominator and a 95% interval of about ±8 points. The gate thresholds are
25 points apart, so ±8 places a measurement only when it does not land near a
boundary — and 85% is exactly where the next one will land.

**Size the sample on the expected first-party count.** Roughly 1,000 draws for
400 in the denominator, which brings the interval to about ±5.

The packet is prepared with two commands and needs no network:

```
cargo run --release -p trellis-index --example audit_sample -- \
    .bench/poetry-2.5.0 1000 <fresh-seed> .bench/pregate
python docs/bench/split_audit.py .bench/pregate 40
```

That writes 25 blind chunks of 40. `answers.json` sits beside them and is
opened only by the scorer — the auditor never sees a resolver verdict, which
is the entire reason this measurement is worth more than the coverage report
the resolver computes about itself.

Filled chunks go back as `chunk-N-filled.json` carrying `results` (not
`sample`), then:

```
python docs/bench/merge_audit.py .bench/pregate 1000
cargo run --release -p trellis-index --example audit_score -- .bench/pregate
```

Neither the draw nor the chunking is committed: both are reproducible from the
seed, so only the filled judgements are worth storing. `merge_audit.py` refuses
a partial or malformed audit rather than scoring one, and `split_audit.py`
refuses to re-split over filled chunks.

**Two things this audit must report that earlier ones did not need to.**

- **Wrongly excluded.** ADR 0015 lets a site leave the denominator on the
  strength of an annotation. A wrong exclusion is invisible to
  `check_soundness` and to the repository-wide report — both are structurally
  unable to see a site that emits no edge — so this figure is the only
  detector. It has been 0 on both re-scored audits; that is evidence, not
  proof, and S21 says it must be measured again rather than inherited.
- **Whether the resolver changed since the draw.** The seed reproduces the
  sites, not the verdicts. If `resolve.rs` has moved, regenerate
  `answers.json` with the same seed and size before scoring.

A draw at seed 20260921 was taken as a dry run of this procedure and discarded.
Its composition, as a sizing preview: of 1,000 sites the resolver judged 284
`Resolved`, 34 `Candidates`, 296 `Unresolved` and 386 external. The auditor
decides the denominator, so that is a hint about the shape of the work and not
a prediction of the result.
