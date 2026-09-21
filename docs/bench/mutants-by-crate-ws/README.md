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
trellis-vcs         59.3%        92.6%     +33.3 pts     (16/11 -> 25/2)
trellis-core        50.0%        70.0%     +20.0 pts     (5/5  -> 7/3)
trellis-model       82.9%    not computable                see below
```

`trellis-checks` was never reached: the job was killed for system memory,
the third such kill, and it has not been restarted.

Zero timeouts in both, and the same viable-mutant count each way, which is
the check that the two runs are comparable at all.

**The self-test figures were badly misleading, and `trellis-vcs` is the clear
case.** 59.3% reads like a crate with thin tests. 92.6% is what it actually
is: nine of its eleven survivors die the moment `trellis-cli`'s session tests
run, because `pending`, `diff` and `discard_pending` are driven from there and
from nowhere inside `trellis-vcs`.

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

### `trellis-vcs`: two survivors, one of them already known

```
lib.rs:246  delete ! in reset_and_clean
lib.rs:353  Worktree::discard -> Ok(())
```

The second is **C-002** — `Worktree` is dead code written ahead of need for
brief 4.13, and this is the mutation evidence for a finding that was made by
reading. Nothing calls it, so nothing can catch it.

The first is triaged and deliberately not fixed. It is `if !detail.is_empty()`,
the separator between two error strings when **both** `reset --hard` and
`clean -fd` fail. Reaching it needs two simultaneous git failures, and the
damage is a missing line break in a message. The rollback-failure path matters
— it is the worst moment TRELLIS has — but what matters about it is that it is
loud, names the worktree and gives the commands, and all three of those are
tested. A separator is not. Class C.

### `trellis-model`: five mutants with no verdict, and the reason was ours

The run returned 31 caught, 6 missed and **5 timeouts**. The script's own rule
is that more than two timeouts makes a result unusable, so no score is quoted
here — the honest statement is that it lies somewhere in **[73.8%, 85.7%]**,
an interval too wide to be worth anything.

The five are worth more than the score, because they were not a tool
artefact:

```
runtime.rs:249  probe_constraint -> Ok(true)
runtime.rs:249  probe_constraint -> Ok(false)
runtime.rs:283  probe_slots      -> Ok(None)
runtime.rs:300  get              -> Ok(String::new())
runtime.rs:300  get              -> Ok("xyzzy".into())
```

All five make the client skip its HTTP request. The test stubs then had an
unbounded `listener.accept().await`, so with nobody connecting the stub task
waited forever and `server.await` hung the test. A hang is recorded as a
timeout, which is neither caught nor missed — so five mutants that the suite
*does* detect were reported as undetected.

Both stubs now bound the accept and yield `NO_REQUEST` on expiry. Verified by
applying `get -> Ok(String::new())` by hand: **three tests fail in 7 seconds**
where the same mutant previously ran to a 300-second timeout.

The second reason this mattered is CI. A hang stalls a build instead of
reporting, and it would have done so on any machine, not only under
mutation.

### `trellis-core`: what survives even with everything running

```
ids.rs:55       RunId's Display impl -> Ok(())
interner.rs:101 Interner::is_empty   -> true
interner.rs:101 Interner::is_empty   -> false
```

Three real gaps rather than artefacts. `Interner::is_empty` is also
unreferenced — it appeared in the coverage report's never-executed list — so
it is dead code as well as untested, the same shape as C-002 and C-008.
