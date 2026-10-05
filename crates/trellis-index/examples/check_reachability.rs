//! The third mechanical condition of the v1.0 precision audit: for every
//! cross-file `Resolved` edge, does an import path actually connect the two
//! files?
//!
//! `check_soundness` verifies that the resolver named the symbol the source
//! called. It says so itself: *"an edge to the WRONG `foo` in another module
//! satisfies it."* This is the check that reaches that case, mechanically,
//! over every edge rather than a sample.
//!
//! # Four buckets, and only one is a violation
//!
//! - **connected** — an import in the calling file resolves to the module
//!   defining the target, directly or through one re-export hop. The hop is
//!   not optional: a package's `__init__.py` re-exporting its submodules is
//!   the normal shape of Python, and without it the check reports a flood of
//!   false violations.
//! - **inherited** — a `self.m()` call whose target is a method on a base
//!   class two or more links up. The calling file imports only its immediate
//!   base, so the connection is verified by walking the class hierarchy
//!   rather than an import.
//! - **ambiguous** — a star import, a relative import naming no file in the
//!   corpus, or a re-export deeper than one hop. Mechanical verification
//!   cannot settle these and the tool does not pretend to. Counted, listed,
//!   and handed to the hand audit.
//! - **disconnected** — every import resolved and none reaches the target.
//!   Each one is either an unsound edge or a gap in this check, and both need
//!   a fix plus a regression test. Never a threshold adjustment.
//!
//! # What this check is worth, by callee shape
//!
//! **It is close to circular for `Name` edges and independent for `Attribute`
//! edges, and the split is reported because of that.**
//!
//! A `Name` call (`foo()`) is resolved by a scope walk that consults the
//! calling file's imports. If it produced a cross-file edge, it did so through
//! an import, so finding that import again establishes little. An `Attribute`
//! call (`obj.method()`) is resolved through the receiver's *type* — ADR 0012
//! — which never consults the calling file's imports for the method. There an
//! import connecting the files is a genuinely separate fact, and that is
//! exactly where the risk lives: CLAUDE.md records that every receiver-typing
//! rule can only add resolutions, so every one is a new chance to emit an
//! unsound edge.
//!
//! Reporting a single pooled "0 disconnected" would let the circular half
//! borrow credibility for the independent half.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example check_reachability -- <root>

use std::collections::{HashMap, HashSet};
use std::path::Path;
use trellis_core::FileId;
use trellis_index::{CalleeKind, SymbolKind, Target};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: check_reachability <repository root>");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let sources = trellis_index::read_sources(root);
    if sources.is_empty() {
        eprintln!("no Python sources under {}", root.display());
        std::process::exit(2);
    }
    let facts: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, (_, src))| trellis_index::parse_facts(src, FileId::from_index(i)))
        .collect();
    let index = trellis_index::build_from_facts(&sources, &facts);

    // module path -> the file defining it.
    let mut module_map: HashMap<&str, usize> = HashMap::new();
    for (i, f) in index.files.iter().enumerate() {
        module_map.insert(f.module.as_str(), i);
    }

    // Per file: the absolute modules it imports that name a file in this
    // corpus, and whether anything about its imports is beyond this check.
    let mut imports_of: Vec<HashSet<usize>> = vec![HashSet::new(); sources.len()];
    let mut opaque: Vec<bool> = vec![false; sources.len()];
    for (i, f) in facts.iter().enumerate() {
        let pkg = &index.files[i].module;
        for imp in &f.imports {
            if imp.is_star {
                // `from x import *` — the resolver reports StarImport rather
                // than guessing, and neither does this.
                opaque[i] = true;
            }
            let base = trellis_index::absolute_module(&imp.module, imp.level, pkg);
            let mut hit = false;
            if let Some(&t) = module_map.get(base.as_str()) {
                imports_of[i].insert(t);
                hit = true;
            }
            // `from pkg import mod` imports a module, not a name in one.
            for (name, _alias) in &imp.names {
                let sub = if base.is_empty() {
                    name.clone()
                } else {
                    format!("{base}.{name}")
                };
                if let Some(&t) = module_map.get(sub.as_str()) {
                    imports_of[i].insert(t);
                    hit = true;
                }
            }
            // A relative import that names no file here is either third-party
            // or a layout this check does not model. Either way it cannot be
            // ruled out as the connection.
            if !hit && imp.level > 0 {
                opaque[i] = true;
            }
        }
    }

    let mut counts: HashMap<(CalleeKind, &'static str), u32> = HashMap::new();
    let mut disconnected: Vec<String> = Vec::new();
    let mut ambiguous_sample: Vec<String> = Vec::new();
    let mut same_file = 0u32;

    for edge in &index.edges {
        let Target::Resolved(sym) = &edge.target else {
            continue;
        };
        let site = &index.sites[edge.from.index()];
        let from = site.file.index();
        let to = index.symbols[sym.index()].file.index();
        if from == to {
            same_file += 1;
            continue;
        }

        // Direct import, or one re-export hop. The hop is not optional: a
        // package's `__init__.py` re-exporting its submodules is the normal
        // shape of Python, and without it this reports 740 false violations
        // on the benchmark corpus.
        let direct_or_reexport = imports_of[from].contains(&to)
            || imports_of[from]
                .iter()
                .any(|&hop| imports_of[hop].contains(&to));
        let bucket = if direct_or_reexport {
            "connected"
        } else if is_self_receiver(&sources[from].1, site.span)
            && inherits_to(
                from,
                to,
                site.span,
                index.interner.resolve(index.symbols[sym.index()].name),
                &facts,
                &imports_of,
            )
        {
            // `self.m()` reaching a method defined on a base class two or more
            // links up. The calling file imports its immediate base and
            // nothing further, so no import of *its* names the target — the
            // connection is the class hierarchy, and that is what gets
            // verified instead of an import.
            "inherited"
        } else if opaque[from] {
            "ambiguous"
        } else {
            "disconnected"
        };
        *counts.entry((site.kind, bucket)).or_default() += 1;

        if bucket == "ambiguous" || bucket == "disconnected" {
            let (path, src) = &sources[from];
            let line = src[..site.span.0].matches('\n').count() + 1;
            let call: String = src[site.span.0..site.span.1]
                .replace('\n', " ")
                .chars()
                .take(60)
                .collect();
            let entry = format!(
                "  {}:{}  {:?}  `{}`\n      -> {} in {}",
                path,
                line,
                site.kind,
                call,
                index.interner.resolve(index.symbols[sym.index()].name),
                sources[to].0
            );
            if bucket == "disconnected" {
                disconnected.push(entry);
            } else if ambiguous_sample.len() < 25 {
                ambiguous_sample.push(entry);
            }
        }
    }

    let get = |k: CalleeKind, b: &str| *counts.get(&(k, b)).unwrap_or(&0);
    println!("Import reachability over every cross-file Resolved edge");
    println!("{}", "=".repeat(62));
    println!("{same_file} same-file edges need no import and are not counted below.\n");
    println!(
        "{:<12} {:>10} {:>10} {:>10} {:>13}",
        "shape", "connected", "inherited", "ambiguous", "DISCONNECTED"
    );
    let mut totals = [0u32; 4];
    for kind in [CalleeKind::Name, CalleeKind::Attribute, CalleeKind::Dynamic] {
        let row = [
            get(kind, "connected"),
            get(kind, "inherited"),
            get(kind, "ambiguous"),
            get(kind, "disconnected"),
        ];
        if row.iter().all(|&x| x == 0) {
            continue;
        }
        for (t, r) in totals.iter_mut().zip(row) {
            *t += r;
        }
        println!(
            "{:<12} {:>10} {:>10} {:>10} {:>13}",
            format!("{kind:?}"),
            row[0],
            row[1],
            row[2],
            row[3]
        );
    }
    println!(
        "{:<12} {:>10} {:>10} {:>10} {:>13}",
        "total", totals[0], totals[1], totals[2], totals[3]
    );

    println!(
        "\n`Name` is close to circular: a scope walk that produced a cross-file edge\n\
         did so through an import, so finding it again establishes little. `Attribute`\n\
         resolves through the receiver's type and never consults the calling file's\n\
         imports, so there the connection is an independent fact. Read the rows, not\n\
         the total."
    );

    if !ambiguous_sample.is_empty() {
        println!(
            "\nAmbiguous, first {} of {} (star imports, or relative imports naming no\n\
             file in this corpus). Mechanical verification cannot settle these; they\n\
             are the hand-audit residue, not findings:",
            ambiguous_sample.len(),
            totals[2]
        );
        for e in &ambiguous_sample {
            println!("{e}");
        }
    }

    if disconnected.is_empty() {
        println!("\nNo cross-file Resolved edge lacks an import path connecting it.");
        return;
    }
    println!("\nDISCONNECTED: {}", disconnected.len());
    for e in &disconnected {
        println!("{e}");
    }
    std::process::exit(1);
}

/// Whether the call's receiver is `self` or `cls`.
///
/// Read from the source rather than inferred: these are the two receivers
/// whose type comes from the enclosing class rather than from a binding, so
/// they are the only ones whose connection can be an inheritance chain.
fn is_self_receiver(src: &str, span: (usize, usize)) -> bool {
    let call = &src[span.0..span.1];
    call.starts_with("self.") || call.starts_with("cls.")
}

/// Whether `to` is reachable from the class enclosing `span` by following base
/// classes, with every link import-connected and **nothing shadowing the
/// method on the way**.
///
/// This is the soundness condition for an inherited `self.m()` written out:
/// the enclosing class really does descend from the class defining the method,
/// each step of that descent is one the calling file could see, and no class
/// between the two defines `m` itself.
///
/// The shadowing guard is rule 1 of the audit protocol in `sample.json`:
/// *"`self.m()` where `m` is defined in the enclosing class resolves to THAT
/// definition. Only record an inherited base-class method when the enclosing
/// class does not define `m` itself."* Without it this blesses exactly the
/// edge the protocol calls wrong — a resolver that walked past a nearer
/// definition — which is the failure an audit of Resolved edges exists to
/// catch.
///
/// Bounded at eight links, deeper than any hierarchy in the benchmark corpus;
/// an unbounded walk over a corpus with a cycle would not terminate.
fn inherits_to(
    from: usize,
    to: usize,
    span: (usize, usize),
    method: &str,
    facts: &[trellis_index::FileFacts],
    imports_of: &[HashSet<usize>],
) -> bool {
    let Some(start) = enclosing_class(&facts[from], span) else {
        return false;
    };
    // The enclosing class defining `m` itself means the edge should have
    // pointed here, wherever else it points.
    if defines(&facts[from], start, method) {
        return false;
    }
    let mut frontier = vec![(from, start.to_string())];
    let mut seen: HashSet<(usize, String)> = HashSet::new();
    for _ in 0..8 {
        let mut next = Vec::new();
        for (file, class) in frontier {
            if !seen.insert((file, class.clone())) {
                continue;
            }
            let Some(def) = facts[file]
                .defs
                .iter()
                .find(|d| d.kind == SymbolKind::Class && d.name == class && d.scope.is_empty())
            else {
                continue;
            };
            for base in &def.bases {
                // `a.b.C` and `C` both name the class `C`; which module it
                // came from is decided by the import, checked next.
                let name = base.rsplit('.').next().unwrap_or(base);
                for &cand in imports_of[file].iter().chain(std::iter::once(&file)) {
                    if !facts[cand]
                        .defs
                        .iter()
                        .any(|d| d.kind == SymbolKind::Class && d.name == name)
                    {
                        continue;
                    }
                    if defines(&facts[cand], name, method) {
                        // A definition here. It verifies the edge only if it
                        // is the one the edge names; otherwise the resolver
                        // walked past a nearer definition and this path does
                        // not vouch for it.
                        if cand == to {
                            return true;
                        }
                        continue;
                    }
                    next.push((cand, name.to_string()));
                }
            }
        }
        if next.is_empty() {
            return false;
        }
        frontier = next;
    }
    false
}

/// Whether class `class` in `facts` defines a method named `method`.
fn defines(facts: &trellis_index::FileFacts, class: &str, method: &str) -> bool {
    facts
        .defs
        .iter()
        .any(|d| d.name == method && d.scope.len() == 1 && d.scope[0] == class)
}

/// The innermost class whose body contains `span`.
fn enclosing_class(facts: &trellis_index::FileFacts, span: (usize, usize)) -> Option<&str> {
    facts
        .defs
        .iter()
        .filter(|d| d.kind == SymbolKind::Class && d.span.0 <= span.0 && span.1 <= d.span.1)
        .max_by_key(|d| d.span.0)
        .map(|d| d.name.as_str())
}
