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

mod ask;
mod config;
mod hosted;
mod orchestrate;
mod process_agent;
mod session;
#[cfg(test)]
mod testutil;
mod tooling;
mod trust_cmd;
mod tui_bridge;
mod tui_run;
mod vault_cmd;
mod work;
// ADR 0025 (W-D).
mod context_files;
mod output;
mod rpc;
mod session_extra;
mod templates;
mod user_config;
// Integration across waves (doctor's report of the inputs above).
mod doctor_extra;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write as _;
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

    /// Also write every tracing event as one JSON object per line to this
    /// file, appending. For `run` and `work` left unattended (ADR 0020 §7).
    ///
    /// A second formatter on the one subscriber, not a second telemetry
    /// path (ADR 0008). Global so it reaches `run` and `work` without
    /// adding a field to either.
    #[arg(long, global = true, value_name = "PATH")]
    log_json: Option<PathBuf>,

    /// Write JSONL to stdout: the same events `--log-json` writes, between
    /// a `run_start` and a `run_end` record. Human text goes to stderr.
    /// For `run`, `work` and `log` (ADR 0025 §4).
    ///
    /// Global for the reason `--log-json` is: it reaches `work` without a
    /// field in its arguments. Refused by name on any other command.
    #[arg(long, global = true)]
    json: bool,

    /// Omit to enter an interactive session.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Build or refresh the index; print coverage.
    Index {
        /// Also draw the codebase as a graph of modules.
        ///
        /// Coverage says how much of the graph the resolver bound. This says
        /// what the graph *is*: which modules call into which, and how much
        /// doubt sits on each.
        #[arg(long)]
        graph: bool,
        /// How many modules to draw, busiest first.
        #[arg(long, default_value = "40")]
        limit: usize,
    },

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
        ///
        /// Falls back to `[model] spec` in `.trellis/config.toml`, which is
        /// where a repository states the runtime it actually has. Requiring
        /// the flag anyway meant `trellis run` refused to start beside a
        /// config that named the model precisely.
        #[arg(long)]
        model: Option<String>,
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
        /// Arguments for the `--prompt` template, as one string split
        /// shell-style: `--args '"app/x.py" strict'` gives `$1` and `$2`.
        ///
        /// One string rather than a repeated flag so the session and `rpc`,
        /// which also receive one string, split it the same way (ADR 0025).
        #[arg(long, requires = "prompt", value_name = "STRING")]
        args: Option<String>,
        /// Load this skill's body into the run's context, rather than waiting
        /// for the model to ask for it. Repeatable. Informs; never binds.
        #[arg(long, value_name = "NAME")]
        skill: Vec<String>,
        /// Compile this note from the `[context] vault` and show it to the
        /// model, overriding `[context] focus`.
        ///
        /// A flag because the alternative, editing config.toml to switch
        /// note, dirties the worktree, and a run refuses a dirty worktree.
        /// The note informs; it cannot make a run converge (ADR 0017).
        #[arg(long)]
        focus: Option<String>,
        /// Ask before each listed tool call (`[control] approve`, default
        /// `edit`), whatever `[control] mode` says. Needs a terminal.
        #[arg(long, conflicts_with = "auto")]
        approve: bool,
        /// Ask nothing, whatever `[control] mode` says. Budgets, governance
        /// and the checks still apply.
        #[arg(long)]
        auto: bool,
    },

    /// The line-based session: one command at a time, scrollback kept.
    ///
    /// What bare `trellis` used to do. It is still what runs without a
    /// terminal, and it holds the commands the full-screen view does not —
    /// /diff, /accept, /revert, /radius.
    Session,
    /// Scaffold `.trellis/` and report what is missing before it bites.
    Init,

    /// Watch a run happen: actions, transcript, edits, checks, radius, budget.
    ///
    /// The line-based session stays the default. This is the surface for
    /// watching one run rather than issuing a sequence of commands, and it
    /// renders the same `AppState` the standalone `tui_demo` binary does —
    /// only the source of events differs.
    Tui {
        /// A task to start immediately. Omit it to open idle and type one.
        task: Option<String>,
        /// Override the configured model, same spec as `trellis run --model`.
        #[arg(long)]
        model: Option<String>,
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

    /// Dry run: which symbols may be worked on in parallel, and why not.
    ///
    /// Takes the symbols to be changed rather than a task sentence.
    /// Decomposing prose into work units needs a model, and the planner does
    /// not — so the debugging tool works today and does not wait on one.
    Partitions {
        /// One or more symbols this change would touch.
        #[arg(required = true)]
        symbols: Vec<String>,
    },

    /// One line per recorded run: task, outcome, integrity.
    Log {
        /// How many to show, newest first.
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Also list what each run did, action by action, in order.
        #[arg(long)]
        actions: bool,
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

    /// Add, list, change, approve and export tasks for `trellis work`
    /// (ADR 0020, ADR 0026).
    ///
    /// The queue lives in the git directory, so it never dirties the
    /// worktree and survives a rollback.
    Queue {
        #[command(subcommand)]
        action: work::QueueAction,
    },

    /// Run queued tasks one at a time, oldest first, through `trellis run`.
    ///
    /// Stops at the first converged change, which waits for review; never
    /// claims a task while the worktree is dirty.
    Work(work::WorkArgs),

    /// Check, search, inspect and edit an Apex-format note vault (ADR 0022).
    ///
    /// Prints Apex's report line; writes notes with exactly two frontmatter
    /// keys and keeps manifest.json, layout.json and dates.json in step.
    Vault(vault_cmd::VaultArgs),

    /// Show what this repository's hooks, custom tools and bash settings
    /// would execute, and trust them (ADR 0023).
    ///
    /// Trust is kept outside the repository and covers a digest of every
    /// definition and every file they name; any change undoes it.
    Trust {
        /// Show what would run and whether it is trusted; record nothing.
        #[arg(long, conflicts_with = "revoke")]
        show: bool,
        /// Forget this repository's trust.
        #[arg(long)]
        revoke: bool,
    },

    /// Agent profiles in .trellis/agents/: list, show, pause, resume
    /// (ADR 0026).
    Agents {
        #[command(subcommand)]
        action: orchestrate::AgentsAction,
    },

    /// Scheduled tasks in .trellis/routines/: list, or fire one now
    /// (ADR 0026). `work` fires them when due.
    Routines {
        #[command(subcommand)]
        action: orchestrate::RoutinesAction,
    },

    /// JSONL commands on stdin, responses and events on stdout (ADR 0025 §5).
    ///
    /// `prompt` runs a task exactly as `trellis run` does. One run at a
    /// time; the clean-worktree rule is unchanged. Human text goes to stderr.
    Rpc,
}

#[derive(Subcommand)]
enum ModelAction {
    /// Download weights, resuming an interrupted fetch.
    Pull { name: Option<String> },
    /// Print where the weights live, fetched or not.
    Path { name: Option<String> },
}

fn main() -> Result<()> {
    // `Targets` rather than `EnvFilter`.
    //
    // `EnvFilter` parses field and span predicates, which needs `matchers` and
    // therefore the whole regex engine. Measured two ways, because they answer
    // different questions and were confused for each other once already:
    // `cargo bloat --release --crates` attributed 184 KiB of `regex_syntax`
    // plus 146 KiB of `regex_automata` to the `.text` section, and the release
    // binary on disk fell 5,855,232 -> 4,810,240 bytes, the larger figure
    // being the Unicode tables in `.rdata` that `.text` does not count.
    //
    // This project only ever filters as `target=level`, and `Targets` matches
    // targets by prefix exactly as `EnvFilter` did, so `trellis_loop=trace`
    // and `trellis_loop::adapter=trace` behave identically.
    //
    // Two forms behave differently and neither is used here: a span-field
    // predicate (`[span{field=x}]`) fails to parse and falls back to the
    // default below, and a target with a field predicate (`trellis[{turn}]=…`)
    // parses but matches nothing, silently. Named because "the trade is
    // recorded" is only true if the record is accurate.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let default =
        || tracing_subscriber::filter::Targets::new().with_target("trellis", tracing::Level::INFO);
    let filter = match std::env::var("RUST_LOG") {
        // Set but empty means "say nothing", which is what `EnvFilter` did
        // with it. Parsing it as a directive instead yields one empty
        // directive, which `Targets` reads as ERROR for every target in the
        // process — turning silence into global error logging.
        Ok(s) if s.trim().is_empty() => tracing_subscriber::filter::Targets::new(),
        Ok(s) => s.parse().unwrap_or_else(|_| default()),
        Err(_) => default(),
    };
    // Parsed first so `--log-json` is known before the subscriber is built.
    let cli = Cli::parse();
    let json = match &cli.log_json {
        Some(p) => {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .with_context(|| format!("cannot open --log-json {}", p.display()))?;
            Some(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(f)),
            )
        }
        None => None,
    };
    // ADR 0025 §4: `--json` and `rpc` make stdout the JSON channel. Checked
    // before anything prints, and refused by name where it would do nothing.
    let is_rpc = matches!(cli.command, Some(Command::Rpc));
    if cli.json
        && !matches!(
            cli.command,
            Some(Command::Run { .. } | Command::Work(_) | Command::Log { .. })
        )
    {
        anyhow::bail!("--json applies to `run`, `work` and `log`; `rpc` always writes JSON");
    }
    output::set_json(cli.json || is_rpc);
    // Per-layer filters rather than one global one: the stdout event layer
    // has a fixed `trellis=info` so a `RUST_LOG` meant for debugging cannot
    // change what a machine reader receives. `RUST_LOG` still governs the
    // human stream and `--log-json`, exactly as before.
    use tracing_subscriber::Layer;
    let stdout_json = output::json().then(|| {
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(std::io::stdout)
            .with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target("trellis", tracing::Level::INFO),
            )
    });
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(output::human)
                .with_filter(filter.clone()),
        )
        .with(json.map(|l| l.with_filter(filter)))
        .with(stdout_json)
        .with(is_rpc.then(rpc::TurnCapture::default))
        .init();

    let explicit = cli.root.is_some();
    // Two commands are exempt, and both for the same reason: they are how
    // someone gets ready to use a repository, so requiring one inverts the
    // order. `init` scaffolds `.trellis/` in a directory that is not set up
    // yet; `model` fetches weights, which are user state and live outside any
    // repository — the dispatch arm below says so and, until this list
    // included it, was wrong, because discovery already failed up here.
    //
    // Every other command refusing is deliberate. `trellis index` in a home
    // folder once walked 25,147 files before anyone could tell it had not
    // found a repository.
    //
    // `vault init` joins them for the same reason (ADR 0022 §3, integrated):
    // it creates a vault, possibly before there is a repository to hold it.
    // Every other `vault` action still reads one.
    let needs_repo = !matches!(cli.command, Some(Command::Init | Command::Model { .. }))
        && !matches!(&cli.command, Some(Command::Vault(a)) if a.is_init());
    let root = match cli.root {
        Some(r) => r,
        None => match discover_root() {
            Ok(r) => r,
            Err(_) if !needs_repo => std::env::current_dir()?,
            Err(e) => return Err(e.context("cannot determine the repository root")),
        },
    };
    // ADR 0024 §3: every `key_env` in the effective configuration, the
    // user's file included, is removed from each child process trellis
    // starts. Before any command runs, so no spawn precedes it. A config that
    // does not load registers nothing here; the command reports that itself,
    // and the two built-in key variables are removed regardless.
    if let Ok(cfg) = config::RepoConfig::load(&root) {
        trellis_core::secrets::register_key_vars(
            cfg.providers.iter().flatten().map(|p| p.key_env.as_str()),
        );
    }
    // Say which tree is about to be read whenever it was inferred.
    //
    // Both directions are surprises worth naming. Acting on a directory
    // several levels up ends with someone running a task against the wrong
    // tree. Falling back to the working directory is worse if it happens to
    // be a home folder — `trellis index` there walked 1,295 files before
    // anyone could tell it had not found a repository.
    if !explicit && root != std::env::current_dir().unwrap_or_default() {
        // To the human stream: under `--json` stdout is JSONL only.
        let _ = writeln!(
            output::human(),
            "root:     {} (repository root)",
            root.display()
        );
    }

    let Some(command) = cli.command else {
        // Bare `trellis` opens the full-screen interface. It used to open the
        // line session, and people kept typing `trellis` expecting the thing
        // they had been shown — which is a fair expectation to have of the
        // default surface.
        //
        // The line session is still there as `trellis session`, and is still
        // what runs when there is no terminal to take over: a pipe, a
        // redirect, or CI.
        let spec = config::RepoConfig::load(&root).ok().and_then(|c| c.model());
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return tui_run::run(&root, None, spec, false);
        }
        return session::Session::open(&root, None)?.run();
    };

    match command {
        Command::Index { graph, limit } => cmd_index(&root, graph, limit),
        Command::Doctor { model } => {
            // Falls back to the repository's configured model.
            //
            // It did not, and the result was `doctor` reporting "not
            // configured" about a repository whose `.trellis/config.toml`
            // names a model and whose TUI shows it in the status bar. The
            // one command whose job is "is this set up" was the one command
            // that did not look.
            let configured = config::RepoConfig::load(&root).ok().and_then(|c| c.model());
            let spec = model.as_deref().map(str::to_string).or(configured);
            cmd_doctor(&root, spec.as_deref(), model.is_some())
        }
        Command::Check { paths } => cmd_check(&root, paths),
        Command::Radius { symbol } => cmd_radius(&root, &symbol),
        Command::Run {
            task,
            model,
            unconstrained,
            agents,
            resume,
            prompt,
            args,
            skill,
            focus,
            approve,
            auto,
        } => {
            // Checked here rather than in `cmd_run`: the session and the
            // interface always run one agent, and this is the only caller
            // that takes the flag.
            if agents != "1" {
                anyhow::bail!(concat!(
                    "multi-agent mode is v1.1 and is gated on single-agent numbers that ",
                    "do not exist yet. Use --agents 1."
                ));
            }
            let task = match &prompt {
                Some(name) => {
                    // ADR 0025 §3: frontmatter stripped, `--args` substituted.
                    let template = templates::render(&root, name, args.as_deref())?;
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
            let model_from_user = model.is_some();
            // A config that fails to load says why, rather than reading as
            // "no model": the refusal may be the point (ADR 0024 §3).
            let model = match model {
                Some(m) => m,
                None => config::RepoConfig::load(&root)?.model().context(
                    "no model. Pass --model, or set [model] spec in \
                     .trellis/config.toml",
                )?,
            };
            let flag = match (approve, auto) {
                (true, _) => Some(config::ControlMode::Approve),
                (_, true) => Some(config::ControlMode::Auto),
                _ => None,
            };
            let exit = cmd_run_with(
                &root,
                &task,
                &model,
                unconstrained,
                RunSeeding {
                    resume,
                    focus: focus.as_deref(),
                    profile: None,
                    model_from_user,
                },
                RunControl {
                    flag,
                    human: Human::terminal(),
                },
                output::RunExtras {
                    skills: skill,
                    ..output::RunExtras::default()
                },
                &mut output::human(),
            )?;
            // The one place that decides the process ends, because this is
            // the one caller that *is* the process.
            if exit.code() != 0 {
                std::process::exit(exit.code());
            }
            Ok(())
        }
        Command::Session => session::Session::open(&root, None)?.run(),
        Command::Init => cmd_init(&root),
        Command::Tui { task, model } => {
            let from_user = model.is_some();
            let spec = model.or_else(|| config::RepoConfig::load(&root).ok()?.model());
            tui_run::run(&root, task, spec, from_user)
        }
        Command::Resume => cmd_resume(&root),
        Command::Log { limit, .. } if output::json() => {
            output::log_json(&root, limit, &mut std::io::stdout())
        }
        Command::Log { limit, actions } => cmd_log(&root, limit, actions),
        Command::Why { symbol } => cmd_why(&root, &symbol),
        // Takes no root at all: models are user state, not repository state.
        // The exemption that makes this reachable outside a repository is in
        // `needs_repo` above — this arm alone was never enough.
        Command::Model { action } => cmd_models(action),
        Command::Partitions { symbols } => cmd_partitions(&root, &symbols),
        Command::Queue { action } => work::queue(&root, action),
        Command::Agents { action } => orchestrate::agents(&root, action),
        Command::Routines { action } => orchestrate::routines(&root, action),
        Command::Work(args) => {
            // ADR 0025 §4: each task's run brackets itself; these bracket
            // the invocation, so a reader knows when `work` itself is done.
            let frames = output::json().then(output::stdout_sink);
            if let Some(s) = &frames {
                output::frame(s, &serde_json::json!({"type": "work_start", "version": 1}));
            }
            let result = work::work(&root, args);
            if let Some(s) = &frames {
                output::frame(
                    s,
                    &match &result {
                        Ok(code) => serde_json::json!({"type": "work_end", "exit": code}),
                        Err(e) => serde_json::json!({
                            "type": "work_end", "exit": 1, "error": format!("{e:#}")
                        }),
                    },
                );
            }
            let code = result?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Command::Vault(args) => vault_cmd::run(&root, args),
        Command::Trust { show, revoke } => trust_cmd::run(&root, show, revoke),
        Command::Rpc => rpc::serve(
            &root,
            std::io::stdin().lock(),
            output::stdout_sink(),
            rpc::Mode::Process,
        ),
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
fn cmd_log(root: &Path, limit: usize, actions: bool) -> Result<()> {
    let entries = trellis_trail::query::all(root);
    if entries.is_empty() {
        println!("no trails under .trellis/runs. `trellis run` writes one.");
        return Ok(());
    }

    println!("{} run(s), newest first", entries.len());
    println!();
    for e in entries.iter().take(limit) {
        let f = &e.trail.factual;
        println!("{}  {}", plain(&e.run_id()), e.outcome());
        println!("    {}", plain(&e.trail.task));
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
        if actions {
            print_actions(f);
        }
        println!();
    }
    if entries.len() > limit {
        println!("... {} older, use --limit", entries.len() - limit);
    }
    Ok(())
}

/// A string from a trail, made safe to print to a terminal.
///
/// A trail is read back as untrusted data (S10) and then printed straight to a
/// terminal, which interprets what it is given. An escape sequence in a task
/// description or a file path can move the cursor, repaint a line, or clear
/// the screen — so a run that did not converge can be made to *look* like one
/// that did, in the one command a user reads to find out which.
///
/// That matters more here than in most tools. The claim this project makes is
/// that work comes back verified rather than merely written, and a forged
/// terminal defeats the reading of the evidence rather than the evidence
/// itself.
///
/// Control characters become their escaped form rather than vanishing: a path
/// that contains one is worth seeing, and silently dropping it would print a
/// name that does not match the file.
fn plain(s: &str) -> String {
    if !s.chars().any(char::is_control) {
        return s.to_string();
    }
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The action log for one run: what it did, in order, and what came back.
///
/// **A trail with no actions and a run that took none are different things**,
/// and printing "0 actions" for both would make the older trails lie. Trails
/// written before the field existed say so; a run that genuinely did nothing
/// says that instead.
///
/// Read back as data, never as instruction (S10). These strings come from a
/// file in a repository that takes contributions, so they are rendered inside
/// a marked block and nothing here interprets them.
fn print_actions(f: &trellis_trail::UntrustedFactual) {
    if f.actions.is_empty() {
        // The turn count is the tell. A run that reached turns but recorded
        // no actions was written by a version that had nowhere to put them.
        let recorded_turns = f.context.as_ref().map_or(0, |c| c.turns_measured);
        println!(
            "    actions: {}",
            if recorded_turns > 0 {
                format!(
                    "not recorded — this trail predates the action log ({recorded_turns} turns)"
                )
            } else {
                "none; the run ended before it acted".to_string()
            }
        );
        return;
    }
    println!("    actions ({}), as recorded:", f.actions.len());
    for a in &f.actions {
        // The marker sits where a reader's eye already is, and says the one
        // thing the rest of the line cannot: whether the tree changed.
        let wrote = if a.wrote { "*" } else { " " };
        println!(
            "    {wrote} {:>2}. {} {}",
            a.turn,
            plain(&a.tool),
            plain(&a.target)
        );
        if !a.outcome.is_empty() {
            println!("          {}", plain(&a.outcome));
        }
    }
    if f.actions.iter().any(|a| a.wrote) {
        println!("    * wrote to the working tree");
    }
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
        println!("{}  {}", plain(&e.run_id()), e.outcome());
        println!("    task: {}", plain(&e.trail.task));

        // The distinction is the whole value. A run that *edited* the file is
        // evidence about why the code looks like this; a run that merely named
        // it in a task string is context, and reporting them alike would let
        // the weaker one be read as the stronger.
        // The guard is always true when it is reached, and is kept as
        // documentation rather than as a branch. `mentions_any` only yields a
        // `Mention` when `touched_file.is_some() || task_mentions`, so a
        // `None` here implies the task mentioned it. Replacing it with `true`
        // is therefore an equivalent mutant — recorded as such in
        // CORRECTNESS_LEDGER.md rather than left looking like a test gap,
        // because the next run will surface it again.
        match &m.touched_file {
            Some(f) => println!("    changed: {}", plain(f)),
            None if m.task_mentions => {
                println!("    named in the task; no file matching `{query}` was changed")
            }
            None => {}
        }
        // A person's answer in approve mode. A declined edit never reaches
        // `changed`, and "someone looked at this change and said no" is as
        // much an answer to "why does this look like this" as an edit is.
        for (target, outcome) in &m.decisions {
            println!("    {} by a person: {}", plain(outcome), plain(target));
        }

        if let Some(base) = &e.trail.base_commit {
            println!("    from commit {}", plain(base));
        }
        if let Some(p) = &e.trail.provenance {
            let model = p.model.as_deref().unwrap_or("none");
            println!(
                "    by trellis {} driving {}",
                plain(&p.trellis_version),
                plain(model)
            );
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
    let status = trellis_core::secrets::scrub(&mut std::process::Command::new("curl"))
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
}

/// What drives the loop this run.
///
/// Not a trait object: `Model::next_step` is an `async fn` in a trait, which
/// is not dyn-compatible, and the loop is generic over it.
enum Driver {
    Scripted(trellis_loop::ScriptedModel),
    Runtime(trellis_loop::RuntimeModel),
    /// Tier 1: a hosted provider's own tool calling (ADR 0024).
    ///
    /// Boxed: it carries the conversation's replayed provider messages, and
    /// the other variants should not pay for its size.
    Hosted(Box<trellis_loop::HostedModel>),
}

impl Driver {
    /// Builds the driver a parsed spec names.
    ///
    /// Takes a [`ModelSpec`] rather than a string: the string form was parsed
    /// here, in `cmd_doctor` and in the session, each deciding for itself what
    /// was valid. Construction is now the only thing this does.
    fn build(
        spec: &trellis_model::ModelSpec,
        unconstrained: bool,
        tools: &trellis_loop::ToolSet,
        cfg: &config::RepoConfig,
        from_user: bool,
    ) -> Result<Self> {
        if let Some(r) = hosted::resolve(spec, cfg, from_user)? {
            if unconstrained {
                anyhow::bail!(
                    "--unconstrained is meaningless for a hosted model: it calls tools \
                     through the provider's own API (tier 1)"
                );
            }
            return Ok(Driver::Hosted(Box::new(hosted::build(&r, cfg)?)));
        }
        match spec {
            trellis_model::ModelSpec::Scripted(path) => {
                if unconstrained {
                    anyhow::bail!("--unconstrained is meaningless for a scripted model");
                }
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read script {}", path.display()))?;
                // Against the run's own tool set: a step calling a tool this
                // run does not offer fails here, before the snapshot.
                let m = trellis_loop::ScriptedModel::from_json_with(&text, tools)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                Ok(Driver::Scripted(m))
            }
            trellis_model::ModelSpec::Runtime {
                dialect,
                base,
                model,
            } => {
                let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?;
                // Named rather than `new(client, !unconstrained)`: the flag is
                // called --unconstrained, so a negated bool at the call site
                // meant reading two negations to learn which tier runs.
                Ok(Driver::Runtime(
                    if unconstrained {
                        trellis_loop::RuntimeModel::unconstrained(client)
                    } else {
                        trellis_loop::RuntimeModel::constrained(client)
                    }
                    .with_context_bytes(cfg.context_bytes()),
                ))
            }
            trellis_model::ModelSpec::Hosted { .. } | trellis_model::ModelSpec::Provider { .. } => {
                unreachable!("resolved above")
            }
        }
    }

    /// Every compaction the driver made (ADR 0024 §5).
    fn compactions(&self) -> Vec<trellis_loop::Compacted> {
        use trellis_loop::Model;
        match self {
            Driver::Scripted(m) => m.compactions().to_vec(),
            Driver::Runtime(m) => m.compactions().to_vec(),
            Driver::Hosted(m) => m.compactions().to_vec(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Driver::Scripted(m) => format!(
                "scripted, {} steps — real loop, real checks, real rollback; judgement stubbed",
                m.len()
            ),
            // `degradation()` answers "what do you lose at this tier", and
            // printed bare it read as `LlamaCpp via RuntimeConstrained —
            // none for correctness; grammar compilation happens in the
            // serving runtime`, which says nothing to a reader who has not
            // memorised the tier table. Number the tier and label the string.
            Driver::Runtime(m) => format!(
                "{:?}, tier {} — degradation: {}",
                m.dialect(),
                match m.tier() {
                    trellis_model::AdapterTier::Native => "1, native tool calls",
                    trellis_model::AdapterTier::RuntimeConstrained =>
                        "2, schema enforced by the runtime",
                    trellis_model::AdapterTier::HarnessConstrained => "3",
                    trellis_model::AdapterTier::ValidateAndRetry =>
                        "4, no schema; the reply is validated after",
                },
                m.tier().degradation()
            ),
            Driver::Hosted(m) => hosted::describe(m),
        }
    }
}

/// What a run is seeded with beyond the task: text for the prompt that
/// informs the model and never counts toward convergence.
///
/// Grouped because both are the same kind of thing, and because the session
/// and the TUI pass neither — `default()` there says so in one word rather
/// than in a row of positional `false, None`.
#[derive(Debug, Default, Clone, Copy)]
struct RunSeeding<'a> {
    /// `--resume`: the last trail, fenced as data.
    resume: bool,
    /// `--focus`: overrides `[context] focus` for this run only.
    focus: Option<&'a str>,
    /// The agent profile a queued task runs as (ADR 0026 §4): its approve
    /// list and skill filter apply to this run, and the trail records its
    /// name. `None` for every caller but `work`.
    profile: Option<&'a trellis_queue::profile::Profile>,
    /// Whether a person chose the model spec for this run (`--model`,
    /// `/model`, an RPC request) rather than the repository (`[model] spec`,
    /// an agent profile). A hosted endpoint the repository chose must be on
    /// the user's allowlist before a key is sent to it (ADR 0024 §3).
    /// `false` by default, which is the stricter reading.
    model_from_user: bool,
}

/// Who answers an approval question in approve mode (ADR 0018).
pub(crate) enum Human {
    /// Nobody. A run in approve mode refuses to start rather than start and
    /// refuse every edit, which would look like a run that did the work.
    Absent,
    /// This terminal: the diff and the question are written to the run's
    /// output, and the answer is one line from this reader. `None` is end of
    /// input, which is a no.
    Line(Box<dyn FnMut() -> Option<String> + Send>),
    /// The full-screen interface, which shows `Modal::Approval`.
    Channel(tokio::sync::mpsc::Sender<trellis_loop::ApprovalRequest>),
}

impl Human {
    /// This process's terminal, if a person is both reading it and typing
    /// into it.
    ///
    /// Stdin because the answer has to come from somebody, and a pipe is not
    /// somebody. Stdout as well because the diff and the question are written
    /// there: with stdout redirected to a file, a person at the keyboard would
    /// be answering a question they cannot see, and a `y` given blind is not
    /// an approval.
    pub(crate) fn terminal() -> Self {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            Human::Line(Box::new(|| {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }))
        } else {
            Human::Absent
        }
    }
}

/// How much a run asks before it acts: the flag, if any, over `[control]`.
pub(crate) struct RunControl {
    /// `--approve` or `--auto`; `None` takes the configured mode.
    flag: Option<config::ControlMode>,
    human: Human,
}

impl Default for RunControl {
    fn default() -> Self {
        Self {
            flag: None,
            human: Human::Absent,
        }
    }
}

/// The exact refusal ADR 0018 names, so a script can match on it.
///
/// Both ways out are named because `session` reaches this too, and `--auto`
/// is a `run` flag: a session user told only to pass it has nowhere to pass it.
const NEEDS_A_TERMINAL: &str =
    "approve mode needs an interactive terminal; pass --auto, or set [control] mode = \"auto\"";

/// Whether a typed answer is a yes. Only an explicit one counts: the prompt
/// says `[y/N]`, and an empty line, a typo or end of input is the N.
fn is_yes(answer: Option<&str>) -> bool {
    matches!(
        answer.map(|a| a.trim().to_ascii_lowercase()).as_deref(),
        Some("y" | "yes")
    )
}

/// Answers approval questions on a line terminal until the channel closes.
///
/// The read runs on the blocking pool, not on the runtime thread, so a
/// Ctrl-C while the question is up still reaches the select in `cmd_run`
/// and aborts the run with a rollback. The reader is handed to the pool and
/// back each time because it is one stdin and there is only ever one
/// question open.
async fn answer_on_the_line(
    mut rx: tokio::sync::mpsc::Receiver<trellis_loop::ApprovalRequest>,
    mut read: Box<dyn FnMut() -> Option<String> + Send>,
    out: &mut dyn std::io::Write,
) {
    while let Some(req) = rx.recv().await {
        let _ = writeln!(out);
        for line in req.detail.lines() {
            let _ = writeln!(out, "  {}", plain(line));
        }
        let _ = write!(out, "{}? [y/N] ", plain(&req.question));
        let _ = out.flush();
        let (back, answer) = match tokio::task::spawn_blocking(move || {
            let a = read();
            (read, a)
        })
        .await
        {
            Ok(r) => r,
            // The reader panicked. Nothing can be asked again, so every
            // later question is declined by the dropped channel.
            Err(_) => return,
        };
        read = back;
        let yes = is_yes(answer.as_deref());
        if answer.is_none() {
            // End of input leaves the cursor on the question line.
            let _ = writeln!(out);
        }
        let _ = writeln!(out, "  {}", if yes { "approved" } else { "declined" });
        let _ = req.reply.send(yes);
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
    seeding: RunSeeding<'_>,
    control: RunControl,
    out: &mut dyn std::io::Write,
) -> Result<RunExit> {
    cmd_run_with(
        root,
        task,
        model_spec,
        unconstrained,
        seeding,
        control,
        output::RunExtras::default(),
        out,
    )
}

/// `cmd_run` with what ADR 0025 adds: forced skills and JSON framing.
///
/// A second entry point rather than a new parameter, so every existing
/// caller (`work`, `session`, `tui`) keeps calling `cmd_run` unchanged.
#[allow(clippy::too_many_arguments)]
fn cmd_run_with(
    root: &Path,
    task: &str,
    model_spec: &str,
    unconstrained: bool,
    seeding: RunSeeding<'_>,
    control: RunControl,
    extras: output::RunExtras,
    out: &mut dyn std::io::Write,
) -> Result<RunExit> {
    output::framed(root, task, model_spec, &extras, || {
        run_body(
            root,
            task,
            model_spec,
            unconstrained,
            seeding,
            control,
            &extras,
            out,
        )
    })
}

/// The body of a run; see `cmd_run`.
#[allow(clippy::too_many_arguments)]
fn run_body(
    root: &Path,
    task: &str,
    model_spec: &str,
    unconstrained: bool,
    seeding: RunSeeding<'_>,
    control: RunControl,
    extras: &output::RunExtras,
    out: &mut dyn std::io::Write,
) -> Result<RunExit> {
    let RunSeeding {
        resume,
        focus,
        profile,
        model_from_user,
    } = seeding;
    let cfg = config::RepoConfig::load(root)?;

    // First, because it costs nothing and needs nothing: a run in approve
    // mode with nobody to ask would deny every listed call and end looking
    // like a run that tried (ADR 0018). Before the model, before the
    // snapshot, before any file is read.
    let mut dial = cfg.control(control.flag)?;
    // A profile's approve list replaces the repository's for its runs; the
    // mode itself arrives as the flag (ADR 0026 §4).
    if let Some(list) = profile
        .and_then(|p| p.file.control.as_ref())
        .and_then(|c| c.approve.clone())
    {
        dial.approve = list;
    }
    if dial.mode == config::ControlMode::Approve && matches!(control.human, Human::Absent) {
        anyhow::bail!(NEEDS_A_TERMINAL);
    }

    // Tools, hooks and trust (ADR 0023), decided before the model is built
    // so a script is parsed against the tools this run actually offers, and
    // before the snapshot so a bad `[tools]` section costs nothing. The id is
    // stamped now and given to the trail, so `TRELLIS_RUN_ID` names the trail
    // file this run writes.
    let stamp = trellis_trail::now();
    let run_id = format!("{}-{}", stamp.0, trellis_trail::slugify(task));
    let mut tooling = tooling::prepare(root, &cfg, &run_id, model_spec)?;
    // Integrated (ADR 0026 × 0023): a profile's `tools` list narrows the
    // set here, before the driver parses a script against it and before
    // bash joins the approve list, so every consumer gets the narrowed set.
    if let Some(p) = profile
        && let Some(list) = &p.file.tools
    {
        tooling::check_profile_tools(&p.name, list, &cfg)?;
        tooling.narrow_for_profile(&p.name, list);
    }
    if tooling.tools.bash().is_some()
        && dial.mode == config::ControlMode::Approve
        && !dial.approve.iter().any(|t| t == "bash")
    {
        // In approve mode every command is put to the person.
        dial.approve.push("bash".to_string());
    }

    // The vault is compiled here, in memory, before the model is contacted
    // and before the snapshot. Writing the compiled file anywhere in the
    // repository would dirty the worktree the snapshot needs clean; and an
    // invalid vault or an over-cap note is a run that cannot start as
    // configured, which should cost nothing (ADR 0017).
    let vault_setting = cfg.vault(root, focus)?;
    // The note tools read the configured vault (ADR 0024 §6). Attached
    // before the driver is built, so a script is parsed against the same set,
    // and before the snapshot, so a note tool enabled with no vault costs
    // nothing.
    tooling.tools = tooling
        .tools
        .clone()
        .with_notes(vault_setting.as_ref().map(|v| trellis_loop::NotesSource {
            dir: v.dir.clone(),
            source: v.source.clone(),
        }))
        .map_err(|e| anyhow::anyhow!(e))?;
    let vault = match &vault_setting {
        Some(config::VaultSetting {
            dir,
            source,
            focus: Some(stem),
        }) => {
            // Integrated (ADR 0022 §7): `[context] dates` annotates headings;
            // off, this is `trellis_vault::compile` byte for byte.
            let dates_on = cfg.context.as_ref().and_then(|c| c.dates).unwrap_or(false);
            let (compiled, dates_note) = vault_cmd::compile_for_run(dir, stem, dates_on)
                .with_context(|| {
                    format!(
                        "the vault at {source} would not compile for focus `{stem}`; \
                         the run did not start and nothing was snapshotted"
                    )
                })?;
            Some((stem.clone(), source.clone(), compiled, dates_note))
        }
        _ => None,
    };
    let spec: trellis_model::ModelSpec = model_spec.parse()?;
    let mut driver = Driver::build(&spec, unconstrained, &tooling.tools, &cfg, model_from_user)?;

    // Reachability before the snapshot, and the ordering is the whole point.
    //
    // `Driver::build` parses a URL; it does not connect. So a runtime that
    // is not listening used to be discovered by the loop's *first* model
    // call — after a git snapshot had been taken — and the run would then
    // roll back and write a trail for work that never began. Four dead runs
    // in a row look identical to four real failures, and each one leaves a
    // trail file behind.
    //
    // A refused connection is not a failed run. It is a run that could not
    // start, and it should cost nothing.
    let probe_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if let Driver::Runtime(m) = &driver
        && let Err(e) = probe_rt.block_on(m.client().reachable())
    {
        anyhow::bail!(
            concat!(
                "{}

",
                "Nothing was snapshotted and nothing was rolled back — the run
",
                "did not start. Start the runtime, or point at a different one
",
                "with `--model <runtime>:http://host:port`, then try again.

",
                "To exercise the loop with no runtime at all — real checks,
",
                "real edits, real rollback, scripted decisions — use
",
                "`--model scripted:<file.json>`. `trellis model` lists
",
                "weights you can fetch."
            ),
            e
        );
    }

    // Whether the runtime actually honours the schema, rather than whether
    // its dialect name suggests it should.
    //
    // The tier came from the `--unconstrained` flag alone, so every run
    // against ollama printed "tier 2, schema enforced by the runtime —
    // degradation: none for correctness". Ollama does not implement
    // `guided_json`; that is vLLM's parameter, and ollama accepts and ignores
    // it. Measured rather than inferred: the same prompt three times with and
    // without the schema returned byte-identical text and an identical
    // completion-token count, which a schema that changed sampling could not
    // produce.
    //
    // `doctor` has probed this all along and reports it correctly. The run
    // did not, and the run is where the claim was read. One request, against
    // a model already loaded, ahead of a run measured in minutes.
    let mut schema_note: Option<String> = None;
    if let Driver::Runtime(m) = &driver
        && !unconstrained
    {
        let probed = probe_rt
            .block_on(m.client().probe_constraint())
            .map_err(|e| e.to_string());
        let (unconstrain, note) = schema_report(&probed);
        if unconstrain {
            // Not a warning and not a failure. The loop already validates and
            // retries; what was wrong was the claim, so the fix is to report
            // the tier that is actually running.
            driver = Driver::build(&spec, true, &tooling.tools, &cfg, model_from_user)?;
        }
        schema_note = Some(note);
    }

    // Printed after reachability, not before. Describing the tier of a
    // runtime that is not listening is noise ahead of the only line that
    // matters.
    let _ = writeln!(out, "model:    {}", driver.describe());
    if let Driver::Runtime(m) = &driver
        && let Some(n) = hosted::thinking_ignored(&cfg, &format!("{:?}", m.dialect()))
    {
        let _ = writeln!(out, "{n}");
    }
    let _ = writeln!(out, "control:  {}", dial.describe());
    if let Some(line) = tooling.line(dial.mode) {
        let _ = writeln!(out, "tools:    {line}");
    }
    if let Some(note) = schema_note {
        let _ = writeln!(out, "{note}");
    }

    // Read before the snapshot, so a missing or corrupt trail fails the run
    // before anything has been written.
    let prior = if resume {
        match trellis_trail::latest(root)? {
            Some((path, trail)) => {
                let _ = writeln!(out, "resumed:  {}", path.display());
                let _ = writeln!(
                    out,
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
                let _ = writeln!(out, "resumed:  nothing — no trail under .trellis/runs/");
                None
            }
        }
    } else {
        None
    };

    // ADR 0025: context files, system prompt files and forced skills. Read
    // before the snapshot, so an unreadable or over-cap file costs nothing.
    // User-level skill directories (ADR 0025, integrated), for the listing,
    // `read_skill` and forced skills alike, so all three agree on a name.
    let skill_dirs = user_config::skill_dirs();
    let added = context_files::load(
        root,
        user_config::dir().as_deref(),
        cfg.context.as_ref().and_then(|c| c.files).unwrap_or(true),
        &extras.skills,
        &skill_dirs,
    )?;

    // A clean worktree is required so rollback is `git reset --hard` rather
    // than a restore of a dirty tree, which is where data-loss bugs live
    // (ADR 0009).
    let snapshot =
        trellis_vcs::snapshot(root).context("a clean git worktree is required before a run")?;
    let _ = writeln!(out, "snapshot at {}", snapshot.head);

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
    // Skills (ADR 0019): names and descriptions to the prompt, capped. Read
    // before the snapshot like everything else the run is seeded with.
    let mut skills = trellis_loop::SkillCatalog::discover_with(root, &skill_dirs);
    // A profile's [skills] allow/deny filters what is disclosed (ADR 0026 §4),
    // and the same policy goes to the orchestrator, which refuses `read_skill`
    // for the rest (integrated).
    let skill_policy = profile.and_then(context_files::skill_policy);
    if let Some(pol) = &skill_policy {
        skills.valid.retain(|s| pol.check(&s.name).is_ok());
    }
    let disclosed = skills.disclosed();
    if !governance.is_empty()
        || notes.is_some()
        || vault.is_some()
        || !skills.valid.is_empty()
        || !skills.invalid.is_empty()
        || tooling.untrusted.is_some()
        || !added.report.is_empty()
    {
        let _ = writeln!(
            out,
            "repo:     {} write rule(s){}{}{}{}{}",
            governance.rules().len(),
            if notes.is_some() {
                ", .trellis/context.md loaded"
            } else {
                ""
            },
            match &vault {
                // The byte count is what the prompt pays on every turn, and
                // the drop count is said aloud because a reference heading
                // with no body under it is easy to read past.
                Some((stem, source, c, dates)) => format!(
                    ", vault note `{stem}` from {source} loaded ({} B{}{})",
                    c.bytes,
                    match c.dropped_reference_bodies {
                        0 => String::new(),
                        n => format!(", {n} reference bod(ies) dropped to fit the cap"),
                    },
                    dates.as_ref().map(|d| format!(", {d}")).unwrap_or_default()
                ),
                None => String::new(),
            },
            // Said whenever there are skills, with both numbers: the cap
            // leaves skills out, and a list that was cut must not read as
            // the library.
            if skills.valid.is_empty() && skills.invalid.is_empty() {
                String::new()
            } else {
                format!(
                    ", skills: {} listed, {} omitted (cap){}{}",
                    disclosed.listed.len(),
                    disclosed.omitted,
                    match skills.invalid.len() {
                        0 => String::new(),
                        n => format!(", {n} invalid and skipped (`trellis doctor` says why)"),
                    },
                    // Integrated (ADR 0025): says these were not trust-gated.
                    context_files::user_skills_note(&skills)
                        .map(|n| format!(", {n}"))
                        .unwrap_or_default()
                )
            },
            match &tooling.untrusted {
                Some(u) => format!(", {u}"),
                None => String::new(),
            },
            // ADR 0025: what was loaded and what was left out, by name.
            added
                .report
                .iter()
                .map(|r| format!(", {r}"))
                .collect::<String>(),
        );
    }

    // Configured and silently unused is the failure this line prevents: a
    // user who set up a vault would otherwise assume the model saw it.
    if let Some(v) = &vault_setting
        && v.focus.is_none()
    {
        let _ = writeln!(
            out,
            "vault:    {} configured but no focus set; no vault note sent. \
             Pass --focus <stem> or set [context] focus",
            v.source
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
    // What the repository contains, from the index's own walk.
    //
    // Without it the model guesses a path from the symbol in the task and
    // every tool call lands on a file that does not exist. This is a walk,
    // not a parse: the paths are wanted, not the graph, and a run should not
    // pay for an index it may never query.
    let files = trellis_index::discover_relative(root);
    let _ = writeln!(
        out,
        "files:    {} first-party Python file(s) named to the model",
        files.len()
    );

    let mut seed = trellis_loop::Seed {
        prior,
        repo: trellis_loop::RepoContext {
            notes,
            vault: vault.map(|(focus, source, c, _)| trellis_loop::VaultContext {
                focus,
                source,
                text: c.text,
            }),
            rules: prompt_rules,
            files,
            skills: disclosed.listed,
            skills_omitted: disclosed.omitted,
            tools: tooling.tools.clone(),
            extra: added.extra,
        },
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    // After the snapshot, because a hook can write to the tree and the
    // rollback has to cover it. Its output joins the notes inside the
    // repository fence (ADR 0023 §5).
    if let Some(h) = &tooling.hooks {
        match rt.block_on(h.before_run()) {
            Some(Ok(text)) if !text.trim().is_empty() => {
                let block = trellis_loop::fence_before_run(&text);
                seed.repo.notes = Some(match seed.repo.notes.take() {
                    Some(n) => format!("{}\n{block}", n.trim_end()),
                    None => block,
                });
                let _ = writeln!(
                    out,
                    "hook:     before_run added {} B of context",
                    text.len()
                );
            }
            Some(Err(e)) => {
                let _ = writeln!(out, "hook:     before_run added nothing: {e}");
            }
            _ => {}
        }
    }
    // The approval gate and whoever answers it. In auto mode there is no
    // gate at all, rather than a gate that allows everything: which gates
    // are installed is the whole difference between the two ends of the
    // dial, and the trail records it.
    let (gate, mut answering) = match (dial.mode, control.human) {
        (config::ControlMode::Auto, _) | (_, Human::Absent) => (None, None),
        (config::ControlMode::Approve, Human::Channel(tx)) => (
            Some(trellis_loop::ApprovalGate::new(
                root,
                dial.approve.clone(),
                tx,
            )),
            None,
        ),
        (config::ControlMode::Approve, Human::Line(read)) => {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            (
                Some(trellis_loop::ApprovalGate::new(
                    root,
                    dial.approve.clone(),
                    tx,
                )),
                Some((rx, read)),
            )
        }
    };
    let (outcome, transcript, gate_log) = rt.block_on(async {
        let mut orchestrator = trellis_loop::Orchestrator::new(root, cfg.checks())
            .with_governance(governance)
            .with_rules(rules)
            .with_tools(tooling.tools.clone())
            .with_env(tooling.env.clone())
            // Integrated: `read_skill` looks where the listing looked, and
            // refuses what the profile excludes (ADR 0026 §4).
            .with_skills(trellis_loop::SkillAccess {
                user_dirs: skill_dirs.clone(),
                policy: skill_policy.clone(),
            });
        if let Some(h) = &tooling.hooks {
            orchestrator = orchestrator.with_hooks(h.clone());
        }
        // Governance first by construction, then the bash policy and the
        // pre_tool hook, then the person: nobody is asked about a call a
        // rule or a hook has already refused.
        for g in tooling.gates(dial.mode) {
            orchestrator = orchestrator.with_gate(g);
        }
        if let Some(g) = gate {
            orchestrator = orchestrator.with_gate(Box::new(g));
        }
        let (handle, _join) = orchestrator.spawn();
        // Questions are answered beside the loop, on this thread, so the
        // prompt shares the run's output and nothing else writes to it
        // while a person is reading a diff.
        let answer = async {
            if let Some((rx, read)) = answering.take() {
                answer_on_the_line(rx, read, &mut *out).await;
            }
            // Pending once there is nothing left to answer, so it never wins
            // the select below against the loop it serves.
            std::future::pending::<()>().await
        };
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
                    Driver::Hosted(m) => {
                        trellis_loop::run_seeded(task, &mut **m, &handle, &cfg.budget(), seed)
                            .await
                    }
                }
            } => Some(out),
            _ = answer => None,
            _ = tokio::signal::ctrl_c() => None,
            // `rpc`'s `abort` (ADR 0025 §5): the same path as Ctrl-C.
            _ = output::aborted(extras.abort.clone()) => None,
        };
        // Asked after the race either way: the orchestrator is still alive
        // and still knows what this run wrote, which is what the rollback and
        // the trail both need.
        let touched = handle.touched().await;
        let gate_log = handle.gate_log().await;
        // Before the rollback, so on a run that did not converge the
        // rollback also undoes what the hook wrote in the worktree. Not on
        // an interrupt: Ctrl-C means stop.
        //
        // On a converged run the hook runs after the last check, so any byte
        // it writes is one no check saw. The worktree is fingerprinted
        // around it, and a difference (or a fingerprint that could not be
        // taken) demotes the run: it is reported, and the rollback below
        // removes the hook's bytes together with the run's (ADR 0023,
        // "after a pass").
        let mut out = out;
        if let (Some(h), Some((o, _))) = (&tooling.hooks, &out)
            && h.config().after_run.is_some()
        {
            let guard = o
                .converged()
                .then(|| trellis_vcs::worktree_fingerprint(root).ok());
            if let Some(Err(e)) = h
                .after_run(&tooling::outcome_json(o, &touched, &run_id))
                .await
            {
                tracing::warn!(error = %e, "after_run hook failed; it only observes");
            }
            let changed = match guard {
                None => false,
                Some(None) => true,
                Some(Some(before)) => trellis_vcs::worktree_fingerprint(root)
                    .ok()
                    .is_none_or(|after| after != before),
            };
            if changed && let Some((o, _)) = &mut out {
                let iterations = match o {
                    trellis_loop::Outcome::Converged { iterations, .. }
                    | trellis_loop::Outcome::NotConverged { iterations, .. } => *iterations,
                };
                tracing::warn!("after_run changed the worktree of a converged run; rolling back");
                *o = trellis_loop::Outcome::NotConverged {
                    reason: trellis_loop::NotConverged::AfterRunChangedTree,
                    iterations,
                };
            }
        }
        (out, touched, gate_log)
    });
    // Not a plain drop. A line reader blocked on stdin when Ctrl-C arrived
    // is still on the blocking pool, and dropping a runtime waits for that
    // pool — so an interrupted run would hang until someone pressed Enter.
    rt.shutdown_background();

    // Completion tokens as the adapter counted them (ADR 0026 §8). A
    // scripted driver has no count, which is `None`, not a free run.
    let tokens = match &driver {
        Driver::Scripted(_) => None,
        Driver::Runtime(m) => Some(trellis_loop::Model::tokens_used(m)),
        // Hosted runs count against a profile's budget too; the provider
        // reports output tokens and the adapter sums them (ADR 0024).
        Driver::Hosted(m) => Some(trellis_loop::Model::tokens_used(m.as_ref())),
    };
    let compactions = driver.compactions();
    let facts = |approved| TrailFacts {
        approved,
        base_commit: &snapshot.head,
        model: Some(model_spec),
        control: Some(dial.describe()),
        gates: &gate_log,
        stamp: Some(&stamp),
        tokens,
        agent: profile.map(|p| p.name.clone()),
        inputs: added.inputs.clone(),
        compactions: &compactions,
    };
    let Some(outcome) = outcome else {
        return interrupted_run(
            root,
            task,
            &transcript,
            &snapshot,
            facts(trellis_trail::Approval::None),
            out,
        );
    };

    let (outcome, turns) = outcome;
    let _ = writeln!(out);
    for (i, t) in turns.turns.iter().enumerate() {
        let mark = match t.verdict_passed {
            Some(true) => "check passed",
            Some(false) => "check FAILED",
            None => match t.human {
                Some(true) => "approved by you",
                Some(false) => "declined by you",
                None => "",
            },
        };
        let _ = writeln!(out, "  {:>2}. {:?} {}", i + 1, t.call, mark);
    }

    // The list above says a check failed. This says why.
    //
    // The observation already holds the kind, the exact command and the tail
    // of its output — it is what the model was shown — and until now the user
    // was the only party in the loop who could not see it. "check FAILED"
    // three times is not something anyone can act on.
    //
    // Printed once, for the most recent failure, rather than under every
    // turn: a no-progress stop means the same call produced the same result
    // repeatedly, and three copies of one mypy report is not three pieces of
    // information.
    if let Some(t) = turns
        .turns
        .iter()
        .rev()
        .find(|t| t.verdict_passed == Some(false))
    {
        const SHOWN: usize = 24;
        let _ = writeln!(out);
        let _ = writeln!(out, "why the check failed:");
        let lines: Vec<&str> = t.observation.lines().collect();
        for line in lines.iter().take(SHOWN) {
            let _ = writeln!(out, "  {line}");
        }
        if lines.len() > SHOWN {
            let _ = writeln!(
                out,
                "  … {} more line(s); the full output is in the trail",
                lines.len() - SHOWN
            );
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "files written: {}",
        if transcript.is_empty() {
            // "files written:" followed by nothing reads as truncated
            // output. A run that wrote nothing is an ordinary outcome and
            // should say so.
            "none".to_string()
        } else {
            transcript.join(", ")
        }
    );
    // Context fill (v0.2 gate). Bytes, because TRELLIS does not tokenize.
    let measured: Vec<usize> = turns.turns.iter().filter_map(|t| t.prompt_bytes).collect();
    if let (Some(first), Some(peak)) = (measured.first(), measured.iter().max()) {
        let _ = writeln!(
            out,
            "context:  {} B first prompt, {} B peak over {} turns ({:.1}x)",
            first,
            peak,
            measured.len(),
            *peak as f64 / *first as f64
        );
    }
    if let Some(line) = hosted::compaction_line(&compactions) {
        let _ = writeln!(out, "{line}");
    }

    match outcome {
        trellis_loop::Outcome::Converged {
            iterations,
            checks_passed,
        } => {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "CONVERGED after {iterations} iterations, {checks_passed} checks passed."
            );
            let trail = record_trail(
                root,
                task,
                &turns,
                &transcript,
                facts(trellis_trail::Approval::None),
                &mut *out,
            )?;
            let _ = writeln!(out, "trail:    {}", trail.display());

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
                    let _ = writeln!(out, "coverage: {}", doubt.one_line());
                    let _ = writeln!(
                        out,
                        "{}",
                        concat!(
                            "          Any of them may call what changed. The ",
                            "checks ran on what the graph could see."
                        )
                    );
                }
            }
            let _ = writeln!(
                out,
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
            let _ = writeln!(out);
            let _ = writeln!(out, "NOT CONVERGED after {iterations} iterations.");
            let _ = writeln!(out, "  {}", reason.explain());
            // Deliberately not `?`. A failed rollback used to propagate here,
            // which meant the one run whose record matters most — the tree is
            // in an unknown state — was also the one run that never got a
            // trail written. The failure is reported loudly and the recording
            // continues.
            let restored = rollback_and_report(&snapshot, &mut *out);

            // Written *after* the rollback, deliberately. Rollback cleans
            // untracked files, and a trail written before it would be one of
            // them — deleting the record of exactly the run worth recording.
            match record_trail(
                root,
                task,
                &turns,
                &transcript,
                facts(trellis_trail::Approval::None),
                &mut *out,
            ) {
                Ok(p) => {
                    let _ = writeln!(out, "trail:    {} (survives the rollback)", p.display());
                }
                Err(e) => {
                    let _ = writeln!(out, "could not write the trail: {e}");
                }
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
        println!("  {}", plain(&line));
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
    facts: TrailFacts<'_>,
    out: &mut dyn std::io::Write,
) -> Result<RunExit> {
    let restored = handle_interrupt(root, task, touched, snapshot, facts, out);
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
    facts: TrailFacts<'_>,
    out: &mut dyn std::io::Write,
) -> bool {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "INTERRUPTED. Rolling back — nothing is accepted on an abort."
    );
    if !touched.is_empty() {
        let _ = writeln!(out, "files this run had written: {}", touched.join(", "));
    }

    let restored = rollback_and_report(snapshot, &mut *out);

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
    match record_trail(root, task, &turns, touched, facts, &mut *out) {
        Ok(p) => {
            let _ = writeln!(
                out,
                "trail:    {} (interrupted; no turns recorded)",
                p.display()
            );
        }
        Err(e) => {
            let _ = writeln!(out, "could not write the trail: {e}");
        }
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
fn rollback_and_report(snapshot: &trellis_vcs::Snapshot, out: &mut dyn std::io::Write) -> bool {
    match snapshot.rollback() {
        Ok(()) => {
            let restored = snapshot.is_restored().unwrap_or(false);
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Rolled back to {}. Worktree restored: {restored}.",
                snapshot.head
            );
            if !restored {
                let _ = writeln!(out);
                let _ = writeln!(
                    out,
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
            let _ = writeln!(out);
            let _ = writeln!(out, "{e}");
            false
        }
    }
}

/// Records what happened, factual and judgment kept apart.
///
/// Everything in `factual` comes from a tool result the run actually received.
/// Nothing is defaulted into it: a figure nobody measured has no business in
/// the section whose promise is that it is machine-checkable.
/// Splits the callers of what changed into verified and reviewed.
///
/// **S3, finally populated from evidence.** "The callers still work" means two
/// things: covered by a test that ran and passed, or read by a model that did
/// not object. The second is a weak signal. Until there was coverage data both
/// counts were 0 and the tools had to explain that the zero meant "not
/// measured" rather than "nothing verified".
///
/// The population is the **callers** — call sites reaching a symbol defined in
/// a file the run changed — because that is what the sentence is about. Not
/// the changed files, and not the repository.
///
/// Returns zeros when there is no `coverage.json`, which is still the common
/// case. That is the same answer as before and means the same thing: nothing
/// was measured. The difference is that it is now an absence of data rather
/// than an absence of code, and a repository can fix it by adding
/// `--cov-report=json` to the test command it already has.
pub(crate) fn audit_modes(root: &Path, touched: &[String]) -> trellis_trail::ModeCounts {
    let mut counts = trellis_trail::ModeCounts::default();
    let Some(coverage) = trellis_checks::coverage::LineCoverage::load(root) else {
        return counts;
    };
    if coverage.is_empty() || touched.is_empty() {
        return counts;
    }

    let index = trellis_index::build_cached(root);
    let callers = trellis_index::callers_of_files(&index, touched);

    // Sources are read once each rather than per call site: a hot file can
    // hold hundreds of callers, and re-reading it for every one turns an
    // attribution pass into the slowest part of a run.
    let mut sources: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for c in &callers {
        let src = sources
            .entry(c.path.clone())
            .or_insert_with(|| std::fs::read_to_string(root.join(&c.path)).unwrap_or_default());
        let line = src[..c.offset.min(src.len())].lines().count() as u32;
        counts.record(if coverage.covers(&c.path, line) {
            trellis_checks::AuditMode::Verified
        } else {
            trellis_checks::AuditMode::Reviewed
        });
    }
    counts
}

/// One turn, as a line in the action log.
///
/// The observation is summarised rather than stored. A check's output runs to
/// thousands of lines and is truncated before the model ever sees it, so a
/// trail that kept it would be mostly type-checker output — and the question
/// this log answers is "what did it do", not "what did the compiler say".
///
/// `wrote` is the one bit that cannot be recovered from anywhere else.
/// `proposed` names the files a run ended up touching, not which action did
/// the touching; a run whose every edit was refused has an empty `proposed`
/// and a full list of actions, and those are different stories.
fn action_from(index: usize, t: &trellis_loop::Turn) -> trellis_trail::Action {
    use trellis_loop::ToolCall;

    let (tool, target) = match &t.call {
        ToolCall::BlastRadius { symbol } => ("blast_radius", symbol.clone()),
        ToolCall::ReadFile { path } => ("read_file", path.clone()),
        ToolCall::Edit { path, .. } => ("edit", path.clone()),
        ToolCall::RunChecks { paths, kinds } => {
            let kinds: Vec<String> = kinds.iter().map(|k| format!("{k:?}")).collect();
            // The checks requested, then what they were pointed at. An empty
            // `paths` is the whole repository and says so, rather than
            // rendering as a blank the reader has to interpret.
            let where_ = if paths.is_empty() {
                "whole repository".to_string()
            } else {
                paths.join(", ")
            };
            ("run_checks", format!("{} on {where_}", kinds.join("+")))
        }
        ToolCall::ReadSkill { name } => ("read_skill", name.clone()),
        // ADR 0023's tools. A repository's own tool is recorded under its
        // own name, with the arguments it was sent.
        ToolCall::WriteFile { path, .. } => ("write_file", path.clone()),
        ToolCall::Grep { pattern, path } => (
            "grep",
            match path {
                Some(p) => format!("{pattern} in {p}"),
                None => pattern.clone(),
            },
        ),
        ToolCall::Find { glob } => ("find", glob.clone()),
        ToolCall::Ls { path } => ("ls", path.clone()),
        ToolCall::Bash { command } => ("bash", summarise(command)),
        ToolCall::SearchNotes { query, .. } => ("search_notes", summarise(query)),
        ToolCall::ReadNote { note } => ("read_note", note.clone()),
        ToolCall::Custom { name, args } => (name.as_str(), summarise(&args.to_string())),
    };

    trellis_trail::Action {
        turn: index as u32 + 1,
        tool: tool.to_string(),
        target,
        outcome: summarise(&t.observation),
        // Only an applied edit or a created file changes the tree through
        // the orchestrator. A refused one is an action that happened and
        // wrote nothing, which is exactly what this distinguishes. What bash
        // or a custom tool wrote is not known here and not claimed
        // (ADR 0023 §3).
        wrote: match &t.call {
            ToolCall::Edit { .. } => t.observation.contains("applied: true"),
            ToolCall::WriteFile { path, .. } => {
                t.observation.starts_with(&format!("created {path} "))
            }
            _ => false,
        },
    }
}

/// An observation reduced to one line a person can read in a list.
fn summarise(observation: &str) -> String {
    const MAX: usize = 120;
    let first = observation
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.chars().count() <= MAX {
        return first.to_string();
    }
    // Cut on a character boundary, and say it was cut — a silently truncated
    // line reads as the whole outcome.
    let head: String = first.chars().take(MAX - 1).collect();
    format!("{head}…")
}

/// What a trail records about a run beyond its turns.
///
/// Grouped because they travel together and always have: the tree the run
/// started from, what drove it, and whether a human has accepted it. Passing
/// them singly pushed `record_trail` past the argument limit, and the limit
/// was right — three loose values in a row is where a caller swaps two.
struct TrailFacts<'a> {
    approved: trellis_trail::Approval,
    base_commit: &'a str,
    model: Option<&'a str>,
    /// Where the control dial stood (ADR 0018): `auto`, or `approve (edit)`.
    /// Hands-off and fine-grained runs go through one loop, so this is the
    /// only field that says which one the reader is looking at.
    control: Option<String>,
    /// Calls a named gate refused (ADR 0023 §7), each recorded as an action
    /// just before the call it refused.
    gates: &'a [trellis_loop::GateDenial],
    /// `(timestamp, started_at)` taken when the run began, so the trail's
    /// run id is the `TRELLIS_RUN_ID` bash and hooks were given. `None`
    /// stamps it now.
    stamp: Option<&'a (String, String)>,
    /// Completion tokens and the agent profile, for the budget ledger
    /// (ADR 0026 §8).
    tokens: Option<u64>,
    agent: Option<String>,
    /// Context files, system files and forced skills, by name and size
    /// (ADR 0025, integrated).
    inputs: Vec<trellis_trail::Input>,
    /// Compactions (ADR 0024 §5), each an action under the turn whose
    /// request was compacted. Empty on a run that never compacted, so older
    /// trails keep their shape.
    compactions: &'a [trellis_loop::Compacted],
}

fn record_trail(
    root: &Path,
    task: &str,
    turns: &trellis_loop::Transcript,
    touched: &[String],
    facts: TrailFacts<'_>,
    out: &mut dyn std::io::Write,
) -> Result<std::path::PathBuf> {
    let TrailFacts {
        approved,
        base_commit,
        model,
        control,
        gates,
        stamp,
        tokens,
        agent,
        inputs,
        compactions,
    } = facts;
    use trellis_trail::{CheckOutcome, Factual, Judgment, Provenance, RadiusCounts, SCHEMA, Trail};

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

    // A human's decision is an action of its own, recorded before the call it
    // decided and under the same turn, so `trellis log --actions` reads
    // "approval: declined, edit: declined by the user" in the order it
    // happened.
    let actions: Vec<trellis_trail::Action> = turns
        .turns
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let act = action_from(i, t);
            let decision = t.human.map(|approved| trellis_trail::Action {
                turn: act.turn,
                tool: "approval".to_string(),
                target: format!("{} {}", act.tool, act.target),
                outcome: if approved { "approved" } else { "declined" }.to_string(),
                wrote: false,
            });
            // A named gate's refusal, the same way: its own action, under
            // the turn and before the call it refused. None on a run where
            // no such gate refused anything, so older trails keep their shape.
            let denied: Vec<trellis_trail::Action> = gates
                .iter()
                .filter(|g| g.call == act.turn)
                .map(|g| trellis_trail::Action {
                    turn: act.turn,
                    tool: g.gate.clone(),
                    target: summarise(&g.target),
                    outcome: summarise(&format!("denied: {}", g.reason)),
                    wrote: false,
                })
                .collect();
            let at = act.turn;
            let compacted: Vec<trellis_trail::Action> = compactions
                .iter()
                .filter(|c| c.turn == at)
                .map(hosted::compaction_action)
                .collect();
            compacted
                .into_iter()
                .chain(decision)
                .chain(denied)
                .chain(std::iter::once(act))
        })
        .collect();
    // A compaction for the request that produced `done` has no turn of its
    // own to sit under; it goes last, where it happened.
    let mut actions = actions;
    actions.extend(
        compactions
            .iter()
            .filter(|c| c.turn as usize > turns.turns.len())
            .map(hosted::compaction_action),
    );

    let (timestamp, started_at) = stamp.cloned().unwrap_or_else(trellis_trail::now);

    let factual = Factual {
        proposed: touched.to_vec(),
        radius,
        // `Verdict` cannot be constructed outside `trellis-checks`, so the
        // sequence travels as `check_outcomes` instead. See its docs.
        checks: Vec::new(),
        check_outcomes,
        approved_by: approved,
        mode_counts: audit_modes(root, touched),
        context,
        actions,
        control,
        tokens,
        agent,
        inputs,
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
        let _ = writeln!(out, "trail written but not committed: {e}");
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
fn cmd_partitions(root: &Path, symbols: &[String]) -> Result<()> {
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

pub fn radius_of(index: &trellis_index::Index, query: &str) -> Result<()> {
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
fn cmd_index(root: &Path, graph: bool, limit: usize) -> Result<()> {
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
const MAX_EDGES_DRAWN: usize = 10;

fn graph_lines(g: &trellis_index::ModuleGraph, limit: usize) -> Vec<String> {
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

/// What a constraint probe means for the run about to start.
///
/// Returns whether to rebuild the driver unconstrained, and the line to print.
/// Separated from `cmd_run` so the three outcomes are testable without a
/// runtime — which matters, because the failure this exists to prevent is a
/// confident sentence rather than a crash.
fn schema_report(probed: &Result<bool, String>) -> (bool, String) {
    match probed {
        Ok(true) => (false, "schema:   honoured by this runtime (probed)".into()),
        Ok(false) => (
            true,
            "schema:   sent and ignored by this runtime, so it is not enforced. Running\n\
             \x20         unconstrained; malformed calls are retried rather than prevented."
                .into(),
        ),
        // A probe that could not run leaves the tier assumed. Saying so is the
        // difference between an unverified claim and a false one — and it must
        // not silently change the tier, because a transient network error is
        // not evidence about the runtime's capabilities.
        Err(e) => (
            false,
            format!("schema:   enforcement not verified ({e}); the tier above is assumed"),
        ),
    }
}

/// The codebase graph for `root`, as lines, for a caller with no stdout.
///
/// Builds the index rather than reading the cache: the interface offers this
/// while a repository is being edited, and a cached graph would describe the
/// tree as it stood before the run the user has just watched.
pub(crate) fn codebase_lines(root: &Path, limit: usize) -> Vec<String> {
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    graph_lines(&trellis_index::module_graph(&index), limit)
}

/// Cuts a module path to the column, keeping the end.
///
/// The tail is what distinguishes `poetry.console.commands.add` from
/// `poetry.console.commands.remove`; the head is shared by hundreds of them.
fn truncate_module(m: &str, width: usize) -> String {
    let n = m.chars().count();
    if n <= width {
        return m.to_string();
    }
    let keep = width.saturating_sub(1);
    std::iter::once('…')
        .chain(m.chars().skip(n - keep))
        .collect()
}

fn cmd_doctor(root: &Path, model_spec: Option<&str>, from_user: bool) -> Result<()> {
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
        // Tier 1 is the protocol, not a probe finding (ADR 0024 §2).
        if let Some(h) = hosted::resolve(&spec, &repo_cfg, from_user)? {
            let (m, r, n) = hosted::doctor(&h);
            model_id = m;
            runtime = r;
            notes.extend(n);
            tier = Some(AdapterTier::Native);
            batching = BatchingCapability::ContinuousUnmeasured;
        } else {
            let trellis_model::ModelSpec::Runtime {
                dialect,
                base,
                model,
            } = &spec
            else {
                anyhow::bail!(
                    concat!(
                        "`{}` is a scripted model, which has no runtime to probe. ",
                        "Pass a serving runtime, e.g. llamacpp:http://127.0.0.1:8080."
                    ),
                    spec
                );
            };
            let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?
                .with_timeout(std::time::Duration::from_secs(30));
            runtime = format!("{dialect:?} at {base}");
            // True of llama.cpp, which serves what it was started with and is
            // never told. Not true once the spec names a weight — saying it
            // anyway would report ignorance the harness does not have, on the
            // one line a reader checks to confirm they are about to drive what
            // they think they are.
            model_id = match model {
                Some(m) => format!("{m} (named in the spec)"),
                None => "whatever the runtime has loaded".into(),
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
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
                        batching = BatchingCapability::ContinuousUnmeasured;
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

# [context]
# A note vault in Apex format (ADR 0017). One focus note, its parent chain,
# its children's titles and the notes it links, compiled in memory when a run
# starts and shown to the model. It informs; it does not bind. Switch note
# per run with `trellis run --focus <stem>` rather than by editing this file,
# which would dirty the worktree a run needs clean.
# vault = "notes"
# focus = "some-note"
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
    trellis_core::secrets::scrub(&mut c);
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
    trellis_core::secrets::scrub(&mut c);
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

    // Skills (ADR 0019): what was found, what a run would list and leave out,
    // and every directory skipped with the reason. A skill that is silently
    // not loaded is one its author believes the model can read.
    let skills = trellis_loop::SkillCatalog::discover(root);
    let found = skills.valid.len() + skills.invalid.len();
    if found == 0 {
        out.push(format!(
            "skills:     none under {}",
            trellis_loop::SKILLS_DIR
        ));
    } else {
        let d = skills.disclosed();
        out.push(format!(
            "skills:     {found} found · {} listed · {} omitted (cap {} B) · {} invalid",
            d.listed.len(),
            d.omitted,
            trellis_loop::DISCLOSURE_CAP,
            skills.invalid.len()
        ));
        for i in &skills.invalid {
            out.push(format!(
                "            skipped {}: {}",
                plain(&i.dir),
                plain(&i.reason)
            ));
        }
    }

    // The control dial (ADR 0018). Said here because approve mode refuses to
    // start without a terminal, and that is better learned from `doctor`
    // than from a CI job.
    out.push(match cfg.control(None) {
        Ok(c) if c.mode == config::ControlMode::Approve => format!(
            "control:    {} — a run asks before each; needs a terminal, or `--auto`",
            c.describe()
        ),
        Ok(c) => format!("control:    {}", c.describe()),
        Err(e) => format!("control:    INVALID — {e}"),
    });

    // Budget, in the units it is actually enforced in.
    let b = cfg.budget();
    out.push(format!(
        "budget:     {:?} — {} iterations / {} s / {} tokens",
        b.regime,
        b.max_iterations,
        b.max_wall_clock.as_secs(),
        b.max_tokens
    ));

    // Integrated (ADR 0021 wave I): the user config, context files, user
    // skills, trust, profiles and routines, and the vault.
    out.extend(doctor_extra::lines(root, cfg));

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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

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

    fn facts_for(snap: &trellis_vcs::Snapshot) -> TrailFacts<'_> {
        TrailFacts {
            approved: trellis_trail::Approval::None,
            base_commit: &snap.head,
            model: Some("scripted:none"),
            control: None,
            gates: &[],
            stamp: None,
            tokens: None,
            agent: None,
            inputs: Vec::new(),
            compactions: &[],
        }
    }

    fn temp_repo(name: &str) -> PathBuf {
        let dir = crate::testutil::scratch(&format!("cli-test-{name}"));
        crate::testutil::git_init(&dir);
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

        let mut said = Vec::new();
        let restored = handle_interrupt(
            &dir,
            "a task that was interrupted",
            &["a.py".to_string()],
            &snap,
            facts_for(&snap),
            &mut said,
        );

        assert!(restored, "an abort must restore the tree");
        // The narration goes to the sink the caller supplied, which is what
        // lets a full-screen interface show it instead of being drawn over.
        let said = String::from_utf8(said).unwrap();
        assert!(said.contains("INTERRUPTED"), "{said}");
        assert!(said.contains("a.py"), "the files it had written: {said}");
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

        let restored = handle_interrupt(
            &dir,
            "t",
            &["a.py".to_string()],
            &snap,
            facts_for(&snap),
            &mut Vec::new(),
        );
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
            !rollback_and_report(&snap, &mut Vec::new()),
            "the recovery path must never report a restoration it did not make"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_rollback_reports_true() {
        let dir = temp_repo("recovery-clean");
        let snap = trellis_vcs::snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 5\n").unwrap();

        assert!(rollback_and_report(&snap, &mut Vec::new()));
        assert!(snap.is_restored().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = crate::testutil::scratch(&format!("doctor-{name}"));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        crate::testutil::git_init(&dir);
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
        let dir = std::env::temp_dir().join(format!("trellis-doctor-empty-{}", std::process::id()));
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
        crate::testutil::scratch(&format!("init-{name}"))
    }

    fn repo(name: &str) -> PathBuf {
        let d = bare(name);
        crate::testutil::git_init(&d);
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

    /// A turn, for building action-log cases without a model.
    fn turn(call: trellis_loop::ToolCall, observation: &str) -> trellis_loop::Turn {
        trellis_loop::Turn {
            call,
            observation: observation.to_string(),
            verdict_passed: None,
            check_kind: None,
            prompt_bytes: Some(1_000),
            radius_counts: None,
            human: None,
            may_have_changed_tree: false,
        }
    }

    #[test]
    fn an_escape_sequence_from_a_trail_cannot_repaint_the_terminal() {
        // A trail is untrusted on read-back (S10) and `trellis log` prints it
        // to a terminal, which interprets what it is given. A run that did not
        // converge could otherwise be made to look like one that did, in the
        // command a user reads to find out which.
        let forged = "fix the parser\x1b[2K\x1b[Aconverged, 2 checks passed";
        let out = plain(forged);
        assert!(!out.contains('\x1b'), "{out:?}");
        assert!(
            out.contains("fix the parser"),
            "the real text survives: {out:?}"
        );
        assert!(
            out.contains("\\u{1b}") || out.contains("\\x1b") || out.contains("\\e"),
            "and the escape is shown rather than dropped: {out:?}"
        );
    }

    #[test]
    fn ordinary_text_passes_through_a_sanitiser_unchanged() {
        // Including the em dashes and arrows this project's own messages use.
        for s in [
            "the describe function is annotated to return int",
            "app/calc.py",
            "2 rule(s) checked over 1 file(s); none broken — nothing to report",
        ] {
            assert_eq!(plain(s), s);
        }
    }

    #[test]
    fn a_hub_module_does_not_draw_an_unbounded_number_of_edges() {
        use trellis_index::{ModuleGraph, ModuleNode};
        // `--limit` caps modules and used to cap nothing else, so one module
        // every other one imports could emit more rows than the whole rest of
        // the listing — including into the interface's transcript.
        let g = ModuleGraph {
            nodes: vec![ModuleNode {
                module: "app.cli".into(),
                path: "app/cli.py".into(),
                defines: 1,
                calls: (0..40).map(|i| (format!("app.m{i}"), 40 - i)).collect(),
                ambiguous: 0,
                unresolved: 0,
            }],
        };
        let lines = graph_lines(&g, 40);
        let edges = lines.iter().filter(|l| l.contains("─▶")).count();
        assert_eq!(edges, MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains("30 more module(s) called")),
            "and the remainder is stated, not dropped: {lines:#?}"
        );
        // The heaviest edges are the ones kept — they are sorted by count.
        assert!(lines.iter().any(|l| l.contains("app.m0")), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("app.m39")), "{lines:#?}");
    }

    #[test]
    fn an_ignored_schema_downgrades_the_tier_rather_than_the_sentence() {
        // The defect: every run against ollama printed "tier 2, schema
        // enforced by the runtime" for a runtime that accepts `guided_json`
        // and ignores it. Rewording would have swapped a false claim for a
        // vaguer one; the tier itself has to change.
        let (unconstrain, note) = schema_report(&Ok(false));
        assert!(
            unconstrain,
            "the run must not claim a tier it does not have"
        );
        assert!(note.contains("ignored"), "{note}");
        assert!(
            note.contains("retried rather than prevented"),
            "and say what that costs: {note}"
        );
    }

    #[test]
    fn an_honoured_schema_keeps_the_tier_and_says_it_was_probed() {
        let (unconstrain, note) = schema_report(&Ok(true));
        assert!(!unconstrain);
        assert!(
            note.contains("probed"),
            "the claim names its evidence: {note}"
        );
    }

    #[test]
    fn a_probe_that_could_not_run_says_the_tier_is_assumed() {
        // A transient network error is not evidence about the runtime's
        // capabilities, so it must not silently change the tier — but it must
        // also not leave the banner asserting something nothing checked.
        let (unconstrain, note) = schema_report(&Err("connection reset".into()));
        assert!(
            !unconstrain,
            "a failed probe is not a measurement of an ignored schema"
        );
        assert!(note.contains("not verified"), "{note}");
        assert!(note.contains("connection reset"), "with the reason: {note}");
        assert!(note.contains("assumed"), "{note}");
    }

    #[test]
    fn an_applied_edit_is_the_only_action_that_records_a_write() {
        use trellis_loop::ToolCall;
        // `proposed` names the files a run ended up touching, not which action
        // did the touching. A run whose every edit was refused has an empty
        // `proposed` and a full list of actions.
        let refused = action_from(
            0,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "error: edit anchor not found in app/calc.py",
            ),
        );
        assert_eq!(refused.turn, 1, "turns are 1-based, as a run prints them");
        assert!(!refused.wrote, "a refused edit wrote nothing");

        let applied = action_from(
            1,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "edited app/calc.py (applied: true, bytes 59..87)",
            ),
        );
        assert!(applied.wrote, "an applied edit did");
        assert_eq!(applied.target, "app/calc.py");
    }

    #[test]
    fn reading_a_file_never_counts_as_a_write() {
        use trellis_loop::ToolCall;
        // The `applied: true` test is a substring match, so a read whose
        // content happened to contain that phrase must still not count.
        let a = action_from(
            0,
            &turn(
                ToolCall::ReadFile {
                    path: "app/calc.py".into(),
                },
                "applied: true",
            ),
        );
        assert!(!a.wrote, "only an edit can write");
        assert_eq!(a.tool, "read_file");
    }

    #[test]
    fn reading_a_skill_is_recorded_by_name_and_writes_nothing() {
        // ADR 0019 adds one closed-enum variant; the trail has to name it
        // rather than fold it into `read_file`, or a reader cannot tell which
        // procedure the model consulted.
        let a = action_from(
            2,
            &turn(
                trellis_loop::ToolCall::ReadSkill {
                    name: "rust-unsafe".into(),
                },
                "skill `rust-unsafe`, from .trellis/skills. applied: true",
            ),
        );
        assert_eq!(
            (a.turn, a.tool.as_str(), a.target.as_str(), a.wrote),
            (3, "read_skill", "rust-unsafe", false)
        );
    }

    #[test]
    fn checks_against_no_paths_say_so_rather_than_rendering_blank() {
        use trellis_checks::CheckKind;
        use trellis_loop::ToolCall;
        let a = action_from(
            0,
            &turn(
                ToolCall::RunChecks {
                    paths: Vec::new(),
                    kinds: vec![CheckKind::Tests],
                },
                "Tests: passed",
            ),
        );
        assert_eq!(a.target, "Tests on whole repository", "{a:?}");
    }

    #[test]
    fn an_outcome_is_one_line_and_says_when_it_was_cut() {
        // A check's output runs to thousands of lines. A trail that stored it
        // would be mostly type-checker output, and a silently truncated line
        // reads as the whole outcome.
        assert_eq!(summarise("first\nsecond\nthird"), "first");
        assert_eq!(summarise("\n\n  indented  \nnext"), "indented");
        assert_eq!(summarise(""), "");
        let long = "x".repeat(400);
        let cut = summarise(&long);
        assert_eq!(cut.chars().count(), 120);
        assert!(cut.ends_with('…'), "a cut line says it was cut: {cut}");
    }

    #[test]
    fn a_multibyte_outcome_is_cut_on_a_character_boundary() {
        // `…` and an em dash both appear in this repository's own messages,
        // and slicing by bytes would panic on one.
        let long = "é".repeat(400);
        assert_eq!(summarise(&long).chars().count(), 120);
    }
}

#[cfg(test)]
mod approval_tests {
    //! ADR 0018 end to end through `cmd_run`: real files, a real snapshot and
    //! rollback, a scripted model, and a person played by a closure.

    use super::*;

    /// A committed repository in approve mode, and a script outside it.
    ///
    /// The script lives elsewhere because an untracked file inside the
    /// repository would make the worktree dirty, and a run refuses that.
    fn fixture(name: &str, script: &str) -> (PathBuf, String) {
        let dir = crate::testutil::scratch(&format!("approval-{name}"));
        crate::testutil::git_init(&dir);
        std::fs::create_dir_all(dir.join(".trellis")).unwrap();
        std::fs::write(
            dir.join(".trellis/config.toml"),
            "[control]\nmode = \"approve\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        let scripts = crate::testutil::scratch(&format!("approval-{name}-script"));
        let path = scripts.join("script.json");
        std::fs::write(&path, script).unwrap();
        (dir, format!("scripted:{}", path.display()))
    }

    const EDIT_THEN_STOP: &str = r#"{"steps":[
        {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
        {"tool":"done","summary":"changed it"}
    ]}"#;

    /// A person who gives these answers, in order, then closes the input.
    fn answering(answers: &[&str]) -> Human {
        let mut left: Vec<String> = answers.iter().rev().map(|s| format!("{s}\n")).collect();
        Human::Line(Box::new(move || left.pop()))
    }

    fn run_with(dir: &Path, spec: &str, human: Human) -> (Result<RunExit>, String) {
        let mut out = Vec::new();
        let r = cmd_run(
            dir,
            "change f",
            spec,
            false,
            RunSeeding::default(),
            RunControl { flag: None, human },
            &mut out,
        );
        (r, String::from_utf8(out).unwrap())
    }

    /// The one trail the run wrote, read back the way `trellis log` reads it.
    fn trail(dir: &Path) -> trellis_trail::UntrustedFactual {
        let runs = dir.join(".trellis").join("runs");
        let file = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == "json"))
            .expect("a trail was written");
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let t = trellis_trail::UntrustedTrail::from_json(&raw).unwrap();
        // Every trail here carries the new `control` field and approval
        // actions, so every read-back also checks the digest still covers
        // them: a field added to `factual` that the digest missed would make
        // each of these trails read as altered.
        assert_eq!(t.integrity(&raw), trellis_trail::Integrity::Intact);
        t.factual
    }

    fn actions(f: &trellis_trail::UntrustedFactual) -> Vec<(String, String, String, bool)> {
        f.actions
            .iter()
            .map(|a| (a.tool.clone(), a.target.clone(), a.outcome.clone(), a.wrote))
            .collect()
    }

    #[test]
    fn approve_mode_with_nobody_to_ask_refuses_before_the_snapshot() {
        // Starting and then denying every edit would be a run that looks as
        // though it tried. It must not start, and it must cost nothing: no
        // snapshot, no rollback, no trail.
        let (dir, spec) = fixture("no-tty", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, Human::Absent);
        let e = r.expect_err("approve mode with nobody to ask must refuse");
        assert_eq!(format!("{e}"), NEEDS_A_TERMINAL);
        assert!(!said.contains("snapshot"), "{said}");
        assert!(
            !dir.join(".trellis/runs").exists(),
            "no trail for a run that never began"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn the_auto_flag_needs_nobody_even_where_the_config_says_approve() {
        let (dir, spec) = fixture("auto-flag", EDIT_THEN_STOP);
        let mut out = Vec::new();
        let r = cmd_run(
            &dir,
            "change f",
            &spec,
            false,
            RunSeeding::default(),
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
            },
            &mut out,
        );
        assert!(r.is_ok(), "{r:?}");
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("control:  auto"), "{said}");
        assert!(!said.contains("[y/N]"), "auto asks nothing: {said}");
        assert_eq!(trail(&dir).control.as_deref(), Some("auto"));
    }

    #[test]
    fn a_declined_edit_is_not_applied_and_the_trail_says_declined() {
        let (dir, spec) = fixture("declined", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["n"]));
        assert!(r.is_ok(), "{r:?}");

        // Asked on the line, with the diff first.
        let diff_at = said.find("- ").expect("the removed line is shown");
        let question = said
            .find("edit a.py (+1 -1)? [y/N]")
            .unwrap_or_else(|| panic!("the question in the ADR's words:\n{said}"));
        assert!(
            diff_at < question,
            "the diff comes before the question:\n{said}"
        );
        assert!(
            said.contains("-     return 1") && said.contains("+     return 2"),
            "{said}"
        );
        assert!(said.contains("declined"), "{said}");

        // Not applied. The run did not converge and rolled back, so the file
        // being unchanged proves nothing on its own; the trail's `wrote` bit
        // is what says the edit never touched the tree.
        let t = trail(&dir);
        assert_eq!(t.control.as_deref(), Some("approve (edit)"));
        let a = actions(&t);
        assert_eq!(
            a[0],
            (
                "approval".to_string(),
                "edit a.py".to_string(),
                "declined".to_string(),
                false
            ),
            "{a:?}"
        );
        assert_eq!(a[1].0, "edit");
        assert!(
            a[1].2.contains("declined by the user"),
            "the model is told why: {a:?}"
        );
        assert!(!a[1].3, "a declined edit wrote nothing: {a:?}");
        assert!(t.proposed.is_empty(), "{:?}", t.proposed);
    }

    #[test]
    fn an_approved_edit_is_applied_and_the_trail_says_approved() {
        let (dir, spec) = fixture("approved", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&["y"]));
        assert!(r.is_ok(), "{r:?}");
        assert!(said.contains("approved"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("approval", "approved"));
        assert_eq!(a[1].0, "edit");
        assert!(a[1].3, "an approved edit is applied: {a:?}");
    }

    #[test]
    fn end_of_input_on_the_question_is_a_no() {
        // A closed or piped stdin returns nothing. Reading that as yes would
        // let a run in approve mode edit with nobody watching.
        let (dir, spec) = fixture("eof", EDIT_THEN_STOP);
        let (r, said) = run_with(&dir, &spec, answering(&[]));
        assert!(r.is_ok(), "{r:?}");
        assert!(said.contains("declined"), "{said}");
        let a = actions(&trail(&dir));
        assert_eq!(a[0].2, "declined");
        assert!(!a[1].3);
    }

    #[test]
    fn only_an_explicit_yes_counts() {
        for yes in ["y", "Y", "yes", " y \r\n"] {
            assert!(is_yes(Some(yes)), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yy", "ok"] {
            assert!(!is_yes(Some(no)), "{no:?}");
        }
        assert!(!is_yes(None));
    }

    #[test]
    fn the_interface_s_channel_is_asked_and_its_no_is_recorded() {
        // The path `trellis tui` takes: the gate's question goes out on a
        // channel rather than to a terminal, and the answer comes back on the
        // request's own `oneshot`.
        let (dir, spec) = fixture("channel", EDIT_THEN_STOP);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<trellis_loop::ApprovalRequest>(1);
        let asker = std::thread::spawn(move || {
            let req = rx.blocking_recv().expect("the gate asks");
            let q = req.question.clone();
            let _ = req.reply.send(false);
            q
        });
        let (r, said) = run_with(&dir, &spec, Human::Channel(tx));
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(asker.join().unwrap(), "edit a.py (+1 -1)");
        assert!(
            !said.contains("[y/N]"),
            "nothing is asked on the line: {said}"
        );
        let a = actions(&trail(&dir));
        assert_eq!((a[0].0.as_str(), a[0].2.as_str()), ("approval", "declined"));
        assert!(!a[1].3);
    }
}
