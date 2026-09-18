//! Blast radius: which first-party call sites reach a symbol.

use crate::coverage::Coverage;
use crate::graph::{Index, Reason};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use trellis_core::{CallSiteId, SymbolId};

/// A call site that reaches the queried symbol, and how sure we are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusEntry {
    pub site: CallSiteId,
    pub certain: bool,
}

/// The answer to "what reaches this symbol", with its own limits attached.
///
/// There is deliberately no constructor that omits `coverage`. A caller
/// cannot obtain the reach without also obtaining the honest account of
/// what was missed (S2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadiusReport {
    pub root: SymbolId,
    /// Call sites reaching `root` through a `Resolved` edge.
    pub certain: Vec<RadiusEntry>,
    /// Call sites reaching `root` through a `Candidates` edge: `root` is
    /// one of several possibilities and we did not choose.
    pub ambiguous: Vec<RadiusEntry>,
    /// Symbols transitively reached.
    pub symbols: Vec<SymbolId>,
    pub coverage: Coverage,
    /// True when every edge involved was `Resolved`, so the radius may be
    /// used for partitioning (S19).
    pub partitionable: bool,
}

impl RadiusReport {
    pub fn site_count(&self) -> usize {
        self.certain.len() + self.ambiguous.len()
    }

    /// Deliberately not named `is_complete`. Under Constraint 1 no radius
    /// is ever known to be complete; this reports only that nothing we saw
    /// was ambiguous, which is a weaker and truthful claim.
    pub fn all_edges_certain(&self) -> bool {
        self.ambiguous.is_empty() && self.coverage.unresolved == 0
    }
}

/// Computes the reverse reachable set of `root`, following incoming edges.
///
/// Traversal is breadth-first over `reverse` adjacency, visiting each
/// symbol once. Unresolved edges anywhere in the walk are counted into
/// `coverage` and clear `partitionable`.
#[tracing::instrument(skip(index), fields(root = root.0))]
pub fn blast_radius(index: &Index, root: SymbolId) -> RadiusReport {
    let mut report = RadiusReport {
        root,
        certain: Vec::new(),
        ambiguous: Vec::new(),
        symbols: Vec::new(),
        coverage: Coverage::default(),
        partitionable: true,
    };

    let mut seen: HashSet<SymbolId> = HashSet::from([root]);
    let mut queue = std::collections::VecDeque::from([root]);

    while let Some(sym) = queue.pop_front() {
        for edge in index.incoming(sym) {
            match &edge.target {
                crate::graph::Target::Resolved(_) => {
                    report.coverage.record_resolved();
                    report.certain.push(RadiusEntry {
                        site: edge.from,
                        certain: true,
                    });
                }
                crate::graph::Target::Candidates(_) => {
                    report.coverage.record_candidates();
                    report.partitionable = false;
                    report.ambiguous.push(RadiusEntry {
                        site: edge.from,
                        certain: false,
                    });
                }
                // An unresolved edge cannot point *at* this symbol by
                // definition — it points nowhere we can name — so it is
                // never in `reverse`. Kept for exhaustiveness; if the
                // resolver ever changes, this must not silently pass.
                crate::graph::Target::Unresolved(r) => {
                    report.coverage.record_unresolved(*r);
                    report.partitionable = false;
                }
            }

            // Walk outward to the caller's own callers.
            if let Some(caller) = index.sites[edge.from.index()].enclosing
                && seen.insert(caller)
            {
                queue.push_back(caller);
            }
        }

        // A symbol reached only through uncertain edges taints the whole
        // radius for partitioning purposes (S19).
        if !index.is_partitionable(sym) {
            report.partitionable = false;
        }
    }

    seen.remove(&root);
    report.symbols = {
        let mut v: Vec<_> = seen.into_iter().collect();
        v.sort_unstable();
        v
    };
    report
}

/// Records that a call site could not be bound, for the coverage figures
/// the resolver reports per run.
pub fn note_unresolved(coverage: &mut Coverage, reason: Reason) {
    coverage.record_unresolved(reason);
}
