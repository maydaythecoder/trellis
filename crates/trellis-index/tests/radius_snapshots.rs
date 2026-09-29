//! Snapshots of the blast radius, with its coverage, over two fixtures.
//!
//! The assertions elsewhere check single facts about a radius. These pin the
//! whole report, so a change to what reaches a symbol, or to how much doubt is
//! reported beside it, shows up as a reviewed diff rather than passing
//! because no individual assertion happened to cover it.
//!
//! Each snapshot renders the reach and its `RadiusCoverage` together. ADR 0002
//! and ADR 0014 require that reach never ships without its coverage, and a
//! snapshot of the reach alone would let the doubt count drift to zero
//! without anything failing, which is the failure that matters most here.
//!
//! The rendering names symbols and call sites by module, name, relative path
//! and line rather than by arena id, and sorts every list, so the snapshot
//! depends on what the graph says and not on the order the ids were assigned
//! in or the order a `HashSet` iterated in.

use std::fmt::Write as _;
use trellis_core::{CallSiteId, SymbolId};
use trellis_index::radius::blast_radius;
use trellis_index::{Index, build_from_sources};

fn index_of(sources: &[(&str, &str)]) -> (Index, Vec<(String, String)>) {
    let owned: Vec<(String, String)> = sources
        .iter()
        .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
        .collect();
    (build_from_sources(&owned), owned)
}

/// `module.Class.name`. A method's entry carries only its bare name, so the
/// class is recovered as the innermost definition in the same file whose span
/// contains it; without that, `Shape.area` and `Square.area` would render
/// identically and a snapshot could not tell which one a radius reached.
fn symbol_name(index: &Index, s: SymbolId) -> String {
    let sym = &index.symbols[s.index()];
    let owner = index
        .symbols
        .iter()
        .filter(|o| {
            o.file == sym.file
                && o.span != sym.span
                && o.span.0 <= sym.span.0
                && sym.span.1 <= o.span.1
        })
        .min_by_key(|o| o.span.1 - o.span.0);
    let name = index.interner.resolve(sym.name);
    let module = &index.files[sym.file.index()].module;
    match owner {
        Some(o) => format!("{module}.{}.{name}", index.interner.resolve(o.name)),
        None => format!("{module}.{name}"),
    }
}

/// `path:line  call-text`, from the fixture's own sources, so the snapshot
/// reads as the Python it describes.
fn site_label(index: &Index, sources: &[(String, String)], c: CallSiteId) -> String {
    let site = &index.sites[c.index()];
    let path = &index.files[site.file.index()].path;
    let src = sources
        .iter()
        .find(|(p, _)| p == path)
        .map_or("", |(_, s)| s.as_str());
    let line = src[..site.span.0].matches('\n').count() + 1;
    let text = src[site.span.0..site.span.1].replace('\n', " ");
    format!("{path}:{line}  {text}")
}

/// Finds the root by its qualified name. Panics with the available names so
/// a fixture typo is readable rather than an index-out-of-bounds.
fn symbol_named(index: &Index, qualified: &str) -> SymbolId {
    (0..index.symbols.len())
        .map(|i| SymbolId(i as u32))
        .find(|&s| symbol_name(index, s) == qualified)
        .unwrap_or_else(|| {
            let all: Vec<String> = (0..index.symbols.len())
                .map(|i| symbol_name(index, SymbolId(i as u32)))
                .collect();
            panic!("no symbol `{qualified}`; have {all:?}")
        })
}

fn render(sources: &[(&str, &str)], root: &str) -> String {
    let (index, owned) = index_of(sources);
    let root = symbol_named(&index, root);
    let r = blast_radius(&index, root);

    let sorted = |v: Vec<String>| {
        let mut v = v;
        v.sort();
        v
    };
    let certain = sorted(
        r.certain
            .iter()
            .map(|e| site_label(&index, &owned, e.site))
            .collect(),
    );
    let ambiguous = sorted(
        r.ambiguous
            .iter()
            .map(|e| site_label(&index, &owned, e.site))
            .collect(),
    );
    let symbols = sorted(r.symbols.iter().map(|&s| symbol_name(&index, s)).collect());

    let mut out = String::new();
    let _ = writeln!(out, "root: {}", symbol_name(&index, r.root));
    let _ = writeln!(out, "partitionable: {}", r.partitionable);
    let _ = writeln!(out, "certain ({}):", certain.len());
    for s in &certain {
        let _ = writeln!(out, "  {s}");
    }
    let _ = writeln!(out, "ambiguous ({}):", ambiguous.len());
    for s in &ambiguous {
        let _ = writeln!(out, "  {s}");
    }
    let _ = writeln!(out, "symbols reached ({}):", symbols.len());
    for s in &symbols {
        let _ = writeln!(out, "  {s}");
    }

    // Every field of `RadiusCoverage`, including the two maps; they are
    // `BTreeMap`s, so their order is already a function of their keys.
    let c = &r.coverage;
    let _ = writeln!(out, "coverage:");
    let _ = writeln!(out, "  reached_resolved: {}", c.reached_resolved);
    let _ = writeln!(out, "  reached_ambiguous: {}", c.reached_ambiguous);
    let _ = writeln!(
        out,
        "  unbound_in_touched_files: {}",
        c.unbound_in_touched_files
    );
    let _ = writeln!(out, "  touched_files: {}", c.touched_files);
    let _ = writeln!(out, "  by_reason:");
    for (reason, n) in &c.by_reason {
        let _ = writeln!(out, "    {reason:?}: {n}");
    }
    let _ = writeln!(out, "  by_file:");
    for (path, n) in &c.by_file {
        let _ = writeln!(out, "    {path}: {n}");
    }
    let _ = writeln!(out, "  summary: {}", c.summary());
    out
}

#[test]
fn radius_snapshot_all_resolved_reach_reports_zero_doubt() {
    // Every edge into `core.greet` is a plain imported name, so the whole
    // radius is certain. The risk defended: a clean radius acquiring phantom
    // doubt, or losing a caller, without a reviewed change.
    let sources = [
        ("core.py", "def greet(name):\n    return name\n"),
        (
            "app.py",
            concat!(
                "from core import greet\n",
                "\n",
                "def hello():\n",
                "    return greet(\"a\")\n",
                "\n",
                "def main():\n",
                "    hello()\n",
                "    greet(\"b\")\n",
            ),
        ),
    ];
    let out = render(&sources, "core.greet");
    insta::assert_snapshot!(out);
}

#[test]
fn radius_snapshot_candidate_and_unbound_edges_carry_doubt() {
    // `shape.area()` has a declared type whose method a first-party subclass
    // overrides, so it binds to both as `Candidates` (ADR 0012). `callback()`
    // names a parameter and binds to nothing. The risk defended: a radius
    // reached through an uncertain edge reporting itself as certain, or as
    // partitionable, or with its unbound count dropping to zero (ADR 0014,
    // S19).
    let sources = [
        (
            "shapes.py",
            concat!(
                "class Shape:\n",
                "    def area(self):\n",
                "        return 0\n",
                "\n",
                "class Square(Shape):\n",
                "    def area(self):\n",
                "        return 1\n",
            ),
        ),
        (
            "report.py",
            concat!(
                "from shapes import Shape\n",
                "\n",
                "def measure(shape: Shape, callback):\n",
                "    callback()\n",
                "    return shape.area()\n",
                "\n",
                "def run(s: Shape):\n",
                "    return measure(s, print)\n",
            ),
        ),
    ];
    let out = render(&sources, "shapes.Shape.area");
    insta::assert_snapshot!(out);
}
