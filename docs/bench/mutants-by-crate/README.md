# Mutation testing, one crate at a time

Tier 6 of the correctness campaign, after two whole-workspace attempts were
stopped by the system for memory pressure at 842 and 295 of ~1,253 mutants.

```
scripts/mutate-by-crate.sh            # rustc 1.93.0, cargo-mutants 27.1.0, -j 4
```

Each crate runs alone and its results are saved before the next starts, so a
kill costs at most the crate in flight rather than everything since the
beginning. `trellis-loop` took 12 minutes; the whole set is well inside what
survives.

## Scores

```
crate               caught  missed     score   measured at
trellis-checks          31       2     93.9%   8850288
trellis-model           29       6     82.9%   8850288
trellis-trail           98      22     81.7%   8850288
trellis-loop           123      57     68.3%   8850288
trellis-vcs             16      11     59.3%   8850288
trellis-core             5       5     50.0%   8850288
                       302     103     74.6%

trellis-index                                  running
trellis-cli                                    not run — see below
```

**This is not a workspace score yet.** Two crates are missing and they are the
two largest: `trellis-index` is in flight, and `trellis-cli` has not been run
here because both whole-workspace attempts covered it and the figures are in
`docs/bench/mutants-486fff1/`.

## What the numbers are worth, per crate

**`trellis-checks` 93.9%** is the highest and it is the right crate to have
there. It owns `Verdict`, and the grounding contract is what the whole design
rests on. Two survivors, both in `coverage.rs`.

**`trellis-core` 50.0%** is the lowest and the least alarming: ten mutants
total, on newtype accessors and the interner. A percentage over ten mutants is
noise, and this is the clearest case in the set for reading the count rather
than the ratio.

Worth noting it moved: run 1 measured this crate at 62.5% over **eight**
mutants. The count changed because `Interner` gained `from_store` and a hand-
written `Serialize`/`Deserialize` pair when C-003 was fixed. Same crate,
different code, so the two figures are not a regression — they are two
measurements of two things.

**`trellis-vcs` 59.3%** has a known cause. `Worktree::{create,discard}` is
dead code written ahead of need for brief 4.13's per-partition isolation, and
its mutants survive because nothing calls it. That is C-002, already recorded,
and it inflates the survivor count without saying anything about the tests
that exist.

**`trellis-loop` 68.3% is stale by one commit.** It was measured at `8850288`;
`ed7e14e` then added the tier/schema and JSON-escape tests, which kill at
least four of the 57 survivors by direct verification. The crate needs
re-running before the figure is quoted as current.

## Triage

`missed.txt` in each directory is the survivor list. The `trellis-loop` triage
and its two resulting fixes are in `CORRECTNESS_LEDGER.md`; the rest are not
triaged yet.

## Reproducing

```
scripts/mutate-by-crate.sh                    # every crate not yet recorded
scripts/mutate-by-crate.sh trellis-loop       # one, re-run
```

The script skips a crate that already has a `caught.txt`, so re-running it
resumes rather than starting over. Delete a crate's directory to force it.
