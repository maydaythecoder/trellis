//! Why work units are unpartitionable, broken down by cause.
//!
//! `docs/bench/decomposition-20260921.md` measured 98.1% of symbols
//! partitionable against 2.9% of units, and named
//! `add_touched_file_misses` as the difference. This says which of that
//! function's three sources of "touched" is doing it, because a narrowing
//! has to target the right one.
//!
//! That narrowing has landed (ADR 0016): the root's own file no longer
//! disqualifies a unit, so its row is shown for comparison only. The
//! breakdown counts unpartitionable units alone.
use std::collections::HashSet;
use trellis_core::{FileId, SymbolId};
use trellis_index::Target;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = trellis_index::build_cached(std::path::Path::new(&args[1]));
    let n = index.symbols.len();
    let step = (n / 400).max(1);

    // Files holding a call site that bound to nothing.
    let mut dirty: HashSet<FileId> = HashSet::new();
    for e in &index.edges {
        if matches!(e.target, Target::Unresolved(_)) {
            dirty.insert(index.sites[e.from.index()].file);
        }
    }
    println!(
        "{} of {} files contain at least one unbound call site\n",
        dirty.len(),
        index.files.len()
    );

    let (mut sym_ok, mut unit_ok, mut sampled) = (0usize, 0usize, 0usize);
    let (mut by_root, mut by_radius, mut by_caller) = (0usize, 0usize, 0usize);

    for i in (0..n).step_by(step) {
        let id = SymbolId::from_index(i);
        sampled += 1;
        if index.is_partitionable(id) {
            sym_ok += 1;
        }
        let r = trellis_index::radius::blast_radius(&index, id);
        if r.partitionable {
            unit_ok += 1;
            continue;
        }

        // Which source of `touched` contributes a dirty file to a unit that
        // is unpartitionable.
        if dirty.contains(&index.symbols[id.index()].file) {
            by_root += 1;
        }
        if r.symbols
            .iter()
            .any(|s| dirty.contains(&index.symbols[s.index()].file))
        {
            by_radius += 1;
        }
        if r.certain
            .iter()
            .chain(&r.ambiguous)
            .any(|e| dirty.contains(&index.sites[e.site.index()].file))
        {
            by_caller += 1;
        }
    }

    let pct = |x: usize| 100.0 * x as f64 / sampled as f64;
    println!("sampled {sampled} symbols (every {step}th of {n})");
    println!(
        "  symbol partitionable (S19's stated rule): {sym_ok:>4} ({:.1}%)",
        pct(sym_ok)
    );
    println!(
        "  UNIT partitionable (radius report):       {unit_ok:>4} ({:.1}%)",
        pct(unit_ok)
    );
    println!();
    println!(
        "Which source of `touched` supplies a file with an unbound call, over the {} \
         unpartitionable units (% of all sampled):",
        sampled - unit_ok
    );
    println!(
        "  the root's own file:            {by_root:>4} ({:.1}%)  not disqualifying since ADR 0016",
        pct(by_root)
    );
    println!(
        "  a file defining a radius symbol:{by_radius:>4} ({:.1}%)",
        pct(by_radius)
    );
    println!(
        "  a file holding a caller site:   {by_caller:>4} ({:.1}%)",
        pct(by_caller)
    );
}
