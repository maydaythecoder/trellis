//! Building the same sources twice must produce the same graph.
//!
//! Recorded as C-003 in CORRECTNESS_LEDGER.md: the property was relied on and
//! never asserted.
//!
//! Two things depend on it. `.trellis/index.bin` is keyed by a fingerprint of
//! the file set, so a cached index is returned for any later build over the
//! same files — if the graph were not a function of those files alone, the
//! cache would serve a different answer from the one a fresh build gives, and
//! the difference would show up as a blast radius that changed without the
//! code changing. And every recorded radius in a trail is read back later
//! against an index built again from scratch.
//!
//! The property holds because `build_from_sources` uses
//! `par_iter().map().collect()`, which rayon documents as order-preserving,
//! and because `discover` sorts before assigning ids. Both of those are one
//! edit away from not being true, and one of them is a dependency's guarantee
//! rather than ours. That is what makes this worth a test rather than a
//! comment.

use trellis_index::build_from_sources;

/// Enough files, with enough in them, that rayon genuinely splits the work.
///
/// A two-file corpus would be parsed on one thread and the test would pass
/// without exercising anything. Each module defines a class and calls into
/// the one before it, so the bind step has real cross-file work to do and the
/// symbol arena is filled from several threads at once.
fn corpus(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|i| {
            let prev = i.saturating_sub(1);
            (
                format!("pkg/mod_{i}.py"),
                format!(
                    concat!(
                        "from pkg.mod_{prev} import Widget{prev}\n\n",
                        "class Widget{i}:\n",
                        "    def build(self, other):\n",
                        "        w = Widget{prev}()\n",
                        "        return w.build(other) + other.unknown()\n\n",
                        "def make_{i}(arg):\n",
                        "    return Widget{i}().build(arg)\n"
                    ),
                    i = i,
                    prev = prev
                ),
            )
        })
        .collect()
}

#[test]
fn the_graph_is_a_function_of_the_sources_alone() {
    let sources = corpus(64);
    let a = build_from_sources(&sources);
    let b = build_from_sources(&sources);

    // Serialising covers the private adjacency lists too, which is the part
    // most likely to diverge: `forward` and `reverse` are built by pushing
    // edge indices, so an edge vector in a different order would produce the
    // same edge *set* and different adjacency.
    //
    // Compared as text rather than as a `Value`, and the difference is the
    // point. `to_value` normalises map key order, so it would have reported
    // two builds identical while the bytes written to `.trellis/index.bin`
    // differed — which is exactly what the first version of this test did,
    // and exactly the defect it was meant to catch.
    let (ja, jb) = (
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
    );
    assert_eq!(
        ja, jb,
        "two builds over identical sources serialised differently; \
         the index cache is keyed on the file set and would serve one of them"
    );
}

#[test]
fn the_serialised_form_is_reproducible_to_the_byte() {
    // The property `.trellis/index.bin` actually needs. Content equality is
    // not enough: the cache is a file, and a file that differs on every
    // rebuild of unchanged sources cannot be compared, hashed, or reasoned
    // about as evidence that nothing moved.
    //
    // This failed before the interner stopped serialising its lookup map.
    let sources = corpus(64);
    let a = serde_json::to_vec(&build_from_sources(&sources)).unwrap();
    let b = serde_json::to_vec(&build_from_sources(&sources)).unwrap();
    assert_eq!(
        a.len(),
        b.len(),
        "same sources, different serialised length"
    );
    assert!(
        a == b,
        "the serialised index is not byte-reproducible over identical sources"
    );
}

#[test]
fn ids_are_positional_so_a_reordered_build_is_a_different_graph() {
    // The other half of the property, and the reason the first test is not
    // vacuous. Ids are assigned by position, so order *does* matter — the
    // guarantee is that rayon preserves the order it was given, not that the
    // graph is order-independent.
    //
    // Asserting this pins the direction of the invariant. If someone were to
    // make ids content-addressed instead, this test failing is the signal
    // that `discover`'s sort has stopped being load-bearing and the comment
    // saying so should go.
    let sources = corpus(16);
    let mut reordered = sources.clone();
    reordered.reverse();

    let a = build_from_sources(&sources);
    let b = build_from_sources(&reordered);

    assert_eq!(
        a.files.len(),
        b.files.len(),
        "the same files, in either order"
    );
    assert_ne!(
        a.files[0].path, b.files[0].path,
        "file ids follow input order; `discover` sorting is what makes that stable"
    );
    assert_eq!(
        a.coverage.resolved, b.coverage.resolved,
        "order decides ids, not what resolves"
    );
}

#[test]
fn repeated_builds_agree_on_coverage_to_the_last_call_site() {
    // The coverage counts are what the CLI prints and what the gate compares
    // against. A build that varied here would make a recall figure
    // irreproducible, and `BENCHMARK_AND_EVAL.md` is built on the assumption
    // that a seed and a corpus determine a number.
    let sources = corpus(32);
    let first = build_from_sources(&sources);
    for _ in 0..3 {
        let again = build_from_sources(&sources);
        assert_eq!(first.coverage.resolved, again.coverage.resolved);
        assert_eq!(first.coverage.candidates, again.coverage.candidates);
        assert_eq!(first.coverage.unresolved, again.coverage.unresolved);
        assert_eq!(first.coverage.external, again.coverage.external);
        assert_eq!(
            first.fingerprint, again.fingerprint,
            "the cache key must not move under a rebuild of the same sources"
        );
    }
}
