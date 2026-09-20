//! The CLI. Thin: parses, dispatches, formats.
//!
//! Status: `index`, `radius`, `check`, `doctor` and `run` are wired to real
//! implementations. `run` currently drives the loop with a scripted model —
//! real files, real checks, real rollback, and only judgement stubbed — which
//! is what lets v0.2's exit condition be demonstrated before inference exists.
//! `resume` needs the trail (v0.3) and `partitions` needs multi-agent planning
//! (v1.1); both exit non-zero with a clear message rather than pretending.

mod config;

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
        /// Which model drives the loop:
        ///
        ///   `scripted:<path>`            a fixed JSON script
        ///   `llamacpp:http://host:port`  llama.cpp server
        ///   `vllm:http://host:port`      vLLM OpenAI-compatible server
        ///   `sglang:http://host:port`    SGLang server
        ///
        /// A scripted run exercises the real loop, real files, real checks and
        /// real rollback — everything except judgement. Results from it are
        /// never a model floor (S7).
        #[arg(long)]
        model: String,
        /// Send no schema and validate the reply instead (tier 4).
        ///
        /// Only for a runtime that does not honour guided decoding. It is
        /// materially worse — a run can fail on format alone — so it is opt-in
        /// rather than a silent fallback.
        #[arg(long)]
        unconstrained: bool,
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
    Doctor {
        /// Probe this runtime, e.g. `llamacpp:http://127.0.0.1:8080`.
        ///
        /// Without it, tier and batching are reported as unknown rather than
        /// guessed — the pessimistic default, because an optimistic guess is
        /// exactly the failure S17 and S18 describe.
        #[arg(long)]
        model: Option<String>,
    },

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
        Command::Doctor { model } => cmd_doctor(&root, model.as_deref()),
        Command::Check { paths } => cmd_check(&root, paths),
        Command::Radius { symbol } => cmd_radius(&root, &symbol),
        Command::Run {
            task,
            model,
            unconstrained,
            agents,
        } => cmd_run(&root, &task, &model, unconstrained, &agents),
        Command::Resume | Command::Partitions { .. } => {
            anyhow::bail!(
                "not implemented yet: `resume` needs the trail (milestone v0.3) and \
                 `partitions` needs multi-agent planning (v1.1). `trellis index`, \
                 `trellis radius`, `trellis run`, `trellis check` and `trellis doctor` \
                 work today."
            )
        }
    }
}

/// What drives the loop this run.
///
/// Not a trait object: `Model::next_step` is an `async fn` in a trait, which
/// is not dyn-compatible, and the loop is generic over it.
enum Driver {
    Scripted(trellis_loop::ScriptedModel),
    Runtime(trellis_loop::RuntimeModel),
}

impl Driver {
    fn parse(spec: &str, unconstrained: bool) -> Result<Self> {
        if let Some(path) = spec.strip_prefix("scripted:") {
            if unconstrained {
                anyhow::bail!("--unconstrained is meaningless for a scripted model");
            }
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read script {path}"))?;
            let m = trellis_loop::ScriptedModel::from_json(&text)
                .map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
            return Ok(Driver::Scripted(m));
        }

        let (kind, base) = spec.split_once(':').ok_or_else(|| {
            anyhow::anyhow!(
                "--model `{spec}` is not recognised. Expected `scripted:<path>` or                  `<runtime>:http://host:port` where <runtime> is llamacpp, vllm or sglang."
            )
        })?;
        let dialect = trellis_model::Dialect::parse(kind).ok_or_else(|| {
            anyhow::anyhow!("unknown runtime `{kind}`; expected llamacpp, vllm or sglang")
        })?;
        let client = trellis_model::RuntimeClient::new(base, dialect, None)?;
        Ok(Driver::Runtime(trellis_loop::RuntimeModel::new(
            client,
            !unconstrained,
        )))
    }

    fn describe(&self) -> String {
        match self {
            Driver::Scripted(m) => format!(
                "scripted, {} steps — real loop, real checks, real rollback; judgement stubbed",
                m.len()
            ),
            Driver::Runtime(m) => format!(
                "{:?} via {:?} — {}",
                m.dialect(),
                m.tier(),
                m.tier().degradation()
            ),
        }
    }
}

/// Runs the agent loop, and rolls back anything that did not converge.
///
/// The ordering matters and is the whole safety story: snapshot before the
/// first write, let the orchestrator be the only writer, and on any outcome
/// that is not convergence restore the tree. A partially-applied change that
/// nobody accepted is the worst thing this tool could leave behind.
fn cmd_run(
    root: &Path,
    task: &str,
    model_spec: &str,
    unconstrained: bool,
    agents: &str,
) -> Result<()> {
    if agents != "1" {
        anyhow::bail!(
            "multi-agent mode is v1.1 and is gated on single-agent numbers that do not \n             exist yet. Use --agents 1."
        );
    }
    let cfg = config::RepoConfig::load(root)?;
    let mut driver = Driver::parse(model_spec, unconstrained)?;
    println!("model:    {}", driver.describe());

    // A clean worktree is required so rollback is `git reset --hard` rather
    // than a restore of a dirty tree, which is where data-loss bugs live
    // (ADR 0009).
    let snapshot =
        trellis_vcs::snapshot(root).context("a clean git worktree is required before a run")?;
    println!("snapshot at {}", snapshot.head);

    let rt = tokio::runtime::Runtime::new()?;
    let (outcome, transcript) = rt.block_on(async {
        let (handle, _join) = trellis_loop::Orchestrator::new(root, cfg.checks()).spawn();
        // The loop is generic over the model, so each driver gets its own
        // call rather than a trait object — the `Model` trait uses `async fn`
        // and is not dyn-compatible.
        let out = match &mut driver {
            Driver::Scripted(m) => trellis_loop::run(task, m, &handle, &cfg.budget()).await,
            Driver::Runtime(m) => trellis_loop::run(task, m, &handle, &cfg.budget()).await,
        };
        let touched = handle.touched().await;
        (out, touched)
    });

    let (outcome, turns) = outcome;
    println!();
    for (i, t) in turns.turns.iter().enumerate() {
        let mark = match t.verdict_passed {
            Some(true) => "check passed",
            Some(false) => "check FAILED",
            None => "",
        };
        println!("  {:>2}. {:?} {}", i + 1, t.call, mark);
    }
    println!();
    println!("files written: {}", transcript.join(", "));

    match outcome {
        trellis_loop::Outcome::Converged {
            iterations,
            checks_passed,
        } => {
            println!();
            println!("CONVERGED after {iterations} iterations, {checks_passed} checks passed.");
            println!(
                "Changes are left in the working tree for you to review. Nothing is \n                 committed: acceptance is yours, and the trail that records it is v0.3."
            );
            Ok(())
        }
        trellis_loop::Outcome::NotConverged { reason, iterations } => {
            println!();
            println!("NOT CONVERGED after {iterations} iterations.");
            println!("  {}", reason.explain());
            snapshot
                .rollback()
                .context("rollback failed — inspect the worktree by hand before continuing")?;
            let restored = snapshot.is_restored().unwrap_or(false);
            println!();
            println!(
                "Rolled back to {}. Worktree restored: {restored}.",
                snapshot.head
            );
            std::process::exit(1);
        }
    }
}

/// Blast radius for a symbol, with its confidence breakdown.
///
/// The coverage block is printed unconditionally and the unresolved count is
/// never suppressed when it is inconvenient: a radius that omits what it
/// could not see is the failure mode the whole design exists to prevent.
fn cmd_radius(root: &Path, query: &str) -> Result<()> {
    let index = trellis_index::build_cached(root);

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
    // `index` always rebuilds — it is the command whose job is to refresh
    // the cache, so reading the cache here would make it a no-op.
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

fn cmd_doctor(root: &Path, model_spec: Option<&str>) -> Result<()> {
    use trellis_model::{AdapterTier, BatchingCapability, Probe};

    let cfg = config::RepoConfig::load(root)?.checks();

    // Unknown until measured. Reporting an optimistic guess is the failure
    // S17 and S18 both describe: the user cannot tell tier 2 from tier 4, or
    // a batching runtime from one that will serialize their agents.
    let mut model_id = "not configured".to_string();
    let mut runtime = "not configured".to_string();
    let mut tier = AdapterTier::ValidateAndRetry;
    let mut batching = BatchingCapability::Unknown;
    let mut notes: Vec<String> = Vec::new();

    if let Some(spec) = model_spec {
        let (kind, base) = spec.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("--model `{spec}` should look like `llamacpp:http://host:port`")
        })?;
        let dialect = trellis_model::Dialect::parse(kind)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime `{kind}`"))?;
        let client = trellis_model::RuntimeClient::new(base, dialect, None)?
            .with_timeout(std::time::Duration::from_secs(30));
        runtime = format!("{dialect:?} at {base}");
        model_id = "whatever the runtime has loaded".into();

        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            match client.probe_constraint().await {
                Ok(true) => {
                    tier = AdapterTier::RuntimeConstrained;
                    notes.push(
                        concat!(
                            "The runtime returned the key the probe schema demanded, so it ",
                            "appears to honour guided decoding. This is evidence, not proof: ",
                            "a capable model can produce conforming JSON unprompted."
                        )
                        .into(),
                    );
                }
                Ok(false) => {
                    tier = AdapterTier::ValidateAndRetry;
                    notes.push(
                        concat!(
                            "The runtime ignored the schema, so output is NOT constrained. Runs ",
                            "will retry on malformed tool calls and can fail on format alone. ",
                            "Pass --unconstrained to `trellis run` to make that explicit. "
                        )
                        .into(),
                    );
                }
                Err(e) => notes.push(format!("Could not probe the runtime: {e}")),
            }

            match client.probe_slots().await {
                Ok(Some(n)) if n > 1 => {
                    batching = BatchingCapability::Continuous {
                        measured_concurrency: n,
                    };
                }
                Ok(Some(_)) => {
                    batching = BatchingCapability::SingleStream;
                    notes.push(
                        concat!(
                            "llama.cpp reports a single slot. Multi-agent mode would serialize; ",
                            "start it with --parallel N for real concurrency (S18). "
                        )
                        .into(),
                    );
                }
                Ok(None) => {
                    // Continuous batching is inherent here, but the concurrency
                    // its scheduler will actually grant is not a number to
                    // invent, so `--agents auto` stays conservative.
                    batching = BatchingCapability::Unknown;
                    notes.push(
                        concat!(
                            "This runtime batches continuously, so multi-agent mode is supported ",
                            "in principle. The concurrency it will grant is its scheduler's ",
                            "decision and is not reported, so `--agents auto` stays at 1 rather ",
                            "than guessing. "
                        )
                        .into(),
                    );
                }
                Err(e) => notes.push(format!("Could not read slot count: {e}")),
            }
        });
    }

    let probe = Probe {
        model_id,
        runtime,
        tier,
        batching,
        typecheck_found: which(&cfg.typecheck[0]),
        tests_found: which(&cfg.tests[0]),
    };

    for line in probe.report() {
        println!("{line}");
    }
    println!("root:       {}", root.display());

    if model_spec.is_none() {
        println!();
        println!(concat!(
            "No --model given, so tier and batching are reported as unknown ",
            "rather than guessed.
",
            "Pass e.g. `--model llamacpp:http://127.0.0.1:8080` to probe."
        ));
    }
    for n in &notes {
        println!();
        println!("{n}");
    }
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
    let cfg = config::RepoConfig::load(root)?.checks();
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
