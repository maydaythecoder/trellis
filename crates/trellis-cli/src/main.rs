//! The CLI. Thin: parses, dispatches, formats.
//!
//! Status: `index`, `radius`, `check`, `doctor`, `run` and `resume` are wired
//! to real implementations.
//!
//! `run` drives the loop with either a scripted model or a real serving
//! runtime (`llamacpp:`, `vllm:`, `sglang:`). The scripted driver is not a
//! placeholder — it exercises real files, real checks and real rollback with
//! only judgement stubbed, which is what makes the loop's control flow
//! testable without inference. Results from it are never a model floor (S7).
//!
//! `partitions` needs concurrent multi-agent execution (v1.1) and exits
//! non-zero with a message naming what is missing, rather than pretending.

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
        /// Start from the last trail in `.trellis/runs/`, so the model is not
        /// re-told what the previous run established.
        ///
        /// Opt-in rather than automatic. A trail is about a task that already
        /// finished, and quietly carrying it into an unrelated one is how a
        /// stale fact acquires authority (S11). The seeded text is fenced as
        /// data and cannot make this run converge — only a check executed in
        /// this session does that.
        #[arg(long)]
        resume: bool,
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
            resume,
        } => cmd_run(&root, &task, &model, unconstrained, &agents, resume),
        Command::Resume => cmd_resume(&root),
        Command::Partitions { .. } => {
            anyhow::bail!(concat!(
                "not implemented yet: `partitions` needs concurrent multi-agent ",
                "execution, which is v1.1 and is gated on single-agent numbers ",
                "that do not exist yet.\n\n",
                "Working today: index, radius, run, resume, check, doctor.\n\n",
                "The partition planner itself is built and property-tested ",
                "(`trellis_loop::partition::plan`); what is missing is running the ",
                "partitions concurrently and reconciling their checks."
            ))
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
                concat!(
                    "--model `{}` is not recognised. Expected `scripted:<path>` or ",
                    "`<runtime>:http://host:port` where <runtime> is llamacpp, vllm or sglang."
                ),
                spec
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
    resume: bool,
) -> Result<()> {
    if agents != "1" {
        anyhow::bail!(concat!(
            "multi-agent mode is v1.1 and is gated on single-agent numbers that ",
            "do not exist yet. Use --agents 1."
        ));
    }
    let cfg = config::RepoConfig::load(root)?;
    let mut driver = Driver::parse(model_spec, unconstrained)?;
    println!("model:    {}", driver.describe());

    // Read before the snapshot, so a missing or corrupt trail fails the run
    // before anything has been written.
    let prior = if resume {
        match trellis_trail::latest(root)? {
            Some((path, trail)) => {
                println!("resumed:  {}", path.display());
                println!(
                    "          {} lines of prior context, fenced as data",
                    trail.context_lines().len()
                );
                Some(trellis_loop::PriorContext {
                    source: path.display().to_string(),
                    lines: trail.context_lines(),
                })
            }
            None => {
                // Not an error. A first run in a repository has nothing to
                // resume from, and refusing to start would make `--resume`
                // unusable as a habit.
                println!("resumed:  nothing — no trail under .trellis/runs/");
                None
            }
        }
    } else {
        None
    };

    // A clean worktree is required so rollback is `git reset --hard` rather
    // than a restore of a dirty tree, which is where data-loss bugs live
    // (ADR 0009).
    let snapshot =
        trellis_vcs::snapshot(root).context("a clean git worktree is required before a run")?;
    println!("snapshot at {}", snapshot.head);

    let governance = cfg.governance();
    let notes = config::RepoConfig::notes(root)?;
    if !governance.is_empty() || notes.is_some() {
        println!(
            "repo:     {} write rule(s){}",
            governance.rules().len(),
            if notes.is_some() {
                ", .trellis/context.md loaded"
            } else {
                ""
            }
        );
    }
    let seed = trellis_loop::Seed {
        prior,
        repo: trellis_loop::RepoContext {
            notes,
            rules: governance.rules().iter().map(|r| r.as_written()).collect(),
        },
    };

    let rt = tokio::runtime::Runtime::new()?;
    let (outcome, transcript) = rt.block_on(async {
        let (handle, _join) = trellis_loop::Orchestrator::new(root, cfg.checks())
            .with_governance(governance)
            .spawn();
        // The loop is generic over the model, so each driver gets its own
        // call rather than a trait object — the `Model` trait uses `async fn`
        // and is not dyn-compatible.
        let out = match &mut driver {
            Driver::Scripted(m) => {
                trellis_loop::run_seeded(task, m, &handle, &cfg.budget(), seed).await
            }
            Driver::Runtime(m) => {
                trellis_loop::run_seeded(task, m, &handle, &cfg.budget(), seed).await
            }
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
            let trail = record_trail(
                root,
                task,
                &turns,
                &transcript,
                trellis_trail::Approval::None,
            )?;
            println!("trail:    {}", trail.display());
            println!(
                "{}",
                concat!(
                    "Changes are left in the working tree for you to review. ",
                    "Nothing is committed:\n",
                    "acceptance is yours, and the trail records the run as unapproved ",
                    "until you give it."
                )
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

            // Written *after* the rollback, deliberately. Rollback cleans
            // untracked files, and a trail written before it would be one of
            // them — deleting the record of exactly the run worth recording.
            match record_trail(
                root,
                task,
                &turns,
                &transcript,
                trellis_trail::Approval::None,
            ) {
                Ok(p) => println!("trail:    {} (survives the rollback)", p.display()),
                Err(e) => println!("could not write the trail: {e}"),
            }
            std::process::exit(1);
        }
    }
}

/// Continues from the last trail entry.
///
/// This is the wedge the product exists for: reopen a repository, and the
/// previous session's decisions are already there. The trail is read back as
/// `UntrustedTrail` — a repository may take contributions, so reading a trail
/// is reading attacker-influenceable text into an agent's context (S10).
fn cmd_resume(root: &Path) -> Result<()> {
    let Some((path, trail)) = trellis_trail::latest(root)? else {
        println!("No trail under .trellis/runs/. Nothing to resume from.");
        println!("`trellis run` writes one, and the next session starts from it.");
        return Ok(());
    };

    println!("resuming from {}", path.display());
    println!();
    for line in trail.context_lines() {
        println!("  {line}");
    }
    println!();
    println!(concat!(
        "Everything above is read back as data, never as instruction. ",
        "The judgment
",
        "section is the previous run's opinion and is fenced accordingly (S10, S11)."
    ));
    println!();
    println!(concat!(
        "To carry this into a run rather than only read it, pass --resume:
",
        "  trellis run --resume --model <spec> \"<task>\"
",
        "The lines above are then fenced into the model's prompt as data. They ",
        "cannot
make the run converge — only a check executed in that session ",
        "can do that."
    ));
    Ok(())
}

/// Records what happened, factual and judgment kept apart.
///
/// Everything in `factual` comes from a tool result the run actually received.
/// Nothing is defaulted into it: a figure nobody measured has no business in
/// the section whose promise is that it is machine-checkable.
fn record_trail(
    root: &Path,
    task: &str,
    turns: &trellis_loop::Transcript,
    touched: &[String],
    approved: trellis_trail::Approval,
) -> Result<std::path::PathBuf> {
    use trellis_trail::{CheckOutcome, Factual, Judgment, ModeCounts, RadiusCounts, Trail};

    // Reach counts sum across queries; the unbound count does not.
    //
    // "How many call sites did this run examine" genuinely is the sum, since
    // each query asks about a different symbol. A run that never asked records
    // zeros, and `radius_queries` is what tells a reader that apart from a run
    // that asked and found nothing.
    let mut radius = RadiusCounts::default();
    for t in &turns.turns {
        if let Some((r, c, u)) = t.radius_counts {
            radius.reached_resolved += r;
            radius.reached_ambiguous += c;
            // Not a sum: radii in one repository touch overlapping files, so
            // adding these counts the same unbindable call sites once per
            // query — a figure that grows with curiosity rather than with
            // doubt. The widest single query is a true statement about a real
            // radius; a sum is not a statement about anything.
            radius.widest_unbound_in_touched_files = radius.widest_unbound_in_touched_files.max(u);
            radius.radius_queries += 1;
        }
    }

    let check_outcomes: Vec<CheckOutcome> = turns
        .turns
        .iter()
        .filter_map(|t| {
            Some(CheckOutcome {
                kind: t.check_kind.clone()?,
                passed: t.verdict_passed?,
            })
        })
        .collect();

    let (timestamp, started_at) = trellis_trail::now();

    let trail = Trail {
        task: task.to_string(),
        started_at,
        trellis_version: env!("CARGO_PKG_VERSION").to_string(),
        factual: Factual {
            proposed: touched.to_vec(),
            radius,
            // `Verdict` cannot be constructed outside `trellis-checks`, so the
            // sequence travels as `check_outcomes` instead. See its docs.
            checks: Vec::new(),
            check_outcomes,
            approved_by: approved,
            mode_counts: ModeCounts::default(),
        },
        // Left empty deliberately. The judgment section is the *model's*
        // opinion, and `resume` fences it as unverified — so putting a check
        // result here would label an executed fact as speculation, which is
        // the S11 confusion the split exists to prevent, inverted. It is
        // filled when a model supplies a `Done { summary }`, and not before.
        judgment: Judgment::default(),
    };

    let slug = trellis_trail::slugify(task);
    let path = trellis_trail::write(root, &trail, &timestamp, &slug)?;

    // Committed, not merely written. The brief calls the trail durable and in
    // the repository, and leaving it untracked fails that twice: the worktree
    // would never be clean again so no later run could start, and the next
    // rollback's `git clean` would delete every trail accumulated so far.
    //
    // Only the trail is staged. The code the run proposed stays uncommitted,
    // because accepting it is the user's decision to make.
    let rel = trellis_trail::Trail::path(&timestamp, &slug);
    if let Err(e) = trellis_vcs::commit_paths(root, &[&rel], &format!("trellis: trail for {task}"))
    {
        println!("trail written but not committed: {e}");
    }
    Ok(path)
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
            println!(
                "          {reason:<22} {n:>6}   {}",
                trellis_index::graph::Reason::explain_named(reason).unwrap_or("")
            );
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
    let mut tier: Option<AdapterTier> = None;
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
                    tier = Some(AdapterTier::RuntimeConstrained);
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
                    tier = Some(AdapterTier::ValidateAndRetry);
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
