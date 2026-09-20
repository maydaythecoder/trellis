//! Blast radius: which first-party call sites reach a symbol.

use crate::coverage::Coverage;
use crate::graph::{Index, Reason};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use trellis_core::{CallSiteId, FileId, SymbolId};

/// What a radius found, and what it could not look at — as two separately
/// denominated facts, with no ratio between them and no way to compute one.
///
/// This type exists because there was one. `RadiusReport` used to carry the
/// repository-wide [`Coverage`], whose `resolved_fraction` divides `resolved`
/// by `resolved + candidates + unresolved`. That is correct for the index,
/// where all three count the same population: every first-party call site in
/// the repository.
///
/// It is not correct for a radius, where the two halves are filled from
/// different populations. `reached_*` counts call sites that reach *this
/// symbol*. `unbound_in_touched_files` counts every call site in the files the
/// radius touches that could not be bound *to anything*, which is an
/// over-approximation of the doubt and deliberately so. Dividing the first by
/// the second produced, for `poetry.factory.Factory`:
///
/// ```text
/// 92 of 2474 first-party call sites resolved (3.7%)
/// ```
///
/// Every number there is true and the sentence is false. It reads as the
/// resolver's reach, which the hand audit puts at 72%, and it was printed to
/// the user and handed to the model as context. A tool whose honesty mechanism
/// is a coverage line cannot afford for that line to understate itself by a
/// factor of twenty — S2 is about not implying completeness, and this is the
/// same failure inverted.
///
/// So: no `total()`, no fraction, and the two counts are named for the
/// populations they come from. See `docs/adr/0014-radius-coverage.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusCoverage {
    /// Call sites reaching the root through a single `Resolved` edge.
    pub reached_resolved: u32,
    /// Call sites naming the root as one of several `Candidates`.
    pub reached_ambiguous: u32,
    /// Call sites in the files this radius touches that bound to nothing.
    ///
    /// A different population from `reached_*`, and an over-approximation:
    /// most of these calls go somewhere else entirely. Over-approximating the
    /// doubt is the safe direction — under-reporting it is how a tool produces
    /// a blast radius missing the thing that breaks.
    pub unbound_in_touched_files: u32,
    /// How many files that count ranges over, so it has a scale. `2382` means
    /// nothing on its own; `2382 across 41 files` is a density a reader can
    /// compare against a language where roughly three call edges in ten are
    /// statically invisible.
    pub touched_files: u32,
    /// The unbound count, split by reason. Deterministically ordered.
    pub by_reason: BTreeMap<String, u32>,
    /// The unbound count, split by file path, so the doubt has somewhere to be
    /// looked at rather than only a size.
    pub by_file: BTreeMap<String, u32>,
}

impl RadiusCoverage {
    /// Call sites that reach the root at all, certain or not.
    pub fn reached(&self) -> u32 {
        self.reached_resolved + self.reached_ambiguous
    }

    /// Reasons, most frequent first, then alphabetical for stability.
    pub fn reasons_by_frequency(&self) -> Vec<(&str, u32)> {
        let mut v: Vec<(&str, u32)> = self
            .by_reason
            .iter()
            .map(|(k, n)| (k.as_str(), *n))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }

    /// Files carrying the most unbound call sites, most first.
    pub fn files_by_doubt(&self) -> Vec<(&str, u32)> {
        let mut v: Vec<(&str, u32)> = self.by_file.iter().map(|(k, n)| (k.as_str(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }

    /// Two sentences, never one ratio.
    ///
    /// There is no `summary()` that yields a single figure, because every
    /// caller that wanted one was the bug.
    pub fn summary(&self) -> String {
        if self.reached() == 0 && self.unbound_in_touched_files == 0 {
            return "no call sites examined".to_string();
        }
        format!(
            concat!(
                "{} call sites reach this symbol ({} resolved, {} ambiguous). ",
                "Separately, {} call sites across the {} files this radius ",
                "touches could not be bound to any definition; any of them may ",
                "also reach it."
            ),
            self.reached(),
            self.reached_resolved,
            self.reached_ambiguous,
            self.unbound_in_touched_files,
            self.touched_files,
        )
    }

    fn record_resolved(&mut self) {
        self.reached_resolved += 1;
    }

    fn record_ambiguous(&mut self) {
        self.reached_ambiguous += 1;
    }

    fn record_unbound(&mut self, r: Reason, path: &str) {
        self.unbound_in_touched_files += 1;
        *self.by_reason.entry(format!("{r:?}")).or_insert(0) += 1;
        *self.by_file.entry(path.to_string()).or_insert(0) += 1;
    }
}

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
    /// Deliberately **not** a [`Coverage`]. See [`RadiusCoverage`].
    pub coverage: RadiusCoverage,
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
        self.ambiguous.is_empty() && self.coverage.unbound_in_touched_files == 0
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
        coverage: RadiusCoverage::default(),
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
                    report.coverage.record_ambiguous();
                    report.partitionable = false;
                    report.ambiguous.push(RadiusEntry {
                        site: edge.from,
                        certain: false,
                    });
                }
                // Structurally unreachable: an unresolved edge names no
                // target, so it never enters `reverse`. The unresolved count
                // is therefore taken from the files the radius touches, after
                // the walk — see `add_touched_file_misses` below, which is
                // what closes OPEN_QUESTIONS Q-13. The arm stays because
                // `Target` is the authority on what an edge can be, and a
                // wildcard here would silently absorb a future variant.
                crate::graph::Target::Unresolved(_) => {
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

    add_touched_file_misses(index, &mut report);
    report
}

/// Adds the unresolved call sites in the files this radius touches.
///
/// Closes OPEN_QUESTIONS Q-13. The reverse walk cannot see an unresolved
/// edge — such an edge names no target, so it never enters `reverse` — which
/// left the radius reporting zero misses no matter how incomplete it was.
/// That is exactly backwards from what S2 requires, because the radius is
/// where the user reads coverage.
///
/// The honest substitute: an unresolved call site *in a file the radius
/// already touches* is a site that might have reached this symbol and that we
/// could not bind. It is an over-approximation — some of those calls go
/// elsewhere entirely — and over-approximating the doubt is the safe
/// direction. Under-reporting it is how a tool produces a blast radius
/// missing the thing that breaks.
fn add_touched_file_misses(index: &Index, report: &mut RadiusReport) {
    use crate::graph::Target;

    let mut touched: HashSet<FileId> = HashSet::new();
    touched.insert(index.symbols[report.root.index()].file);
    for s in &report.symbols {
        touched.insert(index.symbols[s.index()].file);
    }
    for e in report.certain.iter().chain(&report.ambiguous) {
        touched.insert(index.sites[e.site.index()].file);
    }
    // Recorded before the walk below, because it is the scale the unbound
    // count is read against and it is true whether or not anything is unbound.
    report.coverage.touched_files = touched.len() as u32;

    for edge in &index.edges {
        if let Target::Unresolved(r) = &edge.target {
            let file = index.sites[edge.from.index()].file;
            if touched.contains(&file) {
                report
                    .coverage
                    .record_unbound(*r, &index.files[file.index()].path);
                // An unbindable call in a file this radius touches means the
                // radius cannot be trusted to be disjoint from another (S19).
                report.partitionable = false;
            }
        }
    }
}

/// Records that a call site could not be bound, for the coverage figures
/// the resolver reports per run.
pub fn note_unresolved(coverage: &mut Coverage, reason: Reason) {
    coverage.record_unresolved(reason);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::build_from_sources;

    fn index(files: &[(&str, &str)]) -> Index {
        let sources: Vec<(String, String)> = files
            .iter()
            .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
            .collect();
        build_from_sources(&sources)
    }

    fn symbol(index: &Index, name: &str) -> SymbolId {
        let want = index
            .interner
            .get(name)
            .unwrap_or_else(|| panic!("{name} was never interned"));
        SymbolId(
            index
                .symbols
                .iter()
                .position(|s| s.name == want)
                .unwrap_or_else(|| panic!("no symbol named {name}")) as u32,
        )
    }

    /// The defect ADR 0014 exists for, as a test.
    ///
    /// Reach and doubt are filled from different populations: one counts call
    /// sites that reach the queried symbol, the other counts every unbindable
    /// call in the files the radius touches. The type must make it impossible
    /// to divide them, so this asserts on the two counts separately and there
    /// is deliberately no third assertion combining them.
    #[test]
    fn reach_and_doubt_are_counted_over_different_populations() {
        let i = index(&[
            ("app/target.py", "def target():\n    return 1\n"),
            (
                "app/caller.py",
                // One call that reaches `target`, and several that reach
                // nothing we can name — the shape of every real file.
                "from app.target import target\n\
                 def go(handler, obj):\n\
                 \x20   target()\n\
                 \x20   handler()\n\
                 \x20   obj.method()\n\
                 \x20   obj.other()\n",
            ),
        ]);

        let r = blast_radius(&i, symbol(&i, "target"));
        assert_eq!(r.coverage.reached_resolved, 1, "only target() reaches it");
        assert_eq!(r.coverage.reached_ambiguous, 0);
        assert_eq!(r.coverage.reached(), 1);
        assert!(
            r.coverage.unbound_in_touched_files >= 3,
            "handler() and both obj.*() calls are unbindable: {:?}",
            r.coverage.by_reason
        );
        assert!(
            r.coverage.unbound_in_touched_files > r.coverage.reached(),
            "this is the case that used to print as a 25% resolution rate"
        );
    }

    /// A count with no denominator in sight is not a fact a reader can use.
    #[test]
    fn the_doubt_count_arrives_with_the_scale_it_ranges_over() {
        let i = index(&[
            ("app/target.py", "def target():\n    return 1\n"),
            (
                "app/a.py",
                "from app.target import target\ndef a(h):\n    target()\n    h()\n",
            ),
            (
                "app/b.py",
                "from app.target import target\ndef b(h):\n    target()\n    h()\n",
            ),
        ]);

        let r = blast_radius(&i, symbol(&i, "target"));
        assert_eq!(
            r.coverage.touched_files, 3,
            "the definition's file and both callers': {:?}",
            r.coverage.by_file
        );
        assert_eq!(
            r.coverage.by_file.values().sum::<u32>(),
            r.coverage.unbound_in_touched_files,
            "the per-file split must account for every unbound site, or the \
             collapsed view in the CLI would quietly drop some"
        );
    }

    /// Ordering has to be stable or a user cannot tell a changed repository
    /// from a reshuffled report.
    #[test]
    fn reasons_and_files_are_ordered_by_frequency_then_name() {
        let mut c = RadiusCoverage::default();
        c.record_unbound(Reason::LocalBinding, "b.py");
        c.record_unbound(Reason::MethodDispatchUntyped, "a.py");
        c.record_unbound(Reason::MethodDispatchUntyped, "a.py");
        c.record_unbound(Reason::UnboundName, "c.py");

        assert_eq!(
            c.reasons_by_frequency(),
            vec![
                ("MethodDispatchUntyped", 2),
                ("LocalBinding", 1),
                ("UnboundName", 1)
            ],
            "most frequent first, then alphabetical so ties do not shuffle"
        );
        assert_eq!(
            c.files_by_doubt(),
            vec![("a.py", 2), ("b.py", 1), ("c.py", 1)]
        );
    }

    /// "We looked at nothing" and "we resolved nothing" are different claims.
    #[test]
    fn an_empty_radius_makes_no_claim() {
        let c = RadiusCoverage::default();
        assert_eq!(c.summary(), "no call sites examined");
        assert_eq!(c.reached(), 0);
    }

    /// The summary is the string a user and a model both read.
    #[test]
    fn the_summary_states_two_facts_and_no_ratio() {
        let mut c = RadiusCoverage::default();
        c.record_resolved();
        for _ in 0..40 {
            c.record_unbound(Reason::MethodDispatchUntyped, "a.py");
        }
        c.touched_files = 1;

        let s = c.summary();
        assert!(!s.contains('%'), "no percentage is computable here: {s}");
        // This string is assembled from several source lines, and a
        // backslash-continued literal gets its indentation folded in by
        // `cargo fmt` — which has silently put runs of twenty spaces into
        // user-facing output in this repository four times. `concat!` avoids
        // it; this assertion is what notices if someone switches back.
        assert!(
            !s.contains("  "),
            "formatter debris in a user-facing line: {s:?}"
        );
        assert!(s.contains("1 call sites reach this symbol"), "{s}");
        assert!(s.contains("40 call sites across the 1 files"), "{s}");
        assert!(
            s.contains("Separately"),
            "the two counts must be separated in the prose, not merely adjacent: {s}"
        );
    }
}
