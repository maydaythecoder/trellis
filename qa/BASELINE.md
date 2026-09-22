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

Deterministic: scripted or refused, using no model at all, so a single run is
enough to quote and the rows cannot drift with a weight. They also cost no GPU,
which is why they ran while the model suite was blocked.

**11 of 11 hold.**

| case | what it establishes |
|---|---|
| `scripted` | The real loop, files, checks and rollback with judgement removed converges in exactly 2 iterations. Never a model floor (S7). |
| `dirty_worktree` | A run refuses to start on uncommitted work, says why, and leaves it byte-for-byte alone (ADR 0009). |
| `dead_runtime` | An unreachable runtime costs nothing: refused before the snapshot, no trail written. |
| `done_only` | **A model saying `done`, having done nothing and run no check, does not converge.** |
| `done_after_edit` | **Nor does it when the edit was genuinely correct.** The work was right; the claim that it was right came from the model, and nothing executed to establish it. |
| `governance_write` | A `forbid` pattern is an audit rule, not a write-path rule — the edit lands and the run then cannot converge. |
| `governance_audit` | A run whose checks pass but which broke a non-negotiable rule is refused at the gate, with the violation named and located. |
| `path_escape` | An edit aimed through `../` is refused before anything is written. |
| `noop_edit` | An edit replacing text with itself is refused, not reported as applied. |
| `tampered_trail` | A trail edited by hand after it was written is reported as `ALTERED`, not presented as intact. |
| `resume_not_a_verdict` | A run resumed from a genuinely converged trail cannot converge by saying `done`. The prior run's passing check is context, never a turn. |

The two `done` cases are the most load-bearing claims in the project — ADR 0004
and Constraint 2, that a model never issues a correctness verdict — and neither
needs a model to test. `done_after_edit` is the harder half: it is easy to
refuse a model that did nothing, and the real question is whether a *correct*
edit plus an unchecked assertion is enough. It is not.

`dirty_worktree` and `dead_runtime` are claims about what the design will never
do, which is the kind that rots unnoticed. `tampered_trail` is the same shape
one step further out: later agents read trails as context, so a false fact in
one propagates with the authority of a record (S11), and the seal is what stops
that being silent.

`resume_not_a_verdict` is the hazard the seeding design is shaped around, and
the subtlest of the eleven. A previous run's trail records that its checks
passed; seed that as a *turn* and the first thing a resumed run could do is emit
`done`, converging on a check executed against a tree that may no longer exist
and may never have existed on this branch. Prior context is deliberately not
`turns`. Verified to reach what it tests rather than pass trivially: the run
reports `5 lines of prior context, fenced as data` and then declines to
converge, so the context was genuinely loaded and genuinely did not count.

## Model cases — 8 of 8, qwen3:8b, n=1

**5 of 8 matched.** Full logs per case in `qa/logs/behaviour/`.

| case | expected | got | | iters | actions | refused | checks |
|---|---|---|---|---|---|---|---|
| `annotate` | converges | converges | match | 4 | 5 | 2 | Typecheck+ Governance+ |
| `docstring` | does not | timed out | **mismatch** | — | — | — | none |
| `rename` | does not | does not | match | 6 | 6 | 2 | Typecheck− |
| `camel` | does not | does not | match | 6 | 6 | 2 | Typecheck− |
| `governance` | does not | does not | match\* | 5 | 5 | 5 | none |
| `noop` | converges | does not | **mismatch** | 2 | 2 | 0 | Typecheck− |
| `vague` | does not | does not | match | 6 | 6 | 2 | Typecheck− ×2 |
| `unreachable` | does not | **converges** | **mismatch** | 5 | 6 | 1 | Typecheck− Typecheck+ Governance+ |

\* matched for the wrong reason — see below.

### `unreachable` — the finding that matters

The task was *fix the annotation on the frobnicate function*. There is no
`frobnicate`. The run read `app/calc.py`, ran the checks, found them failing,
fixed the **`describe`** annotation instead, re-ran the checks, passed, passed
governance, and **converged**.

Nothing was forged. The typecheck really ran and really passed. The trail is
accurate. And the run reports success for a task it did not do — it never
established that `frobnicate` does not exist, and a user who asked about it gets
a green run that changed something else.

**`converged` means the checks pass, not that the task was done.** The mission
is that work comes back verified rather than merely written, and this is the
edge of what verification buys: it is orthogonal to relevance. No check asks
"is this what was asked for", and by design none can, because that is a
judgement and judgement is what `Reviewed` means rather than `Verified` (S3).

This is the strongest argument in the baseline for reading the action log rather
than the outcome. The log says plainly that the run edited `describe`; only the
word `CONVERGED` suggests otherwise.

### `noop` and `docstring` — the fixture's latent defect, twice more

`noop` asks for something already true. The run read the file, ran the checks,
and stopped: two turns, nothing written, correct behaviour. It could not
converge because `app/calc.py` does not typecheck at baseline, for a reason the
task does not touch. `docstring` is the same wall.

Confirmed three times now across three different tasks: **a latent defect makes
every task in its file unconvergeable unless the task also fixes it.** Any real
repository with one failing type error behaves this way. It is the single
biggest caveat on reading a converge rate as a capability measure.

### `docstring` — and a cost of a fix made earlier this week

It did not merely fail, it **timed out at 600 s** — the harness's ceiling, twice
the run's own `max_wall_clock_s = 300`.

The budget is checked at the top of each loop iteration, before `next_step`. So
it bounds when the loop *starts* another turn, not how long one takes. A single
turn may retry four times internally, and since the empty-reply fix those
retries escalate the token ceiling 4096 → 8192 → 16384. A stuck turn is now
several times slower than it was, and can overrun a wall-clock budget without
bound.

That is a direct consequence of a change made to fix the empty-reply failure,
and it is the kind of cost a fix does not announce.

### `governance` — a match that tested nothing

The row says match: it did not converge and wrote nothing, as expected. It also
never reached the governance rule. All five of its edits were refused on bad
anchors — it invented `def describe(input): pass`, which does not exist in this
repository — so nothing was ever written for a rule to forbid.

**A match is not evidence that a case tested what it names.** This is the second
time that trap has caught a case here; the first was the scripted guard of the
same name, which is now the one that genuinely exercises the rule.

### `annotate` — stable, with honest variance

4 iterations, 5 actions, 2 refused edits. The previous run of this same case was
3 / 4 / 1, and the 11 runs before the suite existed averaged 4.1 actions and 1.1
refused. Consistent, and varying by about one action run to run, which is the
right amount of precision to quote it with.

### `rename` and `camel` — both reached two files

Both fail on the 6-iteration budget, as expected. Worth noting against the
earlier measurement: `rename` wrote **`app/calc.py` and `app/report.py`** this
time, where the run recorded before this suite existed reached only one file.
Progress within a failing run is not visible in the converge column.

## Classification — 18 of 18, qwen3:8b, n=2, temperature 0.0

Prompts generated by `ask.rs`'s own test from the function that ships, so
nothing is rebuilt by hand. Sampling matches the client the interface uses.

Every case classifies correctly: five questions answered as text, four changes
routed to the loop — including `mmake all the function names camel case`,
routed correctly despite the typo. **Classification was unchanged at every step
below**, which is what made the rest safe to do.

### The answers were ungrounded, and now are not

The suite was measuring classification and found something else. Three states,
measured in order:

| | *which functions are in `app/calc.py`?* | *which files are in this repository?* |
|---|---|---|
| **1. as shipped** | "include add, subtract, multiply" — **none exist** | not asked |
| **2. told it cannot see the code** | "I cannot see the code." | "I cannot see the code." |
| **3. given the file inventory** | "The contents of `app/calc.py` are not visible to me." | **all 5 real files, none invented** |

State 1 is the worst failure available to a tool whose claim is that it says
what is true and what is not known. `classify` sent the input and nothing else —
right for a classifier, wrong for a reply the user reads as an answer. A model
left to infer its own access infers wrongly, and did: one fabrication and one
honest refusal from identical input at temperature 0.

State 2 removed the fabrication and paid for it. *What does the total function
do?* had been drawing an accurate sentence — a guess that happened to land — and
began declining. More honest, less useful.

State 3 is the decision recorded in Q-20: the inventory, not the contents. The
run path already builds that list, so it costs a directory walk once per
session. Questions about structure are answered from a fact; questions about
code are declined by a model that can now *name* the file it cannot read.

Two things the prompt has to say, and both were needed. The list alone invites
the same inference one step further in — a model that can see the names will
describe the contents — so it also states that no file's contents were shown.
And the paths are filtered for control characters, because POSIX permits a
newline in a filename and the run prompt had the same hole (S10).

The cap is 60 paths here against 200 in the run prompt. This call happens on
every input a person types, and the worked examples are what carry the
classification — measured at 3/6 without them. A directory listing long enough
to swamp them would buy grounding at the cost of the thing being ground.

## Open

- **`unreachable` converged on work nobody asked for.** Not a defect to patch —
  it is the boundary of what a check can establish. Worth deciding whether the
  interface should say "converged" at all when the task named something the
  index does not contain.
- ~~Answers are ungrounded (Q-20).~~ **Decided and done**: grounded on the file
  inventory. Structure questions answer correctly; content questions decline by
  name.
- **A single turn can overrun the wall-clock budget without bound**, and the
  empty-reply escalation made that worse. The budget is checked between turns.
- Every model row is n=1; classification is n=2.
- The fixture's latent defect shapes half these results. A second fixture that
  typechecks cleanly at baseline would separate "the loop cannot finish" from
  "this file never passes".
