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
