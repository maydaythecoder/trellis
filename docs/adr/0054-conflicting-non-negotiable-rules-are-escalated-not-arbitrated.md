# ADR 0054 — Conflicting non-negotiable rules are escalated verbatim, not arbitrated

**Status:** Proposed. Built in #11; awaiting the owner's confirmation (G7).
The behaviour is the one #11's `## Done when` specifies; this ADR records
it and the choices the ticket left open.
**Date:** 2026-10-12
**Decided by:** #11 (filed from `history/03-v0.3-memory(1).md` §2), and the
owner's comment of 2026-10-12 dropping #4 from its `blocked_by` ("Build with
the scripted conflict_escalation_ tests").
**Holds:** CLAUDE.md's ten rules unchanged. No file under
`crates/trellis-checks/` changes; `Verdict` is not created, altered or
suppressed (rule 1, ADR 0004). No new dependency.

## Context

A governance rule is `.trellis/governance/<file>.toml`, ranked
`non-negotiable` or `nice-to-have`. A non-negotiable violation fails the
convergence audit, the run does not converge, and it rolls back.

STATUS, "The one that is open", records the missing half: two
non-negotiable rules in conflict should escalate, and the harness should not
arbitrate between rules the user declared equally inviolable. Deciding that
two rules *really* conflict needs the set of possible fixes, which the
harness does not have; asking a model to decide it is Constraint 2, the
model adjudicating the rules it is held to. The honest shape STATUS names is
a non-negotiable failure where the model's own summary names a second rule
it could not satisfy, escalated verbatim and attributed, with the harness
declining to rule on it. It was left open while no measured configuration
could say so; the owner lifted that block on 2026-10-12.

## Decision

1. **Detection is two substring facts.** After the convergence audit fails,
   a loaded rule is *broken* when it is non-negotiable and the audit found a
   non-negotiable violation of it (`Violation.rule == Rule.name`), and
   *named* when it is non-negotiable, its `name` is not empty, its exact
   `name` is a case-sensitive substring of the model's `done` summary, and it
   is not broken. An escalation is shown when at least one rule is broken and
   at least one is named. Nothing else is inferred: not whether the conflict
   is real, not which rule the model meant, not why.
2. **What is shown.** Under the failure's existing explanation, one line
   reading `conflicting non-negotiable rules; trellis does not arbitrate
   between them`, then for every broken or named rule its `name`, its source
   path (`.trellis/governance/<file>.toml`) and its `description`, verbatim.
   The rules are listed in the order `trellis_checks::governance::load` read
   them, the order of their file names. No rule is labelled the winner,
   preferred, or first to fix, and the escalation does not say which rule was
   broken and which named: the audit's `VIOLATION` lines above it already say
   which broke, and a label beside each rule would read as an order of repair.
3. **The outcome does not change.** The run ends as
   `NotConverged::GovernanceViolation`, with the same `detail` and the same
   iterations and turns as without the escalation, so it rolls back the same
   way. The escalation is a new field on that variant,
   `conflict: Option<ConflictEscalation>`, read by `NotConverged::explain`,
   which is what `trellis run` prints under `NOT CONVERGED` and what an
   `after_run` hook is given as the outcome's reason.
   It is asked for only after the outcome is settled; an orchestrator that is
   gone by then answers `None`, which loses the escalation and changes no
   outcome.
4. **Source paths without changing `trellis-checks`.** `Rule` does not
   record its file. The ticket forbids changing `crates/trellis-checks/`, so
   the loop lists `.trellis/governance/*.toml` and loads it again the way
   `load` does (sorted, one rule per file) and attributes a rule only to a
   file whose parsed rule is equal to it field for field. When the run's
   rules are exactly what is on disk, position decides. A rule no file now
   holds is shown with `source: not known (no file under
   .trellis/governance/ holds this rule now)`. An attribution to the wrong
   file would be worse than none.
5. **Where it lives.** `trellis-loop`: `governance::conflict_escalation`
   (pure over rules, violations and summary, plus the source lookup), the
   orchestrator's ungated `Conflict` request (the audit's own scope, the
   files the run touched), and the loop's `GovernanceViolation` arm. The
   public surface this adds or changes, which is why the change needs this
   ADR: the `conflict` field on `NotConverged::GovernanceViolation`, the
   `Request::Conflict` variant, `Handle::conflict_escalation`, and the
   exported `ConflictEscalation`, `EscalatedRule` and `CONFLICT_LINE`. No
   on-disk format changes.

## Alternatives rejected

- **Ask the model whether the rules conflict.** Constraint 2.
- **Match names case-insensitively or by token.** Wider matching finds more
  escalations and more false ones; the ticket specifies the exact name, and a
  user can see why an exact match fired.
- **Count a second *broken* rule as the conflict.** When the summary names a
  rule that also broke, the model failed both; that is two failures, not one
  rule held at another's expense. Such a rule is listed when another rule is
  named, because it is broken, but naming it alone escalates nothing.
- **Record the escalation in the trail.** The trail records the audit as a
  failed governance turn and does not record the outcome's explanation, so
  this change leaves it as it is: adding the escalation would be a trail
  format change (ADR 0007) for text the run's own output already shows.
  Revisit, with that ADR, if `trellis why` needs to answer from it.
- **Add `source` to `trellis_checks::governance::Rule`.** The cleaner
  attribution, and outside this ticket's files. A later ticket in
  `area:checks` can add it and drop the second read in decision 4.

## What is not built

Any judgment that the conflict is real. An escalation says the summary named
a rule; it does not say the rule could not be satisfied.

## Reversal

Remove the `conflict` field and the `Conflict` request; the run's outcome and
rollback are unchanged by doing so, because they never depended on it.
