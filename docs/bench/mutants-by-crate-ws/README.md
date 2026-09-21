# Mutation testing with the whole suite running

The correct measurement, and the one `docs/bench/mutants-by-crate/` is not.

```
WORKSPACE_TESTS=1 scripts/mutate-by-crate.sh <crate>
# cargo mutants -p <crate> --test-workspace true --timeout 300 -j 1
```

`cargo mutants -p X` mutates X and runs **only X's tests**, so a mutant that a
dependent's tests would catch is reported as surviving. `--test-workspace true`
mutates one crate and runs everything, which is what "is this code tested"
actually means.

## The cost, and why only some crates are here

The workspace suite takes **46 seconds warm**, against one second for
`trellis-core` alone. That is the price per mutant, so this is affordable for
a small crate and not for a large one:

| crate | mutants | at ~40s each |
|---|---|---|
| `trellis-core` | 13 | 10 min — **done** |
| `trellis-vcs` | ~29 | ~20 min |
| `trellis-model` | ~37 | ~25 min |
| `trellis-checks` | ~35 | ~23 min |
| `trellis-trail` | ~130 | 1.5 h |
| `trellis-loop` | ~220 | 2.5 h |
| `trellis-index` | ~450 | 5 h |

The bottom three stay as self-test scores, labelled as such.

## `-j 1`, and why that is not the `-j 2` mistake repeated

The first attempt ran this at `-j 4` and **every mutant timed out** — 10
timeouts and 0 verdicts on `trellis-core`, 23 timeouts and 4 verdicts on
`trellis-vcs`. A timeout is neither caught nor missed, so both runs said
nothing and were deleted.

Four concurrent copies of a 46-second suite contend on git and temp-directory
I/O badly enough to blow the auto-derived timeout. That is a different failure
from the one that killed the whole-workspace runs, where the bottleneck was
memory across many *short* test runs and `-j 2` did not help. Here the
bottleneck is contention within one long run, and `-j 1` removes it by
definition rather than by hoping.

The script now warns and marks a result unusable above two timeouts, so this
cannot be read as a score again by mistake.

## Results

```
crate            self-test    workspace    delta
trellis-core        50.0%        70.0%     +20.0 pts     (5/5 -> 7/3, 0 timeouts)
```

Same ten viable mutants both ways, which is the check that the two runs are
comparable at all.

### What the difference is made of

Two survivors died when the rest of the suite ran, and both are the ones
predicted before the run:

```
Interner::deserialize -> Ok(Default::default())
Interner::from_store  -> Default::default()
```

Killed by `trellis-index`'s `cache::tests::roundtrips_and_preserves_the_graph`
and `a_changed_file_set_invalidates_the_blob`. `trellis-core` has one test of
its own and is exercised almost entirely by its dependents, which is exactly
the shape this measurement exists to expose.

### What survives even with everything running

```
ids.rs:55       RunId's Display impl -> Ok(())
interner.rs:101 Interner::is_empty   -> true
interner.rs:101 Interner::is_empty   -> false
```

Three real gaps rather than artefacts. `Interner::is_empty` is also
unreferenced — it appeared in the coverage report's never-executed list — so
it is dead code as well as untested, the same shape as C-002 and C-008.
