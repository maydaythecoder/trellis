//! The invariant `partition::plan` exists to provide.
//!
//! Its output means "these may run concurrently". If any two planned
//! partitions intersect — on symbols or on files — two agents get
//! intersecting work while the harness reports them disjoint. That is the
//! S19 failure arriving by a different route, and it is exactly the kind of
//! bug an example-based test misses: the first version of this code passed
//! every hand-written case while the conservative partition was appended
//! without ever being compared against the regular ones.
//!
//! Hand-rolled generator rather than a proptest dependency, matching
//! `trellis-index/tests/bidirectional.rs`. The seed prints on failure.

use smallvec::SmallVec;
use std::collections::HashSet;
use trellis_core::{CallSiteId, FileId, SymbolId};
use trellis_index::{CallSiteEntry, Edge, FileEntry, Index, SymbolEntry, SymbolKind, Target};
use trellis_loop::partition::{WorkUnit, plan};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next() % u64::from(n)) as u32
        }
    }

    fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }
}

/// An index over `n_symbols`, where some symbols are made unpartitionable by
/// giving them an incoming `Candidates` edge.
fn index_with_uncertainty(rng: &mut Rng, n_symbols: u32) -> Index {
    let mut index = Index::new();
    index.files.push(FileEntry {
        path: "m.py".into(),
        module: "m".into(),
        hash: 0,
    });
    for i in 0..n_symbols {
        let name = index.interner.intern(&format!("s{i}"));
        index.symbols.push(SymbolEntry {
            name,
            file: FileId(0),
            span: (0, 1),
            kind: SymbolKind::Function,
        });
    }

    for s in 0..n_symbols {
        if !rng.chance(3) {
            continue;
        }
        index.sites.push(CallSiteEntry {
            kind: trellis_index::CalleeKind::Name,
            file: FileId(0),
            span: (0, 1),
            enclosing: Some(SymbolId(0)),
        });
        let site = CallSiteId::from_index(index.sites.len() - 1);
        index.push_edge(Edge {
            from: site,
            target: Target::Candidates(SmallVec::from_slice(&[SymbolId(s)])),
        });
    }
    index
}

fn arbitrary_units(rng: &mut Rng, n_symbols: u32, n_files: u32, n_units: u32) -> Vec<WorkUnit> {
    (0..n_units)
        .map(|_| {
            let target = SymbolId(rng.below(n_symbols));
            let mut radius: HashSet<SymbolId> = HashSet::from([target]);
            for _ in 0..rng.below(4) {
                radius.insert(SymbolId(rng.below(n_symbols)));
            }
            let mut files: HashSet<FileId> = HashSet::new();
            for _ in 0..=rng.below(3) {
                files.insert(FileId(rng.below(n_files)));
            }
            WorkUnit {
                target,
                radius,
                files,
                // Mixed flags: some units are independently flagged
                // unpartitionable even where the index is clean.
                partitionable: !rng.chance(5),
            }
        })
        .collect()
}

/// The S19 property, restated for a staged plan.
///
/// Staging changed what the invariant *is*, so it is written out rather than
/// relaxed. Three claims, and only the first is about concurrency:
///
/// 1. **Parallel partitions are pairwise disjoint** in radius and files.
///    These genuinely run at the same time, and this is the claim that makes
///    that safe.
/// 2. **The shared stage is file-disjoint from every parallel partition.** It
///    runs first, so a shared *radius* is fine — but a shared file would be a
///    merge conflict across stages, which time does not fix.
/// 3. **Every unit the index cannot vouch for is in the shared stage**, never
///    in a parallel one.
#[test]
fn a_staged_plan_keeps_concurrent_work_disjoint() {
    for seed in 1..400u64 {
        let mut rng = Rng(seed);
        let n_symbols = 2 + rng.below(12);
        let n_files = 1 + rng.below(5);
        let n_units = 1 + rng.below(10);

        let index = index_with_uncertainty(&mut rng, n_symbols);
        let units = arbitrary_units(&mut rng, n_symbols, n_files, n_units);
        let p = plan(&index, units);

        // 1 — the partitions that actually run together.
        for (i, a) in p.parallel.iter().enumerate() {
            for b in &p.parallel[i + 1..] {
                assert!(
                    a.radius.is_disjoint(&b.radius),
                    "seed {seed}: parallel partitions {} and {} share a symbol",
                    a.id.0,
                    b.id.0
                );
                assert!(
                    a.files.is_disjoint(&b.files),
                    "seed {seed}: parallel partitions {} and {} share a file",
                    a.id.0,
                    b.id.0
                );
            }
        }

        // 2 — across stages, files still cannot overlap.
        if let Some(shared) = &p.shared {
            for b in &p.parallel {
                assert!(
                    shared.files.is_disjoint(&b.files),
                    "seed {seed}: shared stage and partition {} share a file, \
                     which running in sequence does not make safe",
                    b.id.0
                );
            }
        }

        // 3 — S19 itself.
        for b in &p.parallel {
            for u in &b.units {
                assert!(
                    u.partitionable && index.is_partitionable(u.target),
                    "seed {seed}: partition {} runs a unit the index cannot \
                     vouch for; it belongs in the shared stage",
                    b.id.0
                );
            }
        }
    }
}

/// Ids are assigned once, densely, shared first.
///
/// Integration order is id order, and the milestone requires it deterministic
/// so a failure reproduces. A duplicate or a gap would make "apply in
/// partition order" ambiguous.
#[test]
fn partition_ids_are_dense_and_shared_is_first() {
    for seed in 1..400u64 {
        let mut rng = Rng(seed);
        let n_symbols = 2 + rng.below(12);
        let n_files = 1 + rng.below(5);
        let n_units = 1 + rng.below(10);
        let index = index_with_uncertainty(&mut rng, n_symbols);
        let units = arbitrary_units(&mut rng, n_symbols, n_files, n_units);
        let p = plan(&index, units);

        let ids: Vec<u32> = p.in_order().map(|x| x.id.0).collect();
        let expected: Vec<u32> = (0..ids.len() as u32).collect();
        assert_eq!(ids, expected, "seed {seed}: ids are not dense in order");

        if let Some(shared) = &p.shared {
            assert_eq!(
                shared.id.0, 0,
                "seed {seed}: the shared stage must be first"
            );
        }
    }
}

#[test]
fn planning_never_loses_or_duplicates_a_unit() {
    for seed in 1..400u64 {
        let mut rng = Rng(seed);
        let n_symbols = 2 + rng.below(12);
        let n_files = 1 + rng.below(5);
        let n_units = 1 + rng.below(10);

        let index = index_with_uncertainty(&mut rng, n_symbols);
        let units = arbitrary_units(&mut rng, n_symbols, n_files, n_units);
        let expected = units.len();

        let p = plan(&index, units);
        let got: usize = p.in_order().map(|x| x.units.len()).sum();

        // The merge-to-fixpoint step removes partitions from the vector while
        // iterating; dropping a unit there would silently skip work the user
        // asked for.
        assert_eq!(
            got, expected,
            "seed {seed}: unit count changed during planning"
        );
    }
}
