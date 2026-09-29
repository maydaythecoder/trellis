//! Snapshots of whole partition plans over two small fixtures.
//!
//! `partition_invariants.rs` checks properties every plan must have. These pin
//! what one plan actually is, so a change to which units run apart, or to
//! which ones are held back in the shared stage, shows up as a reviewed diff
//! rather than passing because no property happened to forbid it. Rule 6
//! (unresolved and candidate edges are unpartitionable, S19) is the change
//! that matters most to see.
//!
//! `Plan` derives only `Debug`, and its partitions and units hold `HashSet`s,
//! so a `{:?}` snapshot would depend on hash iteration order. The rendering
//! here names every symbol by `module.Class.name` and every file by its
//! relative path, and sorts every collection, so the snapshot depends on what
//! the plan says and not on arena ids or hashing.

use std::fmt::Write as _;
use trellis_core::SymbolId;
use trellis_index::{Index, build_from_sources};
use trellis_loop::partition::{Partition, plan, unit_for};

fn index_of(sources: &[(&str, &str)]) -> Index {
    let owned: Vec<(String, String)> = sources
        .iter()
        .map(|(p, s)| ((*p).to_string(), (*s).to_string()))
        .collect();
    build_from_sources(&owned)
}

/// `module.Class.name`. A method's entry carries only its bare name, so the
/// class is recovered as the innermost definition in the same file whose span
/// contains it; without that, `Shape.area` and `Square.area` would render
/// identically and the snapshot could not tell which unit went where.
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

/// Finds a target by its qualified name. Panics with the available names so
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

fn render_partition(out: &mut String, index: &Index, stage: &str, p: &Partition) {
    let mut units: Vec<String> = p
        .units
        .iter()
        .map(|u| {
            format!(
                "{}{}",
                symbol_name(index, u.target),
                if u.partitionable {
                    ""
                } else {
                    "  (unpartitionable)"
                }
            )
        })
        .collect();
    units.sort();
    let mut radius: Vec<String> = p.radius.iter().map(|&s| symbol_name(index, s)).collect();
    radius.sort();
    let mut files: Vec<String> = p
        .files
        .iter()
        .map(|f| index.files[f.index()].path.clone())
        .collect();
    files.sort();

    // The id is the plan's own ordinal (shared first, then by smallest
    // target), not an arena offset, so it is stable for a fixed fixture.
    let _ = writeln!(
        out,
        "partition {} ({stage}, conservative: {}):",
        p.id.0, p.conservative
    );
    let _ = writeln!(out, "  units ({}):", units.len());
    for u in &units {
        let _ = writeln!(out, "    {u}");
    }
    let _ = writeln!(out, "  radius ({}):", radius.len());
    for r in &radius {
        let _ = writeln!(out, "    {r}");
    }
    let _ = writeln!(out, "  files ({}):", files.len());
    for f in &files {
        let _ = writeln!(out, "    {f}");
    }
}

fn render(sources: &[(&str, &str)], targets: &[&str]) -> String {
    let index = index_of(sources);
    let units = targets
        .iter()
        .map(|t| unit_for(&index, symbol_named(&index, t)))
        .collect();
    let p = plan(&index, units);

    let mut out = String::new();
    let _ = writeln!(out, "width: {}", p.width());
    match &p.shared {
        Some(s) => render_partition(&mut out, &index, "shared", s),
        None => {
            let _ = writeln!(out, "shared: none");
        }
    }
    // `plan` already orders `parallel` by smallest target id, which is fixed
    // by the fixture's source order; the ids printed make that order visible.
    for par in &p.parallel {
        render_partition(&mut out, &index, "parallel", par);
    }
    out
}

#[test]
fn partition_snapshot_resolved_edges_only_join_callers_and_split_the_rest() {
    // Every call here is a plain imported name, so every edge is resolved and
    // no unit is held back. `use_alpha.call_f` is in `alpha.f`'s radius, so
    // those two units share a partition; `beta.g` touches nothing of theirs
    // and runs beside them. The risk defended: two units joined by a certain
    // edge being split apart, or a clean plan acquiring a shared stage,
    // without a reviewed change.
    let sources = [
        ("alpha.py", "def f():\n    return 1\n"),
        ("beta.py", "def g():\n    return 2\n"),
        (
            "use_alpha.py",
            "from alpha import f\n\ndef call_f():\n    return f()\n",
        ),
        (
            "use_beta.py",
            "from beta import g\n\ndef call_g():\n    return g()\n",
        ),
    ];
    let out = render(&sources, &["alpha.f", "beta.g", "use_alpha.call_f"]);
    insta::assert_snapshot!(out);
}

#[test]
fn partition_snapshot_candidate_edge_holds_its_reach_in_the_shared_partition() {
    // `shape.area()` has a declared type whose method a first-party subclass
    // overrides, so it binds to both as `Candidates` (ADR 0012). Both `area`
    // units are therefore unpartitionable and go to the one conservative
    // partition (rule 6, S19). `report.run` is clean itself but edits
    // `report.py`, which the shared stage already holds, so it waits there
    // too. `util.tidy` shares nothing and stays parallel. The risk defended:
    // a unit reached through a candidate edge being planned as if it were
    // disjoint from the rest.
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
                "def measure(shape: Shape):\n",
                "    return shape.area()\n",
                "\n",
                "def run(s: Shape):\n",
                "    return measure(s)\n",
            ),
        ),
        ("util.py", "def tidy():\n    return 3\n"),
        (
            "main.py",
            "from util import tidy\n\ndef go():\n    return tidy()\n",
        ),
    ];
    let out = render(
        &sources,
        &[
            "shapes.Shape.area",
            "shapes.Square.area",
            "report.run",
            "util.tidy",
        ],
    );
    insta::assert_snapshot!(out);
}
