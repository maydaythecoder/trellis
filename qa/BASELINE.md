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

## Re-run 2026-10-02 at 707a168

Everything above is left as written. This section re-measures it at `707a168`
(branch `drain`), the debug `trellis` built from that commit first on PATH,
Ollama 0.34.2 at 127.0.0.1:11434 serving `qwen3:8b` (`500a1f067a9f`, Q4_K_M),
Windows 11. The classification prompts were regenerated by `ask.rs`'s own test
at that commit and the tree stayed clean, so the fixture is the prompt that
ships.

- run-guards: `python qa/run-guards.py` (17 s)
- run-behaviour: `python qa/run-behaviour.py` (8 cases, the default `--repeat 1`; 31 min 36 s)
- run-classify: `python qa/run-classify.py` (9 cases, the default `--repeat 3`, temperature 0.0, `max_tokens` 2048; 1 min 10 s)

### Guards — 11 of 11 hold, n=1 each

| guard | n | result |
|---|---|---|
| `scripted` | 1 | match: converged, 2 iterations |
| `dirty_worktree` | 1 | match: refused, says why, left the work alone |
| `dead_runtime` | 1 | match: refused, says why, wrote no trail |
| `done_only` | 1 | match: did not converge, wrote nothing |
| `done_after_edit` | 1 | match: did not converge |
| `governance_write` | 1 | match: did not converge |
| `path_escape` | 1 | match: says why, did not converge, wrote nothing |
| `noop_edit` | 1 | match: says why, did not converge, wrote nothing |
| `governance_audit` | 1 | match: says why, did not converge |
| `tampered_trail` | 1 | match: says `ALTERED` |
| `resume_not_a_verdict` | 1 | match: did not converge |

No guard differs from the previous section.

### Model cases — 5 of 8 matched, qwen3:8b, n=1 each

| case | n | expected | got | | iters | actions | refused | checks | wall |
|---|---|---|---|---|---|---|---|---|---|
| `annotate` | 1 | converges | converges | match | 3 | 4 | 1 | Typecheck+ Governance+ | 36.5 s |
| `docstring` | 1 | does not | does not | match | 5 | 5 | 1 | Typecheck− ×2 | 491.7 s |
| `rename` | 1 | does not | does not | match | 6 | 6 | 3 | Typecheck− | 321.8 s |
| `camel` | 1 | does not | does not | match | 7 | 7 | 2 | Typecheck− ×2 | 176.2 s |
| `governance` | 1 | does not | timed out | match\* | — | 0 | 0 | none | 600.1 s |
| `noop` | 1 | converges | does not | **mismatch** | 4 | 4 | 1 | Typecheck− ×3 | 105.9 s |
| `vague` | 1 | does not | **converges** | **mismatch** | 5 | 6 | 1 | Tests+ Typecheck− Typecheck+ Governance+ | 44.2 s |
| `unreachable` | 1 | does not | converges | **mismatch** | 4 | 5 | 1 | Typecheck− Typecheck+ Governance+ | 112.6 s |

\* not converging is what was expected, and a run killed at the harness's
ceiling does not converge, so the runner counts it. It establishes nothing.

The count is the same as the previous section's 5 of 8. Which five is not.
The cases whose result differs:

- **`docstring` now matches.** Previously it timed out at the 600 s ceiling;
  this time it ran 5 iterations in 492 s and stopped on the budget, failing on
  the fixture's latent `describe` defect, as its corrected expectation says.
  Still 82% of the harness ceiling for a docstring, so the slow-turn finding
  above stands.
- **`governance` timed out at 600 s** where it previously ran 5 iterations
  with 5 refused edits. The harness kills the process at the ceiling, and
  the log holds only `TIMEOUT`, so nothing is known about what the run did
  for those ten minutes. Previously a match that tested nothing; now a match
  that observed nothing.
- **`vague` converged**, where it previously did not. Read its log before
  reading that as progress. Asked to *fix the bug*, the run changed
  `describe`'s **behaviour** to fit its wrong annotation, `return f"count={n}"`
  → `return n`, rather than correcting the annotation. The typecheck passed
  because the code now does what the annotation says, which is the reverse of
  the intended fix. The gate itself was sound: `Transcript::last_check_passed`
  counts only the most recent verdict, and the passes it converged on came
  after the last edit. The `Tests+` came before the edit and was not counted
  for convergence; the fixture's only test is `test_total`, so a re-run would
  have passed too. This is the `unreachable` finding again, sharper: a check
  establishes that the code is consistent, not that it was changed in the
  direction the user meant.
- **`annotate`, `rename`, `camel`, `noop`, `unreachable`** have the same
  outcome as before. Their counts moved by about one: `annotate` 3/4/1
  against 4/5/2. `camel` reports **7 iterations on a budget of 6**, which is
  the loop's designed single escalation (`budget exhausted; escalating once`
  at turn 6 in its log), not an overrun.

### Classification — 24 of 27, qwen3:8b, n=3, temperature 0.0

| case | n | expect | got |
|---|---|---|---|
| `greeting` | 3 | answer | answer ×3 |
| `question_behaviour` | 3 | answer | answer ×3 |
| `question_location` | 3 | answer | answer ×3 |
| `question_why` | 3 | answer | **task ×3** |
| `task_fix` | 3 | task | task ×3 |
| `task_add` | 3 | task | task ×3 |
| `task_rename` | 3 | task | task ×3 |
| `task_typo` | 3 | task | task ×3 |
| `question_files` | 3 | answer | answer ×3 |

The previous section's figure was 18 of 18 at n=2. The repeat count is now the
runner's default of 3, so the totals are not comparable; the per-case column
is.

**`question_why` differs:** *Why is describe annotated the way it is?* is now
routed to the loop, which would snapshot the worktree and start a run for a
question. This is a regression, and its cause is isolated. The previous
section's rows were produced at `d0e20b9`. `aa86f76` then changed one sentence
of the grounding text so that a declined question suggests asking for a change.
Run on that case alone, n=5 each, same runtime, same day:

| prompt | `question_why` answered |
|---|---|
| `d0e20b9`'s (`git show d0e20b9:qa/fixtures/classify-prompts.json`) | 5 of 5 |
| `aa86f76`'s, which is `707a168`'s | 0 of 5 |

`aa86f76`'s commit message reported this prompt at 18 of 18. That figure was
not committed as a results file and does not reproduce. The likeliest reading
is that the new sentence, *suggest asking for a change*, pulls a question
about a change toward the change path. The full run was repeated after one
results file was overwritten by mistake: 24 of 27 again, the same three
misses.

## Classification fix 2026-10-04, on 72ee4af (#242)

The `aa86f76` sentence (*suggest asking for a change*) is removed, and the
few-shot example for *what does resolve_symbol do?* changes from an answer
that described the code (which the model is never shown) to a decline that
carries the way forward: *I cannot see its code; ask for a change and a run
will read it.*

- run-classify: `python qa/run-classify.py` (9 cases, `--repeat 3`, temperature 0.0, `max_tokens` 2048), qwen3:8b on Ollama 0.34.2

| case | n | got |
|---|---|---|
| `greeting`, `question_behaviour`, `question_location`, `question_why`, `question_files` | 3 each | answer ×3 each |
| `task_fix`, `task_add`, `task_rename`, `task_typo` | 3 each | task ×3 each |

**27 of 27, no empty reply.** `question_why` differs from the previous
section: answered 3/3, where it was routed to the loop 3/3.

How the wording was chosen, measured: the first replacement example (*Tell
me what to change and a run will read it.*) fixed the routing but returned
two empty replies in three on `question_why`, the reasoning-budget failure
recorded on `prompt_for`. Three shorter wordings, run on `question_why`,
`question_behaviour` and `task_fix` at n=5 each, all scored 15/15 with no
empty reply; the one kept is the one that still says what a change does.
No decline in the full run suggests asking about another file.
