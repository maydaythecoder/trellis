# ADR 0012 — Deviating from the S16 rule to build a receiver-typing backstop

**Status:** Accepted
**Date:** 2026-09-19
**Supersedes the decision deferred in:** ADR 0011

## Context

ADR 0011 measured recall at 47.8% (95% CI [40.0%, 55.6%]) against a 60% pivot
trigger that ADR 0002 pre-registered on 2026-09-18, before the resolver
existed. The trigger fired. ADR 0011 listed three options and deferred the
choice to the project owner.

**The project owner chose option C: build a bounded backstop and re-measure.**

## Decision

Build annotation- and assignment-guided receiver typing in `trellis-index`,
then re-measure recall against the same audit sample.

**This deviates from the pre-registered rule, and that deviation is recorded
here rather than absorbed quietly.** ADR 0002 says a threshold chosen after
seeing the number is a threshold chosen to be met. The same logic applies to
setting one aside after seeing the number. What makes this a decision rather
than a rationalization is that it is written down, owned, and constrained:

- The 60% and 85% thresholds in ADR 0002 are **unchanged**. This ADR does not
  move them, and nothing here licenses moving them later.
- The backstop is **one bounded attempt**, scoped below. If the re-measurement
  does not clear 60%, ADR 0011 option A executes without further argument.
- The pivot is deferred, **not cancelled**.

## Why this and not a general semantic backstop

The evidence in ADR 0011 is specific about where the deficit is:

- `Name` callees resolve at 84.7%, close to the 85% gate already.
- `Attribute` callees resolve at 17.2% and carry 55% of the denominator.
- `MethodDispatchUntyped` is 10,646 of 11,233 unresolved sites corpus-wide.
- 51% of first-party misses cite a pytest fixture, and the recurring pattern is
  a receiver whose type is stated locally: `self._x = SomeClass()` in
  `__init__`, or a parameter with a type annotation. poetry is mypy-strict.

So the missing capability is narrow: *given `obj.method()`, what is `obj`?*
Answering it from annotations and constructor assignments in the same file is
far cheaper than the Pyright-over-LSP dependency S1 reaches for, and it does
not put a Node process in the critical path of a tool whose selling point is
that it runs locally.

## Scope, deliberately bounded

In:

1. **Base classes.** `class C(Base)` records `Base`, so `self.method()` can
   resolve to an inherited definition.
2. **Parameter annotations.** `def f(x: Foo)` types `x` as `Foo` within `f`.
3. **Annotated assignments.** `x: Foo = ...`, including at class scope.
4. **Constructor assignments.** `x = Foo()` and `self._x = Foo()` type the
   name by the class constructed.

Out, for now: cross-module type inference, generics and containers
(`list[Foo]` does not type its elements), return-type propagation, and
`conftest.py` fixture return types. Each is a further step and none is needed
to test the hypothesis.

## Soundness is not negotiable

Precision measured 100% and v1.0 gate item 2 forbids unsound edges. Every rule
above can only *add* resolutions, so each is a new opportunity to be wrong. The
constraints:

- A name with **conflicting** type facts in one scope resolves to
  `Candidates` or not at all — never to one of them.
- A method found on **more than one** base class is `Candidates`. Python's MRO
  is linearizable but we do not model it, so we do not pretend to choose.
- A class whose bases are not all first-party cannot have its attribute lookup
  called exhaustive: a miss stays `MethodDispatchUntyped` rather than becoming
  a confident answer.
- `examples/check_soundness.rs` must stay at zero violations corpus-wide, and
  precision must stay at 100% on the audited sample.

## How it is measured, and the trap in measuring it

The audit's ground truth — `true_callee` and `is_first_party` — is a property
of poetry, not of the resolver. So the same 400-site sample can be re-scored
against a new resolver with no new audit.

**That is also the trap.** Re-measuring on the sample the improvement was aimed
at is measuring on the test set, and it will flatter the result. Mitigations,
all of which are weaker than a fresh sample and are stated as such:

- The implementation is written from the *aggregate* pattern (annotations,
  constructor assignments), not by inspecting individual audited misses.
- A second, independent sample is drawn with a **different seed** and audited
  before any claim about clearing the 60% trigger is treated as final.
- The corpus-wide figure and the mechanical soundness check are reported
  alongside, since neither depends on the audit sample at all.

## What changes our mind

If the re-measurement does not clear 60% on a fresh sample, ADR 0011 option A
executes. The pre-registered rule was suspended once, for one bounded attempt,
and is not suspended again.
