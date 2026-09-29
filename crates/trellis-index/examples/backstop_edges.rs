//! Lists every edge the Pyright backstop bound, so a person can check them.
//!
//! `trellis index` prints the backstop's counts and nothing else: its edges
//! are kept out of the graph and out of `.trellis/index.bin` (ADR 0031). A
//! count cannot be hand-checked, so this prints each bound site beside the
//! definition it was bound to, and applies `check_soundness`'s necessary
//! condition to them: the callee's name must equal the identifier the call
//! calls. A mismatch is not automatically a defect here, because Pyright
//! follows aliases (`run = _run; run()`), which the resolver never binds; it
//! is printed so it gets read.
//!
//! Needs `pyright-langserver` on `PATH`. Without it the report says the
//! backstop did not run, as `trellis index` does.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example backstop_edges -- <root>

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: backstop_edges <repository root>");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    let sites = trellis_index::doubt_sites(&index);
    let report = trellis_index::run_backstop(
        &index,
        root,
        &sites,
        &trellis_index::BackstopServer::pyright(),
    );

    println!("status: {:?}", report.status);
    println!(
        "doubt sites {}  queried {}  bound {}  no answer {}  refused {}  not queried {}",
        sites.len(),
        report.queried,
        report.edges.len(),
        report.no_answer,
        report.refused_total(),
        report.not_queried
    );
    // By reason, as `trellis index` prints them: a refusal is the server
    // answering something this backstop would not bind, and each reason
    // calls for a different fix.
    for (why, n) in &report.refused {
        println!("    refused, {}: {n}", why.describe());
    }

    let line_of = |src: &str, at: usize| src[..at].matches('\n').count() + 1;
    let mut mismatched = 0u32;
    let mut names = std::collections::HashMap::new();
    for edge in &report.edges {
        let site = &index.sites[edge.from.index()];
        let (path, src) = &sources[site.file.index()];
        let call = &src[site.span.0..site.span.1];
        let sym = &index.symbols[edge.callee.index()];
        let (def_path, def_src) = &sources[sym.file.index()];
        let name = index.interner.resolve(sym.name);
        let called = callee_names(&sources, site.file, &mut names).get(&site.span);
        let mark = if called.map(String::as_str) == Some(name) {
            ""
        } else {
            mismatched += 1;
            "  NAME DIFFERS"
        };
        let target = match &index
            .edges
            .iter()
            .find(|e| e.from == edge.from)
            .map(|e| &e.target)
        {
            Some(trellis_index::Target::Candidates(c)) => format!("Candidates({})", c.len()),
            Some(_) => "Unresolved".to_string(),
            None => "?".to_string(),
        };
        println!(
            "{}:{}  {}  [{}]\n    -> {} at {}:{}{}",
            path,
            line_of(src, site.span.0),
            call.replace(['\r', '\n'], " ")
                .chars()
                .take(70)
                .collect::<String>(),
            target,
            name,
            def_path,
            line_of(def_src, sym.span.0),
            mark
        );
    }
    println!();
    println!("bound edges whose callee name differs from the call's identifier: {mismatched}");
}

/// The identifier a call site calls, from the parser's own shape, so a
/// chained call such as `Factory().create_poetry()` yields `create_poetry`
/// and not the first `(`'s head. Keyed by the site's byte span.
fn callee_names<'c>(
    sources: &[(String, String)],
    file: trellis_core::FileId,
    cache: &'c mut std::collections::HashMap<
        trellis_core::FileId,
        std::collections::HashMap<(usize, usize), String>,
    >,
) -> &'c std::collections::HashMap<(usize, usize), String> {
    cache.entry(file).or_insert_with(|| {
        let facts = trellis_index::parse_facts(&sources[file.index()].1, file);
        facts
            .call_sites
            .into_iter()
            .filter_map(|c| match c.shape {
                trellis_index::CalleeShape::Name(n) => Some((c.span, n)),
                trellis_index::CalleeShape::Attribute { attr, .. } => Some((c.span, attr)),
                trellis_index::CalleeShape::Dynamic => None,
            })
            .collect()
    })
}
