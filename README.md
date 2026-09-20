# TRELLIS

An agent harness for Python repositories. It maps what a change can reach,
runs your repository's own type checker and test suite against it, rolls back
anything that does not converge, and commits a record of what happened.

The one-line version: **work comes back verified rather than merely written.**

```
trellis init                     # scaffold .trellis/, report what is missing
trellis doctor                   # model, tier, checks, index, worktree, budget
trellis run --model <spec> "fix the boolean_validator return annotation"
trellis                          # a session, holding the index between tasks
trellis tui                      # the same, full-screen
trellis why <symbol>             # what past runs did to it, from the trail
```

**Status: v0.1 complete, v0.2 at five met with one descoped by ADR, v0.3 at eight of nine.** The
resolver, the agent loop, rollback, governance, the trail and resume all work
and are tested. What is not done is listed in `docs/STATUS.md`, item by item,
against the code rather than against the plan.

---

## Coverage is partial, and here is why

**This is the thing to understand before anything else.** If you understand
it, TRELLIS's unresolved edges are honest reporting. If you do not, they look
like a broken tool.

Python cannot be statically resolved completely. `obj.method()` where `obj`'s
type is unknown, `getattr(x, name)()`, a decorator that returns something
other than what it wrapped, `from x import *` — none of these can be followed
without running the program. This is not a gap in TRELLIS; it is a property of
the language. The best published figure for a Python call-graph builder is
**PyCG at roughly 99.2% precision and 69.9% recall**, and the recall number is
the one that matters here.

So TRELLIS makes a specific trade, and states it everywhere:

- **High precision.** If it says a call site reaches your symbol, it does. On
  the benchmark corpus, precision measured **100%** on two hand-audited
  samples, and all 5,908 `Resolved` edges pass two mechanical soundness checks
  over the *full* corpus.
- **Explicitly incomplete recall.** Hand-audited at **72.0%** (95% CI
  63.2–80.8%). It does not reach the 85% the v1.0 gate requires, and the gate
  has not been called.
- **It never says "complete."** There is no API that returns reach without
  also returning what was missed, no `total()` on the coverage type, and no
  percentage combining the two — because the reach and the doubt are counted
  over different populations, and dividing them once printed *"3.7% resolved"*
  for a resolver audited at 72%.

What you will actually see, at the moment you accept a change:

```
coverage: 22 call sites in the 1 file bound to nothing
          (MethodDispatchUntyped 20, DynamicAttribute 1, and 1 more)
          Any of them may call what you changed. The checks ran on what the
          graph could see.
```

Twenty-two calls in that file could not be followed. They are not defects, and
they are not evidence of a problem in your code. They are the part of Python
that static analysis does not reach, counted rather than hidden.

One number you will see and should not misread: `trellis index` reports what
the resolver **bound**, currently 43.7% on the benchmark repo. That is a
*lower bound on recall, not recall* — it counts a call as missed even when its
true callee is in the standard library. Only the hand-audited sample produces
a figure comparable to the gate. Every surface that prints it says so.

---

## `Verified` and `Reviewed` are not the same claim

**"The callers still work" means two different things**, and collapsing them
is how a tool earns distrust the first time something breaks after a clean
run.

- **Verified** — an executed test covered this call site and passed. A process
  ran, exited zero, and touched this code.
- **Reviewed** — a model read it and did not object. No test covers it. This
  is a *weak* signal, and it is not evidence of correctness.

They are never added together. The type carrying them has no `total()` and no
combined percentage, deliberately, so that no future change can quietly merge
them into one reassuring number.

### What TRELLIS reports today, exactly

**`Verified` is currently always 0, and that means "not measured" — not
"nothing is verified."**

Mapping which tests cover which call sites is not built. Your test suite does
run, and a failing suite still blocks convergence — the grounding contract is
intact and a run cannot converge without an executed check that passed. What
is missing is the attribution: TRELLIS cannot yet tell you *which* of the call
sites in the blast radius your tests actually exercised.

So a run that converges means: the checks you configured ran and passed. It
does not yet mean anything more specific than that, and the trail records
`0 verified, 0 reviewed` rather than inventing a figure. When the mapping is
built the numbers will be populated from coverage data; until then the zeros
are an honest absence and are labelled as one.

---

## How a run works

1. **A clean worktree is required.** Rollback is `git reset --hard` to a commit
   recorded before the run, which cannot tell this run's edits from work that
   was already there. The requirement is what makes "nothing was accepted" mean
   exactly that — your own changes are never at risk from a run.
2. The model gets a flat tool list: blast radius, read file, edit, run checks.
   One loop, one model. Not a committee.
3. **The model never issues a verdict.** It can say it is finished; the loop
   asks the checks. `Verdict` is constructible only inside `trellis-checks`,
   only from a process that actually ran, so "the model claims success" and
   "the run converged" are connected by an executed check or not at all.
4. Anything that does not converge is rolled back to the exact pre-run state,
   proven at seven injection points including SIGINT and a dead orchestrator.
5. A trail is committed either way — including for failed runs, which are the
   ones worth a record.
6. **Nothing is committed on your behalf.** A converged run leaves its edits
   in the working tree. Acceptance is yours.

## Governance

Rules in `.trellis/governance/*.toml` are enforced, not suggested:

- `deny-write` refuses an edit at the orchestrator's write path, before
  anything reaches disk.
- `forbid` fails an audit, producing a real verdict — so a broken
  non-negotiable rule stops a run from converging and rolls it back.

Matching is substring only, deliberately. A governance check that exercised
judgment would be a model deciding whether it had followed the rules.

## Documents

| | |
|---|---|
| `docs/BRIEF.md` | the specification |
| `docs/STATUS.md` | every claim, audited against the code |
| `docs/ROADMAP.md` | the v1.0 gate, verbatim, and the non-goals |
| `docs/BENCHMARK_AND_EVAL.md` | how every figure here was measured |
| `docs/RISK_REGISTER.md` | soft spots, with a costed patch each |
| `docs/adr/` | decisions, each with a rejected alternative |
| `docs/bench/` | measurement artefacts and manual walkthroughs |

## On numbers in this repository

No measurement appears in any document without its method beside it. Where
something cannot be measured yet, the documents say "pending" and name the
milestone rather than estimating. Two figures that look like measurements and
are not: the v1.0 model-floor interval is **not located** — two configurations
have been measured and both failed every task — and `trellis index`'s bound is
not recall.

## Supported platforms

Windows and Linux on x86-64, both tested with the full suite. macOS is built
in CI and has never been run by a human on this project; nothing is claimed
about it. See `dist/QUICKSTART.md`.

## Licence

Apache-2.0. See `LICENSE`.
