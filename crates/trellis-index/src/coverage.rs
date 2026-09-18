//! Per-run coverage reporting.
//!
//! S2: recall is partial and the product must never imply otherwise. Every
//! consumer of a blast radius reads coverage alongside it, and there is no
//! API that returns a radius without one — that is why `RadiusReport` owns
//! a `Coverage` by value rather than offering it as an optional extra.

use crate::graph::Reason;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much of what we looked at we could actually bind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub resolved: u32,
    pub candidates: u32,
    pub unresolved: u32,
    /// Distribution over `Reason`, instrumented from day one because it is
    /// the input to the v0.1 gate and to the S1 fallback decision.
    /// `BTreeMap` for deterministic ordering in snapshots and trail output.
    pub by_reason: BTreeMap<String, u32>,
}

impl Coverage {
    pub fn total(&self) -> u32 {
        self.resolved + self.candidates + self.unresolved
    }

    /// Fraction of call sites bound to exactly one first-party definition.
    ///
    /// Returns `None` for an empty sample rather than 0.0 or 1.0: "we
    /// looked at nothing" and "we resolved nothing" are different claims,
    /// and a tool that reports the second when it means the first is the
    /// kind of confident wrongness Constraint 1 warns about.
    pub fn resolved_fraction(&self) -> Option<f64> {
        let total = self.total();
        (total > 0).then(|| f64::from(self.resolved) / f64::from(total))
    }

    pub fn record_resolved(&mut self) {
        self.resolved += 1;
    }

    pub fn record_candidates(&mut self) {
        self.candidates += 1;
    }

    pub fn record_unresolved(&mut self, r: Reason) {
        self.unresolved += 1;
        *self.by_reason.entry(format!("{r:?}")).or_insert(0) += 1;
    }

    /// One line, honest, for the CLI and the trail.
    ///
    /// Never emits a bare "complete": a tool that confidently reports a
    /// blast radius missing the thing that breaks is worse than no tool.
    pub fn summary(&self) -> String {
        match self.resolved_fraction() {
            None => "no call sites examined".to_string(),
            Some(f) => format!(
                "{} of {} call sites resolved ({:.1}%); {} ambiguous, {} unresolved",
                self.resolved,
                self.total(),
                f * 100.0,
                self.candidates,
                self.unresolved,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_coverage_makes_no_claim() {
        let c = Coverage::default();
        assert_eq!(c.resolved_fraction(), None);
        assert_eq!(c.summary(), "no call sites examined");
    }

    #[test]
    fn reasons_are_counted_separately_from_the_total() {
        let mut c = Coverage::default();
        c.record_resolved();
        c.record_resolved();
        c.record_candidates();
        c.record_unresolved(Reason::DecoratorIndirection);
        c.record_unresolved(Reason::DecoratorIndirection);
        c.record_unresolved(Reason::StarImport);

        assert_eq!(c.total(), 6);
        assert_eq!(c.resolved_fraction(), Some(2.0 / 6.0));
        assert_eq!(c.by_reason["DecoratorIndirection"], 2);
        assert_eq!(c.by_reason["StarImport"], 1);
        assert!(c.summary().contains("2 of 6"));
    }
}
