# The governance gate, demonstrated — 2026-09-20

The v0.3 gate item "a governance rule can fail an audit, mechanically" was
recorded as met on the strength of a unit test of `governance_audit`. Two
defects sat between that function and any run reaching it, and only running the
thing found them. Run on **poetry 2.5.0** in `.bench/wedge`, the same fixture as
`wedge-20260920.md`.

The rule:

```toml
name = "No direct DB access outside the repository layer"
rank = "non-negotiable"
forbid = { pattern = "psycopg2.connect" }
scope = ["src/poetry/config/"]
```

## A change the checks accept and the rules do not

The edit has to *pass* the type checker, or the typecheck stops the run first
and the audit is never reached — which is the whole point of the case. A comment
does it.

```
$ trellis run --model scripted:breaks_rule.json "note the database connection plan"
repo:     2 write rule(s), .trellis/context.md loaded

   1. Edit { path: "src/poetry/config/config.py", old: "class Config:", new: "# TODO: swap this for psycopg2.connect(dsn) once the pool lands\nclass Config:" }
   2. RunChecks { paths: ["src/poetry/config/config.py"], kinds: [Typecheck] } check passed
   3. RunChecks { paths: [], kinds: [Governance] } check FAILED

NOT CONVERGED after 2 iterations.
  the checks passed and a project rule was broken, so the run did not converge
  and nothing was accepted:
VIOLATION [No direct DB access outside the repository layer] src/poetry/config/config.py:151  # TODO: swap this for psycopg2.connect(dsn) once the pool lands

Rolled back to aa7c6204. Worktree restored: true.
```

**Step 3 was not in the script.** The model's three steps are edit, typecheck,
done. The audit at step 3 is the loop's, run at `Step::Done` because the
repository has rules — not because the model asked. That is the fix: before it,
`default_kinds` was `["typecheck"]` and a non-negotiable rule was enforceable
only at the discretion of the thing it constrains.

`iterations` reads 2 and the step list shows 3, which is correct: the model took
two turns and the gate is not one of them.

## And the compliant case still converges

The same shape with a comment that breaks nothing:

```
   2. RunChecks { paths: ["src/poetry/config/config.py"], kinds: [Typecheck] } check passed
   3. RunChecks { paths: [], kinds: [Governance] } check passed

CONVERGED after 2 iterations, 2 checks passed.
```

A gate that cannot pass is not a gate, and this one had two ways to fail
closed — the `blind` rule keyed on the wrong field, and the audit scoping to a
path it could not read — both of which would have failed *this* run.

## The two defects, for the record

1. `Orchestrator::run_checks` shadowed its `paths` argument with `["."]`, the
   right default for mypy. The `Governance` arm's "no paths given" branch was
   therefore dead, the audit was handed a directory, `read_to_string` failed
   into a `continue`, and the verdict passed reporting `1 rule(s) checked over
   1 file(s); none broken` having read nothing. A pathless call is what a real
   model emits — 0 of 10 in the floor runs passed a path — so the rule had no
   teeth on the only path that matters.

2. Nothing invoked the audit. `default_kinds()` is `["typecheck"]`.

Both were found by writing the gate table and refusing to cite a unit test one
layer below the claim.
