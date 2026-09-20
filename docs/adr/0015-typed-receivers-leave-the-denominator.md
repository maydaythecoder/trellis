# ADR 0015 — A receiver with a demonstrably non-first-party type leaves the denominator

**Status:** Accepted
**Date:** 2026-09-20
**Relates to:** ADR 0002 (edge confidence), ADR 0012 (receiver typing),
`BENCHMARK_AND_EVAL.md` §2

## Context

`BENCHMARK_AND_EVAL.md` §2 defines recall over **call sites whose true callee
is first-party**. Third-party calls are excluded from the denominator outright,
because counting them would understate recall by roughly the third-party share
of all calls, which is large.

`typed_receiver` did not implement that definition for typed receivers. It
ended:

```rust
let ty = self.stated_type(file, scope, receiver)?;
let class = self.find_class(file, scope, &ty)?;   // <- here
```

When the stated type was not a first-party class, the `?` returned `None`, the
caller fell through, and the site was recorded `MethodDispatchUntyped` — an
unresolved **first-party** miss.

So `def go(val: str): val.lower()` was counted against recall. The source
states the receiver is a `str`; the callee is a standard-library method; the
resolver had every fact it needed to exclude the site and did not.

This was found by classifying every attribute-shaped call site in poetry by the
syntactic shape of its receiver (`docs/bench/receiver_shapes.py`), after the two
rules the roadmap had predicted were measured and found far too small — pytest
fixtures already handled since ADR 0012, and return-type propagation worth 67
sites. The measurement that was supposed to size a backstop found a defect in
the measurement instead.

It is the same error `poetry.core` produced: 379 imports counted as first-party
misses because the resolver treated a separate distribution as first-party on
the strength of its root package. Correcting that moved `Name` recall from
71.7% to 86.2%.

## Decision

A call site leaves the recall denominator when its receiver's stated type can
be **shown** not to be first-party. Two ways count, and both are positive
evidence:

- the type names a Python builtin (`str`, `list`, `dict`, …) and no
  first-party definition in scope shadows that name;
- every binding for the name resolves outside the repository root — the same
  `Bound::External` judgement the import path already uses.

For a qualified type (`pathlib.Path`) the module alias is judged, since that is
what determines where the name comes from.

**`Bound::Unfollowed` is not `Bound::External`,** and the whole asymmetry rests
on that. A name imported from a module that is in the tree, which that module
does not define, follows to `Unfollowed` and stays a miss.
`a_name_we_could_not_follow_inside_a_first_party_module_stays_a_miss` holds it.

One case looks like a hole and is not. `from app.absent import Thing`, where no
such module exists in the tree, is classified external — because it is exactly
the shape of `from poetry.core...`: a first-party root package with a module
under it belonging to a separate distribution. Treating those as first-party
put 379 imports in the denominator as misses and understated `Name` recall by
14.5 points, which is why `external_or_unresolvable` requires the module to be
a real local package. This decision inherits that judgement rather than
reversing it in one path and leaving the import path disagreeing about the same
module. The cost — a genuinely first-party module absent from the tree is
excluded — is recorded in
`a_module_under_a_first_party_root_that_is_not_in_the_tree_is_external`.

**Everything else stays a miss.** A type the resolver merely fails to resolve
is ignorance, not evidence, and ignorance belongs in the denominator. This
asymmetry is the entire safety property of the decision: if failure to resolve
became an exclusion, every resolver gap would remove itself from its own
denominator and recall could only ever go up. A number that cannot fall is not
a measurement. `a_type_we_merely_failed_to_resolve_stays_a_miss` is the test.

The builtin list is deliberately incomplete and says so in the source. A
builtin missing from it leaves its sites in the denominator as misses, which is
the safe direction; a *wrong* entry would exclude first-party calls.

## Consequences

On poetry 2.5.0:

| | before | after |
|---|---|---|
| Denominator (first-party sites) | 15,707 | 13,510 |
| Excluded as external | 6,087 | 8,284 |
| **Resolved** | **5,908** | **5,908** |
| Repository-wide bound | 37.6% | 43.7% |

`resolved` is unchanged, and that is the point. **Not one additional call site
became resolvable.** This is a correction to a figure that was wrong in the
pessimistic direction, and presenting it as a recall improvement would be a
straightforward misrepresentation. Both numbers are stated wherever it appears.

**Hand-audited recall is unchanged**, at 67.9% (seed 20260919) and 72.0% (seed
20260920), because the audited denominator is decided by a person reading the
source, not by the resolver. That is exactly why the gate in §3 is called on
the audit and never on the repository-wide figure.

## How a wrong exclusion would be caught

This is the part that needed care, because the usual check cannot see it. An
excluded site emits **no edge**, so `check_soundness` — which walks `Resolved`
edges — is structurally blind to it, and so is the repository-wide coverage
report, which cannot count what it has excluded from itself.

Only the hand audit can, and it already does: `audit_score` reports **wrongly
excluded** — sites the resolver dropped as external that the auditor found to
be first-party. Re-scoring both completed audits against the new resolver:

```
seed 20260920   recall 72.0% -> 72.0%   precision 100% -> 100%
                unresolved-with-a-reason 124 -> 97
                correctly excluded        94 -> 121
                WRONGLY excluded           0 ->   0

seed 20260919   recall 67.9%   precision 100%   wrongly excluded 0
```

Twenty-seven sites moved from miss to exclusion, and a human had independently
judged every one of them third-party. The samples are the same draw — the
audit sampler is uniform over site indices with a fixed seed and does not
consult the resolver — so the comparison is like for like.

## Residual risk

The rule trusts an annotation in order to *exclude*, so a wrong annotation
removes a real first-party call from the denominator and inflates recall.

ADR 0012 already trusts annotations to *add* edges, and the consequence there
is worse — an unsound edge rather than an omission — so this is not a new class
of trust. But the failure is quieter: an unsound edge shows up in
`check_soundness` and in precision, while a wrong exclusion shows up in
nothing the resolver computes.

The mitigation is that the audit measures it directly and is required before
any v1.0 gate claim (A-34). Recorded as **S21** in `RISK_REGISTER.md`, with the
residual risk stated rather than closed: two samples at zero is evidence, not
proof, and the pre-gate audit must report the figure again.

## Rejected alternatives

**Exclude whenever `find_class` fails.** One line, and it is the unfalsifiable
version: every gap in import resolution, every re-export chain too deep to
follow, every shadowed binding would silently leave the denominator. Recall
would rise monotonically with the resolver's ignorance.

**Leave it and treat the shortfall as a recall problem.** This is what the
roadmap assumed, and it sent two iterations of work at rules worth 0.4
percentage points while a 2,200-site denominator error sat underneath. A
measurement that is wrong in the pessimistic direction is still wrong, and it
misdirects the work that follows it.

**Use a type checker's inferred types instead of annotations.** mypy already
knows the receiver's type exactly, and would resolve far more than annotations
do.

It is also the thing issuing the verdicts. Constraint 2 says a correctness
claim comes only from a process that executes, and `trellis-checks` runs mypy
to produce exactly that claim (ADR 0004). A graph built from mypy's inferences
would put the blast radius *downstream of the verifier* — the tool deciding
what a change can affect would be reading the output of the tool deciding
whether the change is correct. That is a circularity, not merely a
dependency, and it would make the two agree by construction in precisely the
cases where an independent check is worth having.

The reproducibility objection stands alongside it: a graph whose shape depends
on a separate tool's configuration is not derivable from the repository alone.
ADR 0001 keeps the resolver self-contained for both reasons.
