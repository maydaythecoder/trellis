# ADR 0010 — BYOK and key handling

**Status:** Accepted
**Date:** 2026-09-18

## Context

TRELLIS runs locally. When a user configures a frontier API, they supply their
own key. Local execution with the user's key going directly to the provider is
standard practice; proxying or storing keys server-side is where products get
into trouble (S12).

v1 has no networked component at all, which makes this rule free to hold today.
That is exactly why it must be written today — the temptation arrives later,
and it arrives disguised as a convenience feature.

## Decision

**The user's API key never touches infrastructure we control.** Not in transit,
not at rest, not in logs, not in telemetry, not in a trail.

Concretely:

1. Keys are read from the environment or from `.trellis/config.toml` in the
   user's repository, and used to make requests **directly** from the user's
   machine to the provider.
   **Amended 2026-10-05 (#416):** narrowed by ADR 0024 §3 and ADR 0025 §7:
   keys are read from environment variables only; a config file naming a key
   is refused at load.
2. No proxy, relay, or gateway operated by us sits in that path. Ever.
3. No key value is written to a trail, a log line, or a `tracing` span. Where a
   key must be referenced, reference the *source* ("from `ANTHROPIC_API_KEY`"),
   never the value.
4. `.trellis/config.toml` is in the repository, so the documentation must warn
   that a key placed there is a key committed to version control. Prefer the
   environment.
   **Amended 2026-10-05 (#416):** narrowed by ADR 0024 §3 and ADR 0025 §7:
   keys are read from environment variables only; a config file naming a key
   is refused at load.

This rule is carried forward into any future cloud tier without exception. If a
cloud tier cannot be built without proxying keys, the cloud tier does not get
built that way.

## Alternatives considered

**A convenience proxy** so users need not configure a provider. Rejected. It is
the single change that would destroy the safe harbor, and it will be proposed,
because it genuinely improves onboarding. Writing the rejection down now is
cheaper than arguing it later against a working prototype.

**Storing keys encrypted server-side.** Rejected: encrypted-at-rest on our
infrastructure is still on our infrastructure. The property that matters is
custody, not encryption.

**Supporting only local models.** Rejected as too restrictive — frontier
escalation is a planned v1.x capability and needs a key path.

## Why

The defensibility of "your code stays on your machine, your key stays on your
machine" rests entirely on it being literally true. A single proxy makes it
false and no amount of policy language repairs that.

## What changes our mind

Nothing. This is the one ADR intended to be unchangeable. A future team that
wants to change it should have to explain, in writing, why the claim in
`MISSION.md` no longer applies.
