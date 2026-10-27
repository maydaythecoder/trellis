//! The codebase graph as data and the module-name cut its drawing uses:
//! shared by the Atlas's reads, `overview` and `index`.

use std::path::Path;

/// The module graph `codebase_json` draws: built in memory, never reading
/// or writing `.trellis/index.bin`. The index is built rather than read from
/// the cache because an interface asks for this while a repository is being
/// edited, and a cached graph would describe the tree as it stood before the
/// run the user has just watched.
fn codebase_graph(root: &Path) -> trellis_index::ModuleGraph {
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    trellis_index::module_graph(&index)
}

/// How many of a module's edges [`graph_json`] lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edges {
    /// The heaviest [`MAX_EDGES_DRAWN`](crate::index_cmd::MAX_EDGES_DRAWN),
    /// as the drawing shows them.
    Drawn,
    /// Every one, for a client that draws the whole graph itself (the
    /// desktop's Atlas, owner request of 2026-10-11).
    All,
}

/// The codebase graph for `root` as data, for rpc's `get_codebase` at a
/// commit, whose tree is copied out and never cached. The worktree's answer
/// builds its index itself, since it saves it (`atlas::codebase`, #654).
///
/// What `graph_lines` draws for the same `limit`, field for field: the
/// same modules, busiest first, and the same edges per module, with what was
/// left out counted rather than dropped. Bound, ambiguous and unresolved stay
/// separate counts with no denominator beside them (CLAUDE.md rule 3).
/// Module names are whole: cutting them to a column is the drawing's.
pub(crate) fn codebase_json(root: &Path, limit: usize, edges: Edges) -> serde_json::Value {
    graph_json(&codebase_graph(root), limit, edges)
}

/// `codebase_json`'s shape for a graph already in hand: the overview's,
/// from a cached index it must not rebuild (#558).
pub(crate) fn graph_json(
    g: &trellis_index::ModuleGraph,
    limit: usize,
    edges: Edges,
) -> serde_json::Value {
    let shown = match edges {
        Edges::Drawn => crate::index_cmd::MAX_EDGES_DRAWN,
        Edges::All => usize::MAX,
    };
    let modules: Vec<serde_json::Value> = g
        .nodes
        .iter()
        .take(limit)
        .map(|n| {
            let calls: Vec<serde_json::Value> = n
                .calls
                .iter()
                .take(shown)
                .map(|(m, c)| serde_json::json!({"module": m, "call_sites": c}))
                .collect();
            serde_json::json!({
                "module": n.module,
                // Repository-relative and `/`-separated, so a client opens
                // the file without guessing it from the dotted name, which
                // a package's `__init__.py` does not spell (#559).
                "path": n.path,
                "defines": n.defines,
                "calls": calls,
                "calls_not_drawn": n.calls.len().saturating_sub(shown),
                "ambiguous": n.ambiguous,
                "unresolved": n.unresolved,
            })
        })
        .collect();
    serde_json::json!({
        "modules_indexed": g.nodes.len(),
        "bound_call_sites": g.bound_edges(),
        "ambiguous": g.ambiguous(),
        "unresolved": g.unresolved(),
        "modules": modules,
        "modules_not_drawn": g.nodes.len().saturating_sub(limit),
    })
}

/// Cuts a module path to the column, keeping the end.
///
/// The tail is what distinguishes `poetry.console.commands.add` from
/// `poetry.console.commands.remove`; the head is shared by hundreds of them.
pub(crate) fn truncate_module(m: &str, width: usize) -> String {
    let n = m.chars().count();
    if n <= width {
        return m.to_string();
    }
    let keep = width.saturating_sub(1);
    std::iter::once('…')
        .chain(m.chars().skip(n - keep))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_cmd::{MAX_EDGES_DRAWN, graph_lines};

    fn repo(name: &str) -> crate::testutil::Scratch {
        let d = crate::testutil::scratch(&format!("init-{name}"));
        crate::testutil::git_init(&d);
        d
    }

    #[test]
    fn a_hub_module_does_not_draw_an_unbounded_number_of_edges() {
        use trellis_index::{ModuleGraph, ModuleNode};
        // `--limit` caps modules and used to cap nothing else, so one module
        // every other one imports could emit more rows than the whole rest of
        // the listing — including into the interface's transcript.
        let g = ModuleGraph {
            nodes: vec![ModuleNode {
                module: "app.cli".into(),
                path: "app/cli.py".into(),
                defines: 1,
                calls: (0..40).map(|i| (format!("app.m{i}"), 40 - i)).collect(),
                ambiguous: 0,
                unresolved: 0,
            }],
        };
        let lines = graph_lines(&g, 40);
        let edges = lines.iter().filter(|l| l.contains("─▶")).count();
        assert_eq!(edges, MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains("30 more module(s) called")),
            "and the remainder is stated, not dropped: {lines:#?}"
        );
        // The heaviest edges are the ones kept — they are sorted by count.
        assert!(lines.iter().any(|l| l.contains("app.m0")), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("app.m39")), "{lines:#?}");
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn a_module_path_too_wide_for_the_column_keeps_its_tail() {
        // The tail is what tells two siblings apart; a cut that kept the
        // head, or the wrong number of characters, prints a column of
        // identical names.
        assert_eq!(truncate_module("app.cli", 7), "app.cli");
        assert_eq!(truncate_module("app.commands.add", 6), "…s.add");
        assert_eq!(truncate_module("app.commands.add", 6).chars().count(), 6);
    }

    fn node(module: &str, calls: usize) -> trellis_index::ModuleNode {
        trellis_index::ModuleNode {
            module: module.into(),
            path: format!("{module}.py"),
            defines: 1,
            calls: (0..calls).map(|i| (format!("{module}.c{i}"), 1)).collect(),
            ambiguous: 0,
            unresolved: 0,
        }
    }

    #[test]
    fn only_a_module_s_last_drawn_edge_closes_its_branch() {
        // The risk: `└` on the wrong row, or "and 0 more" under a module
        // whose edges were all drawn, so the listing claims a remainder that
        // does not exist.
        let g = trellis_index::ModuleGraph {
            nodes: vec![node("a", 2), node("b", MAX_EDGES_DRAWN)],
        };
        let lines = graph_lines(&g, 40);
        let edges: Vec<&String> = lines.iter().filter(|l| l.contains("─▶")).collect();
        assert_eq!(edges.len(), 2 + MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(edges[0].contains("├─▶ a.c0"), "{lines:#?}");
        assert!(edges[1].contains("└─▶ a.c1"), "{lines:#?}");
        let last = MAX_EDGES_DRAWN - 1;
        assert!(
            edges[2 + last].contains(&format!("└─▶ b.c{last}")),
            "{lines:#?}"
        );
        let closed = edges.iter().filter(|l| l.contains('└')).count();
        assert_eq!(closed, 2, "{lines:#?}");
        assert!(
            !lines.iter().any(|l| l.contains("called from here")),
            "{lines:#?}"
        );
    }

    #[test]
    fn modules_past_the_limit_are_counted_and_none_past_it_are_not() {
        let g = trellis_index::ModuleGraph {
            nodes: vec![node("a", 0), node("b", 0)],
        };
        let more = |limit| {
            graph_lines(&g, limit)
                .into_iter()
                .find(|l| l.contains("not drawn"))
        };
        assert_eq!(more(2), None);
        assert_eq!(
            more(1).as_deref(),
            Some("  ... 1 more module(s), not drawn.")
        );
    }

    #[test]
    fn codebase_graph_draws_the_repository_s_own_modules() {
        let d = repo("codebase-lines");
        std::fs::create_dir_all(d.join("app")).unwrap();
        std::fs::write(d.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
        let lines = graph_lines(&codebase_graph(&d), 40);
        assert_eq!(lines[0], "codebase graph — 1 module(s)", "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("app.util")), "{lines:#?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
