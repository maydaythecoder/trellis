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
mod session;
mod tui;

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

    /// Omit to enter an interactive session.
    #[command(subcommand)]
    command: Option<Command>,
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
        /// Optional when `--prompt` names a template.
        #[arg(default_value = "")]
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
        /// Start from a named template in `.trellis/prompts/<name>.md`.
        ///
        /// Any task text given as well is appended, so a template can be a
        /// standing instruction with a specific ask after it.
        #[arg(long)]
        prompt: Option<String>,
    },

    /// Scaffold `.trellis/` and report what is missing before it bites.
    Init,

    /// Full-screen interface: transcript, run state and task entry at once.
    ///
    /// The line-based session stays the default. This is the surface for
    /// watching a run rather than issuing one command at a time.
    Tui,

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

    /// One line per recorded run: task, outcome, integrity.
    Log {
        /// How many to show, newest first.
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// What past runs did to a symbol or file, from the committed trail.
    ///
    /// The test of whether the trail is worth writing: if this cannot answer
    /// "why does this look like this" two weeks later, the grain is wrong.
    Why { symbol: String },

    /// List, fetch, or locate the models TRELLIS knows about.
    ///
    /// Weights are fetched rather than bundled, and every catalogue entry is
    /// Apache-2.0 or MIT so the default is usable for commercial work.
    Model {
        #[command(subcommand)]
        action: Option<ModelAction>,
    },
}

#[derive(Subcommand)]
enum ModelAction {
    /// Download weights, resuming an interrupted fetch.
    Pull { name: Option<String> },
    /// Print where the weights live, fetched or not.
    Path { name: Option<String> },
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
    let explicit = cli.root.is_some();
    // `init` is the one command whose whole purpose is a directory that is
    // not set up yet, so it may not require a discovered repository. Every
    // other command refusing here is deliberate — `trellis index` in a home
    // folder once walked 25,147 files before anyone could tell it had not
    // found one — but making `init` share that refusal means the command for
    // getting started cannot be run before you have started.
    let needs_repo = !matches!(cli.command, Some(Command::Init));
    let root = match cli.root {
        Some(r) => r,
        None => match discover_root() {
            Ok(r) => r,
            Err(_) if !needs_repo => std::env::current_dir()?,
            Err(e) => return Err(e.context("cannot determine the repository root")),
        },
    };
    // Say which tree is about to be read whenever it was inferred.
    //
    // Both directions are surprises worth naming. Acting on a directory
    // several levels up ends with someone running a task against the wrong
    // tree. Falling back to the working directory is worse if it happens to
    // be a home folder — `trellis index` there walked 1,295 files before
    // anyone could tell it had not found a repository.
    if !explicit && root != std::env::current_dir().unwrap_or_default() {
        println!("root:     {} (repository root)", root.display());
    }

    let Some(command) = cli.command else {
        // Bare `trellis` is a session, not a usage error. One-shot commands
        // re-derived the index for every question and made the user re-type
        // --root and --model; holding them is the shape the orchestrator's
        // single-owner index was built for.
        return session::Session::open(&root, None)?.run();
    };

    match command {
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
            prompt,
        } => {
            let task = match &prompt {
                Some(name) => {
                    let template = config::RepoConfig::prompt(&root, name)?;
                    if task.trim().is_empty() {
                        template
                    } else {
                        format!(
                            "{template}

{task}"
                        )
                    }
                }
                None => task,
            };
            let exit = cmd_run(&root, &task, &model, unconstrained, &agents, resume)?;
            // The one place that decides the process ends, because this is
            // the one caller that *is* the process.
            if exit.code() != 0 {
                std::process::exit(exit.code());
            }
            Ok(())
        }
        Command::Init => cmd_init(&root),
        Command::Tui => tui::run(&root, config::RepoConfig::load(&root)?.model()),
        Command::Resume => cmd_resume(&root),
        // Deliberately before the root is used for anything: models are user
        // state, not repository state, and `trellis model` must work outside
        // a repository.
        Command::Log { limit } => cmd_log(&root, limit),
        Command::Why { symbol } => cmd_why(&root, &symbol),
        Command::Model { action } => cmd_models(action),
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

/// The repository root, walking up from the working directory.
///
/// Typing `trellis index` three directories deep used to index whatever
/// happened to be under the cwd — which for most subdirectories is nothing at
/// all, reported as "indexed 0 files" with no hint that the user was standing
/// in the wrong place. Every other tool in this space walks up to find its
/// project marker, and `.git` is the right marker here: a run already requires
/// a git repository with a clean worktree (ADR 0009), so a TRELLIS root that
/// is not a git root is a root half the commands cannot work with.
///
/// `--root` still wins outright. An explicit path is a decision, and a tool
/// that overrides one because it thinks it knows better is worse than one that
/// makes the user type more.
fn discover_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    root_from(&cwd).ok_or_else(|| {
        anyhow::anyhow!(
            concat!(
                "no git repository found at {} or above it.

",
                "TRELLIS works on a repository, and without one there is no way to ",
                "tell how much of the filesystem you meant. Run it from inside a ",
                "repository, or name the tree explicitly:
",
                "    trellis <command> --root <path>"
            ),
            cwd.display()
        )
    })
}

/// The walk itself, separated from the process-global working directory so it
/// can be tested without one.
///
/// `None` when there is no repository above `start`. An earlier version fell
/// back to `start` on the reasoning that `index` and `radius` are useful on a
/// plain source tree — which is true, and was still the wrong default.
/// `trellis index` typed in a home folder walked the entire home folder, and
/// the line printed to say so appeared after the walk had already begun.
///
/// Git refuses here, and so does this. The plain-tree case is served by
/// `--root .`, which is one flag and states the intent.
fn root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// One line per run: what was asked, what came of it, whether it is intact.
fn cmd_log(root: &Path, limit: usize) -> Result<()> {
    let entries = trellis_trail::query::all(root);
    if entries.is_empty() {
        println!("no trails under .trellis/runs. `trellis run` writes one.");
        return Ok(());
    }

    println!("{} run(s), newest first", entries.len());
    println!();
    for e in entries.iter().take(limit) {
        let f = &e.trail.factual;
        println!("{}  {}", e.run_id(), e.outcome());
        println!("    {}", e.trail.task);
        let checks: Vec<String> = f
            .check_outcomes
            .iter()
            .map(|c| format!("{}{}", c.kind, if c.passed { "+" } else { "-" }))
            .collect();
        println!(
            "    {} file(s) proposed, {} reached, checks: {}",
            f.proposed.len(),
            f.radius.reached_resolved + f.radius.reached_ambiguous,
            if checks.is_empty() {
                "none".into()
            } else {
                checks.join(" ")
            }
        );
        // Integrity is printed only when it is not the ordinary case, so a
        // long listing stays readable and the one line that matters stands
        // out rather than blending into a column of "ok".
        match e.integrity {
            trellis_trail::Integrity::Altered => {
                println!("    ALTERED: the factual section no longer matches its digest")
            }
            trellis_trail::Integrity::Unrecorded => {
                println!("    (older than the digest field; integrity unknown)")
            }
            trellis_trail::Integrity::Intact => {}
        }
        println!();
    }
    if entries.len() > limit {
        println!("... {} older, use --limit", entries.len() - limit);
    }
    Ok(())
}

/// What past runs did to a symbol or file, and what they were asked to do.
///
/// The v0.3 document calls this the test of whether the trail's grain is
/// right: if "why does this look like this" cannot be answered here two weeks
/// later, the trail is a log nobody reads.
fn cmd_why(root: &Path, query: &str) -> Result<()> {
    let entries = trellis_trail::query::all(root);

    // Resolve the symbol to the file that defines it before matching.
    //
    // The factual section records FILES; a user asking "why does this
    // function look like this" holds a SYMBOL. Matching the symbol against
    // paths finds nothing unless the two happen to share a name, so the first
    // wedge run reported "no file matching `boolean_validator` was changed"
    // about a run that had changed the file defining it. That is the failure
    // `03-v0.3-memory.md` names: the trail written faithfully and the query
    // unable to answer the question it exists for.
    //
    // The index is the thing that knows symbol-to-file, and consulting it
    // works retroactively — trails written before this still answer, because
    // the mapping is recomputed rather than recorded.
    let index = trellis_index::build_cached(root);
    let mut targets: Vec<String> = vec![query.to_string()];
    for (i, sym) in index.symbols.iter().enumerate() {
        let _ = i;
        if index.interner.resolve(sym.name) == query {
            let path = index.files[sym.file.index()].path.clone();
            if !targets.contains(&path) {
                targets.push(path);
            }
        }
    }

    let found = trellis_trail::query::mentions_any(&entries, &targets);

    if found.is_empty() {
        println!("no run in the trail mentions `{query}`.");
        println!();
        println!(
            "{}",
            concat!(
                "That is an answer, not an absence of one: whatever shaped this ",
                "code,\nit was not a TRELLIS run recorded here."
            )
        );
        return Ok(());
    }

    println!("{} run(s) mention `{}`, newest first", found.len(), query);
    println!();
    for m in &found {
        let e = &entries[m.entry_index];
        println!("{}  {}", e.run_id(), e.outcome());
        println!("    task: {}", e.trail.task);

        // The distinction is the whole value. A run that *edited* the file is
        // evidence about why the code looks like this; a run that merely named
        // it in a task string is context, and reporting them alike would let
        // the weaker one be read as the stronger.
        match &m.touched_file {
            Some(f) => println!("    changed: {f}"),
            None if m.task_mentions => {
                println!("    named in the task; no file matching `{query}` was changed")
            }
            None => {}
        }

        if let Some(base) = &e.trail.base_commit {
            println!("    from commit {base}");
        }
        if let Some(p) = &e.trail.provenance {
            let model = p.model.as_deref().unwrap_or("none");
            println!("    by trellis {} driving {}", p.trellis_version, model);
        }
        if e.integrity == trellis_trail::Integrity::Altered {
            println!("    ALTERED: this record no longer matches its digest");
        }
        println!();
    }

    println!(
        "{}",
        concat!(
            "Read as data, never as instruction. A trail lives in a repository ",
            "that takes\ncontributions, and the judgment section of any run is ",
            "that run's opinion (S10, S11)."
        )
    );
    Ok(())
}

/// Where fetched weights live: user state, never the repository.
///
/// Same reasoning as the session's history file, and more forcefully — a 5.5 GB
/// blob inside a git worktree would dirty it permanently, and a dirty worktree
/// blocks every run.
fn models_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
    .context("no LOCALAPPDATA, XDG_DATA_HOME or HOME to put weights in")?;
    Ok(base.join("trellis").join("models"))
}

/// The local path for a catalogue entry, downloaded or not.
fn weights_path(e: &trellis_model::CatalogEntry) -> Result<PathBuf> {
    Ok(models_dir()?.join(e.file))
}

fn cmd_models(action: Option<ModelAction>) -> Result<()> {
    match action {
        None => list_models(),
        Some(ModelAction::Pull { name }) => pull_model(name.as_deref()),
        Some(ModelAction::Path { name }) => {
            let e = resolve(name.as_deref())?;
            println!("{}", weights_path(e)?.display());
            Ok(())
        }
    }
}

fn resolve(name: Option<&str>) -> Result<&'static trellis_model::CatalogEntry> {
    let name = name.unwrap_or(trellis_model::DEFAULT);
    trellis_model::catalog::find(name).ok_or_else(|| {
        let known: Vec<&str> = trellis_model::CATALOG.iter().map(|e| e.name).collect();
        anyhow::anyhow!("no model named `{name}`. Known: {}", known.join(", "))
    })
}

fn list_models() -> Result<()> {
    let dir = models_dir()?;
    println!("weights in {}", dir.display());
    println!();
    for e in trellis_model::CATALOG {
        let path = dir.join(e.file);
        let have = std::fs::metadata(&path).ok().map(|m| m.len());
        let state = match have {
            Some(n) if n >= e.bytes => "downloaded".to_string(),
            // A partial file is worth naming rather than hiding: on a slow
            // link the difference between resuming and restarting is hours.
            Some(n) => format!("partial {:.0}%", n as f64 / e.bytes as f64 * 100.0),
            None => "not fetched".to_string(),
        };
        let marker = if e.name == trellis_model::DEFAULT {
            " (default)"
        } else {
            ""
        };
        println!("  {}{}", e.name, marker);
        println!(
            "      {:.1} GiB download, ~{:.1} GB resident, {} — {}",
            e.gib(),
            e.resident_gb,
            e.license,
            state
        );
        println!("      {}", e.expectation);
        println!();
    }
    println!("  trellis model pull [name]   fetch (resumable)");
    println!("  trellis model path [name]   where it would live");
    println!();
    println!(
        "{}",
        concat!(
            "Weights are fetched, never bundled: a multi-gigabyte payload inside ",
            "the binary\nis not a distribution anyone wants. Every entry is ",
            "Apache-2.0 or MIT, so the\ndefault is usable for commercial work."
        )
    );
    Ok(())
}

fn pull_model(name: Option<&str>) -> Result<()> {
    let e = resolve(name)?;
    let dir = models_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(e.file);

    let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if have >= e.bytes {
        println!("{} is already here: {}", e.name, path.display());
        return Ok(());
    }

    println!("{}  {}", e.name, e.license);
    println!("  from {}", e.url());
    println!("  to   {}", path.display());
    if have > 0 {
        println!(
            "  resuming at {:.1} GiB of {:.1}",
            have as f64 / 1_073_741_824.0,
            e.gib()
        );
    } else {
        println!("  {:.1} GiB", e.gib());
    }
    println!();

    // curl rather than an HTTP client of our own, and `-C -` rather than a
    // fresh GET. This is not laziness: fetching multi-gigabyte weights over a
    // connection that drops is the normal case, and a range-resuming fetch
    // that survives a hundred attempts is worth more here than avoiding a
    // dependency on a binary every platform already ships.
    let status = std::process::Command::new("curl")
        .args([
            "-L",
            "-C",
            "-",
            "--retry",
            "0",
            "--fail",
            "-o",
            &path.display().to_string(),
            &e.url(),
        ])
        .status()
        .context("could not run curl; it is needed to fetch weights")?;

    let now = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if now >= e.bytes {
        println!();
        println!("complete: {} bytes", now);
        println!("serve it with llama.cpp, then:");
        println!("  trellis run --model llamacpp:http://127.0.0.1:8080 \"<task>\"");
        return Ok(());
    }

    // An incomplete download is not a failure to start over from. Saying so
    // is the difference between a user re-fetching gigabytes and resuming.
    anyhow::bail!(
        "incomplete: {} of {} bytes ({:.0}%){}. \
         Run the same command again to resume — the partial file is kept.",
        now,
        e.bytes,
        now as f64 / e.bytes as f64 * 100.0,
        if status.success() {
            ""
        } else {
            ", and curl reported a failure"
        }
    )
}

/// How a run ended, and what the process should exit with.
///
/// **Returned rather than acted on.** `cmd_run` used to call
/// `std::process::exit` on the non-converged path, which is fine for a
/// one-shot command and fatal everywhere else: the TUI calls it from a worker
/// thread, so a run that did not converge took the whole interface down
/// mid-frame, without unwinding the terminal guard and without the panic hook
/// firing — `exit` runs no destructors. The exit code is the caller's
/// decision because only the caller knows whether it *is* the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunExit {
    Converged,
    /// The tree was restored, or it was not — which is not the same failure
    /// and must not share an exit code.
    NotConverged {
        restored: bool,
    },
    Interrupted {
        restored: bool,
    },
}

impl RunExit {
    /// 0 converged. 1 did not converge and the tree is back as it was.
    /// 2 the tree is NOT back, which a wrapping script must not treat as an
    /// ordinary failed run. 130 a deliberate abort.
    fn code(self) -> i32 {
        match self {
            RunExit::Converged => 0,
            RunExit::NotConverged { restored: true } => 1,
            RunExit::NotConverged { restored: false } => 2,
            RunExit::Interrupted { restored: true } => 130,
            RunExit::Interrupted { restored: false } => 2,
        }
    }

    fn converged(self) -> bool {
        matches!(self, RunExit::Converged)
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
    /// Builds the driver a parsed spec names.
    ///
    /// Takes a [`ModelSpec`] rather than a string: the string form was parsed
    /// here, in `cmd_doctor` and in the session, each deciding for itself what
    /// was valid. Construction is now the only thing this does.
    fn build(spec: &trellis_model::ModelSpec, unconstrained: bool) -> Result<Self> {
        match spec {
            trellis_model::ModelSpec::Scripted(path) => {
                if unconstrained {
                    anyhow::bail!("--unconstrained is meaningless for a scripted model");
                }
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read script {}", path.display()))?;
                let m = trellis_loop::ScriptedModel::from_json(&text)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                Ok(Driver::Scripted(m))
            }
            trellis_model::ModelSpec::Runtime { dialect, base } => {
                let client = trellis_model::RuntimeClient::new(base, *dialect, None)?;
                Ok(Driver::Runtime(trellis_loop::RuntimeModel::new(
                    client,
                    !unconstrained,
                )))
            }
        }
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
) -> Result<RunExit> {
    if agents != "1" {
        anyhow::bail!(concat!(
            "multi-agent mode is v1.1 and is gated on single-agent numbers that ",
            "do not exist yet. Use --agents 1."
        ));
    }
    let cfg = config::RepoConfig::load(root)?;
    let spec: trellis_model::ModelSpec = model_spec.parse()?;
    let mut driver = Driver::build(&spec, unconstrained)?;
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

    // Rule documents from `.trellis/governance/`, plus the older
    // `[governance] deny_write` in config.toml. Both feed the same write-path
    // refusal; the documents additionally carry rank and prose, which the
    // audit and the prompt need and a config list cannot express.
    let rules = trellis_checks::governance::load(root)
        .context("a governance rule would not load; runs would silently ignore it")?;
    let mut deny: Vec<String> = cfg
        .governance()
        .rules()
        .iter()
        .map(|r| r.as_written())
        .collect();
    deny.extend(trellis_checks::governance::deny_write_paths(&rules));
    let governance = trellis_loop::Governance::new(&deny);
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
    // The model is told the rules it will be held to, ranked, with the prose
    // that explains them. A refusal a contributor cannot act on is a wall;
    // the description is what turns it into a direction.
    let mut prompt_rules: Vec<String> = governance
        .rules()
        .iter()
        .map(|r| format!("{} (no writes)", r.as_written()))
        .collect();
    for r in &rules {
        let rank = match r.rank {
            trellis_checks::governance::Rank::NonNegotiable => "non-negotiable",
            trellis_checks::governance::Rank::NiceToHave => "nice-to-have",
        };
        prompt_rules.push(if r.description.trim().is_empty() {
            format!("{} [{rank}]", r.name)
        } else {
            format!("{} [{rank}] — {}", r.name, r.description.trim())
        });
    }
    let seed = trellis_loop::Seed {
        prior,
        repo: trellis_loop::RepoContext {
            notes,
            rules: prompt_rules,
        },
    };

    let rt = tokio::runtime::Runtime::new()?;
    let (outcome, transcript) = rt.block_on(async {
        let (handle, _join) = trellis_loop::Orchestrator::new(root, cfg.checks())
            .with_governance(governance)
            .with_rules(rules)
            .spawn();
        // The loop is generic over the model, so each driver gets its own
        // call rather than a trait object — the `Model` trait uses `async fn`
        // and is not dyn-compatible.
        //
        // Raced against Ctrl-C, because Ctrl-C is how a user actually aborts
        // a run that is going nowhere, and an unhandled SIGINT kills the
        // process with this run's edits still in the tree — the one state
        // TRELLIS promises never to leave behind. Cancelling here is safe
        // precisely because the orchestrator is a separate task that owns
        // the only write path: dropping this future cannot interrupt a write
        // half-done, it can only stop new ones being asked for.
        let out = tokio::select! {
            out = async {
                match &mut driver {
                    Driver::Scripted(m) => {
                        trellis_loop::run_seeded(task, m, &handle, &cfg.budget(), seed).await
                    }
                    Driver::Runtime(m) => {
                        trellis_loop::run_seeded(task, m, &handle, &cfg.budget(), seed).await
                    }
                }
            } => Some(out),
            _ = tokio::signal::ctrl_c() => None,
        };
        // Asked after the race either way: the orchestrator is still alive
        // and still knows what this run wrote, which is what the rollback and
        // the trail both need.
        let touched = handle.touched().await;
        (out, touched)
    });

    let Some(outcome) = outcome else {
        return interrupted_run(root, task, &transcript, &snapshot, model_spec);
    };
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
    // Context fill (v0.2 gate). Bytes, because TRELLIS does not tokenize.
    let measured: Vec<usize> = turns.turns.iter().filter_map(|t| t.prompt_bytes).collect();
    if let (Some(first), Some(peak)) = (measured.first(), measured.iter().max()) {
        println!(
            "context:  {} B first prompt, {} B peak over {} turns ({:.1}x)",
            first,
            peak,
            measured.len(),
            *peak as f64 / *first as f64
        );
    }

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
                &snapshot.head,
                Some(model_spec),
            )?;
            println!("trail:    {}", trail.display());

            // S6: the coverage warning attaches to the approval, and for a
            // one-shot run this line *is* the approval moment — the session's
            // `/accept` prompt is never reached. Scoped to the files the run
            // proposes, because repository-wide doubt is 11,233 sites on the
            // benchmark corpus and teaches a reader to skip the line.
            //
            // The index is rebuilt rather than reused: the run edited these
            // files, so the orchestrator's copy predates what is being
            // reported on. That costs about 200 ms at the end of a run, which
            // is the right trade for a figure that describes the tree the
            // user is about to look at.
            if !transcript.is_empty() {
                let doubt =
                    trellis_index::doubt_in_files(&trellis_index::build_cached(root), &transcript);
                if doubt.unbound > 0 {
                    println!("coverage: {}", doubt.one_line());
                    println!(
                        "{}",
                        concat!(
                            "          Any of them may call what changed. The ",
                            "checks ran on what the graph could see."
                        )
                    );
                }
            }
            println!(
                "{}",
                concat!(
                    "Changes are left in the working tree for you to review. ",
                    "Nothing is committed:\n",
                    "acceptance is yours, and the trail records the run as unapproved ",
                    "until you give it."
                )
            );
            Ok(RunExit::Converged)
        }
        trellis_loop::Outcome::NotConverged { reason, iterations } => {
            println!();
            println!("NOT CONVERGED after {iterations} iterations.");
            println!("  {}", reason.explain());
            // Deliberately not `?`. A failed rollback used to propagate here,
            // which meant the one run whose record matters most — the tree is
            // in an unknown state — was also the one run that never got a
            // trail written. The failure is reported loudly and the recording
            // continues.
            let restored = rollback_or_explain(&snapshot);

            // Written *after* the rollback, deliberately. Rollback cleans
            // untracked files, and a trail written before it would be one of
            // them — deleting the record of exactly the run worth recording.
            match record_trail(
                root,
                task,
                &turns,
                &transcript,
                trellis_trail::Approval::None,
                &snapshot.head,
                Some(model_spec),
            ) {
                Ok(p) => println!("trail:    {} (survives the rollback)", p.display()),
                Err(e) => println!("could not write the trail: {e}"),
            }
            Ok(RunExit::NotConverged { restored })
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

/// Ctrl-C during a run.
///
/// v1.0 gate condition 4 lists SIGINT as an injection point, and
/// `04-v1.0-harness.md` calls it one of the two usually missed: "Ctrl-C is how
/// users will actually abort." An unhandled SIGINT kills the process with the
/// run's edits still in the tree, which is the one state TRELLIS promises
/// never to leave behind — and the next run would then refuse to start on a
/// dirty worktree, so the user's escape hatch becomes a wall.
///
/// Exits 130, the shell convention for a process terminated by SIGINT, so a
/// script wrapping TRELLIS can tell an abort from a failed run.
fn interrupted_run(
    root: &Path,
    task: &str,
    touched: &[String],
    snapshot: &trellis_vcs::Snapshot,
    model_spec: &str,
) -> Result<RunExit> {
    let restored = handle_interrupt(root, task, touched, snapshot, model_spec);
    Ok(RunExit::Interrupted { restored })
}

/// Everything an interrupt does, minus the exit.
///
/// Split out so it can be tested. Whether `tokio::signal::ctrl_c()` fires is
/// tokio's contract; what TRELLIS does once it has fired is this project's,
/// and that is the half worth a test — a rollback that did not happen, or a
/// trail that was never written, would both be invisible from the outside
/// until a user hit it.
fn handle_interrupt(
    root: &Path,
    task: &str,
    touched: &[String],
    snapshot: &trellis_vcs::Snapshot,
    model_spec: &str,
) -> bool {
    println!();
    println!("INTERRUPTED. Rolling back — nothing is accepted on an abort.");
    if !touched.is_empty() {
        println!("files this run had written: {}", touched.join(", "));
    }

    let restored = rollback_or_explain(snapshot);

    // The turn list cannot be recovered: it lives in the loop's future, which
    // was just cancelled. The factual half that matters survives, because the
    // orchestrator holds it — the task, the base commit, and every file the
    // run wrote. A trail saying "this happened and here is what it touched"
    // is worth more than no record of the run at all, and it must not imply
    // turns were inspected when none were.
    let turns = trellis_loop::Transcript {
        task: task.to_string(),
        turns: Vec::new(),
        final_notice: false,
        prior: None,
        repo: trellis_loop::RepoContext::default(),
    };
    match record_trail(
        root,
        task,
        &turns,
        touched,
        trellis_trail::Approval::None,
        &snapshot.head,
        Some(model_spec),
    ) {
        Ok(p) => {
            println!("trail:    {} (interrupted; no turns recorded)", p.display());
        }
        Err(e) => println!("could not write the trail: {e}"),
    }

    // 130 is the shell convention for a process terminated by SIGINT, so a
    // script wrapping TRELLIS can tell a deliberate abort from a failed run.
    // An abort that could not restore the tree is not an ordinary abort, and
    // gets the same code a failed rollback gets anywhere else.
    restored
}

/// Rolls back, and if that fails says so at the top of its voice.
///
/// `04-v1.0-harness.md`, task item 3: "Never fail silently on the recovery
/// path." Every other error leaves the repository as the user last saw it;
/// this one leaves it mid-run, so it goes to stderr with the exact commands
/// rather than into an error type someone may or may not print.
///
/// Returns whether the worktree was restored, so the caller can pick an exit
/// code that says which.
fn rollback_or_explain(snapshot: &trellis_vcs::Snapshot) -> bool {
    match snapshot.rollback() {
        Ok(()) => {
            let restored = snapshot.is_restored().unwrap_or(false);
            println!();
            println!(
                "Rolled back to {}. Worktree restored: {restored}.",
                snapshot.head
            );
            if !restored {
                eprintln!();
                eprintln!(
                    "{}",
                    concat!(
                        "WARNING: git reported success but the worktree still ",
                        "differs from the snapshot. Check `git status` before ",
                        "continuing; something changed the tree after the ",
                        "rollback ran."
                    )
                );
            }
            restored
        }
        Err(e) => {
            eprintln!();
            eprintln!("{e}");
            false
        }
    }
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
    base_commit: &str,
    model: Option<&str>,
) -> Result<std::path::PathBuf> {
    use trellis_trail::{
        CheckOutcome, Factual, Judgment, ModeCounts, Provenance, RadiusCounts, SCHEMA, Trail,
    };

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

    // Context fill, from the turns that carried a prompt. A scripted run
    // carries none and records nothing rather than a plausible-looking zero.
    let measured: Vec<usize> = turns.turns.iter().filter_map(|t| t.prompt_bytes).collect();
    let context = (!measured.is_empty()).then(|| trellis_trail::ContextFill {
        first_prompt_bytes: measured[0],
        peak_prompt_bytes: measured.iter().copied().max().unwrap_or(0),
        turns_measured: measured.len() as u32,
    });

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

    let factual = Factual {
        proposed: touched.to_vec(),
        radius,
        // `Verdict` cannot be constructed outside `trellis-checks`, so the
        // sequence travels as `check_outcomes` instead. See its docs.
        checks: Vec::new(),
        check_outcomes,
        approved_by: approved,
        mode_counts: ModeCounts::default(),
        context,
    };

    let slug = trellis_trail::slugify(task);
    let run_id = format!("{timestamp}-{slug}");

    let trail = Trail {
        schema: SCHEMA,
        run_id,
        task: task.to_string(),
        base_commit: base_commit.to_string(),
        started_at,
        provenance: Provenance {
            trellis_version: env!("CARGO_PKG_VERSION").to_string(),
            model: model.map(str::to_string),
            tier: None,
        },
        // Computed over the section as built, before anything else touches
        // it. A digest taken later would attest to whatever had happened in
        // between, which is the opposite of the point.
        factual_sha256: Trail::compute_factual_sha256(&factual),
        factual,
        // Left empty deliberately. The judgment section is the *model's*
        // opinion, and `resume` fences it as unverified — so putting a check
        // result here would label an executed fact as speculation, which is
        // the S11 confusion the split exists to prevent, inverted. It is
        // filled when a model supplies a `Done { summary }`, and not before.
        judgment: Judgment::default(),
    };

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
    radius_of(&index, query)
}

/// The radius report, over an index the caller already holds.
///
/// Split out so the interactive session can answer from an index built once
/// per session rather than once per question. The two paths must not drift:
/// a session that renders a radius differently from `trellis radius` would be
/// two products wearing one name.
pub fn radius_of(index: &trellis_index::Index, query: &str) -> Result<()> {
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
    Ok(())
}

fn cmd_doctor(root: &Path, model_spec: Option<&str>) -> Result<()> {
    use trellis_model::{AdapterTier, BatchingCapability, Probe};

    let repo_cfg = config::RepoConfig::load(root)?;
    let cfg = repo_cfg.checks();

    // Unknown until measured. Reporting an optimistic guess is the failure
    // S17 and S18 both describe: the user cannot tell tier 2 from tier 4, or
    // a batching runtime from one that will serialize their agents.
    let mut model_id = "not configured".to_string();
    let mut runtime = "not configured".to_string();
    let mut tier: Option<AdapterTier> = None;
    let mut batching = BatchingCapability::Unknown;
    let mut notes: Vec<String> = Vec::new();

    if let Some(spec) = model_spec {
        let spec: trellis_model::ModelSpec = spec.parse()?;
        let trellis_model::ModelSpec::Runtime { dialect, base } = &spec else {
            anyhow::bail!(
                concat!(
                    "`{}` is a scripted model, which has no runtime to probe. ",
                    "Pass a serving runtime, e.g. llamacpp:http://127.0.0.1:8080."
                ),
                spec
            );
        };
        let client = trellis_model::RuntimeClient::new(base, *dialect, None)?
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
        typecheck: version_of(&cfg.typecheck),
        tests: version_of(&cfg.tests),
        coverage_available: coverage_available(&cfg.tests),
    };

    for line in probe.report() {
        println!("{line}");
    }
    println!("{}", floor_line(model_spec.is_some()));
    for line in repo_lines(root, &repo_cfg) {
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

/// Scaffolds `.trellis/` and reports what is missing before it bites.
///
/// `04-v1.0-harness.md`, task item 7. Two jobs in one command, and the second
/// is the one that matters: writing three files is easy, and a user who
/// discovers on their first run that no type checker is installed has already
/// formed an opinion about the tool.
///
/// **Nothing here is overwritten.** These files live in the user's
/// repository, and a scaffolder that clobbers a config someone tuned is worse
/// than one that refuses to run at all.
fn cmd_init(root: &Path) -> Result<()> {
    if !root.join(".git").exists() {
        anyhow::bail!(concat!(
            "this is not a git repository, and TRELLIS rolls back with git.\n",
            "  git init && git add -A && git commit -m 'initial'\n",
            "then run `trellis init` again."
        ));
    }

    let dir = root.join(".trellis");
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    for (rel, body) in [
        ("config.toml", CONFIG_TEMPLATE),
        ("context.md", CONTEXT_TEMPLATE),
        ("governance/README.md", GOVERNANCE_TEMPLATE),
    ] {
        let path = dir.join(rel);
        if path.exists() {
            kept.push(rel);
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        wrote.push(rel);
    }

    for f in &wrote {
        println!("created  .trellis/{f}");
    }
    for f in &kept {
        println!("kept     .trellis/{f} (already present, not overwritten)");
    }
    if wrote.is_empty() {
        println!("nothing to create; `.trellis/` is already set up.");
    }

    println!();
    println!("Prerequisites");
    let cfg = config::RepoConfig::load(root)?.checks();
    let mut blocking = 0;

    match version_of(&cfg.typecheck) {
        Some(v) => println!("  typecheck  {v}"),
        None => {
            blocking += 1;
            println!("  typecheck  NOT FOUND: {}", cfg.typecheck.join(" "));
            println!("             No typecheck verdict can be produced, so no run can");
            println!("             converge. Install it, or set [checks] typecheck in");
            println!("             .trellis/config.toml to the command you already use.");
        }
    }
    match version_of(&cfg.tests) {
        Some(v) => println!("  tests      {v}"),
        None => {
            blocking += 1;
            println!("  tests      NOT FOUND: {}", cfg.tests.join(" "));
            println!("             The grounding contract has nothing to ground on. A");
            println!("             repository without a test suite is a stated v1.0");
            println!("             non-goal, not an oversight.");
        }
    }

    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => println!("  worktree   clean"),
        Ok(p) => {
            // Not counted as blocking: `init` itself just wrote files, so a
            // dirty tree here is usually this command's own doing and saying
            // "blocked" would be alarming and wrong.
            println!("  worktree   {} uncommitted change(s)", p.len());
            println!("             A run needs a clean tree so rollback is exact. Commit");
            println!("             what you want to keep — including .trellis/ — first.");
        }
        Err(e) => println!("  worktree   cannot be read: {e}"),
    }

    println!();
    if blocking > 0 {
        println!("{blocking} prerequisite(s) missing. `trellis doctor` re-checks at any time.");
    } else {
        println!("Ready. `trellis doctor` re-checks at any time, and `trellis` opens a session.");
    }
    Ok(())
}

/// The scaffolded config.
///
/// Every value is commented out and shows the default it would replace, so
/// the file documents what TRELLIS will do rather than changing it. A
/// scaffold that writes active settings makes the tool's behaviour depend on
/// a file the user never read.
const CONFIG_TEMPLATE: &str = r#"# TRELLIS configuration. Every setting below is commented out and shows the
# default it would override, so this file starts by changing nothing.
#
# This file belongs in version control. `.trellis/index.bin` does not, and
# TRELLIS writes a .gitignore next to it saying so.

# [checks]
# What TRELLIS runs to decide whether a change is correct. These are the only
# source of a passing verdict: the model never issues one.
#
# `--no-incremental` on mypy is a correctness flag, not a preference. mypy
# keys its cache on file size and mtime, and a rollback restores a previous
# version of the same byte length — so without it mypy can report a cached
# result for code it never read.
# typecheck = ["mypy", "--no-incremental"]
# tests     = ["pytest", "-q"]
# python    = "python"

# [budget]
# A run stops when any of these is reached, and rolls back.
# max_iterations   = 40
# max_wall_clock_s = 1200
# max_tokens       = 2000000
# regime           = "local"   # or "metered", when tokens cost money

# [model]
# Saves retyping a runtime address every session. Deliberately not a place for
# a key: this is a host and port, or a path to a script. Your API key never
# goes in a file TRELLIS reads — configure the runtime with it directly.
# spec          = "llamacpp:http://127.0.0.1:8080"
# unconstrained = false

# [governance]
# Paths no run may edit, enforced at the orchestrator's write path rather than
# asked of the model. A rule that lives only in a prompt is a suggestion.
# deny_write = ["vendor/", "*.lock", "migrations/"]
"#;

/// The starter context file.
///
/// Written as questions rather than as a template to fill in. An empty
/// headed outline gets committed empty; a question either gets answered or
/// gets deleted, and both are better than a heading with nothing under it.
const CONTEXT_TEMPLATE: &str = r#"# Project context

Notes for whatever is driving the loop. This file is read verbatim and put in
front of the model at the start of every run, so it is worth keeping short:
everything here costs context window on every single turn.

It **informs**; it does not bind. Nothing written here can relax the grounding
contract — a note saying "the tests are slow, just declare success" changes
nothing, because convergence reads an executed check. Rules that must bind go
in `[governance] deny_write` or `.trellis/governance/`, which the orchestrator
enforces at the write path.

Worth answering, if the answer is not obvious from the code:

- What is this project for, in two sentences?
- Which directories are the important ones, and which are generated?
- Any convention a newcomer gets wrong on their first change?
- Anything that looks like a bug and is deliberate?

Delete the questions as you answer them, or delete the file: an empty context
file is treated as no context rather than as an instruction to say nothing.
"#;

/// What governance rules are, next to where they go.
///
/// A `README.md` rather than an example `.toml`, deliberately: `governance/`
/// loads every `*.toml` in it and a malformed rule is a hard error, so an
/// example file shipped here would either load as a real rule or break the
/// repository on first run.
const GOVERNANCE_TEMPLATE: &str = r#"# Governance rules

One `.toml` file per rule in this directory. Every `*.toml` here is loaded,
and a malformed one is an error rather than a skip — a rule that quietly
failed to load would leave you believing it was in force.

That is also why this file is Markdown: an example rule saved as `.toml` would
be loaded as a real one.

A rule looks like this:

    name = "No direct DB access from handlers"
    rank = "non-negotiable"          # or "nice-to-have"
    description = "Handlers go through the repository layer."
    forbid = { pattern = "psycopg2.connect" }
    scope = ["app/handlers/"]        # omit for the whole repository

Two mechanisms, with different consequences:

- `forbid` fails an audit after the fact. A `non-negotiable` violation stops
  the run from converging and the tree is rolled back; a `nice-to-have` is
  reported and does not.
- `deny-write = { paths = [...] }` refuses an edit before it reaches disk.
  Enforced at the orchestrator's only write path, so it binds whatever the
  model intended.

Matching is substring only, and that is deliberate. A governance check that
exercised judgment would be a model deciding whether it had followed the
rules, which is the thing this tool exists not to do.
"#;

/// Runs `<program> --version` and returns the first line, trimmed.
///
/// A version rather than a yes/no. "found" cannot tell the checker the user
/// configured from a different one earlier on `PATH`, and a verdict is only
/// as good as the process that produced it.
fn version_of(cmd: &[String]) -> Option<String> {
    let (program, pre) = cmd.split_first()?;
    // The configured command may be `python -m mypy`, where `--version` has
    // to follow the module, not the interpreter.
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--version");
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Whether the test runner has a coverage plugin available.
///
/// Reported because `Verified` versus `Reviewed` (S3) is the distinction the
/// product turns on, and with no coverage data nothing can populate
/// `Verified` — a user reading `0 verified` should know whether that is a
/// result or a missing tool.
fn coverage_available(tests: &[String]) -> bool {
    let Some((program, pre)) = tests.split_first() else {
        return false;
    };
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--help");
    c.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_lowercase();
            text.contains("--cov")
        })
        .unwrap_or(false)
}

/// The repository-side half of `doctor`: index, worktree, budget, floor.
///
/// `04-v1.0-harness.md` asks for these four alongside the model probe, and
/// every line that can be wrong says what to do about it.
fn repo_lines(root: &Path, cfg: &config::RepoConfig) -> Vec<String> {
    let mut out = Vec::new();

    // Index. Deliberately NOT the milestone document's "91.3% resolved".
    //
    // CLAUDE.md: what the resolver bound is a lower bound on recall, not
    // recall, and it must never be compared to the thresholds. Printing it
    // bare next to a tick would make `doctor` the place that claim finally
    // escapes — the same failure ADR 0014 fixed in `radius`, which had
    // printed "3.7% resolved" for a resolver audited at 72%.
    let started = std::time::Instant::now();
    let index = trellis_index::build_cached(root);
    let built = started.elapsed();
    let c = &index.coverage;
    if index.files.is_empty() {
        out.push(
            concat!(
                "index:      EMPTY — no Python sources found under this root.\n",
                "            Check --root, or whether the tree is one `discover` skips ",
                "(.venv,\n            site-packages, build, dist, __pycache__)."
            )
            .to_string(),
        );
    } else {
        out.push(format!(
            "index:      {} definitions across {} files · {} of {} first-party call sites \
             bound\n            ({:.1}% — a lower bound on recall, NOT recall; only the \
             hand audit is\n            comparable to the gate. See CLAUDE.md) · {} ms",
            index.symbols.len(),
            index.files.len(),
            c.resolved,
            c.total(),
            c.resolved_fraction().unwrap_or(0.0) * 100.0,
            built.as_millis()
        ));
    }

    // Worktree. The first wall users hit, so it says how to get past it.
    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => out.push("worktree:   clean".to_string()),
        Ok(p) => out.push(format!(
            "worktree:   {} uncommitted change(s) — `trellis run` will refuse to start.\n\
             {:12}Commit them, stash them, or `/revert` in a session. A run needs a clean\n\
             {:12}tree so rollback is exact (ADR 0009).",
            p.len(),
            "",
            ""
        )),
        Err(e) => out.push(format!(
            "worktree:   cannot read git status: {e}\n\
             {:12}TRELLIS needs a git repository; `git init` if this is not one.",
            ""
        )),
    }

    // Budget, in the units it is actually enforced in.
    let b = cfg.budget();
    out.push(format!(
        "budget:     {:?} — {} iterations / {} s / {} tokens",
        b.regime,
        b.max_iterations,
        b.max_wall_clock.as_secs(),
        b.max_tokens
    ));

    out
}

/// The model floor line.
///
/// The milestone document's mockup shows `Floor  above measured minimum ✓`.
/// **TRELLIS cannot know that**, and saying it would be the exact failure S7
/// exists to prevent. A floor is an interval located by a configuration that
/// *passes*; two have been measured and both are below it, so there is no
/// minimum to be above. The honest line says so and points at the evidence.
fn floor_line(model_configured: bool) -> String {
    if model_configured {
        concat!(
            "floor:      unknown for this model. The S7 floor is not located: two ",
            "configurations\n            have been measured (0.5B, Coder-3B) and both ",
            "failed every task, so\n            there is no measured minimum to be above. ",
            "See BENCHMARK_AND_EVAL.md §5."
        )
        .to_string()
    } else {
        concat!(
            "floor:      not applicable without --model. The S7 floor is not located ",
            "either:\n            two configurations measured, both below it. See ",
            "BENCHMARK_AND_EVAL.md §5."
        )
        .to_string()
    }
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

#[cfg(test)]
mod interrupt_tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trellis-cli-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.invalid"],
            vec!["config", "user.name", "t"],
            vec!["config", "core.autocrlf", "false"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        dir
    }

    /// v1.0 injection point 6: SIGINT mid-run.
    ///
    /// Whether `tokio::signal::ctrl_c()` fires is tokio's contract. What
    /// TRELLIS does once it has is this: roll back, record, exit 130.
    #[test]
    fn an_interrupt_rolls_back_and_still_records_the_run() {
        let dir = temp_repo("interrupt");
        let snap = trellis_vcs::snapshot(&dir).unwrap();

        // The state an interrupt actually catches: the run has written.
        std::fs::write(dir.join("a.py"), "def f():\n    return 999\n").unwrap();
        std::fs::write(dir.join("scratch.py"), "x = 1\n").unwrap();

        let restored = handle_interrupt(
            &dir,
            "a task that was interrupted",
            &["a.py".to_string()],
            &snap,
            "scripted:none",
        );

        assert!(restored, "an abort must restore the tree");
        assert_eq!(
            RunExit::Interrupted { restored }.code(),
            130,
            "the shell convention for a SIGINT abort"
        );

        // Deliberately not `is_restored()`, which also requires HEAD to be
        // unchanged. Committing the trail moves HEAD forward by exactly one,
        // on purpose — that is how the record survives the rollback. The
        // property an abort owes the user is about the *working tree*: no
        // edit of this run left in it, and clean enough that the next run can
        // start, because a Ctrl-C that blocks the next run is a wall.
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        assert!(!dir.join("scratch.py").exists());
        assert!(
            trellis_vcs::snapshot(&dir).is_ok(),
            "the worktree must be clean, or Ctrl-C has become a wall"
        );

        // And the run is still on the record. A trail written after the
        // rollback survives it; one written before would have been cleaned.
        let runs: Vec<_> = std::fs::read_dir(dir.join(".trellis/runs"))
            .expect("the trail directory exists")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(runs.len(), 1, "an interrupted run is still a run");
        let body = std::fs::read_to_string(runs[0].path()).unwrap();
        assert!(
            body.contains("a task that was interrupted"),
            "the trail records what was being attempted: {body}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_interrupt_whose_rollback_fails_exits_differently() {
        // Exit 2 rather than 130: a script wrapping TRELLIS must not treat a
        // worktree still holding a half-finished run as an ordinary abort.
        let dir = temp_repo("interrupt-unrestorable");
        let mut snap = trellis_vcs::snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 999\n").unwrap();
        snap.head = "0".repeat(40);

        let restored = handle_interrupt(&dir, "t", &["a.py".to_string()], &snap, "scripted:none");
        assert!(!restored);
        assert_eq!(
            RunExit::Interrupted { restored }.code(),
            2,
            "a worktree still holding a half-finished run is not an ordinary abort"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_rollback_reports_false_rather_than_claiming_success() {
        let dir = temp_repo("recovery-reports");
        let mut snap = trellis_vcs::snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "broken\n").unwrap();
        snap.head = "0".repeat(40);

        assert!(
            !rollback_or_explain(&snap),
            "the recovery path must never report a restoration it did not make"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_rollback_reports_true() {
        let dir = temp_repo("recovery-clean");
        let snap = trellis_vcs::snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 5\n").unwrap();

        assert!(rollback_or_explain(&snap));
        assert!(snap.is_restored().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trellis-doctor-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.invalid"],
            vec!["config", "user.name", "t"],
            vec!["config", "core.autocrlf", "false"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 1
",
        )
        .unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        dir
    }

    /// The milestone mockup prints `Index  48,102 symbols · 91.3% resolved ✓`.
    /// CLAUDE.md forbids exactly that: what the resolver bound is a lower
    /// bound on recall, and comparing it to the gate thresholds is the error
    /// ADR 0014 already fixed once in `radius`, which printed "3.7% resolved"
    /// for a resolver audited at 72%.
    #[test]
    fn the_index_line_refuses_to_present_the_bound_as_recall() {
        let dir = repo("index-line");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:"), "{lines}");
        assert!(
            lines.contains("NOT recall"),
            "the caveat is the point of the line: {lines}"
        );
        assert!(
            lines.contains("hand audit"),
            "and it must name what IS comparable: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Every line that can be wrong should say what to do about it."
    #[test]
    fn a_dirty_worktree_says_how_to_get_past_it() {
        let dir = repo("dirty");
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 2
",
        )
        .unwrap();
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("uncommitted change"), "{lines}");
        assert!(
            lines.contains("Commit them") || lines.contains("stash"),
            "the first wall users hit must name the way through: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_worktree_says_so_plainly() {
        let dir = repo("clean");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );
        assert!(lines.contains("worktree:   clean"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mockup's `Floor  above measured minimum ✓` is a claim TRELLIS
    /// cannot make. A floor is located by a configuration that *passes*; two
    /// have been measured and both failed every task, so there is no minimum
    /// to be above. Printing the tick would be the exact failure S7 exists to
    /// prevent.
    #[test]
    fn the_floor_line_never_claims_a_minimum_that_was_never_located() {
        for configured in [true, false] {
            let line = floor_line(configured);
            assert!(
                line.contains("not located"),
                "the floor is an interval and neither endpoint passes: {line}"
            );
            assert!(
                !line.contains("above measured minimum"),
                "TRELLIS cannot know this and must not print it: {line}"
            );
            assert!(
                line.contains("BENCHMARK_AND_EVAL"),
                "and it must point at the evidence: {line}"
            );
        }
    }

    #[test]
    fn an_empty_index_says_what_to_check_rather_than_reporting_zero() {
        // Zero definitions reads as a repository with no code. The likelier
        // cause is a wrong --root or a tree `discover` skips, and the line
        // says so instead of leaving a user staring at 0.
        let dir = std::env::temp_dir().join("trellis-doctor-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:      EMPTY"), "{lines}");
        assert!(lines.contains("--root"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod init_tests {
    use super::*;

    fn bare(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("trellis-init-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn repo(name: &str) -> PathBuf {
        let d = bare(name);
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.invalid"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&d)
                .output()
                .unwrap();
        }
        d
    }

    #[test]
    fn init_refuses_outside_a_repository_and_says_how_to_make_one() {
        // The command for getting started must work before you have started,
        // and must name the missing step rather than the missing root.
        let d = bare("no-git");
        let e = cmd_init(&d).unwrap_err().to_string();
        assert!(e.contains("not a git repository"), "{e}");
        assert!(e.contains("git init"), "it must name the fix: {e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn init_scaffolds_and_never_overwrites() {
        let d = repo("scaffold");
        cmd_init(&d).unwrap();
        assert!(d.join(".trellis/config.toml").exists());
        assert!(d.join(".trellis/context.md").exists());
        assert!(d.join(".trellis/governance/README.md").exists());

        // These files live in the user's repository. A scaffolder that
        // clobbers a config someone tuned is worse than one that refuses.
        std::fs::write(
            d.join(".trellis/config.toml"),
            "# mine
",
        )
        .unwrap();
        cmd_init(&d).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap(),
            "# mine
"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_scaffolded_config_parses_and_changes_nothing() {
        // Every value is commented out on purpose: a scaffold that writes
        // active settings makes the tool's behaviour depend on a file the
        // user has not read.
        let d = repo("config-inert");
        cmd_init(&d).unwrap();
        let cfg = config::RepoConfig::load(&d).expect("the scaffold must parse");
        assert_eq!(
            cfg.checks().typecheck,
            trellis_checks::CheckConfig::default().typecheck,
            "the scaffold must not change what runs"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_governance_example_is_markdown_so_it_is_never_loaded_as_a_rule() {
        // `governance/` loads every *.toml and a malformed one is a hard
        // error, so an example rule shipped as TOML would either take effect
        // or break the repository on first run.
        let d = repo("governance-example");
        cmd_init(&d).unwrap();
        let rules = trellis_checks::governance::load(&d).expect("the scaffold must not break it");
        assert!(rules.is_empty(), "a scaffold must not install live rules");
        let _ = std::fs::remove_dir_all(&d);
    }
}
