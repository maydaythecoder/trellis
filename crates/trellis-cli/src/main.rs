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
        Command::Radius { .. }
        | Command::Run { .. }
        | Command::Resume
        | Command::Partitions { .. } => {
            anyhow::bail!(
                "not implemented yet: this command needs the resolver and the agent loop \
                 (milestone v0.2). `trellis index`, `trellis check` and `trellis doctor` work today."
            )
        }
    }
}

/// Pass 1 across the repository, reporting what it found.
///
/// Resolution (Pass 2 and the bind step) is v0.1 work, so this reports
/// counts rather than a coverage figure. It deliberately does not print a
/// percentage it cannot yet compute.
fn cmd_index(root: &Path) -> Result<()> {
    use rayon::prelude::*;

    let files: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| x == "py"))
        .filter(|p| !p.components().any(|c| c.as_os_str() == ".git"))
        .collect();

    let started = std::time::Instant::now();
    let facts: Vec<_> = files
        .par_iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let src = std::fs::read_to_string(path).ok()?;
            Some(trellis_index::parse::collect(
                &src,
                trellis_core::FileId::from_index(i),
            ))
        })
        .collect();

    let defs: usize = facts.iter().map(|f| f.defs.len()).sum();
    let imports: usize = facts.iter().map(|f| f.imports.len()).sum();
    let sites: usize = facts.iter().map(|f| f.call_sites.len()).sum();
    let decorated: usize = facts
        .iter()
        .flat_map(|f| &f.defs)
        .filter(|d| !d.decorators.is_empty())
        .count();
    let star: usize = facts
        .iter()
        .flat_map(|f| &f.imports)
        .filter(|i| i.is_star)
        .count();
    let dynamic: usize = facts
        .iter()
        .flat_map(|f| &f.call_sites)
        .filter(|c| matches!(c.shape, trellis_index::CalleeShape::Dynamic))
        .count();

    println!("parsed {} files in {:?}", files.len(), started.elapsed());
    println!("  definitions:     {defs} ({decorated} decorated)");
    println!("  imports:         {imports} ({star} star imports)");
    println!("  call sites:      {sites} ({dynamic} syntactically dynamic)");
    println!();
    println!(
        "Resolution is not implemented yet (milestone v0.1), so no coverage figure \
         is reported.\nCounts above are Pass 1 only."
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
