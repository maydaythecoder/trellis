# The decomposition ratio — 2026-09-21

`05-v1.1-multi-agent.md` names this as one of two things that cap multi-agent
gain, and says to measure it before advertising anything:

> What fraction of real tasks partition into more than one group at all? If
> most changes touch an interconnected core, partitioning rarely fires and
> v1.1 is worth less than it looks.

Measured on poetry 2.5.0, 3,735 symbols, 200 trials per size, seed 20260921.

```
size  decomposed       ratio    all-serial  mean groups
   2           8        4.0%           192        1.04
   3          18        9.0%           182        1.09
   4          30       15.0%           170        1.16
   5          31       15.5%           169        1.17
```

**A two-symbol change partitions 4% of the time.** A five-symbol change, 15.5%.
Mean groups never exceeds 1.17, so even when partitioning fires it usually
yields two groups rather than four.

These are symbol sets drawn at random. Real changes are correlated — people
edit things that belong together — so real work partitions *less* often than
random draws. **Read the table as an upper bound.**

## Why: not the reason the milestone expects

The doc's hypothesis is an interconnected core. That is not what this corpus
shows.

```
sampled 415 symbols (every 9th of 3735)
  symbol partitionable (incoming edges certain): 407 (98.1%)
  UNIT partitionable (whole radius report):       12 (2.9%)
```

**98.1% of symbols satisfy S19's stated rule** — no `Candidates` or
`Unresolved` incoming edges. Only **2.9% of work units** are partitionable,
and the gap between those two numbers is the whole finding.

The difference is `add_touched_file_misses` in `radius.rs`. It clears
`partitionable` when *any* unbound call site exists in *any* file the radius
touches:

```rust
// An unbindable call in a file this radius touches means the
// radius cannot be trusted to be disjoint from another (S19).
report.partitionable = false;
```

On this corpus there are 11,233 unresolved call sites spread across the tree,
so nearly every radius touches a file containing at least one. The binding
constraint on multi-agent work is therefore **not** the rule the milestone
document describes.

## Two readings, and this does not choose between them

**It is correct and v1.1 is worth little at 72% recall.** An unbound call in a
file the radius touches has an unknown callee; it *could* be this symbol; so
disjointness cannot be proven and the conservative answer is the right one.
Under this reading the measurement is the point: multi-agent work is gated on
recall, and engineering effort spent on agent supervision buys almost nothing
until the graph improves. Getting recall from 72% toward 85% would move this
table further than any amount of concurrency code.

**It is over-broad for this use.** The touched-file heuristic was introduced
for the *coverage* count (Q-13), where over-approximating doubt is the safe
direction because the cost is a pessimistic report. Partitioning reuses the
same flag for a *safety* decision, and the two do not obviously want the same
threshold. A narrower test — unbound call sites in files containing the
radius's symbols, rather than every file the radius touches — might preserve
the guarantee while firing far less often.

**This document does not pick.** The milestone is explicit that the
conservative version ships first:

> This will feel over-conservative and it is the correct default. Loosening it
> is an experiment to run with measurements in hand, not a starting position.

The measurements are now in hand, which is the condition it names. Loosening
is a change to a safety rule and belongs in an ADR with its own evidence, not
in the commit that measured the problem.

## What this means for the v1.1 gate

The exit gate asks for "a task decomposing into three or more disjoint
partitions" to complete in parallel. **On this corpus, at this recall, such a
task is rare enough that the gate would be demonstrated on a constructed
example rather than a representative one.** That is worth knowing before
building agent supervision, not after.

It also sharpens the honest-speedup rule the milestone already states. The
claim "3.1× on tasks that decompose, which is 40% of tasks" is the shape it
asks for; on this evidence the second number is nearer 4%, and a speedup
multiplied by 4% is not a headline.

## Reproducing

```
cargo run --release -p trellis-loop --example decomposition_ratio \
  -- .bench/poetry-2.5.0 200 20260921
cargo run --release -p trellis-loop --example partitionable_fraction \
  -- .bench/poetry-2.5.0
```

Both are deterministic. `decomposition_ratio` uses the same splitmix64
generator as the audit sampler, so a seed means the same thing across the
project's measurement tools.
