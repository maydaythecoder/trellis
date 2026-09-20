# The precision audit, full corpus — 2026-09-20

v1.0 gate condition 2: *"zero unsound edges — every `Resolved` edge is real."*
A full audit of the benchmark corpus, not the v0.1 sample.

`04-v1.0-harness.md` names three mechanical checks. The first two existed; the
third did not, and it is the one that reaches the case the others cannot.

| Check | Where | Result |
|---|---|---|
| The target definition exists | structural — a `Resolved` edge holds a `SymbolId` into the index | 5,908 |
| The name matches after alias resolution | `examples/check_soundness.rs` | **5,908 / 5,908**, 0 violations |
| **An import path connects the two files** | `examples/check_reachability.rs` (new) | **0 disconnected** |

## Why the third check was needed

`check_soundness` says so in its own module documentation:

> This is a necessary condition, not a sufficient one: an edge to the WRONG
> `foo` in another module satisfies it. Only the hand audit measures that.

A resolver that bound `config.load()` to the `load` in the wrong module passes
the name check and is unsound. Whether an import actually connects the calling
file to the defining file is mechanically decidable, so it does not have to
wait for a human.

## Result

```
1717 same-file edges need no import and are not counted below.

shape         connected  inherited  ambiguous  DISCONNECTED
Name               2537          0          0             0
Attribute          1620         34          0             0
total              4157         34          0             0
```

1,717 + 4,157 + 34 = 5,908, which is the recorded `Resolved` count exactly.

## What the buckets mean, and what the total is worth

**The split by shape is not decoration.** For a `Name` call the check is close
to circular: `foo()` is resolved by a scope walk that consults the calling
file's imports, so if it produced a cross-file edge it did so *through* an
import, and finding that import again establishes little. For an `Attribute`
call it is independent: `obj.method()` resolves through the receiver's type
(ADR 0012), which never consults the calling file's imports for the method.

That is also where the risk is. CLAUDE.md records that every receiver-typing
rule can only *add* resolutions, so every one is a new chance to emit an
unsound edge. A single pooled "0 disconnected" would let the circular half lend
credibility to the independent half, so the tool refuses to print one without
the rows.

**`inherited` — 34 edges, all `self.option(...)`.** The first run reported these
as disconnected. `InstallCommand(InstallerCommand)` → `GroupCommand`/
`EnvCommand` → `Command`, which defines `option`. `install.py` imports its
immediate base and nothing further, so no import *of that file* names the
target — the connection is the class hierarchy.

Rather than excuse them, the check verifies the hierarchy: from the class
enclosing the call, follow base classes, requiring every link to be
import-connected from the file declaring the subclass, **and requiring that no
class between the two defines the method itself**.

That last clause is rule 1 of the audit protocol in `sample.json`:

> `self.m()` where `m` is defined in the enclosing class resolves to THAT
> definition. Only record an inherited base-class method when the enclosing
> class does not define `m` itself.

Without it the check would bless exactly the edge the protocol calls wrong — a
resolver that walked past a nearer definition — which is what an audit of
`Resolved` edges exists to catch.

**These 34 are sound by that rule, verified rather than assumed:** `def option`
occurs exactly once in the corpus, at `command.py:46`. No class in any chain
shadows it.

## Proving the check can fail

A check that cannot report is worth nothing, so both allowances were removed in
turn and the corpus re-run:

| Mutation | Disconnected |
|---|---|
| none (as shipped) | **0** |
| inheritance walk removed | 34 |
| one re-export hop removed | 740 |
| shadowing guard forced to fire | 34 |

Each allowance is load-bearing and the machinery reports rather than silently
passing.

The shadowing guard needs a note. Injecting a real `option` into
`InstallerCommand` does *not* exercise it: the resolver correctly picks the
nearer definition, so 29 edges move to `connected` and the guard never sees a
far target. That mutation tests the resolver, not the check. Forcing the guard
to see a shadow instead turns all 34 into disconnected and exits non-zero, so
it is wired and reports — but on a correct resolver it is **inert by design**,
a necessary condition that currently never binds. It is defence against a
future resolver change, not evidence about this one.

## A stale artifact this found

Scoring `docs/bench/poetry-2.5.0-seed20260919` as stored returned **47.8%** —
the pre-ADR-0012 figure — while every document says 67.9%. The committed
`answers.json` held resolver verdicts from before receiver typing landed, so
scoring that directory replayed history.

The resolver had not regressed: regenerating `answers.json` against current code
and scoring it with the same human judgements gives 67.9% (108/159), and the
same for 72.0% on seed 20260920. Both stored `answers.json` files have been
regenerated so the artifacts agree with the code.

This is the shape gate condition 1 is designed to fail on, appearing in the
artifact rather than the resolver — and it is why the CI job **regenerates**
the verdicts instead of scoring the stored pair, which would have passed
forever.

## Line endings do not move the resolver

Checked because the corpus on this machine is CRLF and the original audit ran
on LF, and because a CRLF/LF mismatch has already cost this project one silent
measurement (the floor runner's anchors). Extracting the sdist fresh and
re-running both seeds: **zero verdicts differ** between the CRLF and LF trees,
and both score their recorded figures.

## Reproducing

```
cargo run --release -p trellis-index --example check_soundness    -- .bench/poetry-2.5.0
cargo run --release -p trellis-index --example check_reachability -- .bench/poetry-2.5.0

# condition 1, regenerating the verdicts rather than replaying them
cargo run --release -p trellis-index --example audit_sample \
  -- .bench/poetry-2.5.0 400 20260919 .bench/regen
cp docs/bench/poetry-2.5.0-seed20260919/sample.json .bench/regen/sample.json
cargo run --release -p trellis-index --example audit_score \
  -- .bench/regen --min-recall 67.9 --min-precision 100.0
```

All four run in CI as `precision-gate`.

## What this does not establish

Mechanical verification is necessary, not sufficient. An edge can name the
right symbol, in a file the caller genuinely imports, and still be the wrong
target — two same-named functions in one module reached by different paths, or
a receiver whose annotation is simply wrong. `0 ambiguous` means this check
left no residue to hand-audit on *this* corpus; it does not mean a human audit
is unnecessary. A-34 still stands, and gate condition 2 is not called until it
is done.
