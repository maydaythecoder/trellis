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

// One line per module: what it holds. Alphabetical; a command's
// implementation lives in its `*_cmd` module or in the module it names.
mod ask; // a question answered, as distinct from a task run
mod atlas; // the Atlas's reads: symbols, history and a codebase at a ref (rpc)
mod config; // `.trellis/config.toml`
mod context_files; // context files, system prompt and skills a run's prompt takes (ADR 0025)
mod doctor_cmd; // `doctor`, `check` and `init`
mod doctor_extra; // `doctor`'s lines on user config, context, trust, profiles and vault
mod first_run; // choosing a model on a first run with none configured (ADR 0050 §2)
mod floor; // the S7 floor as measured: doctor's floor line and run's warning
mod hosted; // hosted providers: a spec resolved against the user's config (ADR 0024)
mod image_input; // `run --image` (ADR 0041)
mod index_cmd; // `index`, `radius` and `partitions`
mod install; // `install`: skills and prompt templates from a package source
mod launch; // starting the desktop from `--dsk` (ADR 0050 §1, ADR 0048)
mod liveness; // whether a pid is running on this machine
mod log_cmd; // `log` and `why`, and `plain`, the terminal-safe printer
mod models_cmd; // `model`: fetched weights
mod orchestrate; // `agents` and `routines` (ADR 0026)
mod output; // where output goes, and the JSONL framing of `--json` and `rpc`
mod overview; // `overview` (ADR 0050 §3)
mod process_agent; // process agents and the carry-back of what they wrote
mod rpc; // `rpc`: JSONL commands on stdin
mod run_cmd; // `run` and `resume`, and the trail a run writes
mod services; // services an agent profile declares (ADR 0026 §12)
mod session; // the interactive session: bare `trellis` in a repository
mod session_extra; // the session's slash commands, `!cmd` and in-flight stdin
mod start_screen; // bare `trellis` outside a repository (ADR 0050 §3)
mod templates; // prompt templates with arguments (ADR 0025 §3)
#[cfg(test)]
mod testutil; // fixtures shared by this crate's tests
mod tooling; // what a run may execute beyond its checks
mod trust_cmd; // `trust` (ADR 0023 §4)
mod tui_bridge; // the agent loop as a stream of TUI messages
mod tui_run; // `tui <task>`
mod user_config; // the user-level configuration directory and its layering
mod vault_cmd; // `vault` (ADR 0022)
mod work; // `queue` and `work` (ADR 0020, ADR 0026)

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
                     trellis          open the desktop window and print the overview\n  \
                     trellis --tui    open the terminal interface\n  \
                     trellis --dsk    open the desktop window only\n  \
                     trellis --web    serve the desktop's page on 127.0.0.1\n\n\
                     {all-args}{after-help}"
)]
struct Cli {
    /// Open the terminal interface rather than the desktop (ADR 0050).
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
        /// Cap concurrent agents: a number of at least 1, or `auto`, which
        /// takes the cap from the backend's measured batching capacity and is
        /// 1 for an unmeasured one. Never derived from CPU count (S18).
        ///
        /// Above 1, and `auto`, need `--target`. With `--target`, each
        /// partition of the plan runs in its own agent and worktree, and the
        /// merged union is checked as a whole (#466); one at a time at 1
        /// (#482), and never more at once than the plan has parallel
        /// partitions.
        #[arg(long, default_value = "1")]
        agents: String,
        /// A symbol this run works on, resolved as `trellis radius` resolves
        /// it. Repeatable: each target is one work unit of the plan `trellis
        /// partitions` shows (ADR 0045). Unknown or ambiguous is refused.
        #[arg(long, value_name = "SYMBOL")]
        target: Vec<String>,
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

    /// Recent repositories' last outcomes and, inside one, its last run,
    /// codebase and board, printed once. Reads only; never indexes.
    ///
    /// Runs with or without a repository (ADR 0050 §3). `--json` prints the
    /// same as one object (ADR 0025 §4).
    Overview,

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

/// The log filter from `RUST_LOG`, read by the caller so this stays pure.
///
/// `Targets` rather than `EnvFilter`.
///
/// `EnvFilter` parses field and span predicates, which needs `matchers` and
/// therefore the whole regex engine. Measured two ways, because they answer
/// different questions and were confused for each other once already:
/// `cargo bloat --release --crates` attributed 184 KiB of `regex_syntax`
/// plus 146 KiB of `regex_automata` to the `.text` section, and the release
/// binary on disk fell 5,855,232 -> 4,810,240 bytes, the larger figure
/// being the Unicode tables in `.rdata` that `.text` does not count.
///
/// This project only ever filters as `target=level`, and `Targets` matches
/// targets by prefix exactly as `EnvFilter` did, so `trellis_loop=trace`
/// and `trellis_loop::adapter=trace` behave identically.
///
/// Two forms behave differently and neither is used here: a span-field
/// predicate (`[span{field=x}]`) fails to parse and falls back to the
/// default below, and a target with a field predicate (`trellis[{turn}]=…`)
/// parses but matches nothing, silently. Named because "the trade is
/// recorded" is only true if the record is accurate.
fn log_filter(rust_log: Result<String, std::env::VarError>) -> tracing_subscriber::filter::Targets {
    let default =
        || tracing_subscriber::filter::Targets::new().with_target("trellis", tracing::Level::INFO);
    match rust_log {
        // Set but empty means "say nothing", which is what `EnvFilter` did
        // with it. Parsing it as a directive instead yields one empty
        // directive, which `Targets` reads as ERROR for every target in the
        // process — turning silence into global error logging.
        Ok(s) if s.trim().is_empty() => tracing_subscriber::filter::Targets::new(),
        Ok(s) => s.parse().unwrap_or_else(|_| default()),
        Err(_) => default(),
    }
}

fn main() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = log_filter(std::env::var("RUST_LOG"));
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
            Some(Command::Run { .. } | Command::Work(_) | Command::Log { .. } | Command::Overview)
        )
    {
        anyhow::bail!(
            "--json applies to `run`, `work`, `log` and `overview`; `rpc` always writes JSON"
        );
    }
    output::set_json(cli.json || is_rpc);
    // Per-layer filters rather than one global one: the stdout event layer
    // has a fixed `trellis=info` so a `RUST_LOG` meant for debugging cannot
    // change what a machine reader receives. `RUST_LOG` still governs the
    // human stream and `--log-json`, exactly as before.
    use tracing_subscriber::Layer;
    //
    // Not for `overview`: its stdout is one JSON object, which an event line
    // beside it would turn into something no client can parse.
    let is_overview = matches!(cli.command, Some(Command::Overview));
    let stdout_json = (output::json() && !is_overview).then(|| {
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
    // `overview` too: outside a repository it prints the recent list, and
    // reads nothing under the directory it fell back to (ADR 0050 §3).
    //
    // `--dsk` and `--web` too: outside a repository the desktop opens on
    // its overview (ADR 0050 section 3), so there is nothing to refuse.
    let needs_repo = !cli.dsk
        && !cli.web
        && !matches!(
            cli.command,
            Some(
                Command::Init | Command::Model { .. } | Command::Install { .. } | Command::Overview
            )
        )
        && !matches!(&cli.command, Some(Command::Vault(a)) if a.is_init());
    // Read once, so the start screen and the interface decide from the same
    // three facts (`bare_route`).
    use std::io::IsTerminal;
    let ttys = (
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    );
    // ADR 0050 §1 (#561): bare `trellis` at a terminal opens the desktop;
    // `--tui` keeps the terminal. Once a fallback has been said, the
    // terminal path runs and the desktop is not tried, or said, again.
    let mut terminal_only = cli.tui;
    let root = match cli.root {
        Some(r) => r,
        None => match discover_root() {
            Ok(r) => r,
            Err(_) if !needs_repo => std::env::current_dir()?,
            // ADR 0050 §3: bare `trellis` at a terminal asks where to go
            // instead. Only bare `trellis`, and only with someone to answer:
            // a pipe, a redirect or CI gets the refusal below, as every
            // other command does.
            Err(_)
                if bare_route(ttys.0, ttys.1, ttys.2, cli.command.is_none(), false)
                    == Route::StartScreen =>
            {
                // With a desktop to open, it opens with no root and the
                // start screen is skipped: the desktop shows its own.
                if !terminal_only
                    && bare_desktop(Route::StartScreen, None, &std::env::current_dir()?)?
                {
                    return Ok(());
                }
                terminal_only = true;
                match start_screen::offer_start_screen()? {
                    Some(r) => r,
                    None => return Ok(()),
                }
            }
            Err(e) => return Err(e.context("cannot determine the repository root")),
        },
    };
    // ADR 0050 §3: the start screen's list. Only a root that is a
    // repository, so an exempt command's fallback to the working directory
    // or a `--root` naming a plain tree is not offered back later. A list
    // that cannot be written costs the next start screen a line, not this
    // command its run.
    //
    // Only from an interface start (bare `trellis`, `--tui`, `--dsk`, which
    // parse with no subcommand, and `trellis tui`): the list is where a
    // person opened trellis, and a script running `trellis index` across
    // checkouts is not that. Never a root under the temporary directory,
    // whose fixtures and scratch checkouts are gone within minutes (#555).
    let interface_start = matches!(cli.command, None | Some(Command::Tui { .. }));
    if interface_start
        && root.join(".git").exists()
        && !start_screen::is_temporary(&root, &std::env::temp_dir())
        && let Some(file) = start_screen::recent_dirs_file()
        && let Err(e) = start_screen::record(&file, &root)
    {
        tracing::debug!(error = %format!("{e:#}"), "recent repositories not recorded");
    }
    // ADR 0024 §3: every `key_env` in the effective configuration, the
    // user's file included, is removed from each child process trellis
    // starts. Before any command runs, so no spawn precedes it. A config that
    // does not load registers nothing here; the command reports that itself,
    // and the two built-in key variables are removed regardless.
    //
    // An overview whose root has no `.git` has no root at all, so its
    // config is not read either (ADR 0050 §3); it starts no child there.
    let unrooted_overview = is_overview && !root.join(".git").exists();
    if !unrooted_overview && let Ok(cfg) = config::RepoConfig::load(&root) {
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

    // ADR 0050 section 1: the desktop, started and left running. A root it
    // fell back to (the working directory, outside any repository) is not
    // one it found, so the desktop is given none.
    if cli.dsk {
        let found = explicit || root.join(".git").exists();
        return launch::launch_desktop(found.then_some(root.as_path()));
    }
    // ADR 0050 section 5: the same desktop, serving its page on 127.0.0.1
    // and waited for, on the same root rule.
    if cli.web {
        let found = explicit || root.join(".git").exists();
        return launch::launch_web(found.then_some(root.as_path()));
    }

    let Some(command) = cli.command else {
        // Bare `trellis` at a terminal opens the desktop on this root and
        // prints the overview (ADR 0050 §1, the owner's decision of
        // 2026-10-07, #561). Without a desktop to open, and for `--tui`, it
        // opens the full-screen interface.
        //
        // The line session is still there as `trellis session`, and is still
        // what runs when there is no terminal to take over: a pipe, a
        // redirect, or CI.
        let route = bare_route(ttys.0, ttys.1, ttys.2, true, true);
        if !terminal_only && bare_desktop(route, Some(&root), &root)? {
            return Ok(());
        }
        let spec = config::RepoConfig::load(&root).ok().and_then(|c| c.model());
        if route == Route::Tui {
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
            target,
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
            // that takes the flags. Every refusal below comes before
            // `cmd_run_with`, which owns the snapshot.
            let width = parse_agents(&agents)?;
            if target.is_empty() && width != Agents::Count(1) {
                anyhow::bail!(
                    concat!(
                        "--agents {} needs the run's work units named: pass --target <symbol> ",
                        "once per unit. `trellis partitions <symbol>...` shows the plan those ",
                        "targets would produce without running it (ADR 0045)."
                    ),
                    agents
                );
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
            // Here, not in `run_body`: this is `trellis run`'s own path, and
            // the session, the TUI and rpc share `run_body`, where a line on
            // stderr would land under a drawn interface. Before `agents_for`
            // and `cmd_run_with`, so before any request reaches the runtime.
            // A spec that does not parse gets no warning; `cmd_run_with`
            // refuses it with the reason.
            if let Ok(spec) = model.parse::<trellis_model::ModelSpec>()
                && let Some(w) = floor::run_warning(&spec)
            {
                eprintln!("{}", plain(&w));
            }
            // With no target the width is 1 (refused above otherwise), and
            // the run is today's single-agent run with nothing added. With
            // targets, the plan's partitions run each in its own agent (#466),
            // one at a time at width 1 (ADR 0045 §2, #482), which is also the
            // width of any N on a backend not measured to batch (#483).
            let mut fanout = None;
            if !target.is_empty() {
                let (n, why) = agents_for(&root, width, &target, &model, model_from_user)?;
                fanout = Some(run_cmd::Fanout::plan(&root, &target, n, why)?);
            }
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
                    fanout,
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
        Command::Overview => overview::run(&root, output::json(), &mut std::io::stdout().lock()),
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
    // A flag and a skip rather than an index stepped by hand: every element
    // is visited at most once, so no edit to this loop can make it endless
    // (#556: a mutated `i += 1` that never advanced ran until killed).
    let mut value_next = false;
    for a in args.iter_mut().skip(1) {
        if std::mem::take(&mut value_next) {
            continue;
        }
        let Some(s) = a.to_str() else { break };
        if TAKES_VALUE.contains(&s) {
            value_next = true;
            continue;
        }
        if s.starts_with('-') {
            continue;
        }
        let lower = s.to_lowercase();
        if lower != s && names.contains(&lower) {
            *a = lower.into();
        }
        break;
    }
    args
}

/// Where `trellis` goes once it knows whether it has a root (ADR 0050 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// A subcommand, with a root to run it in.
    Command,
    /// No root and no one to ask: the refusal every command gives.
    Refuse,
    /// Bare `trellis`, no root, a person at a terminal: ask where to go.
    StartScreen,
    /// Bare `trellis` in a repository with a terminal to take over.
    Tui,
    /// Bare `trellis` in a repository behind a pipe, a redirect or CI.
    Session,
}

/// The routing decision, apart from the process so every combination can be
/// tested. `has_root` is false only for a command that needs a repository
/// and found none; an exempt command never asks.
///
/// The interface needs stdin and stdout. The start screen needs stderr too,
/// since its prompt goes there, and a prompt nobody can see reads as a hang.
fn bare_route(
    stdin_tty: bool,
    stdout_tty: bool,
    stderr_tty: bool,
    command_is_none: bool,
    has_root: bool,
) -> Route {
    match (command_is_none, has_root) {
        (false, true) => Route::Command,
        (true, true) if stdin_tty && stdout_tty => Route::Tui,
        (true, true) => Route::Session,
        (true, false) if stdin_tty && stdout_tty && stderr_tty => Route::StartScreen,
        (_, false) => Route::Refuse,
    }
}

/// What bare `trellis` does about the desktop once `bare_route` has decided
/// (ADR 0050 §1, as built 2026-10-07, #561).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// Start the desktop, print the overview and exit; `root` says whether
    /// it is given the root found.
    Desktop { root: bool },
    /// Say on one stderr line why the desktop cannot start, print the
    /// overview, then go on as the route went before.
    Fallback(Route),
    /// Not bare `trellis` at a terminal: the route, as before.
    Unchanged(Route),
}

/// The desktop decision, apart from the process. Only the two routes with a
/// person at a terminal take the desktop: the interface's, in a repository,
/// and the start screen's, outside one. `desktop` is the one found beside
/// this executable; `display` is false on Linux with no display to open it
/// on. The `--tui` flag is checked by the caller and never reaches here.
fn launch_route(route: Route, desktop: Option<&Path>, display: bool) -> Launch {
    let root = match route {
        Route::Tui => true,
        Route::StartScreen => false,
        other => return Launch::Unchanged(other),
    };
    if desktop.is_some() && display {
        Launch::Desktop { root }
    } else {
        Launch::Fallback(route)
    }
}

/// Bare `trellis` at a terminal: starts the desktop on `root` and prints
/// the overview of `here`, returning true when the caller is done. On a
/// fallback, says why on stderr, prints the overview and returns false for
/// the terminal path to run.
fn bare_desktop(route: Route, root: Option<&Path>, here: &Path) -> Result<bool> {
    let exe = std::env::current_exe().context("cannot find the running trellis executable")?;
    let found = launch::find_desktop(&exe);
    let display = launch::display();
    let started = match launch_route(route, found.as_deref().ok(), display) {
        Launch::Unchanged(_) => return Ok(false),
        Launch::Desktop { root: rooted } => {
            // ADR 0050 §2: with no model, the picker runs here, in the
            // terminal, before the window opens. A config that does not load
            // says why rather than reading as "no model", and quitting the
            // picker starts nothing, both as in `tui_run`.
            if rooted
                && let Some(r) = root
                && config::RepoConfig::load(r)?.model().is_none()
                && first_run::offer_model_choice()?.is_none()
            {
                return Ok(true);
            }
            launch::launch_bare(&found, display, &exe, root)
        }
        // `launch_bare` starts nothing here: one of the checks `launch_route`
        // made failed, and it returns that check's line.
        Launch::Fallback(_) => launch::launch_bare(&found, display, &exe, root),
    };
    if let Err(line) = &started {
        let _ = writeln!(std::io::stderr(), "{line}");
    }
    overview::run(here, false, &mut std::io::stdout().lock())?;
    Ok(started.is_ok())
}

/// `trellis run --agents`, parsed (ADR 0045 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agents {
    /// At least 1.
    Count(u32),
    /// From the backend's measured batching capacity (S18).
    Auto,
}

/// `0` would be a run with no agent, and a fraction or a word other than
/// `auto` has no meaning to guess at, so both are refused naming what is
/// accepted.
fn parse_agents(s: &str) -> Result<Agents> {
    if s == "auto" {
        return Ok(Agents::Auto);
    }
    match s.parse::<u32>() {
        Ok(n) if n >= 1 => Ok(Agents::Count(n)),
        _ => anyhow::bail!("--agents takes a number of at least 1, or `auto`; got `{s}`"),
    }
}

/// Resolves every target and the number of agents `--agents` asks for, with
/// why it is that number.
///
/// Called before the snapshot, so an unknown target costs nothing. Targets
/// resolve before the number is judged, so a mistyped symbol is reported as
/// that rather than hidden behind a width.
fn agents_for(
    root: &Path,
    width: Agents,
    targets: &[String],
    spec: &str,
    from_user: bool,
) -> Result<(u32, String)> {
    let index = trellis_index::build_cached(root);
    for t in targets {
        index_cmd::resolve_symbol(&index, t)?;
    }
    let (n, why) = match width {
        Agents::Count(1) => (1, "--agents is 1".to_string()),
        // An explicit N asks the backend as `auto` does (exit gate item 10,
        // #483): N agents on a runtime that serves one request at a time is
        // slower than one after another, and a user who typed N would not
        // know why. Only measured continuous batching keeps N.
        Agents::Count(n) => match batching(root, spec, from_user)? {
            (cap, why) if cap.supports_multi_agent() => (n, format!("--agents is {n}; {why}")),
            (_, why) => (
                1,
                format!(
                    "--agents {n} is 1 because {why}; {n} at once needs a backend measured to \
                     batch continuously, such as llama.cpp started with --parallel {n}"
                ),
            ),
        },
        Agents::Auto => {
            let (cap, why) = batching(root, spec, from_user)?;
            let n = cap.auto_agents();
            (n, format!("--agents auto is {n} because {why}"))
        }
    };
    Ok((n, why))
}

/// The backend's batching capability, by the probe `doctor` uses, with why it
/// is that. Reads the runtime and writes nothing; the probe's own timeout
/// bounds it. A slot count that could not be read is `Unknown`, said so.
fn batching(
    root: &Path,
    spec: &str,
    from_user: bool,
) -> Result<(trellis_model::BatchingCapability, String)> {
    use trellis_model::BatchingCapability as B;
    let parsed: trellis_model::ModelSpec = spec.parse()?;
    let cfg = config::RepoConfig::load(root)?;
    Ok(match doctor_cmd::probe_batching(&parsed, &cfg, from_user) {
        Ok(
            cap @ B::Continuous {
                measured_concurrency,
            },
        ) => (
            cap,
            format!("the runtime reports {measured_concurrency} slots"),
        ),
        Ok(cap @ B::SingleStream) => (cap, "the runtime reports a single slot".into()),
        Ok(cap @ B::ContinuousUnmeasured) => (
            cap,
            "the backend batches continuously but does not report how much".into(),
        ),
        Ok(cap @ B::Unknown) => (
            cap,
            "the backend's batching capability is unknown (a scripted model has none)".into(),
        ),
        Err(e) => (
            B::Unknown,
            format!(
                "the runtime's slot count could not be read ({e}), so its capability is unknown"
            ),
        ),
    })
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

    #[test]
    fn backstop_lines_print_no_not_queried_row_when_every_site_was_queried() {
        // #248: a "not queried 0" row would read as a figure to act on when
        // there is none; the row is printed only for a count above zero.
        let mut all_queried = report(BackstopStatus::Ran {
            server: None,
            stopped: None,
        });
        all_queried.not_queried = 0;
        let lines = render_backstop(&all_queried);
        assert!(
            !lines.iter().any(|l| l.contains("not queried")),
            "{lines:#?}"
        );
    }
}

#[cfg(test)]
mod startup_seam_tests {
    use super::*;

    fn lowered(args: &[&str]) -> Vec<String> {
        subcommand_lowercased(args.iter().map(std::ffi::OsString::from).collect())
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect()
    }

    #[test]
    fn startup_seam_a_flag_value_is_skipped_and_the_subcommand_after_it_lowered() {
        // `x` is `--root`'s value, not the subcommand: losing the skip would
        // compare `x`, stop there, and leave `RUN` for clap to reject.
        assert_eq!(
            lowered(&["trellis", "--root", "x", "RUN"]),
            ["trellis", "--root", "x", "run"]
        );
    }

    #[test]
    fn startup_seam_a_flag_without_a_value_is_passed_over_one_place() {
        assert_eq!(
            lowered(&["trellis", "--json", "RUN"]),
            ["trellis", "--json", "run"]
        );
    }

    #[test]
    fn startup_seam_a_name_clap_does_not_know_is_left_as_typed() {
        // ADR 0050 §4 lowers only a known subcommand, so a typo is reported
        // as typed rather than as a lowercase word nobody wrote.
        assert_eq!(lowered(&["trellis", "NOPE"]), ["trellis", "NOPE"]);
    }

    #[test]
    fn startup_seam_an_empty_or_blank_rust_log_says_nothing() {
        // Parsed as a directive, an empty value is one empty directive, which
        // `Targets` reads as ERROR for every target: silence asked for,
        // error logging given.
        use std::env::VarError;
        use tracing::Level;
        for set in ["", "  "] {
            let f = log_filter(Ok(set.to_string()));
            assert!(
                !f.would_enable("trellis", &Level::ERROR),
                "RUST_LOG={set:?}"
            );
        }
        let unset = log_filter(Err(VarError::NotPresent));
        assert!(unset.would_enable("trellis", &Level::INFO));
        assert!(!unset.would_enable("trellis", &Level::DEBUG));
        let set = log_filter(Ok("trellis_loop=trace".to_string()));
        assert!(set.would_enable("trellis_loop::adapter", &Level::TRACE));
        assert!(!set.would_enable("trellis", &Level::INFO));
    }

    #[test]
    fn startup_seam_bare_route_follows_adr_0050_over_all_32_combinations() {
        // The three rules, stated as properties rather than a copy of the
        // function: a subcommand goes by its root; bare `trellis` with no
        // root asks only when stdin, stdout and stderr are all terminals;
        // with one, the interface needs stdin and stdout and the line
        // session takes the rest.
        for bits in 0u8..32 {
            let [i, o, e, bare, root] = [0, 1, 2, 3, 4].map(|b| bits & (1 << b) != 0);
            let want = match (bare, root) {
                (false, true) => Route::Command,
                (false, false) => Route::Refuse,
                (true, false) if i && o && e => Route::StartScreen,
                (true, false) => Route::Refuse,
                (true, true) if i && o => Route::Tui,
                (true, true) => Route::Session,
            };
            assert_eq!(
                bare_route(i, o, e, bare, root),
                want,
                "stdin {i} stdout {o} stderr {e} bare {bare} root {root}"
            );
        }
    }

    #[test]
    fn startup_seam_with_no_root_one_redirected_stream_refuses() {
        // A pipe, a redirect or CI gets the refusal every other command
        // gets: a prompt nobody can see would read as a hang.
        assert_eq!(
            bare_route(true, true, true, true, false),
            Route::StartScreen
        );
        for (i, o, e) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert_eq!(
                bare_route(i, o, e, true, false),
                Route::Refuse,
                "{i} {o} {e}"
            );
        }
        assert_eq!(bare_route(true, true, true, false, false), Route::Refuse);
    }

    /// A desktop path for `launch_route`, which only asks whether there is one.
    fn desktop() -> Option<&'static Path> {
        Some(Path::new("trellis-desktop"))
    }

    #[test]
    fn launch_route_desktop_and_root_opens_the_desktop_on_the_root() {
        // The risk: bare `trellis` in a repository still taking over the
        // terminal when the owner asked for the window (ADR 0050 §1, #561).
        assert_eq!(
            launch_route(Route::Tui, desktop(), true),
            Launch::Desktop { root: true }
        );
    }

    #[test]
    fn launch_route_desktop_and_no_root_skips_the_start_screen() {
        // The risk: a start screen in the terminal asking where to go while
        // the desktop, which can ask the same, is right there.
        assert_eq!(
            launch_route(Route::StartScreen, desktop(), true),
            Launch::Desktop { root: false }
        );
    }

    #[test]
    fn launch_route_desktop_missing_falls_back_to_the_terminal_path() {
        // The risk: no desktop installed leaving bare `trellis` with nothing
        // to open; it says so, then goes where it went before.
        assert_eq!(
            launch_route(Route::Tui, None, true),
            Launch::Fallback(Route::Tui)
        );
        assert_eq!(
            launch_route(Route::StartScreen, None, true),
            Launch::Fallback(Route::StartScreen)
        );
    }

    #[test]
    fn launch_route_no_display_falls_back_to_the_terminal_path() {
        // The risk: a window started on a headless Linux box, failing out of
        // sight while the terminal shows nothing.
        assert_eq!(
            launch_route(Route::Tui, desktop(), false),
            Launch::Fallback(Route::Tui)
        );
        assert_eq!(
            launch_route(Route::StartScreen, desktop(), false),
            Launch::Fallback(Route::StartScreen)
        );
    }

    #[test]
    fn launch_route_tui_subcommand_keeps_the_terminal_interface() {
        // The risk: `trellis tui`, a subcommand, opening the desktop. The
        // `--tui` flag never reaches `launch_route` (`main` checks it first);
        // `launch_route_tui_flag_*` in tests/cli.rs holds that end.
        assert_eq!(
            launch_route(Route::Command, desktop(), true),
            Launch::Unchanged(Route::Command)
        );
    }

    #[test]
    fn launch_route_no_terminal_keeps_the_line_session_and_the_refusal() {
        // The risk: a pipe, a redirect or CI starting a window nobody asked
        // for. Without a terminal behaviour is unchanged (ADR 0050 §1).
        assert_eq!(
            launch_route(Route::Session, desktop(), true),
            Launch::Unchanged(Route::Session)
        );
        assert_eq!(
            launch_route(Route::Refuse, desktop(), true),
            Launch::Unchanged(Route::Refuse)
        );
    }
}
