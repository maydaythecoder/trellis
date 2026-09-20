//! Verifies every `Resolved` edge in a repository, not a sample of them.
//!
//! v1.0 gate item 2 is "zero unsound edges — every `Resolved` edge verified
//! real on the benchmark corpus". A 400-site audit cannot establish that: it
//! bounds the *rate* of unsound edges, and at 400 sites a rate low enough to
//! go unobserved still leaves dozens in a corpus of 15,000.
//!
//! This checks a necessary condition over every edge instead, mechanically:
//!
//! > the name of the symbol a `Resolved` edge points at must equal the final
//! > identifier of the call expression that produced it.
//!
//! It is necessary, not sufficient — `foo()` resolving to the wrong `foo` in
//! another module passes here and only the audit catches it. But every
//! failure it reports is unambiguously a defect: the resolver named a symbol
//! that is not what the source called. Nothing is a judgement call, so it can
//! run over the whole corpus in CI, which is exactly what the gate needs and
//! what a sample cannot give.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example check_soundness -- <root>

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: check_soundness <repository root>");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);

    let mut checked = 0u32;
    let mut violations: Vec<String> = Vec::new();
    let mut unnameable = 0u32;

    for edge in &index.edges {
        let trellis_index::Target::Resolved(sym) = &edge.target else {
            continue;
        };
        let site = &index.sites[edge.from.index()];
        let (path, src) = &sources[site.file.index()];
        let call = &src[site.span.0..site.span.1];

        let Some(called) = callee_identifier(call) else {
            // `(a or b)()` and similar: the parser classified it, but there
            // is no identifier to compare against. Counted, not judged.
            unnameable += 1;
            continue;
        };
        let resolved = index.interner.resolve(index.symbols[sym.index()].name);
        checked += 1;

        if called != resolved {
            let line = src[..site.span.0].matches('\n').count() + 1;
            violations.push(format!(
                "  {}:{}\n      source calls: `{}`\n      resolver named: `{}`",
                path,
                line,
                call.replace('\n', " ").chars().take(70).collect::<String>(),
                resolved
            ));
        }
    }

    println!("checked {checked} Resolved edges ({unnameable} had no comparable identifier)");
    if violations.is_empty() {
        println!();
        println!("No edge names a symbol the source did not call.");
        println!(
            "This is a necessary condition, not a sufficient one: an edge to the WRONG\n\
             `foo` in another module satisfies it. Only the hand audit measures that."
        );
        return;
    }

    println!();
    println!("UNSOUND EDGES: {}", violations.len());
    for v in &violations {
        println!("{v}");
    }
    std::process::exit(1);
}

/// The final identifier of a call expression's callee.
///
/// `foo(...)` -> `foo`; `a.b.c(...)` -> `c`; `d["k"](...)` -> none. Takes the
/// text before the first `(` at depth zero, then the segment after the last
/// `.`, and accepts it only when it is a bare identifier.
fn callee_identifier(call: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in call.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            '(' if depth == 0 => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let head = call[..end?].trim();
    let last = head.rsplit('.').next()?.trim();
    let is_ident = !last.is_empty()
        && !last.starts_with(|c: char| c.is_ascii_digit())
        && last.chars().all(|c| c.is_alphanumeric() || c == '_');
    is_ident.then(|| last.to_string())
}
