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
//! See `docs/adr/0005-loop-shape.md`.

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
    /// The one partition that runs alone, before anything else starts.
    ///
    /// Holds every unit the index could not vouch for and every unit that
    /// contends for a file one of those touches (S19).
    fn shared() -> Self {
        Self::empty(true)
    }

    /// A partition that may run concurrently with the others.
    fn parallel() -> Self {
        Self::empty(false)
    }

    /// Private, and the only place the flag is written as a bare bool.
    ///
    /// `Partition::new(true)` at a call site said nothing about what `true`
    /// meant, and the two states are not symmetric: one of them is the
    /// conservative bucket the whole staging exists to keep separate.
    fn empty(conservative: bool) -> Self {
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

/// Builds the work unit for one target symbol.
///
/// The radius is the reverse reachable set — the callers — because that is
/// what two agents can collide through. The file set is every file holding
/// either the target, a symbol in its radius, or a call site reaching it:
/// two agents editing one file conflict no matter how disjoint their call
/// graphs are, which is the failure this exists to prevent.
///
/// `partitionable` is taken from the radius report rather than recomputed.
/// It is false when any edge involved was `Candidates` or `Unresolved`, so a
/// unit the graph cannot vouch for cannot be made to look disjoint (S19).
pub fn unit_for(index: &Index, target: SymbolId) -> WorkUnit {
    let report = trellis_index::radius::blast_radius(index, target);

    let mut files: HashSet<FileId> = HashSet::new();
    files.insert(index.symbols[target.index()].file);
    for s in &report.symbols {
        files.insert(index.symbols[s.index()].file);
    }
    for e in report.certain.iter().chain(&report.ambiguous) {
        files.insert(index.sites[e.site.index()].file);
    }

    let mut radius: HashSet<SymbolId> = report.symbols.iter().copied().collect();
    // The target is in its own radius: two units naming the same symbol are
    // not disjoint, and leaving it out would let them look that way.
    radius.insert(target);

    WorkUnit {
        target,
        radius,
        files,
        partitionable: report.partitionable,
    }
}

/// A staged plan: shared work first, then concurrent work.
///
/// **One agent goes to the shared files; separate agents run from that
/// point.** The orchestrator owns that discretion, which is why the plan has
/// a shape rather than being a flat list.
///
/// The earlier design merged everything the graph could not vouch for into
/// one partition *alongside* the others, then absorbed every partition that
/// collided with it. Staging inverts it: contended work runs alone and
/// first, and once it has landed the rest are free, because the thing that
/// made them unsafe was running *concurrently* with it — not running at all.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Run first, alone, in one agent: work the graph cannot vouch for, and
    /// work contending for the same files.
    pub shared: Option<Partition>,
    /// Run concurrently, once `shared` has landed. Mutually compatible by
    /// construction; deliberately *not* required to be compatible with
    /// `shared`, because they no longer overlap it in time.
    pub parallel: Vec<Partition>,
}

impl Plan {
    /// How many agents this plan can occupy at its widest.
    pub fn width(&self) -> usize {
        self.parallel.len().max(usize::from(self.shared.is_some()))
    }

    /// Every partition, shared first. Integration order is this order, and it
    /// is deterministic: same inputs, same sequence, so a failure reproduces.
    pub fn in_order(&self) -> impl Iterator<Item = &Partition> {
        self.shared.iter().chain(self.parallel.iter())
    }
}

/// Groups work units into a staged plan.
///
/// Optimistic partitioning, pessimistic integration: this is a *filter*
/// deciding what may run concurrently, never a correctness guarantee. It is
/// only as trustworthy as the graph's recall, and recall is partial by
/// Constraint 1. Partition-local checks therefore never substitute for the
/// integrated full check (S19 patch 2).
///
/// Three rules, in order:
///
/// 1. A unit the index cannot vouch for goes to `shared` (S19 patch 1).
/// 2. A unit contending for a file `shared` already holds goes to `shared`
///    too — two agents cannot edit one file, and the one that waits is the
///    one that can.
/// 3. Everything else groups by mutual compatibility, merging transitively:
///    a unit touching two groups joins them, because splitting would hand two
///    agents intersecting work.
pub fn plan(index: &Index, units: Vec<WorkUnit>) -> Plan {
    let mut shared = Partition::shared();
    let mut rest: Vec<WorkUnit> = Vec::new();

    for unit in units {
        if !unit.partitionable || !index.is_partitionable(unit.target) {
            shared.absorb(unit);
        } else {
            rest.push(unit);
        }
    }

    // Rule 2, to a fixpoint: absorbing a unit widens `shared`'s file set and
    // can pull in another that was previously clear of it.
    loop {
        let Some(i) = rest
            .iter()
            .position(|u| !shared.files.is_disjoint(&u.files))
        else {
            break;
        };
        shared.absorb(rest.remove(i));
    }

    let mut parallel: Vec<Partition> = Vec::new();
    for unit in rest {
        let hits: Vec<usize> = parallel
            .iter()
            .enumerate()
            .filter(|(_, p)| p.collides_with_unit(&unit))
            .map(|(i, _)| i)
            .collect();

        match hits.as_slice() {
            [] => {
                let mut p = Partition::parallel();
                p.absorb(unit);
                parallel.push(p);
            }
            [only] => parallel[*only].absorb(unit),
            _ => {
                // Highest index first, so each remove leaves the remaining
                // indices valid; ids are made deterministic by the sort below.
                let mut merged = Partition::parallel();
                for &i in hits.iter().rev() {
                    merged.merge(parallel.remove(i));
                }
                merged.absorb(unit);
                parallel.push(merged);
            }
        }
    }

    // Deterministic ids: shared is always 0 when present, so integration
    // order is stable across runs and a failure reproduces.
    let mut next = 0u32;
    let shared = if shared.units.is_empty() {
        None
    } else {
        shared.id = PartitionId::from_index(next as usize);
        next += 1;
        Some(shared)
    };
    // Sorted by the smallest target id they hold, so the same input always
    // numbers them the same way regardless of how the groups were built.
    parallel.sort_by_key(|p| p.units.iter().map(|u| u.target.0).min());
    for p in &mut parallel {
        p.id = PartitionId::from_index(next as usize);
        next += 1;
    }

    Plan { shared, parallel }
}

/// Debug view behind `trellis partitions`.
///
/// `docs/BRIEF.md` calls this "a debugging tool you will use
/// constantly", and asks it to show which units grouped, which radii
/// collided, which symbols forced serialization, and which were
/// unpartitionable for having unresolved callers. Counts alone answer none of
/// those, so this names symbols.
///
/// Takes the index because a `SymbolId` is an arena offset: printing `17` to
/// a user debugging a partition plan is printing nothing.
pub fn explain(index: &Index, plan: &Plan) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();

    if plan.shared.is_none() && plan.parallel.is_empty() {
        return "no partitions: nothing to do\n".to_string();
    }

    let name = |sym: SymbolId| -> String {
        let e = &index.symbols[sym.index()];
        format!(
            "{}.{}",
            index.files[e.file.index()].module,
            index.interner.resolve(e.name)
        )
    };

    match (&plan.shared, plan.parallel.len()) {
        (Some(_), 0) => {
            let _ = writeln!(
                s,
                "One stage: everything contends, so one agent runs it all."
            );
        }
        (Some(_), n) => {
            let _ = writeln!(
                s,
                "Two stages: one agent takes the shared work first, then {n} run concurrently."
            );
        }
        (None, n) => {
            let _ = writeln!(s, "One stage: {n} partition(s), all concurrent.");
        }
    }
    let _ = writeln!(s);

    for p in plan.in_order() {
        let _ = writeln!(
            s,
            "partition {}  {} unit(s) · {} symbol(s) in radius · {} file(s){}",
            p.id.0,
            p.units.len(),
            p.radius.len(),
            p.files.len(),
            if p.conservative {
                "   SHARED — runs first, alone"
            } else {
                ""
            }
        );

        for u in &p.units {
            let _ = writeln!(
                s,
                "    {}{}",
                name(u.target),
                if u.partitionable {
                    ""
                } else {
                    "   — reached through an edge the graph could not bind"
                }
            );
        }

        // Why these units share a partition. A unit alone is not evidence of
        // a collision; two or more means their radii or files intersected,
        // and naming the overlap is the answer a user came here for.
        if p.units.len() > 1 && !p.conservative {
            let mut shared: Vec<String> = Vec::new();
            for (i, a) in p.units.iter().enumerate() {
                for b in p.units.iter().skip(i + 1) {
                    for sym in a.radius.intersection(&b.radius) {
                        shared.push(name(*sym));
                    }
                    for f in a.files.intersection(&b.files) {
                        shared.push(index.files[f.index()].path.clone());
                    }
                }
            }
            shared.sort();
            shared.dedup();
            if !shared.is_empty() {
                let _ = writeln!(s, "    grouped because they share:");
                for x in shared.iter().take(6) {
                    let _ = writeln!(s, "      {x}");
                }
                if shared.len() > 6 {
                    let _ = writeln!(s, "      ... and {} more", shared.len() - 6);
                }
            }
        }

        if p.conservative {
            let _ = writeln!(
                s,
                "{}",
                concat!(
                    "    One agent runs these, before anything else starts. They ",
                    "either contend for
    a file or reach through an edge the ",
                    "graph could not bind, and two agents
    colliding through an ",
                    "edge nobody saw is the failure this prevents (S19).
",
                    "    The partitions below are safe *because* this one has ",
                    "already landed."
                )
            );
        }
        let _ = writeln!(s);
    }

    s.push_str(concat!(
        "Partitioning is a filter on what MAY run concurrently, never a ",
        "correctness
guarantee: it is only as good as the graph's recall, ",
        "which is partial. Every
run still ends with the full check suite ",
        "across the union.
"
    ));
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
        assert!(
            p.shared.is_none(),
            "nothing contends, so nothing runs first"
        );
        assert_eq!(p.parallel.len(), 2);
        assert!(p.parallel[0].compatible_with(&p.parallel[1]));
    }

    #[test]
    fn units_sharing_a_symbol_are_serialized_into_one_partition() {
        let index = index_with(8);
        let p = plan(&index, vec![unit(1, &[1, 2], &[0]), unit(3, &[2, 3], &[1])]);
        assert_eq!(
            p.parallel.len(),
            1,
            "intersecting radii must not run in parallel"
        );
        assert_eq!(p.parallel[0].units.len(), 2);
    }

    #[test]
    fn units_sharing_only_a_file_are_still_serialized() {
        let index = index_with(8);
        let p = plan(&index, vec![unit(1, &[1], &[0]), unit(2, &[2], &[0])]);
        assert_eq!(
            p.parallel.len(),
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
        assert_eq!(p.parallel.len(), 1);
        assert_eq!(p.parallel[0].units.len(), 3);
    }

    #[test]
    fn s19_units_reached_through_uncertain_edges_share_one_conservative_partition() {
        let mut index = index_with(8);
        // Make symbol 4 reachable through a Candidates edge.
        index.sites.push(CallSiteEntry {
            kind: trellis_index::CalleeKind::Name,
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

        // The uncertain unit runs first, alone; the clean one runs after.
        let shared = p
            .shared
            .as_ref()
            .expect("the uncertain unit is shared work");
        assert_eq!(
            shared.units[0].target,
            SymbolId(4),
            "a unit the graph cannot vouch for must not be allowed to look disjoint"
        );
        assert_eq!(p.parallel.len(), 1);
        assert_eq!(p.parallel[0].units[0].target, SymbolId(1));
    }

    #[test]
    fn a_unit_flagged_unpartitionable_goes_conservative_even_with_a_clean_index() {
        let index = index_with(4);
        let mut u = unit(1, &[1], &[0]);
        u.partitionable = false;
        let p = plan(&index, vec![u]);
        assert!(p.parallel.is_empty());
        assert!(p.shared.expect("shared work").conservative);
    }

    /// The shared stage may overlap a parallel one in *radius* — it runs
    /// first, so they never contend — but never in *files*, because running
    /// in sequence does not resolve a merge conflict.
    #[test]
    fn the_shared_stage_never_shares_a_file_with_a_parallel_one() {
        let mut index = index_with(8);
        index.sites.push(CallSiteEntry {
            kind: trellis_index::CalleeKind::Name,
            file: FileId(0),
            span: (0, 1),
            enclosing: Some(SymbolId(0)),
        });
        index.push_edge(Edge {
            from: CallSiteId(0),
            target: Target::Candidates(SmallVec::from_slice(&[SymbolId(4), SymbolId(5)])),
        });

        // A certain unit whose radius happens to include symbol 4, and an
        // uncertain unit targeting symbol 4. They share a symbol, so they must
        // not end up in two partitions that are declared concurrent-safe.
        let p = plan(&index, vec![unit(1, &[1, 4], &[0]), unit(4, &[4], &[9])]);

        for (i, a) in p.parallel.iter().enumerate() {
            for b in &p.parallel[i + 1..] {
                assert!(
                    a.compatible_with(b),
                    "partitions {} and {} intersect but were planned to run concurrently",
                    a.id.0,
                    b.id.0
                );
            }
        }
        if let Some(shared) = &p.shared {
            for b in &p.parallel {
                assert!(
                    shared.files.is_disjoint(&b.files),
                    "the shared stage and partition {} would collide on a file",
                    b.id.0
                );
            }
        }
    }

    #[test]
    fn default_failure_policy_is_all_or_nothing() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::AllOrNothing);
    }

    #[test]
    fn explain_names_the_symbol_and_why_it_serialized() {
        // The doc calls this "a debugging tool you will use constantly", so
        // counts are not enough: a user asking why their task did not
        // parallelise needs the symbol, not partition 0's cardinality.
        let index = index_with(4);
        let mut u = unit(1, &[1], &[0]);
        u.partitionable = false;
        let text = explain(&index, &plan(&index, vec![u]));

        assert!(text.contains("SHARED"), "{text}");
        assert!(
            text.contains("runs first"),
            "the shared stage's whole point is that it precedes the rest: {text}"
        );
        assert!(
            text.contains("could not bind"),
            "it must say why the unit is untrusted: {text}"
        );
        assert!(
            text.contains("S19"),
            "and point at the risk it is defending: {text}"
        );
        assert!(
            text.contains("m.s1"),
            "the symbol must be named, not its arena offset: {text}"
        );
    }

    #[test]
    fn explain_names_what_two_grouped_units_share() {
        // "Which radii collided" is the question this tool exists to answer.
        let index = index_with(6);
        let a = unit(1, &[1, 2], &[0]);
        let b = unit(3, &[2, 3], &[0]);
        let text = explain(&index, &plan(&index, vec![a, b]));

        assert!(text.contains("grouped because they share"), "{text}");
        assert!(
            text.contains("m.s2"),
            "the overlapping symbol must be named: {text}"
        );
    }

    #[test]
    fn explain_says_a_plan_is_a_filter_rather_than_a_guarantee() {
        // S19's whole point. A dry run that reads as a proof of disjointness
        // is how someone concludes the integrated check is optional.
        let index = index_with(3);
        let text = explain(&index, &plan(&index, vec![unit(1, &[1], &[0])]));
        assert!(text.contains("never a correctness"), "{text}");
        assert!(text.contains("full check suite"), "{text}");
    }

    /// S19, as the one function that states it.
    ///
    /// `compatible_with` is `radius disjoint && files disjoint`, and mutation
    /// reported `&& -> ||` as surviving. That mutant calls two partitions
    /// compatible when *either* is disjoint — so two agents whose call graphs
    /// do not overlap would be handed the same file, which is the merge
    /// conflict the file check exists for and which no amount of graph
    /// disjointness prevents.
    ///
    /// `plan` never produces such a pair, which is why the suite did not
    /// notice: the property was tested through its caller, where the inputs
    /// that would break it cannot arise.
    #[test]
    fn partitions_sharing_a_file_are_never_compatible() {
        let mk = |syms: &[u32], files: &[u32]| {
            let mut p = Partition::parallel();
            p.radius = syms
                .iter()
                .map(|&i| SymbolId::from_index(i as usize))
                .collect();
            p.files = files
                .iter()
                .map(|&i| FileId::from_index(i as usize))
                .collect();
            p
        };

        // Disjoint in both: the only compatible case.
        assert!(mk(&[1, 2], &[10]).compatible_with(&mk(&[3, 4], &[11])));

        // Disjoint radii, **shared file**. Two symbols in one file cannot be
        // edited by two agents however unrelated their call graphs are.
        assert!(
            !mk(&[1, 2], &[10]).compatible_with(&mk(&[3, 4], &[10])),
            "a shared file must defeat compatibility on its own"
        );

        // Shared radius, disjoint files: also incompatible, and this is the
        // half a reader expects.
        assert!(!mk(&[1, 2], &[10]).compatible_with(&mk(&[2, 3], &[11])));

        // Neither disjoint.
        assert!(!mk(&[1], &[10]).compatible_with(&mk(&[1], &[10])));
    }
}
