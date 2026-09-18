//! v1.0 gate condition: bidirectionality verified by property test over
//! `forward`/`reverse`.
//!
//! Because `push_edge` is the only way to add an edge and it writes both
//! adjacency lists in one step, this is a property of the structure rather
//! than something the builder has to remember. The test exists to keep it
//! that way: it fails loudly if anyone adds a second insertion path.
//!
//! Hand-rolled generator rather than a proptest dependency — the shape
//! being generated is small, and the reproduction seed is printed on
//! failure.

use smallvec::SmallVec;
use trellis_core::{CallSiteId, FileId, SymbolId};
use trellis_index::{CallSiteEntry, Edge, FileEntry, Index, SymbolEntry, SymbolKind, Target};

/// xorshift64*, so a failing case reproduces from its printed seed without
/// pulling in a rand dependency.
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
}

/// Builds an index with `n_symbols` symbols and `n_edges` random edges
/// spread across the three `Target` variants.
fn arbitrary_index(seed: u64, n_symbols: u32, n_edges: u32) -> Index {
    let mut rng = Rng(seed | 1);
    let mut index = Index::new();

    index.files.push(FileEntry {
        path: "m.py".into(),
        module: "m".into(),
        hash: 0,
    });

    let mut interner = std::mem::take(&mut index.interner);
    for i in 0..n_symbols {
        let name = interner.intern(&format!("s{i}"));
        index.symbols.push(SymbolEntry {
            name,
            file: FileId(0),
            span: (0, 1),
            kind: SymbolKind::Function,
        });
    }
    index.interner = interner;

    for _ in 0..n_edges {
        // Every call site is enclosed by some symbol, which is the common
        // case: a call at module scope has no enclosing symbol and is
        // covered separately below.
        let enclosing = SymbolId(rng.below(n_symbols));
        index.sites.push(CallSiteEntry {
            file: FileId(0),
            span: (0, 1),
            enclosing: Some(enclosing),
        });
        let site = CallSiteId::from_index(index.sites.len() - 1);

        let target = match rng.below(3) {
            0 => Target::Resolved(SymbolId(rng.below(n_symbols))),
            1 => {
                let k = 1 + rng.below(3);
                let mut v: SmallVec<[SymbolId; 4]> = SmallVec::new();
                for _ in 0..k {
                    v.push(SymbolId(rng.below(n_symbols)));
                }
                Target::Candidates(v)
            }
            _ => Target::Unresolved(trellis_index::Reason::DecoratorIndirection),
        };

        index.push_edge(Edge { from: site, target });
    }

    index
}

/// The property: an edge appears in `reverse[t]` for exactly the targets it
/// names, and in `forward[s]` for its enclosing symbol — with no edge in
/// one list but not the other.
fn assert_bidirectional(index: &Index, seed: u64) {
    let n = index.symbol_count() as u32;

    for (i, edge) in index.edges.iter().enumerate() {
        // Forward: the enclosing symbol must list this edge.
        if let Some(src) = index.sites[edge.from.index()].enclosing {
            let listed = index.outgoing(src).any(|e| std::ptr::eq(e, edge));
            assert!(
                listed,
                "seed {seed}: edge {i} missing from forward[{}]",
                src.0
            );
        }

        // Reverse: every named target must list this edge.
        for &dst in edge.target.symbols() {
            let listed = index.incoming(dst).any(|e| std::ptr::eq(e, edge));
            assert!(
                listed,
                "seed {seed}: edge {i} missing from reverse[{}]",
                dst.0
            );
        }
    }

    // And nothing extra: every edge index reachable through an adjacency
    // list must genuinely name that symbol.
    for s in 0..n {
        let sym = SymbolId(s);
        for edge in index.incoming(sym) {
            assert!(
                edge.target.symbols().contains(&sym),
                "seed {seed}: reverse[{s}] lists an edge that does not target it"
            );
        }
        for edge in index.outgoing(sym) {
            assert_eq!(
                index.sites[edge.from.index()].enclosing,
                Some(sym),
                "seed {seed}: forward[{s}] lists an edge not enclosed by it"
            );
        }
    }
}

#[test]
fn forward_and_reverse_agree_over_random_graphs() {
    for seed in 1..200u64 {
        let index = arbitrary_index(seed, 1 + (seed as u32 % 24), (seed as u32 % 60) + 1);
        assert_bidirectional(&index, seed);
    }
}

#[test]
fn unresolved_edges_are_reachable_forward_but_target_nothing() {
    let index = arbitrary_index(7, 8, 40);
    let unresolved: Vec<_> = index
        .edges
        .iter()
        .filter(|e| matches!(e.target, Target::Unresolved(_)))
        .collect();

    assert!(!unresolved.is_empty(), "generator should produce some");
    for e in unresolved {
        assert!(
            e.target.symbols().is_empty(),
            "an unresolved edge must name no target: it is honest about missing, \
             not silently pointing somewhere"
        );
    }
}

#[test]
fn a_symbol_reached_by_an_uncertain_edge_is_not_partitionable() {
    let mut index = Index::new();
    index.files.push(FileEntry {
        path: "m.py".into(),
        module: "m".into(),
        hash: 0,
    });
    let a = index.interner.intern("a");
    let b = index.interner.intern("b");
    for name in [a, b] {
        index.symbols.push(SymbolEntry {
            name,
            file: FileId(0),
            span: (0, 1),
            kind: SymbolKind::Function,
        });
    }
    let (a_id, b_id) = (SymbolId(0), SymbolId(1));

    index.sites.push(CallSiteEntry {
        file: FileId(0),
        span: (0, 1),
        enclosing: Some(a_id),
    });
    index.push_edge(Edge {
        from: CallSiteId(0),
        target: Target::Resolved(b_id),
    });
    assert!(index.is_partitionable(b_id));

    index.sites.push(CallSiteEntry {
        file: FileId(0),
        span: (2, 3),
        enclosing: Some(a_id),
    });
    index.push_edge(Edge {
        from: CallSiteId(1),
        target: Target::Candidates(SmallVec::from_slice(&[b_id, a_id])),
    });
    assert!(
        !index.is_partitionable(b_id),
        "S19: one ambiguous incoming edge makes the symbol unpartitionable"
    );
}
