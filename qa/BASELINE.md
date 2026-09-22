# Behaviour baseline

What TRELLIS does, measured against expectations written before the runs.

Reproduce with:

```
python qa/run-guards.py       # deterministic; no model
python qa/run-behaviour.py    # eight task cases against the configured model
python qa/run-classify.py     # answer-or-task, against the shipping prompt
```

## Method, and what it deliberately does not do

**Expectations are written first and never edited to match a result.** A case
that disagrees with its expectation is as likely to be a wrong expectation as a
defect, and both are findings. Quietly correcting the fixture until everything
matches produces a suite that agrees with whatever the code does, which is the
one thing a baseline must not be. Where an expectation *was* wrong, this
document keeps the original and says why it changed.

**Four facts per run, never collapsed into a score:** whether the run converged,
which files it left changed, which checks ran and whether each passed, and what
it did in order from its own trail.

**`converged` is not a grade.** A run can converge on a change a person would
reject, and a run that correctly refuses an impossible task does not converge.
The table records which happened. Reading it as a pass rate would be the same
error as dividing reach by doubt (ADR 0014).

**One harness at a time.** Both runners reset the fixture between cases, so both
take a lock — after a first attempt in which two suites reset the repository
under each other and produced a mismatch that looked exactly like a real defect.
The lock lives outside the fixture, because the first version of it was an
untracked file in `.trellis/` that stopped every run from starting while the
refusal cases went on "passing" for the wrong reason.

**Every model row is n=1 unless it says otherwise.** One run is a
demonstration. Where a figure matters it is repeated, and the repeat count is
stated next to it.

## Fixture

`trellis-demo` at commit `31daaaf`, reset before and after every case.

- One real defect: `describe` is annotated `-> int` and returns a string.
- Two governance rules: `no-print` (non-negotiable, scoped to `app/`) and
  `prefer-pathlib` (nice-to-have).
- Budget: 6 iterations, 300 s.
- `app/cli.py` already violates `no-print`. The governance audit is scoped to
  what a run touches, so this only bites a run that edits that file.
- **`app/calc.py` does not typecheck at baseline.** The deliberate defect is in
  it, so mypy fails on that file until a run fixes it. This turns out to matter
  more than it looks — see `docstring` below.

## Guards — behaviour that must not depend on a model

Deterministic, so a single run is enough to quote. **3 of 3 hold.**

| case | holds | what it establishes |
|---|---|---|
| `scripted` | **yes** | The real loop, files, checks and rollback with judgement removed converges in exactly 2 iterations. Never a model floor (S7). |
| `dirty_worktree` | **yes** | A run refuses to start on uncommitted work, says why, and leaves that work byte-for-byte alone (ADR 0009). |
| `dead_runtime` | **yes** | An unreachable runtime costs nothing: refused before the snapshot, and no trail written. |

The second and third are the claims the design makes about what it will never
do. Rollback is `git reset --hard` to a commit taken before the run, which
cannot distinguish the run's edits from work already there — so refusing is the
only thing that makes "nothing was accepted" mean exactly that.

## Model cases — 4 of 8, qwen3:8b, n=1

The suite was killed by the harness for system memory pressure partway through
(0.5 GB free of 15.2 GB; ollama holds 5.1 GB for the loaded weight). Four cases
completed. `governance`, `noop`, `vague` and `unreachable` have not run.

| case | expected | got | | iters | actions | refused | checks |
|---|---|---|---|---|---|---|---|
| `annotate` | converges | converges | **match** | 3 | 4 | 1 | Typecheck+ Governance+ |
| `docstring` | converges | does not | **mismatch** | 4 | 4 | 1 | Typecheck− Typecheck− |
| `rename` | does not | does not | **match** | 7 | 7 | 2 | Typecheck− ×3 |
| `camel` | writes `calc.py` | wrote nothing | **mismatch** | 0 | 0 | 0 | none |

### `annotate` — matches, and is stable

3 iterations, 4 actions, 1 refused edit. Consistent with the 11 runs measured
before this suite existed (mean 4.1 actions, 1.1 refused), which is the first
independent confirmation of that figure.

### `docstring` — the expectation was wrong, and usefully so

Expected to converge; it cannot. The task is to add a docstring to `total`, but
`run_checks` on `app/calc.py` fails on the *unrelated* `describe` defect that
lives in the same file. The loop did the work and then could not reach a passing
check, because nothing it was asked to do would fix what the checker objects to.

**This is a property of the fixture, not a defect in TRELLIS**, and it
generalises: a latent defect makes every task in its file unconvergeable unless
the task also fixes it. A real repository with one failing type error would
behave the same way for every task touching that file — which is worth knowing
before anyone reads a converge rate as a capability measure.

The expectation is now `converges: false` with that reasoning recorded in the
fixture. The original is kept here rather than erased.

### `rename` — matches

7 iterations against a 6-iteration budget, 2 refused edits, three failed
typechecks as it worked through the callers. Fails on the budget, as expected
and as previously measured.

### `camel` — mismatch, and not yet explained

Four minutes, **zero iterations, zero actions, nothing written**. The run never
completed a single turn. That is the shape of the empty-reply failure — the
model spending its whole budget reasoning and returning nothing — which the
ceiling escalation was supposed to have fixed, so either it did not engage or
this is something else.

It is **not explained**, and the harness could not explain it: the run's output
went to a pipe and the trail recorded a run that never took a turn. The runner
now writes each run's full output to `qa/logs/behaviour/`. A row that cannot be
explained is not yet a measurement.

## Classification

Not yet run. `qa/run-classify.py` measures it against prompts written by
`ask.rs`'s own test, so nothing is rebuilt by hand.

## Open

- Four cases unrun: `governance`, `noop`, `vague`, `unreachable`.
- `camel` unexplained, pending a run with per-case logs.
- Every model row is n=1.
