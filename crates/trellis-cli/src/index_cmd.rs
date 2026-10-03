use crate::backstop_lines;
use crate::config;
use crate::doctor_cmd::truncate_module;
use anyhow::Result;
use std::path::Path;

/// Blast radius for a symbol, with its confidence breakdown.
///
/// The coverage block is printed unconditionally and the unresolved count is
/// never suppressed when it is inconvenient: a radius that omits what it
/// could not see is the failure mode the whole design exists to prevent.
pub(crate) fn cmd_radius(root: &Path, query: &str) -> Result<()> {
    let index = trellis_index::build_cached(root);
    radius_of(&index, query)
}

/// The radius report, over an index the caller already holds.
///
/// Split out so the interactive session can answer from an index built once
/// per session rather than once per question. The two paths must not drift:
/// a session that renders a radius differently from `trellis radius` would be
/// two products wearing one name.
/// The partition plan for a set of symbols, without running anything.
///
/// `05-v1.1-multi-agent.md` asks for this early — "a debugging tool you will
/// use constantly" — because every other part of multi-agent work is debugged
/// through it.
///
/// **It takes symbols, not a task sentence.** The milestone's sketch is
/// `trellis partitions "<task>"`, and turning prose into work units needs a
/// model able to decompose a task. None is available above the floor, and
/// waiting for one would mean the planner stayed undebuggable. Naming the
/// symbols is what a user would have to do to check the plan anyway.
pub(crate) fn cmd_partitions(root: &Path, symbols: &[String]) -> Result<()> {
    let index = trellis_index::build_cached(root);

    let mut units = Vec::with_capacity(symbols.len());
    for q in symbols {
        let target = resolve_symbol(&index, q)?;
        units.push(trellis_loop::partition::unit_for(&index, target));
    }

    let plan = trellis_loop::partition::plan(&index, units);
    print!("{}", trellis_loop::partition::explain(&index, &plan));

    // The agent count is bounded by the backend, not by the plan. Saying so
    // here rather than only in `doctor` because this is where someone forms
    // an expectation about how much faster their run will be (S18).
    println!();
    println!(
        "{}",
        concat!(
            "How many of these actually run at once is the serving backend's ",
            "decision,
not this plan's: N agents against a runtime without ",
            "continuous batching queue
on one GPU. `trellis doctor --model ",
            "<spec>` reports which you have."
        )
    );
    Ok(())
}

/// The fully qualified name of a symbol, as every surface shows it.
fn qualified(index: &trellis_index::Index, s: &trellis_index::SymbolEntry) -> String {
    format!(
        "{}.{}",
        index.files[s.file.index()].module,
        index.interner.resolve(s.name)
    )
}

/// Finds the one symbol a query names, or explains why it cannot.
///
/// Extracted from `radius_of` when `partitions` needed the same matching.
/// A second copy would have been a second set of rules about what a user may
/// type, and the two would have drifted the first time either was relaxed.
///
/// Ambiguity is an error rather than a first match: choosing silently between
/// two definitions with the same name would put a blast radius — or a
/// partition boundary — on a symbol the user did not mean.
fn resolve_symbol(index: &trellis_index::Index, query: &str) -> Result<trellis_core::SymbolId> {
    let matches: Vec<trellis_core::SymbolId> = index
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let q = qualified(index, s);
            q == query
                || q.ends_with(&format!(".{query}"))
                || index.interner.resolve(s.name) == query
        })
        .map(|(i, _)| trellis_core::SymbolId::from_index(i))
        .collect();

    match matches.as_slice() {
        [] => anyhow::bail!(
            "no first-party definition matches `{query}`.              Note that only definitions inside the repository root are indexed."
        ),
        [one] => Ok(*one),
        many => {
            eprintln!("`{query}` matches {} definitions:", many.len());
            for &m in many {
                let s = &index.symbols[m.index()];
                eprintln!(
                    "  {}  ({}, byte {})",
                    qualified(index, s),
                    index.files[s.file.index()].path,
                    s.span.0
                );
            }
            anyhow::bail!("ambiguous symbol: qualify it further")
        }
    }
}

pub(crate) fn radius_of(index: &trellis_index::Index, query: &str) -> Result<()> {
    let qualified = |s: &trellis_index::SymbolEntry| qualified(index, s);
    let target = resolve_symbol(index, query)?;

    let report = trellis_index::radius::blast_radius(index, target);
    let s = &index.symbols[target.index()];

    println!("{}  ({})", qualified(s), index.files[s.file.index()].path);
    println!();
    // S6. Two blocks, never one ratio.
    //
    // This printed "92 of 2474 first-party call sites resolved (3.7%)" until
    // ADR 0014. The numerator counted sites reaching the symbol; the
    // denominator counted every unbindable call in the files the radius
    // touches. Both true, the sentence false, and it read as the resolver
    // being twenty times worse than the audit says it is.
    println!("reach — call sites that reach this symbol");
    println!(
        "  {:>6}  resolved     bound here by a single edge",
        report.coverage.reached_resolved
    );
    println!(
        "  {:>6}  ambiguous    this symbol is one of several candidates, none chosen",
        report.coverage.reached_ambiguous
    );
    println!(
        "  {:>6}  symbols reached transitively",
        report.symbols.len()
    );
    println!();

    let c = &report.coverage;
    println!("doubt — a different population, not a fraction of the reach");
    if c.unbound_in_touched_files == 0 {
        println!(
            "  {:>6}  every call site in the {} files this radius touches was bound",
            0, c.touched_files
        );
    } else {
        println!(
            "  {:>6}  call sites across the {} files this radius touches bound to \
             nothing.",
            c.unbound_in_touched_files, c.touched_files
        );
        println!("          Any of them may also reach this symbol.");
        println!();

        // Collapsed, with the residue counted rather than dropped. A screen a
        // user closes is a screen that stopped being honest; a stable "and N
        // more" keeps the whole figure in view while fitting on one.
        const SHOWN: usize = 3;
        let reasons = c.reasons_by_frequency();
        for (reason, n) in reasons.iter().take(SHOWN) {
            // `explain()` directly: the reason arrives typed now, so there is
            // no name to look back up and no second list of variants to keep
            // in step with the enum.
            println!("          {reason:<22} {n:>6}   {}", reason.explain());
        }
        if reasons.len() > SHOWN {
            let rest: u32 = reasons.iter().skip(SHOWN).map(|(_, n)| n).sum();
            println!(
                "          {:<22} {:>6}   across {} further reasons",
                "(other)",
                rest,
                reasons.len() - SHOWN
            );
        }

        let files = c.files_by_doubt();
        if files.len() > 1 {
            println!();
            println!("          concentrated in:");
            for (path, n) in files.iter().take(5) {
                println!("            {n:>6}  {path}");
            }
            if files.len() > 5 {
                let rest: u32 = files.iter().skip(5).map(|(_, n)| n).sum();
                println!(
                    "            {rest:>6}  across {} further files",
                    files.len() - 5
                );
            }
        }
    }
    println!();
    println!(
        "  partitionable: {}  {}",
        report.partitionable,
        if report.partitionable {
            "(every edge involved was resolved)"
        } else {
            "(reached through an uncertain edge; must not be split across agents — S19)"
        }
    );
    println!();
    println!(
        "{}",
        concat!(
            "The two figures above count different things and must not be divided. The\n",
            "first is call sites that reach this symbol; the second is every unbindable\n",
            "call in the files the radius touches, which over-counts on purpose.\n",
            "\n",
            "This is not a complete list of callers. Roughly three call edges in ten are\n",
            "statically invisible in Python (Constraint 1), so treat the resolved list as\n",
            "a floor, not a boundary."
        )
    );
    if c.unbound_in_touched_files > 0 {
        println!();
        println!(
            "{}",
            concat!(
                "To shrink the doubt here, add a test that exercises this symbol. A call\n",
                "site covered by an executed check moves from Reviewed to Verified — the\n",
                "only option that leaves the repository better than it found it."
            )
        );
    }
    Ok(())
}

/// Builds the whole index and reports coverage.
///
/// The coverage line is the product's honesty mechanism, so it is printed
/// every time and never abbreviated to a bare "complete". `resolved` is the
/// only figure that carries a blast-radius claim; `ambiguous` and
/// `unresolved` are the cost the user is asked to read.
pub(crate) fn cmd_index(root: &Path, graph: bool, limit: usize) -> Result<()> {
    // `index` always rebuilds — it is the command whose job is to refresh
    // the cache, so reading the cache here would make it a no-op.
    // Read before the build, so a misspelt `[index] backstop` stops here
    // rather than after a long index that ran without the help it asked for.
    let backstop = config::RepoConfig::load(root)?.pyright_backstop()?;
    let started = std::time::Instant::now();
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    let elapsed = started.elapsed();

    let cache_path = root.join(".trellis").join("index.bin");
    let cache_note = match trellis_index::cache::save(&index, &cache_path) {
        Ok(bytes) => format!("{:.1} KiB", bytes as f64 / 1024.0),
        Err(e) => format!("not written ({e})"),
    };

    let c = &index.coverage;
    println!("indexed {} files in {:?}", index.files.len(), elapsed);
    println!("  definitions:     {}", index.symbols.len());
    println!("  call sites:      {}", index.sites.len());
    println!("  index.bin:       {cache_note}");
    println!("  first-party:     {} (the recall denominator)", c.total());
    println!(
        "  external:        {} (excluded: stdlib, site-packages)",
        c.external
    );
    println!();
    println!("{}", c.summary());

    if let Some(f) = c.resolved_fraction() {
        println!();
        println!("  resolved         {:>6} ({:>5.1}%)", c.resolved, f * 100.0);
        let pct = |n: u32| f64::from(n) / f64::from(c.total()) * 100.0;
        println!(
            "  ambiguous        {:>6} ({:>5.1}%)  known finite set, none chosen",
            c.candidates,
            pct(c.candidates)
        );
        println!(
            "  unresolved       {:>6} ({:>5.1}%)",
            c.unresolved,
            pct(c.unresolved)
        );
        let mut reasons: Vec<_> = c
            .by_reason
            .iter()
            .map(|(r, n)| (*r, *n))
            .collect::<Vec<_>>();
        reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (reason, n) in reasons {
            println!("      {reason:<24} {n:>6} ({:>5.1}%)", pct(n));
        }

        // BENCHMARK_AND_EVAL.md §2: a pooled figure hides the case that
        // matters. `Name` and `Attribute` have different fallback decisions.
        println!();
        println!("by callee shape");
        println!(
            "  {:<12} {:>8} {:>10} {:>10} {:>10}",
            "shape", "bound", "of 1st-party", "unresolved", "external"
        );
        for (shape, sc) in &index.by_shape {
            let bound = sc.resolved_fraction().map_or_else(
                || "     n/a".to_string(),
                |v| format!("{:>7.1}%", v * 100.0),
            );
            println!(
                "  {:<12} {:>8} {:>10} {:>10} {:>10}",
                shape,
                bound,
                sc.total(),
                sc.unresolved,
                sc.external
            );
        }
    }

    println!();
    println!(
        "Recall is measured over call sites whose true callee is first-party \
         (BENCHMARK_AND_EVAL.md §2).\nThe figure above is what the resolver bound, \
         not a verified recall figure: it counts\nagreement with itself. Only the \
         hand-audited sample measures recall."
    );

    println!();
    for line in backstop_lines(&index, root, backstop) {
        println!("{line}");
    }

    if graph {
        println!();
        for line in graph_lines(&trellis_index::module_graph(&index), limit) {
            println!("{line}");
        }
    }
    Ok(())
}

/// The codebase as a graph of modules.
///
/// Drawn the way the other graphs in this project are drawn — nodes with
/// converging edges — because the alternative is a table, and a table of
/// module names is the thing a graph exists instead of.
///
/// Every row carries its own doubt. A module with no outgoing edges is one
/// nothing was *bound* out of, which is not the same as one that calls
/// nothing; `?` is how a reader tells those apart (Constraint 1). The three
/// counts stay three counts and are never divided (ADR 0014).
/// How many outgoing edges one module draws before it says how many it left.
///
/// Ten, because past that the row stops being a shape and becomes a list, and
/// because the point of the picture is which modules a module leans on rather
/// than an exhaustive inventory — `trellis radius` answers the exhaustive
/// question.
pub(crate) const MAX_EDGES_DRAWN: usize = 10;

pub(crate) fn graph_lines(g: &trellis_index::ModuleGraph, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if g.nodes.is_empty() {
        out.push("codebase graph: no first-party modules were indexed.".into());
        return out;
    }

    out.push(format!("codebase graph — {} module(s)", g.nodes.len()));
    out.push(format!(
        "  {} call site(s) bound across modules, {} ambiguous, {} unresolved",
        g.bound_edges(),
        g.ambiguous(),
        g.unresolved()
    ));
    out.push("  three counts, not a ratio: they come from different populations.".into());
    out.push(String::new());

    let width = g
        .nodes
        .iter()
        .take(limit)
        .map(|n| n.module.chars().count())
        .max()
        .unwrap_or(0)
        .min(48);

    for n in g.nodes.iter().take(limit) {
        // The node, then its edges. An arrow per callee, indented under the
        // module they leave, so the shape reads down the page.
        let doubt = match (n.ambiguous, n.unresolved) {
            (0, 0) => String::new(),
            (a, 0) => format!("   ~{a}"),
            (0, u) => format!("   ?{u}"),
            (a, u) => format!("   ~{a} ?{u}"),
        };
        let glyph = if n.calls.is_empty() { '○' } else { '●' };
        out.push(format!(
            "  {glyph} {:<width$}  {:>3} def{}",
            truncate_module(&n.module, width),
            n.defines,
            doubt
        ));
        // Bounded per node, not only per graph. `--limit` caps how many
        // modules are drawn and used to cap nothing else, so a single hub —
        // a `utils` every module imports, a CLI that calls into all of them —
        // could emit as many rows as the whole rest of the listing. That also
        // reaches the interface, where `/codebase` streams these into the
        // transcript.
        //
        // Edges are sorted by count, so the ones kept are the heaviest, and
        // the remainder is stated rather than dropped.
        let shown = n.calls.len().min(MAX_EDGES_DRAWN);
        for (i, (callee, count)) in n.calls.iter().take(shown).enumerate() {
            let last = i + 1 == shown && n.calls.len() == shown;
            out.push(format!(
                "    {}─▶ {callee}  ×{count}",
                if last { '└' } else { '├' }
            ));
        }
        if n.calls.len() > shown {
            out.push(format!(
                "    └─  and {} more module(s) called from here, not drawn.",
                n.calls.len() - shown
            ));
        }
    }

    if g.nodes.len() > limit {
        out.push(String::new());
        out.push(format!(
            "  ... {} more module(s), not drawn.",
            g.nodes.len() - limit
        ));
    }
    out.push(String::new());
    out.push("  ● bound out of   ○ nothing bound out of   ~ambiguous   ?unresolved".into());
    out.push("  An edge is a call site the resolver bound to a definition in another".into());
    out.push("  module. Absence of an edge is absence of a binding, not absence of a call.".into());
    out
}
