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
    /// Call sites whose callee is provably *not* first-party — `os.path.join`,
    /// `len`, anything reached through an import that resolves outside the
    /// repository root.
    ///
    /// Deliberately **not** part of `total()`. Pass 2 discards third-party
    /// names by design, so counting them as misses would understate recall by
    /// roughly the third-party share of all calls, which is large.
    /// `BENCHMARK_AND_EVAL.md` §2 defines recall over call sites whose true
    /// callee is first-party; this field is that exclusion, made auditable
    /// rather than assumed.
    pub external: u32,
    /// Distribution over `Reason`, instrumented from day one because it is
    /// the input to the v0.1 gate and to the S1 fallback decision.
    /// `BTreeMap` for deterministic ordering in snapshots and trail output.
    pub by_reason: BTreeMap<Reason, u32>,
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
        *self.by_reason.entry(r).or_insert(0) += 1;
    }

    /// Records a call site excluded from the denominator because its callee
    /// is not first-party.
    pub fn record_external(&mut self) {
        self.external += 1;
    }

    /// Folds another coverage into this one. Used to aggregate per-file
    /// resolution into a repository figure.
    pub fn merge(&mut self, other: &Coverage) {
        self.resolved += other.resolved;
        self.candidates += other.candidates;
        self.unresolved += other.unresolved;
        self.external += other.external;
        for (k, v) in &other.by_reason {
            *self.by_reason.entry(*k).or_insert(0) += v;
        }
    }

    /// One line, honest, for the CLI and the trail.
    ///
    /// Never emits a bare "complete": a tool that confidently reports a
    /// blast radius missing the thing that breaks is worse than no tool.
    pub fn summary(&self) -> String {
        match self.resolved_fraction() {
            None => "no call sites examined".to_string(),
            Some(f) => format!(
                "{} first-party call sites resolved of {} non-excluded call sites \
                 ({:.1}%, a lower bound on recall; A-30); \
                 {} ambiguous, {} unresolved; {} external (excluded)",
                self.resolved,
                self.total(),
                f * 100.0,
                self.candidates,
                self.unresolved,
                self.external,
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
        assert_eq!(c.by_reason[&Reason::DecoratorIndirection], 2);
        assert_eq!(c.by_reason[&Reason::StarImport], 1);
        assert!(
            c.summary()
                .contains("2 first-party call sites resolved of 6 non-excluded"),
            "{}",
            c.summary()
        );
    }

    #[test]
    fn external_sites_stay_out_of_the_denominator() {
        let mut c = Coverage::default();
        c.record_resolved();
        c.record_unresolved(Reason::MethodDispatchUntyped);
        for _ in 0..50 {
            c.record_external();
        }

        // Recall is defined over call sites whose true callee is first-party.
        // 50 calls into the standard library must not move it.
        assert_eq!(c.total(), 2);
        assert_eq!(c.resolved_fraction(), Some(0.5));
        assert!(c.summary().contains("50 external"));
    }

    #[test]
    fn merge_folds_counts_and_reasons() {
        let mut a = Coverage::default();
        a.record_resolved();
        a.record_unresolved(Reason::LocalBinding);
        a.record_external();

        let mut b = Coverage::default();
        b.record_resolved();
        b.record_unresolved(Reason::LocalBinding);
        b.record_unresolved(Reason::UnboundName);

        a.merge(&b);
        assert_eq!(a.resolved, 2);
        assert_eq!(a.unresolved, 3);
        assert_eq!(a.external, 1);
        assert_eq!(a.by_reason[&Reason::LocalBinding], 2);
        assert_eq!(a.by_reason[&Reason::UnboundName], 1);
    }
}
