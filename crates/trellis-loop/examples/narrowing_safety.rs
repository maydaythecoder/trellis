//! Does narrowing the touched-file test admit a collision the broad rule
//! refused?
//!
//! # The threat model, stated precisely
//!
//! Two partitions A and B are declared compatible: disjoint radii, disjoint
//! file sets. The danger S19 names is a collision through an edge the index
//! never saw.
//!
//! A single unbound call site has exactly one true callee, so one site cannot
//! be a shared caller of both A and B. The real shape is asymmetric:
//!
//! > an unbound call site X, living in a file **partition B will edit**,
//! > whose true callee is a symbol in **A's radius**.
//!
//! Then A changes what X calls while B edits the file X is in. Both partitions
//! pass locally and the integrated result is wrong — which is exactly the
//! failure the milestone calls the most dangerous interaction in the design.
//!
//! # What is computable
//!
//! X's true callee is unknown; that is what "unbound" means. What *is*
//! computable is whether it could be in A's radius at all: a call in B's file
//! can only reach A's code if B's file can see A's module. So:
//!
//! > **plausible collision** = an unbound site in one partition's files whose
//! > file imports — directly or through one re-export hop — a file defining a
//! > symbol in the other partition's radius.
//!
//! This over-approximates: importing a module is not calling into it. A count
//! of zero is therefore a real result, and a count above zero is an upper
//! bound on the risk rather than a list of bugs.
//!
//! Usage:
//!   cargo run --release -p trellis-loop --example narrowing_safety -- <root> [pairs] [seed]

use std::collections::{HashMap, HashSet};
use trellis_core::{FileId, SymbolId};
use trellis_index::{Index, Target};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: narrowing_safety <repository root> [pairs] [seed]");
        std::process::exit(2);
    }
    let root = std::path::Path::new(&args[1]);
    let index = trellis_index::build_cached(root);
    let pairs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20260921);

    let imports = import_closure(root, &index);
    let dirty = dirty_files(&index);
    let n = index.symbols.len();
    if n == 0 {
        eprintln!("no symbols under {}", root.display());
        std::process::exit(2);
    }

    let mut rng = seed;
    let (mut compatible, mut newly, mut plausible) = (0usize, 0usize, 0usize);
    let mut examples: Vec<String> = Vec::new();

    for _ in 0..pairs {
        let a = SymbolId::from_index((next(&mut rng) % n as u64) as usize);
        let b = SymbolId::from_index((next(&mut rng) % n as u64) as usize);
        if a == b {
            continue;
        }
        let (ra, rb) = (
            trellis_index::radius::blast_radius(&index, a),
            trellis_index::radius::blast_radius(&index, b),
        );
        let (fa, fb) = (files_of(&index, &ra, a), files_of(&index, &rb, b));
        let (sa, sb) = (symbols_of(&ra, a), symbols_of(&rb, b));

        // Compatible under the narrowed rule: both units partitionable, and
        // radii and file sets disjoint.
        if !(ra.partitionable && rb.partitionable) {
            continue;
        }
        if !sa.is_disjoint(&sb) || !fa.is_disjoint(&fb) {
            continue;
        }
        compatible += 1;

        // Would the broad rule have refused this pair? It refuses whenever
        // either root's own file holds an unbound call.
        let broad_refused = dirty.contains(&index.symbols[a.index()].file)
            || dirty.contains(&index.symbols[b.index()].file);
        if !broad_refused {
            continue;
        }
        newly += 1;

        if let Some(why) = plausible_collision(&index, &imports, &fa, &sb, &fb, &sa) {
            plausible += 1;
            if examples.len() < 5 {
                examples.push(why);
            }
        }
    }

    println!("Narrowing safety — {pairs} random symbol pairs, seed {seed}\n");
    println!("  compatible under the narrowed rule:      {compatible}");
    println!("  of those, the broad rule refused:        {newly}");
    println!("  of those, a plausible collision exists:  {plausible}");
    if newly > 0 {
        println!(
            "\n  newly admitted pairs carrying any plausible collision: {:.2}%",
            100.0 * plausible as f64 / newly as f64
        );
    }
    for e in &examples {
        println!("    {e}");
    }
    println!(
        "\n{}",
        concat!(
            "A plausible collision is an unbound call site in one partition's ",
            "files whose\nfile imports a file defining a symbol in the other's ",
            "radius. Importing a\nmodule is not calling into it, so this ",
            "over-approximates: zero is a real\nresult, and non-zero is an ",
            "upper bound rather than a list of defects.\n\n",
            "Either way the integrated check across the union still runs. ",
            "Partitioning has\nnever been a correctness guarantee and this ",
            "does not make it one."
        )
    );
}

/// An unbound site in `edit_files` that could reach `other_radius`.
fn plausible_collision(
    index: &Index,
    imports: &HashMap<FileId, HashSet<FileId>>,
    a_files: &HashSet<FileId>,
    b_symbols: &HashSet<SymbolId>,
    b_files: &HashSet<FileId>,
    a_symbols: &HashSet<SymbolId>,
) -> Option<String> {
    let reach = |edit: &HashSet<FileId>, targets: &HashSet<SymbolId>| -> Option<String> {
        let target_files: HashSet<FileId> = targets
            .iter()
            .map(|s| index.symbols[s.index()].file)
            .collect();
        for edge in &index.edges {
            if !matches!(edge.target, Target::Unresolved(_)) {
                continue;
            }
            let f = index.sites[edge.from.index()].file;
            if !edit.contains(&f) {
                continue;
            }
            let seen = imports.get(&f)?;
            if let Some(hit) = target_files.iter().find(|t| seen.contains(t)) {
                return Some(format!(
                    "{} holds an unbound call and imports {}",
                    index.files[f.index()].path,
                    index.files[hit.index()].path
                ));
            }
        }
        None
    };
    reach(b_files, a_symbols).or_else(|| reach(a_files, b_symbols))
}

fn symbols_of(r: &trellis_index::RadiusReport, root: SymbolId) -> HashSet<SymbolId> {
    let mut s: HashSet<SymbolId> = r.symbols.iter().copied().collect();
    s.insert(root);
    s
}

fn files_of(index: &Index, r: &trellis_index::RadiusReport, root: SymbolId) -> HashSet<FileId> {
    let mut f = HashSet::new();
    f.insert(index.symbols[root.index()].file);
    for s in &r.symbols {
        f.insert(index.symbols[s.index()].file);
    }
    for e in r.certain.iter().chain(&r.ambiguous) {
        f.insert(index.sites[e.site.index()].file);
    }
    f
}

fn dirty_files(index: &Index) -> HashSet<FileId> {
    index
        .edges
        .iter()
        .filter(|e| matches!(e.target, Target::Unresolved(_)))
        .map(|e| index.sites[e.from.index()].file)
        .collect()
}

/// File to the files it imports, plus one re-export hop.
///
/// The same rule `examples/check_reachability.rs` verifies edges with: a
/// package's `__init__.py` re-exporting its submodules is the normal shape of
/// Python, and without the hop this under-counts what a file can see — which
/// for a safety test is the wrong direction to be wrong in.
///
/// Index paths are relative to `root`, and an unreadable file is fatal: read
/// as empty it would drop that file's imports and bias the count toward zero.
fn import_closure(root: &std::path::Path, index: &Index) -> HashMap<FileId, HashSet<FileId>> {
    let sources = index
        .files
        .iter()
        .map(|f| {
            let p = root.join(&f.path);
            let src = std::fs::read_to_string(&p).unwrap_or_else(|e| {
                eprintln!("cannot read {}: {e}", p.display());
                std::process::exit(1);
            });
            (f.path.clone(), src)
        })
        .collect::<Vec<_>>();

    let mut module_map: HashMap<&str, FileId> = HashMap::new();
    for (i, f) in index.files.iter().enumerate() {
        module_map.insert(f.module.as_str(), FileId::from_index(i));
    }

    let mut direct: HashMap<FileId, HashSet<FileId>> = HashMap::new();
    for (i, (_, src)) in sources.iter().enumerate() {
        let id = FileId::from_index(i);
        let facts = trellis_index::parse_facts(src, id);
        let pkg = &index.files[i].module;
        let mut seen = HashSet::new();
        for imp in &facts.imports {
            let base = trellis_index::absolute_module(&imp.module, imp.level, pkg);
            if let Some(&t) = module_map.get(base.as_str()) {
                seen.insert(t);
            }
            for (name, _) in &imp.names {
                let sub = if base.is_empty() {
                    name.clone()
                } else {
                    format!("{base}.{name}")
                };
                if let Some(&t) = module_map.get(sub.as_str()) {
                    seen.insert(t);
                }
            }
        }
        direct.insert(id, seen);
    }

    let mut out = direct.clone();
    for (id, seen) in &direct {
        let mut all = seen.clone();
        for hop in seen {
            if let Some(h) = direct.get(hop) {
                all.extend(h);
            }
        }
        out.insert(*id, all);
    }
    out
}

fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
