# ADR 0008 — Budgets

**Status:** Accepted, values uncalibrated
**Date:** 2026-09-18

## Context

An agent loop can run indefinitely. The scarce resource differs by
configuration: on a frontier API tokens cost money; on local weights marginal
token cost approaches zero and wall-clock is what the user is spending.

Keys are the user's, so a runaway loop spends the user's money directly.

## Decision

Hard per-run ceilings on **iterations**, **wall-clock**, and **tokens**, read
from `.trellis/config.toml`, tracked through `tracing` spans so accounting and
observability are one mechanism rather than two. Do not build a second
telemetry path.

`CostRegime::{Metered, Local}` tells the budget code which resource is binding,
so warnings are relevant rather than noise.

**The token ceiling applies in both regimes.** Tested. Local weights make
tokens cheap, not free, and the global ceiling is the backstop against a
runaway loop regardless of tier.

**Budgets divide across partitions; they do not multiply.** `Budget::split(n)`
divides iterations and tokens by `n`. Wall-clock is the exception — partitions
run concurrently, so elapsed time is bounded by the whole ceiling rather than a
share of it. Per-partition spend is tracked in `tracing` spans nested under the
run span.

On exhaustion: escalate once with the current state, then roll back.

## Alternatives considered

**A single wall-clock ceiling only.** Rejected: it is the wrong resource on a
metered API, where a fast loop can be an expensive one.

**Per-partition full budgets.** Rejected: N agents each granted the full
ceiling is an N-times faster way to spend the user's money.

**Soft budgets that warn and continue.** Rejected: the guarantee is that
failure is boring and bounded. A soft ceiling is not a ceiling.

**A separate metrics system alongside `tracing`.** Rejected explicitly by the
brief, and rightly: two sources of truth about spend means neither is trusted.

## Why

The escalate-once-then-roll-back sequence exists because silent exhaustion is
the worst outcome — the user has paid the full cost and received nothing, with
no explanation. One escalation gives them the choice.

## What changes our mind

Current defaults (40 iterations, 20 minutes, 2M tokens) are **placeholders**,
marked as such in `trellis-loop/src/budget.rs` and in `ASSUMPTIONS.md` A-7.
They are not measurements and must not be cited as any. Calibration comes from
real usage after v0.2, per configuration, per `COST_MODEL.md`.
