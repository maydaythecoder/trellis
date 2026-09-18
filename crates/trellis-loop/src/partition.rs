//! Partitioning work by blast radius.
//!
//! Agents parallelize **work**, never **judgment**. No agent reviews
//! another's output for correctness; verdicts still come only from
//! `trellis-checks`. Parallel work multiplies throughput; parallel judgment
//! multiplies error — a scaling study measured independent multi-agent
//! topologies amplifying errors 17.2-fold against a single agent, with
//! centralized coordination containing it to 4.4-fold. So: coordination
//! centralized, judgment mechanical.
//!
//! See `docs/ADRs/0005-loop-shape.md`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use trellis_core::{FileId, PartitionId, SymbolId};
use trellis_index::Index;

/// One unit of work: a symbol to change, and the radius that reaches it.
#[derive(Debug, Clone)]
pub struct WorkUnit {
    pub target: SymbolId,
    pub radius: HashSet<SymbolId>,
    pub files: HashSet<FileId>,
    /// False when any edge in the radius was `Candidates` or `Unresolved`.
    /// S19: such a unit cannot be trusted to be disjoint from anything.
    pub partitionable: bool,
}

/// A group of work units that may run in one worktree.
#[derive(Debug, Clone)]
pub struct Partition {
    pub id: PartitionId,
    pub units: Vec<WorkUnit>,
    /// Union of unit radii.
    pub radius: HashSet<SymbolId>,
    /// Files this partition may touch.
    pub files: HashSet<FileId>,
    /// True when this is the conservative partition holding every unit the
    /// index could not vouch for (S19).
    pub conservative: bool,
}

impl Partition {
    fn new(conservative: bool) -> Self {
        Self {
            id: PartitionId(0),
            units: Vec::new(),
            radius: HashSet::new(),
            files: HashSet::new(),
            conservative,
        }
    }

    /// Two partitions may run concurrently when their radii *and* their
    /// files are both disjoint.
    ///
    /// Files matter separately from symbols: two symbols in one file cannot
    /// be edited by two agents without a merge conflict, however disjoint
    /// their call graphs.
    pub fn compatible_with(&self, other: &Partition) -> bool {
        self.radius.is_disjoint(&other.radius) && self.files.is_disjoint(&other.files)
    }

    fn collides_with_unit(&self, unit: &WorkUnit) -> bool {
        !self.radius.is_disjoint(&unit.radius) || !self.files.is_disjoint(&unit.files)
    }

    fn absorb(&mut self, unit: WorkUnit) {
        self.radius.extend(&unit.radius);
        self.files.extend(&unit.files);
        self.units.push(unit);
    }

    fn merge(&mut self, other: Partition) {
        self.radius.extend(other.radius);
        self.files.extend(other.files);
        self.units.extend(other.units);
        self.conservative |= other.conservative;
    }
}

/// How a failed partition affects the rest of the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FailurePolicy {
    /// Any partition failing rolls back the whole run. The default, and the
    /// only policy unconditionally correct under partial recall.
    #[default]
    AllOrNothing,
    /// Converged partitions land; failed ones are reported with their
    /// worktrees preserved. Valid **only** when the integrated check passes
    /// on the landed subset. A failed partition's work never lands
    /// partially — its worktree is deleted.
    LandClean,
}

/// Groups work units into partitions that may run in parallel.
///
/// Optimistic partitioning, pessimistic integration: this is a *filter*
/// deciding what may run concurrently, never a correctness guarantee. It is
/// only as trustworthy as the graph's recall, and recall is partial by
/// Constraint 1. Partition-local checks therefore never substitute for the
/// integrated full check (S19 patch 2).
///
/// Units the index cannot vouch for are merged into one conservative
/// partition (S19 patch 1) rather than allowed to look disjoint.
pub fn plan(index: &Index, units: Vec<WorkUnit>) -> Vec<Partition> {
    let mut partitions: Vec<Partition> = Vec::new();
    let mut conservative = Partition::new(true);

    for unit in units {
        // A unit reached through an uncertain edge, or targeting a symbol
        // the index cannot vouch for, joins every other such unit. One
        // agent handles them serially, because we cannot prove they do not
        // collide through an edge we never saw.
        if !unit.partitionable || !index.is_partitionable(unit.target) {
            conservative.absorb(unit);
            continue;
        }

        // Collect every existing partition this unit touches. More than one
        // means the unit transitively joins them, so they merge: splitting
        // them would put two agents on intersecting work.
        let hits: Vec<usize> = partitions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.collides_with_unit(&unit))
            .map(|(i, _)| i)
            .collect();

        match hits.as_slice() {
            [] => {
                let mut p = Partition::new(false);
                p.absorb(unit);
                partitions.push(p);
            }
            [i] => partitions[*i].absorb(unit),
            many => {
                // Merge right-to-left so the earlier indices stay valid.
                let keep = many[0];
                for &i in many[1..].iter().rev() {
                    let removed = partitions.remove(i);
                    partitions[keep].merge(removed);
                }
                partitions[keep].absorb(unit);
            }
        }
    }

    if !conservative.units.is_empty() {
        partitions.push(conservative);
    }

    for (i, p) in partitions.iter_mut().enumerate() {
        p.id = PartitionId::from_index(i);
    }
    partitions
}

/// Debug view behind `trellis partitions`: which units grouped, which radii
/// collided, which symbols forced serialization.
pub fn explain(partitions: &[Partition]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for p in partitions {
        let _ = writeln!(
            s,
            "partition {} — {} unit(s), {} symbol(s) in radius, {} file(s){}",
            p.id.0,
            p.units.len(),
            p.radius.len(),
            p.files.len(),
            if p.conservative {
                "  [conservative: contains units reached through uncertain edges; \
                 serialized because the graph cannot prove them disjoint]"
            } else {
                ""
            }
        );
    }
    if partitions.is_empty() {
        s.push_str("no partitions: nothing to do\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;
    use trellis_core::{CallSiteId, FileId};
    use trellis_index::{CallSiteEntry, Edge, FileEntry, SymbolEntry, SymbolKind, Target};

    /// Index with `n` symbols, each called once from symbol 0 through a
    /// `Resolved` edge, so every symbol is partitionable by default.
    fn index_with(n: u32) -> Index {
        let mut index = Index::new();
        index.files.push(FileEntry {
            path: "m.py".into(),
            module: "m".into(),
            hash: 0,
        });
        for i in 0..n {
            let name = index.interner.intern(&format!("s{i}"));
            index.symbols.push(SymbolEntry {
                name,
                file: FileId(0),
                span: (0, 1),
                kind: SymbolKind::Function,
            });
        }
        index
    }

    fn unit(target: u32, radius: &[u32], files: &[u32]) -> WorkUnit {
        WorkUnit {
            target: SymbolId(target),
            radius: radius.iter().copied().map(SymbolId).collect(),
            files: files.iter().copied().map(FileId).collect(),
            partitionable: true,
        }
    }

    #[test]
    fn disjoint_units_partition_separately() {
        let index = index_with(8);
        let p = plan(&index, vec![unit(1, &[1, 2], &[0]), unit(3, &[3, 4], &[1])]);
        assert_eq!(p.len(), 2);
        assert!(p[0].compatible_with(&p[1]));
    }

    #[test]
    fn units_sharing_a_symbol_are_serialized_into_one_partition() {
        let index = index_with(8);
        let p = plan(&index, vec![unit(1, &[1, 2], &[0]), unit(3, &[2, 3], &[1])]);
        assert_eq!(p.len(), 1, "intersecting radii must not run in parallel");
        assert_eq!(p[0].units.len(), 2);
    }

    #[test]
    fn units_sharing_only_a_file_are_still_serialized() {
        let index = index_with(8);
        let p = plan(&index, vec![unit(1, &[1], &[0]), unit(2, &[2], &[0])]);
        assert_eq!(
            p.len(),
            1,
            "two symbols in one file cannot be edited by two agents, \
             however disjoint their call graphs"
        );
    }

    #[test]
    fn a_bridging_unit_merges_previously_separate_partitions() {
        let index = index_with(8);
        let p = plan(
            &index,
            vec![
                unit(1, &[1], &[0]),
                unit(3, &[3], &[1]),
                // Touches both: they are transitively one piece of work.
                unit(5, &[1, 3], &[2]),
            ],
        );
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].units.len(), 3);
    }

    #[test]
    fn s19_units_reached_through_uncertain_edges_share_one_conservative_partition() {
        let mut index = index_with(8);
        // Make symbol 4 reachable through a Candidates edge.
        index.sites.push(CallSiteEntry {
            file: FileId(0),
            span: (0, 1),
            enclosing: Some(SymbolId(0)),
        });
        index.push_edge(Edge {
            from: CallSiteId(0),
            target: Target::Candidates(SmallVec::from_slice(&[SymbolId(4), SymbolId(5)])),
        });
        assert!(!index.is_partitionable(SymbolId(4)));

        let p = plan(
            &index,
            vec![
                unit(1, &[1], &[0]),
                // Radius- and file-disjoint from everything, but reached
                // through an ambiguous edge.
                unit(4, &[4], &[9]),
            ],
        );

        assert_eq!(p.len(), 2);
        let cons: Vec<_> = p.iter().filter(|p| p.conservative).collect();
        assert_eq!(cons.len(), 1);
        assert_eq!(
            cons[0].units[0].target,
            SymbolId(4),
            "a unit the graph cannot vouch for must not be allowed to look disjoint"
        );
    }

    #[test]
    fn a_unit_flagged_unpartitionable_goes_conservative_even_with_a_clean_index() {
        let index = index_with(4);
        let mut u = unit(1, &[1], &[0]);
        u.partitionable = false;
        let p = plan(&index, vec![u]);
        assert_eq!(p.len(), 1);
        assert!(p[0].conservative);
    }

    #[test]
    fn default_failure_policy_is_all_or_nothing() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::AllOrNothing);
    }

    #[test]
    fn explain_names_why_a_partition_is_conservative() {
        let index = index_with(4);
        let mut u = unit(1, &[1], &[0]);
        u.partitionable = false;
        let text = explain(&plan(&index, vec![u]));
        assert!(text.contains("conservative"));
        assert!(text.contains("uncertain edges"));
    }
}
