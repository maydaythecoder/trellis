//! Incremental update: reparse one file, rebuild the graph.
//!
//! The agent loop edits files mid-run, so this sits in the inner loop and its
//! latency is the ceiling on how responsive the loop can feel.
//!
//! **What is incremental and what is not.** Parsing is: the changed file's
//! `Tree` is edited and reparsed, so tree-sitter touches only the affected
//! byte ranges, and no other file is reparsed at all. Resolution is **not**:
//! the graph is rebuilt from the cached per-file facts.
//!
//! That split is deliberate rather than unfinished. Symbol ids are dense
//! indices into one arena, so changing a file's definition count shifts every
//! later id — a partial re-resolve would have to either keep a stable id space
//! with per-file slack, or renumber and patch every edge referring to a moved
//! symbol. Both are real designs and both are a correctness risk in the
//! structure the whole product's claims rest on.
//!
//! The measured trade says it is not worth taking yet: on the benchmark
//! repository a full build is ~120 ms, of which parsing is ~32 ms and
//! resolution ~88 ms. Skipping 450 reparses to redo an 88 ms resolve is a
//! real saving on a cheap operation. When resolution stops being cheap — S9
//! puts that somewhere above 300k lines — the stable-id design is the next
//! thing to build, and `docs/01-v0.1-measure.md` records the number that
//! should trigger it.

use crate::graph::Index;
use crate::parse::FileFacts;
use std::path::Path;
use tree_sitter::{InputEdit, Point, Tree};

/// A repository held open across edits.
///
/// Owns the sources, their parse trees, and their extracted facts, so an edit
/// costs one reparse rather than a directory walk and 450 of them.
pub struct Session {
    sources: Vec<(String, String)>,
    trees: Vec<Option<Tree>>,
    facts: Vec<FileFacts>,
    index: Index,
}

impl Session {
    /// Opens a repository, doing the full build once.
    #[tracing::instrument(skip_all, fields(root = %root.display()))]
    pub fn open(root: &Path) -> Self {
        Self::from_sources(crate::discover::read_sources(root))
    }

    pub fn from_sources(sources: Vec<(String, String)>) -> Self {
        use rayon::prelude::*;

        // Parsed in parallel like the cold path. Retaining the trees is what
        // makes the session worth opening, but it must not cost three times a
        // cold build to do it — a `Tree` is `Send`, so there is no reason to
        // give up rayon for the privilege.
        let (facts, trees): (Vec<_>, Vec<_>) = sources
            .par_iter()
            .map(|(_, src)| crate::parse::collect_with_tree(src, None))
            .unzip();
        let index = crate::resolve::build_from_facts(&sources, &facts);
        Self {
            sources,
            trees,
            facts,
            index,
        }
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn file_count(&self) -> usize {
        self.sources.len()
    }

    /// Applies a new version of one file and rebuilds the graph.
    ///
    /// Returns `false` when the path is not part of this session — adding and
    /// removing files changes the module map and the file-id space, so it is
    /// a reopen, not an update, and pretending otherwise would silently
    /// produce a graph that disagrees with the filesystem.
    #[tracing::instrument(skip_all, fields(path = rel_path))]
    pub fn try_update(&mut self, rel_path: &str, new_src: &str) -> bool {
        let Some(i) = self.sources.iter().position(|(p, _)| p == rel_path) else {
            return false;
        };

        // Describe the change to tree-sitter so it can reuse the unchanged
        // subtrees either side of it.
        if let Some(tree) = self.trees[i].as_mut() {
            tree.edit(&input_edit(&self.sources[i].1, new_src));
        }
        let (f, t) = crate::parse::collect_with_tree(new_src, self.trees[i].as_ref());
        self.sources[i].1 = new_src.to_string();
        self.facts[i] = f;
        self.trees[i] = t;

        self.index = crate::resolve::build_from_facts(&self.sources, &self.facts);
        true
    }
}

/// Derives a single `InputEdit` from the old and new text.
///
/// An editor knows exactly what changed; a file-save does not, so the change
/// is recovered as the span between the common prefix and the common suffix.
/// That is an over-approximation when a file is edited in two distant places
/// at once — correctness is unaffected, since tree-sitter reparses at least
/// the reported range, and it is still far less than the whole file.
fn input_edit(old: &str, new: &str) -> InputEdit {
    let ob = old.as_bytes();
    let nb = new.as_bytes();

    let mut start = 0;
    let max_prefix = ob.len().min(nb.len());
    while start < max_prefix && ob[start] == nb[start] {
        start += 1;
    }
    // Do not let the suffix scan run back past the prefix.
    let mut suffix = 0;
    let max_suffix = max_prefix - start;
    while suffix < max_suffix && ob[ob.len() - 1 - suffix] == nb[nb.len() - 1 - suffix] {
        suffix += 1;
    }

    let old_end = ob.len() - suffix;
    let new_end = nb.len() - suffix;

    InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_at(old, start),
        old_end_position: point_at(old, old_end),
        new_end_position: point_at(new, new_end),
    }
}

/// Row and column of a byte offset, counted the way tree-sitter counts.
fn point_at(text: &str, byte: usize) -> Point {
    let upto = &text.as_bytes()[..byte.min(text.len())];
    let row = upto.iter().filter(|&&c| c == b'\n').count();
    let col = match upto.iter().rposition(|&c| c == b'\n') {
        Some(nl) => byte - nl - 1,
        None => byte,
    };
    Point { row, column: col }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<(String, String)> {
        vec![
            (
                "app/util.py".to_string(),
                "def helper():\n    return 1\n".to_string(),
            ),
            (
                "app/main.py".to_string(),
                "from app.util import helper\n\ndef go():\n    return helper()\n".to_string(),
            ),
        ]
    }

    #[test]
    fn an_edit_is_reflected_in_the_graph() {
        let mut s = Session::from_sources(sources());
        assert_eq!(s.index().coverage.resolved, 1, "go calls helper");

        // Add a second call to the same function.
        let ok = s.try_update(
            "app/main.py",
            "from app.util import helper\n\ndef go():\n    helper()\n    return helper()\n",
        );
        assert!(ok);
        assert_eq!(
            s.index().coverage.resolved,
            2,
            "the new call site must appear without reopening the repository"
        );
    }

    #[test]
    fn an_edit_in_one_file_changes_resolution_in_another() {
        // The reason resolution is rebuilt rather than patched per file:
        // deleting a definition invalidates edges that live somewhere else.
        let mut s = Session::from_sources(sources());
        assert_eq!(s.index().coverage.resolved, 1);

        s.try_update("app/util.py", "def renamed():\n    return 1\n");
        assert_eq!(
            s.index().coverage.resolved,
            0,
            "main.py's call must stop resolving when util.py drops the definition"
        );
        assert_eq!(s.index().coverage.unresolved, 1);
    }

    #[test]
    fn an_incremental_rebuild_equals_a_cold_build() {
        // The property that makes the cache trustworthy: an updated session
        // must be indistinguishable from opening the edited repository fresh.
        let edited = "from app.util import helper\n\ndef go():\n    return helper()\n\ndef second():\n    return helper()\n";

        let mut s = Session::from_sources(sources());
        s.try_update("app/main.py", edited);

        let mut cold_sources = sources();
        cold_sources[1].1 = edited.to_string();
        let cold = crate::build_from_sources(&cold_sources);

        assert_eq!(s.index().coverage, cold.coverage);
        assert_eq!(s.index().edges.len(), cold.edges.len());
        assert_eq!(s.index().symbols.len(), cold.symbols.len());
        assert_eq!(s.index().fingerprint, cold.fingerprint);
    }

    #[test]
    fn updating_an_unknown_path_is_refused() {
        let mut s = Session::from_sources(sources());
        assert!(
            !s.try_update("app/new_file.py", "def x():\n    pass\n"),
            "adding a file changes the module map and the id space; that is a \
             reopen, not an update"
        );
    }

    #[test]
    fn the_derived_edit_spans_only_what_changed() {
        let e = input_edit("abc\ndef\nghi\n", "abc\nXYZ\nghi\n");
        assert_eq!(e.start_byte, 4, "common prefix is `abc\\n`");
        assert_eq!(e.old_end_byte, 7);
        assert_eq!(e.new_end_byte, 7);
        assert_eq!(e.start_position, Point { row: 1, column: 0 });
    }

    #[test]
    fn the_derived_edit_handles_pure_insertion_and_deletion() {
        let ins = input_edit("ab", "aXb");
        assert_eq!(ins.start_byte, 1);
        assert_eq!(ins.old_end_byte, 1, "nothing was removed");
        assert_eq!(ins.new_end_byte, 2);

        let del = input_edit("aXb", "ab");
        assert_eq!(del.start_byte, 1);
        assert_eq!(del.old_end_byte, 2);
        assert_eq!(del.new_end_byte, 1, "nothing was added");

        // Identical input must not claim a change.
        let same = input_edit("abc", "abc");
        assert_eq!(same.start_byte, 3);
        assert_eq!(same.old_end_byte, 3);
        assert_eq!(same.new_end_byte, 3);
    }
}
