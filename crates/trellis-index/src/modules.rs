//! The codebase as a graph of modules, one level up from symbols.
//!
//! The symbol graph is the one the resolver builds and the one blast radius
//! walks. It is also, for any repository worth using this on, far too large
//! to look at: poetry has thousands of definitions and tens of thousands of
//! call sites, and a picture of all of them is not a picture of anything.
//!
//! So this collapses it to files. A node is a module; an edge means some
//! definition in one module calls some definition in another, and carries how
//! many call sites did so.
//!
//! **Two things it refuses to do**, both of which the shorter version of this
//! code did until they were caught elsewhere in this repository:
//!
//! - It never reports a fraction. Reach and doubt come from different
//!   populations, and dividing them once printed "3.7% resolved" for a
//!   resolver that hand-audits at 72% (ADR 0014). Ambiguity and unresolved
//!   call sites are counted beside the edges and never folded into them.
//! - It never claims completeness. Roughly three call edges in ten are
//!   statically invisible in Python, so a module with no outgoing edges is a
//!   module nothing was *bound* from, which is not the same as a module that
//!   calls nothing (Constraint 1). `unresolved` is how a reader tells those
//!   apart, which is why it is on every node rather than in a summary.

use crate::graph::{Index, Target};

/// One module and what was bound out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleNode {
    /// Dotted module path, e.g. `poetry.console.commands.add`.
    pub module: String,
    /// Repository-relative path, `/`-separated.
    pub path: String,
    /// Definitions in this file: functions, methods, classes.
    pub defines: u32,
    /// Modules this one was bound to call, and how many call sites did.
    ///
    /// Sorted by count descending, then by name, so the listing is stable
    /// across runs — a graph that reorders itself between two identical
    /// indexes cannot be diffed.
    pub calls: Vec<(String, u32)>,
    /// Call sites here whose callee is a known finite set rather than one
    /// definition.
    ///
    /// Counted apart from `calls` on purpose. A candidate set is evidence of
    /// a call *somewhere* in it and evidence of a call to no particular one,
    /// and adding it to an edge would assert the second.
    pub ambiguous: u32,
    /// Call sites here that bound to nothing first-party.
    ///
    /// Includes calls into third-party code, which are legitimately outside
    /// the graph, and calls the resolver could not follow, which are not. The
    /// index does not separate them here, and this field does not pretend to:
    /// it is the doubt attached to this module, whatever its source.
    pub unresolved: u32,
}

/// The codebase, as modules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleGraph {
    /// Every first-party module, including ones nothing was bound out of.
    ///
    /// Ordered by outgoing edge count descending, then by module name. A
    /// module with no edges still appears: its absence would read as the file
    /// not existing.
    pub nodes: Vec<ModuleNode>,
}

impl ModuleGraph {
    /// Total call sites bound to a first-party definition in another module.
    ///
    /// Deliberately not paired with a denominator anywhere in this type. The
    /// figure a reader wants beside it is `ambiguous` and `unresolved`, and
    /// those are three facts rather than one ratio (ADR 0014).
    pub fn bound_edges(&self) -> u32 {
        self.nodes
            .iter()
            .map(|n| n.calls.iter().map(|c| c.1).sum::<u32>())
            .sum()
    }

    pub fn ambiguous(&self) -> u32 {
        self.nodes.iter().map(|n| n.ambiguous).sum()
    }

    pub fn unresolved(&self) -> u32 {
        self.nodes.iter().map(|n| n.unresolved).sum()
    }
}

/// Collapses the symbol graph to modules.
pub fn module_graph(index: &Index) -> ModuleGraph {
    // Per file: a map from callee module to count, plus the two doubt counts.
    let mut calls: Vec<std::collections::BTreeMap<String, u32>> =
        vec![std::collections::BTreeMap::new(); index.files.len()];
    let mut ambiguous = vec![0u32; index.files.len()];
    let mut unresolved = vec![0u32; index.files.len()];
    let mut defines = vec![0u32; index.files.len()];

    for sym in &index.symbols {
        if let Some(slot) = defines.get_mut(sym.file.index()) {
            *slot += 1;
        }
    }

    for edge in &index.edges {
        let Some(site) = index.sites.get(edge.from.index()) else {
            continue;
        };
        let from = site.file.index();
        match &edge.target {
            Target::Resolved(sym) => {
                let Some(callee) = index.symbols.get(sym.index()) else {
                    continue;
                };
                // A call inside its own module is not an edge between
                // modules. Counting it would make every file look coupled to
                // itself, which says nothing and hides what does.
                if callee.file.index() == from {
                    continue;
                }
                let Some(to) = index.files.get(callee.file.index()) else {
                    continue;
                };
                if let Some(m) = calls.get_mut(from) {
                    *m.entry(to.module.clone()).or_insert(0) += 1;
                }
            }
            Target::Candidates(_) => {
                if let Some(slot) = ambiguous.get_mut(from) {
                    *slot += 1;
                }
            }
            Target::Unresolved(_) => {
                if let Some(slot) = unresolved.get_mut(from) {
                    *slot += 1;
                }
            }
        }
    }

    let mut nodes: Vec<ModuleNode> = index
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut edges: Vec<(String, u32)> =
                calls[i].iter().map(|(k, v)| (k.clone(), *v)).collect();
            edges.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            ModuleNode {
                module: f.module.clone(),
                path: f.path.clone(),
                defines: defines[i],
                calls: edges,
                ambiguous: ambiguous[i],
                unresolved: unresolved[i],
            }
        })
        .collect();

    nodes.sort_by(|a, b| {
        let (x, y) = (
            a.calls.iter().map(|c| c.1).sum::<u32>(),
            b.calls.iter().map(|c| c.1).sum::<u32>(),
        );
        y.cmp(&x).then_with(|| a.module.cmp(&b.module))
    });
    ModuleGraph { nodes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(files: &[(&str, &str)]) -> Index {
        let sources: Vec<(String, String)> = files
            .iter()
            .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
            .collect();
        crate::build_from_sources(&sources)
    }

    fn node<'a>(g: &'a ModuleGraph, module: &str) -> &'a ModuleNode {
        g.nodes
            .iter()
            .find(|n| n.module == module)
            .unwrap_or_else(|| panic!("no `{module}` in {:?}", g.nodes))
    }

    #[test]
    fn a_call_across_files_becomes_an_edge_between_modules() {
        let g = module_graph(&built(&[
            ("app/util.py", "def helper():\n    return 1\n"),
            (
                "app/main.py",
                "from app.util import helper\n\ndef go():\n    return helper()\n",
            ),
        ]));
        assert_eq!(
            node(&g, "app.main").calls,
            vec![("app.util".to_string(), 1)]
        );
        assert!(
            node(&g, "app.util").calls.is_empty(),
            "the edge has a direction"
        );
    }

    #[test]
    fn a_call_within_one_file_is_not_an_edge_between_modules() {
        // Counting it would make every file look coupled to itself, which
        // says nothing and hides what does.
        let g = module_graph(&built(&[(
            "app/main.py",
            "def helper():\n    return 1\n\ndef go():\n    return helper()\n",
        )]));
        assert!(node(&g, "app.main").calls.is_empty(), "{:?}", g.nodes);
    }

    #[test]
    fn a_module_nothing_was_bound_out_of_is_still_a_node() {
        // Its absence would read as the file not existing. `unresolved` is
        // how a reader tells "calls nothing" from "nothing was bound"
        // (Constraint 1).
        let g = module_graph(&built(&[
            ("app/lonely.py", "X = 1\n"),
            ("app/main.py", "def go():\n    return 2\n"),
        ]));
        let lonely = node(&g, "app.lonely");
        assert!(lonely.calls.is_empty());
        assert_eq!(lonely.defines, 0, "a constant is not a definition here");
    }

    #[test]
    fn a_call_that_binds_to_nothing_is_doubt_and_never_an_edge() {
        let g = module_graph(&built(&[(
            "app/main.py",
            "def go(x):\n    return x.whatever()\n",
        )]));
        let n = node(&g, "app.main");
        assert!(n.calls.is_empty(), "an unbound call is not an edge");
        assert!(n.unresolved > 0, "it is doubt, and it is counted: {n:?}");
    }

    #[test]
    fn the_three_counts_are_three_counts() {
        // There is no ratio on this type and no method that would produce
        // one. Dividing reach by doubt printed "3.7% resolved" for a resolver
        // audited at 72% (ADR 0014).
        let g = module_graph(&built(&[
            ("app/util.py", "def helper():\n    return 1\n"),
            (
                "app/main.py",
                "from app.util import helper\n\ndef go(x):\n    helper()\n    return x.thing()\n",
            ),
        ]));
        assert_eq!(g.bound_edges(), 1);
        assert!(g.unresolved() >= 1);
        // Distinct accessors that a caller has to read separately. If this
        // ever becomes one number, the constraint has been lost.
        assert_ne!(g.bound_edges(), g.unresolved() + g.bound_edges());
    }

    #[test]
    fn the_ordering_is_stable_across_identical_indexes() {
        // A graph that reorders itself between two identical indexes cannot
        // be diffed, and a diff is most of what a codebase graph is for.
        let files: &[(&str, &str)] = &[
            ("app/a.py", "def a():\n    return 1\n"),
            ("app/b.py", "def b():\n    return 1\n"),
            (
                "app/main.py",
                "from app.a import a\nfrom app.b import b\n\ndef go():\n    a()\n    b()\n",
            ),
        ];
        let first = module_graph(&built(files));
        let second = module_graph(&built(files));
        assert_eq!(first, second);
        // Ties break by name, so `app.a` precedes `app.b` in main's edges.
        assert_eq!(
            node(&first, "app.main").calls,
            vec![("app.a".to_string(), 1), ("app.b".to_string(), 1)]
        );
    }

    #[test]
    fn an_empty_repository_is_an_empty_graph_rather_than_a_panic() {
        let g = module_graph(&Index::default());
        assert!(g.nodes.is_empty());
        assert_eq!(g.bound_edges(), 0);
        assert_eq!(g.ambiguous(), 0);
        assert_eq!(g.unresolved(), 0);
    }
}
