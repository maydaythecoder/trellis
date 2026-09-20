//! The CLI. Thin: parses, dispatches, formats.
//!
//! Status: the surface from brief 4.12 is defined and wired to real
//! implementations where they exist (`index`'s parse pass, `doctor`'s tier
//! reporting, `check`). `run` and `resume` need the agent loop and the
//! orchestrator, which are v0.2; they exit non-zero with a clear message
//! rather than pretending.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "trellis",
    about = "An agent harness for Python repositories: blast radius mapped, \
             checks executed, decisions recorded.",
    version
)]
struct Cli {
    /// Repository root. Defaults to the current directory.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build or refresh the index; print coverage.
    Index,

    /// Blast radius for a symbol, with its confidence breakdown.
    Radius {
        /// Fully qualified symbol.
        symbol: String,
    },

    /// Run the agent loop against a task.
    Run {
        task: String,
        /// Cap concurrent agents, or `auto` to derive from measured backend
        /// batching capacity. Never derived from CPU count (S18).
        #[arg(long, default_value = "1")]
        agents: String,
    },

    /// Continue from the last trail entry.
    Resume,

    /// Run the grounded checks standalone.
    Check {
        /// Paths to check. Defaults to the whole repository.
        paths: Vec<PathBuf>,
    },

    /// Probe model, runtime, adapter tier, type checker, and tests.
    ///
    /// Not a nicety: the difference between adapter tier 3 and tier 4 is
    /// invisible to a user otherwise, and so is whether the backend can
    /// support multi-agent mode at all.
    Doctor,

    /// Dry run: show the partition plan and why.
    Partitions { task: String },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trellis=info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let root = match cli.root {
        Some(r) => r,
        None => std::env::current_dir().context("cannot determine current directory")?,
    };

    match cli.command {
        Command::Index => cmd_index(&root),
        Command::Doctor => cmd_doctor(&root),
        Command::Check { paths } => cmd_check(&root, paths),
        Command::Radius { symbol } => cmd_radius(&root, &symbol),
        Command::Run { .. } | Command::Resume | Command::Partitions { .. } => {
            anyhow::bail!(
                "not implemented yet: this command needs the agent loop (milestone v0.2). \
                 `trellis index`, `trellis radius`, `trellis check` and `trellis doctor` \
                 work today."
            )
        }
    }
}

/// Blast radius for a symbol, with its confidence breakdown.
///
/// The coverage block is printed unconditionally and the unresolved count is
/// never suppressed when it is inconvenient: a radius that omits what it
/// could not see is the failure mode the whole design exists to prevent.
fn cmd_radius(root: &Path, query: &str) -> Result<()> {
    let index = trellis_index::build(root);

    let qualified = |s: &trellis_index::SymbolEntry| {
        format!(
            "{}.{}",
            index.files[s.file.index()].module,
            index.interner.resolve(s.name)
        )
    };

    let matches: Vec<trellis_core::SymbolId> = index
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let q = qualified(s);
            q == query
                || q.ends_with(&format!(".{query}"))
                || index.interner.resolve(s.name) == query
        })
        .map(|(i, _)| trellis_core::SymbolId::from_index(i))
        .collect();

    match matches.len() {
        0 => anyhow::bail!(
            "no first-party definition matches `{query}`. \
             Note that only definitions inside the repository root are indexed."
        ),
        1 => {}
        n => {
            eprintln!("`{query}` matches {n} definitions:");
            for &m in &matches {
                let s = &index.symbols[m.index()];
                eprintln!(
                    "  {}  ({}, byte {})",
                    qualified(s),
                    index.files[s.file.index()].path,
                    s.span.0
                );
            }
            anyhow::bail!("ambiguous symbol: qualify it further");
        }
    }

    let target = matches[0];
    let report = trellis_index::radius::blast_radius(&index, target);
    let s = &index.symbols[target.index()];

    println!("{}  ({})", qualified(s), index.files[s.file.index()].path);
    println!();
    println!("  reached by {} call sites", report.site_count());
    println!("    verified-resolved  {:>6}", report.certain.len());
    println!(
        "    ambiguous          {:>6}  one of several, none chosen",
        report.ambiguous.len()
    );
    println!("  transitive symbols   {:>6}", report.symbols.len());
    println!();

    // S2: every consumer reads coverage alongside the radius, and the
    // unresolved figure here is the over-approximation from the files this
    // radius touches — see `radius::add_touched_file_misses`.
    println!("coverage");
    println!("  {}", report.coverage.summary());
    if report.coverage.unresolved > 0 {
        println!();
        println!(
            "  {} call sites in the files this radius touches could not be bound.",
            report.coverage.unresolved
        );
        println!("  Any of them may also reach this symbol. Breakdown:");
        let mut reasons: Vec<_> = report.coverage.by_reason.iter().collect();
        reasons.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (reason, n) in reasons {
            println!("      {reason:<24} {n:>6}");
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
        "This is not a complete list of callers. Roughly three call edges in ten are \
         statically\ninvisible in Python (Constraint 1); the unresolved count above is \
         what we know we missed."
    );
    Ok(())
}

/// Builds the whole index and reports coverage.
///
/// The coverage line is the product's honesty mechanism, so it is printed
/// every time and never abbreviated to a bare "complete". `resolved` is the
/// only figure that carries a blast-radius claim; `ambiguous` and
/// `unresolved` are the cost the user is asked to read.
fn cmd_index(root: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let index = trellis_index::build(root);
    let elapsed = started.elapsed();

    let c = &index.coverage;
    println!("indexed {} files in {:?}", index.files.len(), elapsed);
    println!("  definitions:     {}", index.symbols.len());
    println!("  call sites:      {}", index.sites.len());
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
        let mut reasons: Vec<_> = c.by_reason.iter().collect();
        reasons.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (reason, n) in reasons {
            println!("      {reason:<24} {n:>6} ({:>5.1}%)", pct(*n));
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
    Ok(())
}

fn cmd_doctor(root: &Path) -> Result<()> {
    use trellis_model::{AdapterTier, BatchingCapability, Probe};

    let cfg = trellis_checks::CheckConfig::default();
    let probe = Probe {
        // ASSUMPTION A-9: model and runtime detection is v0.2. Reported as
        // "not configured" rather than guessed.
        model_id: "not configured".into(),
        runtime: "not configured".into(),
        tier: AdapterTier::ValidateAndRetry,
        batching: BatchingCapability::Unknown,
        typecheck_found: which(&cfg.typecheck[0]),
        tests_found: which(&cfg.tests[0]),
    };

    for line in probe.report() {
        println!("{line}");
    }
    println!("root:       {}", root.display());
    println!();
    println!(
        "Model and runtime probing is not implemented yet (milestone v0.2). \
         The tier and batching lines above are defaults, not measurements."
    );
    Ok(())
}

fn which(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn cmd_check(root: &Path, paths: Vec<PathBuf>) -> Result<()> {
    let cfg = trellis_checks::CheckConfig::default();
    // An empty path list would invoke the checker with no arguments, which
    // most tools treat as an error rather than as "everything". Make the
    // documented default real.
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let rt = tokio::runtime::Runtime::new()?;

    let verdicts = rt.block_on(async {
        let tc = trellis_checks::typecheck(root, &cfg, &paths).await;
        let tests = trellis_checks::run_tests(root, &cfg, &paths).await;
        [tc, tests]
    });

    let mut all_passed = true;
    for v in &verdicts {
        let d = v.detail();
        println!(
            "{:?}: {} ({} ms)",
            d.kind,
            if v.passed() { "passed" } else { "FAILED" },
            d.duration_ms
        );
        println!("  command: {}", d.command);
        if !v.passed() {
            all_passed = false;
            let tail: String = d
                .stdout
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !tail.trim().is_empty() {
                println!("{tail}");
            }
            if !d.stderr.trim().is_empty() {
                println!("{}", d.stderr.trim());
            }
        }
    }

    if !all_passed {
        std::process::exit(1);
    }
    Ok(())
}
