//! The CLI. Thin: parses, dispatches, formats.
//!
//! Status: `index`, `radius`, `check`, `doctor`, `run`, `resume` and
//! `partitions` (a dry-run planner) are wired to real implementations.
//!
//! `run` drives the loop with either a scripted model, a real serving
//! runtime (`llamacpp:`, `vllm:`, `sglang:`) or a hosted API (`anthropic:`,
//! `openai-chat:`; ADR 0024). The scripted driver is not a
//! placeholder — it exercises real files, real checks and real rollback with
//! only judgement stubbed, which is what makes the loop's control flow
//! testable without inference. Results from it are never a model floor (S7).

mod ask;
mod config;
mod doctor_cmd;
mod first_run;
mod hosted;
mod image_input;
mod index_cmd;
mod install;
mod liveness;
mod log_cmd;
mod models_cmd;
mod orchestrate;
mod process_agent;
mod run_cmd;
mod services;
mod session;
mod start_screen;
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
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use doctor_cmd::{cmd_check, cmd_doctor, cmd_init};
use index_cmd::{cmd_index, cmd_partitions, cmd_radius};
pub(crate) use log_cmd::plain;
use log_cmd::{cmd_log, cmd_why};
use models_cmd::cmd_models;
pub(crate) use run_cmd::{
    Human, ModelChoice, RunControl, RunExit, RunSeeding, audit_modes, cmd_resume, cmd_run,
    cmd_run_with, show_answer, show_question,
};
#[cfg(test)]
pub(crate) use run_cmd::{
    PANIC_IN_RUN, RUN_POLLED_ON, RUN_THREAD, print_undelivered, steering_entries,
};

#[derive(Parser)]
#[command(
    name = "trellis",
    about = "An agent harness for Python repositories: blast radius mapped, \
             checks executed, decisions recorded.",
    version,
    // ADR 0050 section 1: how to start comes before the subcommands, so the
    // first thing `trellis --help` says is that `trellis` alone is enough.
    help_template = "{about-with-newline}\n{usage-heading} {usage}\n\n\
                     Start:\n  \
                     trellis          open the terminal interface\n  \
                     trellis --tui    the same, said explicitly\n  \
                     trellis --dsk    open the desktop window\n  \
                     trellis --web    serve the desktop's page on 127.0.0.1\n\n\
                     {all-args}{after-help}"
)]
struct Cli {
    /// Open the terminal interface; what bare `trellis` does (ADR 0050).
    #[arg(long, hide = true, conflicts_with_all = ["dsk", "web"])]
    tui: bool,

    /// Open the desktop window (ADR 0048, ADR 0050).
    #[arg(long, hide = true, conflicts_with = "web")]
    dsk: bool,

    /// Serve the desktop's page on 127.0.0.1 (ADR 0050 section 5).
    #[arg(long, hide = true)]
    web: bool,

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
        ///   `anthropic:https://api.anthropic.com#<model>`  hosted (ADR 0024)
        ///   `openai-chat:<base url>#<model>`               hosted (ADR 0024)
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
        /// Cap concurrent agents. Only `1` is accepted until multi-agent
        /// mode is built (v1.1); `auto`, deriving the cap from measured
        /// backend batching capacity, is refused with it. Never derived from
        /// CPU count (S18).
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
        /// Attach an image (PNG, JPEG, GIF or WebP, at most 5 MiB) to the
        /// task. Repeatable. Hosted tier-1 models only; any other model
        /// refuses the run before the snapshot.
        ///
        /// Owner-directed growth beyond brief 4.12 (ADR 0041). Not resized.
        /// Its base64 counts against `[model] context_bytes`, and the trail
        /// records its name, size and SHA-256, never its bytes.
        #[arg(long, value_name = "PATH")]
        image: Vec<PathBuf>,
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

    /// Continue from the last trail entry, or from a named run.
    Resume {
        /// A run id as `trellis log` prints it. Omit it for the newest run.
        ///
        /// Owner-directed growth beyond brief 4.12 (ADR 0038). The named
        /// trail is read back as data exactly as the newest one is.
        #[arg(value_name = "RUN_ID")]
        run_id: Option<String>,
    },

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

    /// Copy skills and prompt templates from a package source into the user
    /// config directory's packages/ (ADR 0039).
    ///
    /// `<git-url|path>@<ref>`: the ref is required and recorded with the
    /// commit it resolved to, in packages/packages.toml. Only
    /// skills/<name>/ text files and prompts/<name>.md are copied; hooks,
    /// tools and anything else in the source are skipped and named. Runs
    /// read them after the repository's own skills and templates, which win
    /// a name.
    Install {
        /// `<git-url|path>@<ref>`.
        #[arg(
            value_name = "SOURCE@REF",
            required_unless_present_any = ["list", "remove"],
            conflicts_with_all = ["list", "remove"]
        )]
        source: Option<String>,
        /// Print each installed package's source, ref and commit.
        #[arg(long, conflicts_with = "remove")]
        list: bool,
        /// Delete one installed package and its manifest entry.
        #[arg(long, value_name = "NAME")]
        remove: Option<String>,
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
    let matches =
        Cli::command().get_matches_from(subcommand_lowercased(std::env::args_os().collect()));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    // ADR 0050 section 1: an interface flag says which interface to open,
    // and a subcommand is not an interface, so the two together are a
    // mistake to name rather than one to pick a winner for.
    let interface = [(cli.tui, "--tui"), (cli.dsk, "--dsk"), (cli.web, "--web")]
        .into_iter()
        .find_map(|(on, name)| on.then_some(name));
    if let (Some(flag), Some(sub)) = (interface, matches.subcommand_name()) {
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                format!("`{flag}` opens an interface and cannot be used with `{sub}`"),
            )
            .exit();
    }
    // Accepted by the parser so the surface ADR 0050 records is the one
    // people learn, and refused here until it is built: a flag that
    // appeared to start something it cannot is worse than one that says so.
    if cli.dsk {
        anyhow::bail!(
            "the desktop is not built yet (ADR 0048, story S19); `trellis` opens the terminal interface"
        );
    }
    if cli.web {
        anyhow::bail!(
            "the web view is not built yet (ADR 0050, story S19); `trellis` opens the terminal interface"
        );
    }
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
    // `install` joins them too: packages are user state (ADR 0039).
    let needs_repo = !matches!(
        cli.command,
        Some(Command::Init | Command::Model { .. } | Command::Install { .. })
    ) && !matches!(&cli.command, Some(Command::Vault(a)) if a.is_init());
    let root = match cli.root {
        Some(r) => r,
        None => match discover_root() {
            Ok(r) => r,
            Err(_) if !needs_repo => std::env::current_dir()?,
            // ADR 0050 §3: bare `trellis` at a terminal asks where to go
            // instead. Only bare `trellis`, and only with someone to answer:
            // a pipe, a redirect or CI gets the refusal below, as every
            // other command does.
            Err(_) if cli.command.is_none() && interactive() => match start_screen::offer()? {
                Some(r) => r,
                None => return Ok(()),
            },
            Err(e) => return Err(e.context("cannot determine the repository root")),
        },
    };
    // ADR 0050 §3: the start screen's list. Only a root that is a
    // repository, so an exempt command's fallback to the working directory
    // or a `--root` naming a plain tree is not offered back later. A list
    // that cannot be written costs the next start screen a line, not this
    // command its run.
    if root.join(".git").exists()
        && let Some(file) = start_screen::file()
        && let Err(e) = start_screen::record(&file, &root)
    {
        tracing::debug!(error = %format!("{e:#}"), "recent repositories not recorded");
    }
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
    // be a home folder — `trellis index` there walked 25,147 files before
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
            image,
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
            // ADR 0025 §3: frontmatter stripped, `--args` substituted.
            let task = templates::task_text(&root, prompt.as_deref(), args.as_deref(), &task)?;
            let model_from_user = model.is_some();
            // A config that fails to load says why, rather than reading as
            // "no model": the refusal may be the point (ADR 0024 §3).
            let model = match model {
                Some(m) => m,
                None => config::RepoConfig::load(&root)?
                    .model()
                    .with_context(first_run::no_model)?,
            };
            let flag = match (approve, auto) {
                (true, _) => Some(config::ControlMode::Approve),
                (_, true) => Some(config::ControlMode::Auto),
                _ => None,
            };
            let exit = cmd_run_with(
                &root,
                &task,
                ModelChoice {
                    spec: &model,
                    unconstrained,
                },
                RunSeeding {
                    resume,
                    focus: focus.as_deref(),
                    profile: None,
                    model_from_user,
                    model_source: crate::hosted::SpecSource::Repository,
                    // `trellis run` has no task for a question to wait on.
                    queued: false,
                    services: &[],
                },
                RunControl {
                    flag,
                    human: Human::terminal(),
                    steering: None,
                },
                output::RunExtras {
                    skills: skill,
                    images: image,
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
        Command::Resume { run_id } => cmd_resume(&root, run_id.as_deref()),
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
        Command::Install {
            source,
            list,
            remove,
        } => install::run(source, list, remove),
        Command::Rpc => rpc::serve(
            &root,
            std::io::stdin().lock(),
            output::stdout_sink(),
            rpc::Mode::Process,
        ),
    }
}

/// `args` with the subcommand's name lowered when its lowercase form is a
/// subcommand (ADR 0050 section 4): `trellis RUN` is `trellis run`.
///
/// Only the first argument that is not a flag, or the value of a global flag
/// that takes one, is compared, and only a name clap knows is changed, so a
/// task text, a path or a symbol is never touched.
fn subcommand_lowercased(mut args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    const TAKES_VALUE: [&str; 2] = ["--root", "--log-json"];
    let names: Vec<String> = Cli::command()
        .get_subcommands()
        .flat_map(|c| {
            std::iter::once(c.get_name().to_string()).chain(c.get_all_aliases().map(str::to_string))
        })
        .collect();
    let mut i = 1;
    while i < args.len() {
        let Some(a) = args[i].to_str() else { break };
        if TAKES_VALUE.contains(&a) {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        let lower = a.to_lowercase();
        if lower != a && names.contains(&lower) {
            args[i] = lower.into();
        }
        break;
    }
    args
}

/// Whether there is a person at a terminal: stdin and stdout for the
/// interface, stderr for the start screen's prompt (ADR 0050 §3).
fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
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

/// The recall backstop's part of `trellis index` (ADR 0031).
///
/// Its counts stand beside the resolver's and are never added to `resolved`
/// or divided by anything (rule 3). It runs after `index.bin` is written and
/// its edges are not stored there: they depend on the server's version, which
/// the file does not record. So the version is printed here instead.
fn backstop_lines(index: &trellis_index::Index, root: &Path, enabled: bool) -> Vec<String> {
    if !enabled {
        return vec![BACKSTOP_OFF.to_string()];
    }
    let sites = trellis_index::doubt_sites(index);
    let r = trellis_index::run_backstop(
        index,
        root,
        &sites,
        &trellis_index::BackstopServer::pyright(),
    );
    render_backstop(&r)
}

const BACKSTOP_OFF: &str = "backstop: off (no `[index] backstop` in .trellis/config.toml); \
     the figures above are the resolver's alone";

/// The report as `trellis index` prints it. Apart from running the server so
/// the text can be asserted exactly without Pyright installed.
fn render_backstop(r: &trellis_index::BackstopReport) -> Vec<String> {
    let mut out = Vec::new();
    match &r.status {
        trellis_index::BackstopStatus::NotRun { reason } => {
            out.push(format!("backstop: pyright did not run ({reason})"));
            out.push(format!(
                "  {} Unresolved or Candidates site(s) keep the resolver's verdict",
                r.not_queried
            ));
            return out;
        }
        trellis_index::BackstopStatus::Ran { server, stopped } => {
            out.push(format!(
                "backstop: pyright ran ({})",
                server.as_deref().unwrap_or("version unknown")
            ));
            if let Some(why) = stopped {
                out.push(format!("  stopped part way: {why}"));
            }
        }
    }
    out.push(format!(
        "  queried          {:>6}  Unresolved or Candidates sites only",
        r.queried
    ));
    out.push(format!(
        "  bound by pyright {:>6}  counted apart: not in `resolved`, not in the graph",
        r.edges.len()
    ));
    out.push(format!(
        "  no answer        {:>6}  the server returned no location; resolver's verdict kept",
        r.no_answer
    ));
    out.push(format!(
        "  refused          {:>6}  an answer this backstop would not bind; resolver's verdict kept",
        r.refused_total()
    ));
    for (why, n) in &r.refused {
        out.push(format!("    {:<28} {n:>6}", why.describe()));
    }
    if r.not_queried > 0 {
        out.push(format!(
            "  not queried      {:>6}  keep the resolver's verdict",
            r.not_queried
        ));
    }
    out.push(
        "  A pyright-bound edge is the server's reading of the annotations, not a verified callee;"
            .to_string(),
    );
    out.push("  those sites stay unpartitionable.".to_string());
    out
}

#[cfg(test)]
mod backstop_tests {
    use super::*;
    use std::collections::BTreeMap;
    use trellis_index::{BackstopReport, BackstopStatus, Refusal};

    fn report(status: BackstopStatus) -> BackstopReport {
        BackstopReport {
            status,
            edges: Vec::new(),
            queried: 7580,
            no_answer: 5000,
            refused: BTreeMap::from([
                (Refusal::OutsideRepository, 700),
                (Refusal::OutsideCandidates, 21),
            ]),
            not_queried: 22,
            skipped_certain: 0,
        }
    }

    #[test]
    fn backstop_lines_print_single_spaces_and_split_no_answer_from_refused() {
        // #177: a line continuation inside two literals printed "not a
        // verified callee" with ten spaces in it, and one count stood for
        // both "no answer" and "refused". The whole text is asserted, so a
        // stray run of spaces or a merged count fails here.
        let ran = report(BackstopStatus::Ran {
            server: Some("pyright 1.1.414, from its package.json".into()),
            stopped: None,
        });
        assert_eq!(
            render_backstop(&ran),
            [
                "backstop: pyright ran (pyright 1.1.414, from its package.json)",
                "  queried            7580  Unresolved or Candidates sites only",
                "  bound by pyright      0  counted apart: not in `resolved`, not in the graph",
                "  no answer          5000  the server returned no location; resolver's verdict kept",
                "  refused             721  an answer this backstop would not bind; resolver's verdict kept",
                "    outside the repository          700",
                "    outside the Candidates set       21",
                "  not queried          22  keep the resolver's verdict",
                "  A pyright-bound edge is the server's reading of the annotations, not a verified callee;",
                "  those sites stay unpartitionable.",
            ]
        );
        assert_eq!(
            backstop_lines(&trellis_index::Index::default(), Path::new("."), false),
            [
                "backstop: off (no `[index] backstop` in .trellis/config.toml); the figures above are the resolver's alone"
            ]
        );
    }

    #[test]
    fn backstop_lines_say_version_unknown_only_when_no_source_named_one() {
        let unknown = report(BackstopStatus::Ran {
            server: None,
            stopped: None,
        });
        assert_eq!(
            render_backstop(&unknown)[0],
            "backstop: pyright ran (version unknown)"
        );
    }
}
