# ADR 0014 — A radius reports two facts, never a ratio

**Status:** Accepted
**Date:** 2026-09-20
**Amends:** ADR 0002 (rule 3: every blast radius ships with its coverage)

## Context

ADR 0002 requires that no API return reach without limits, and `RadiusReport`
enforced it by owning a `Coverage` by value. That was the right requirement
implemented with the wrong type.

`Coverage` is the index's type. Its three counters — `resolved`, `candidates`,
`unresolved` — partition one population: every first-party call site in the
repository. `resolved_fraction` divides the first by their sum, and over that
population the quotient is the resolver's reach, which is what
`BENCHMARK_AND_EVAL.md` §2 defines and what the hand audit measures.

A radius fills the same three counters from two different populations.
`resolved` and `candidates` count call sites that reach *the queried symbol*.
`unresolved` is filled afterwards by `add_touched_file_misses`, which counts
every call site in the files the radius touches that bound to nothing at all —
an over-approximation of the doubt, and deliberately so, because under-reporting
it is how a tool produces a blast radius missing the thing that breaks.

Dividing the first by the sum therefore computes nothing. For
`poetry.factory.Factory` it printed:

```
92 of 2474 first-party call sites resolved (3.7%); 0 ambiguous, 2382 unresolved
```

Every number in that line is correct and the sentence it forms is false. It
reads as the resolver's reach. The resolver's reach, hand-audited on a fresh
sample, is 72.0%. The line understated the product's central capability by a
factor of twenty, in the one place the product promises to be honest about it.

Two things made it worse than a display bug:

1. **The same string was handed to the model.** `agent.rs` rendered
   `coverage.summary()` into the observation for every `blast_radius` call. A
   model told the graph resolves 3.7% of calls has every reason to stop
   consulting it — the exact opposite of what S2's honesty mechanism is for.
2. **The same shape was written into the trail.** `RadiusCounts` carried
   `resolved / candidates / unresolved`, and trails are read as context by
   later runs. A figure that reads as "this repository is 3.7% analysable"
   gains authority with every run that quotes it, which is S11's propagation
   problem applied to a number instead of an opinion.

This was found while working S6, the degradation UX, whose premise was that
2,382 unresolved items is an honest report a user cannot act on. The premise
was incomplete. The list was unusable; the headline above it was wrong.

## Decision

`RadiusReport` carries a `RadiusCoverage`, a distinct type from `Coverage`.

```rust
pub struct RadiusCoverage {
    pub reached_resolved: u32,
    pub reached_ambiguous: u32,
    pub unbound_in_touched_files: u32,
    pub touched_files: u32,
    pub by_reason: BTreeMap<String, u32>,
    pub by_file:   BTreeMap<String, u32>,
}
```

Three properties, each load-bearing:

- **No `total()` and no fraction.** Not "a fraction that callers should avoid"
  — none exists to call. Every caller that wanted one was the bug, in the same
  way `ModeCounts` has no `total()` (ADR 0003).
- **Fields are named for the populations they come from.** `reached_*` against
  `unbound_in_touched_files` does not read as two parts of one whole the way
  `resolved` against `unresolved` does.
- **The doubt arrives with its scale.** `touched_files` is recorded whether or
  not anything is unbound. "2,382" is not a fact a reader can use; "2,382
  across 22 files" is.

`Coverage` is unchanged and keeps `resolved_fraction`. It is correct for the
index, which is the only place all three counters describe one population.

The rendering follows the type. `trellis radius` prints a **reach** block and a
**doubt** block under separate headings, and states in prose that the two must
not be divided. The doubt block collapses to the top three reasons and top five
files with the residue counted — `(other) 2 across 2 further reasons` — so the
whole figure stays in view on one screen. Each reason is printed with a phrase
rather than only its variant name: `MethodDispatchUntyped` is a label, and
"obj.method() where obj's type is unknown" is the thing that tells a reader
whether the doubt is the language's fault or theirs.

The trail's `RadiusCounts` takes the same field names. Its unbound figure is
the **widest single query**, not a sum: radii in one repository touch
overlapping files, so summing counts the same call sites once per query and
grows with the agent's curiosity rather than with the doubt. `radius_queries`
records how many queries the entry summarises. The old `unresolved` field is
**not** carried forward under a serde alias, because its value was that
over-counting sum — migrating it would launder the defect under a name
promising it had been fixed. `resolved` and `candidates` are aliased, since
those were always exactly what the new names say.

## Consequences

The 3.7% figure is gone from the CLI, from the model's context and from the
trail. `trellis radius Factory` now reports 92 reached, and 2,382 unbound
across 22 files — and shows that 1,575 of those sit in five test files, which
is a fact a user can act on and which the ratio hid completely.

Five unit tests were added to `radius.rs`, which had none, plus one in
`trellis-loop` asserting no `%` may appear in a radius observation and one in
`trellis-trail` asserting the old field name cannot reappear under any alias.

The cost is a second coverage type to keep in mind, and the risk that a future
change adds a convenience `total()` because a call site would read more neatly
with one. That is the change this ADR exists to make someone argue for.

Not addressed here: whether doubt in test files should be reported separately
from doubt in source files. The consequences differ — an unbindable call in a
test that might reach the symbol means a test might break, which the checks
catch, while the same call in `src/` means production code might break
silently. Classifying a file as a test requires guessing at project
convention, so for now the per-file breakdown shows the paths and lets the
reader see it.

## Rejected alternatives

**Keep one `Coverage` and add a `denominator_is_comparable: bool`.** The flag
would be checked by whoever remembered it existed. The whole point of the
project's other structural rules — the `Verdict` seal, `UntrustedTrail`,
`ModeCounts` without `total()` — is that the type refuses rather than the
reader remembering.

**Fix the string and leave the type.** The trail and the model observation
formatted the numbers themselves, so a display-layer fix would have left both
wrong and the next consumer free to divide again.

**Make the radius's `unresolved` count only sites that could plausibly reach
the symbol**, so the two populations match and the ratio becomes meaningful.
Attractive and unsound. Filtering unbound sites by whether their callee text
names a radius symbol misses `f = Factory; f()` (`LocalBinding`) and
`self.f = Factory; self.f()` (`MethodDispatchUntyped`) — the two most common
unbound shapes in the corpus, and precisely the ones that do reach the symbol
under another name. Narrowing the doubt to make a percentage computable would
trade a misleading figure for a falsely reassuring one.
