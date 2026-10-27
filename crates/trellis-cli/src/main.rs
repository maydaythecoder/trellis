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
mod approval; // approve mode at a terminal: who answers, and the prompt (ADR 0018)
mod ask; // a question answered, as distinct from a task run
mod atlas; // the Atlas's reads: symbols, history and a codebase at a ref (rpc)
mod check_cmd; // `check`: the checks run on their own
mod codebase_graph; // the codebase graph as data, for the Atlas, `overview` and `index`
mod config; // `.trellis/config.toml`
mod context_files; // context files, system prompt and skills a run's prompt takes (ADR 0025)
mod doctor_cmd; // `doctor`: tier, batching and tooling probes
mod doctor_extra; // `doctor`'s lines on user config, context, trust, profiles and vault
mod doctor_supply; // `doctor`'s lockfile line: advisories in Cargo.lock (ADR 0057)
mod driver; // the model driver a run's loop is generic over
mod first_run; // choosing a model on a first run with none configured (ADR 0050 §2)
mod floor; // the S7 floor as measured: doctor's floor line and run's warning
mod hosted; // hosted providers: a spec resolved against the user's config (ADR 0024)
mod image_input; // `run --image` (ADR 0041)
mod index_cmd; // `index`, `radius` and `partitions`
mod init_cmd; // `init`: the `.trellis/` scaffold and its prerequisites
mod install; // `install`: skills and prompt templates from a package source
mod launch; // starting the desktop from `--dsk` (ADR 0050 §1, ADR 0048)
mod liveness; // whether a pid is running on this machine
mod log_cmd; // `log` and `why`
mod models_cmd; // `model`: fetched weights
mod orchestrate; // `agents` and `routines` (ADR 0026)
mod output; // where output goes, `plain` the terminal-safe printer, and the JSONL framing of `--json` and `rpc`
mod overview; // `overview` (ADR 0050 §3)
mod process_agent; // process agents and the carry-back of what they wrote
mod replay; // the replay recording every run writes (ADR 0056)
mod rpc; // `rpc`: JSONL commands on stdin
mod run_cmd; // `run` and `resume`, and the trail a run writes
mod run_partitioned; // a multi-agent run: `run --agents N --target ...` (ADR 0045)
mod services; // services an agent profile declares (ADR 0026 §12)
mod session; // the interactive session: bare `trellis` in a repository
mod session_extra; // the session's slash commands, `!cmd` and in-flight stdin
mod start_screen; // bare `trellis` outside a repository (ADR 0050 §3)
mod startup; // tracing, the repository root, and where bare `trellis` goes
mod templates; // prompt templates with arguments (ADR 0025 §3)
#[cfg(test)]
mod testutil; // fixtures shared by this crate's tests
mod tooling; // what a run may execute beyond its checks
mod trust_cmd; // `trust` (ADR 0023 §4)
mod user_config; // the user-level configuration directory and its layering
mod vault_cmd; // `vault` (ADR 0022)
mod work; // `queue` and `work` (ADR 0020, ADR 0026)

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) use approval::print_undelivered;
pub(crate) use approval::{Human, RunControl, show_answer, show_question};
use check_cmd::cmd_check;
use doctor_cmd::cmd_doctor;
use index_cmd::{cmd_index, cmd_partitions, cmd_radius};
use init_cmd::cmd_init;
use log_cmd::{cmd_log, cmd_why};
use models_cmd::cmd_models;
pub(crate) use output::plain;
pub(crate) use run_cmd::{
    ModelChoice, RunExit, RunSeeding, audit_modes, cmd_resume, cmd_run, cmd_run_with,
};
#[cfg(test)]
pub(crate) use run_cmd::{PANIC_IN_RUN, RUN_POLLED_ON, RUN_THREAD};

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

    /// Omit to start as the Start lines above say: at a terminal, the
    /// desktop window and the overview, or the terminal interface without a
    /// desktop; on a pipe or redirect, the line-based session.
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
    Run(RunArgs),

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
    /// `trellis --tui` opens it too, as bare `trellis` does at a terminal
    /// with no desktop to open. This is the surface for watching one run
    /// rather than issuing a sequence of commands. It is `trellis-tui`, the
    /// Go client of `trellis rpc`, installed beside this binary (ADR 0052);
    /// without one, this says where it was looked for and stops.
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
    /// Each task runs in a worktree of its own on `trellis/task/<id>`, where
    /// a converged change waits for review (ADR 0053); never claims a task
    /// while the worktree is dirty.
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
            required_unless_present_any = ["list", "remove", "upgrade"],
            conflicts_with_all = ["list", "remove", "upgrade"]
        )]
        source: Option<String>,
        /// Print each installed package's source, ref, commit and whether
        /// its files still match the digest recorded at install.
        #[arg(long, conflicts_with_all = ["remove", "upgrade"])]
        list: bool,
        /// Delete one installed package and its manifest entry.
        #[arg(long, value_name = "NAME", conflicts_with = "upgrade")]
        remove: Option<String>,
        /// Re-resolve one installed package's ref and install its current
        /// commit over the old copy. Refused when the copy was edited since
        /// it was installed (the paths are named); there is no flag to
        /// overwrite them.
        #[arg(long, value_name = "NAME")]
        upgrade: Option<String>,
        /// With a source: fetch and list what would be copied and what
        /// skipped, and write nothing.
        #[arg(long, requires = "source")]
        dry_run: bool,
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

#[derive(clap::Args)]
struct RunArgs {
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
}

fn main() -> Result<()> {
    // Parsed first so `--log-json` is known before the subscriber is built.
    let matches = Cli::command().get_matches_from(startup::subcommand_lowercased(
        std::env::args_os().collect(),
    ));
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
    startup::init_tracing(&cli)?;

    let explicit = cli.root.is_some();
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
    let Some(root) = startup::resolve_root(&cli, ttys, &mut terminal_only)? else {
        return Ok(());
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
    let is_overview = matches!(cli.command, Some(Command::Overview));
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
        // hands the terminal to `trellis-tui`, or refuses naming where it
        // looked (#516).
        //
        // The line session is still there as `trellis session`, and is still
        // what runs when there is no terminal to take over: a pipe, a
        // redirect, or CI.
        let route = startup::bare_route(ttys.0, ttys.1, ttys.2, true, true);
        if !terminal_only && startup::bare_desktop(route, Some(&root), &root)? {
            return Ok(());
        }
        let spec = config::RepoConfig::load(&root).ok().and_then(|c| c.model());
        if route == startup::Route::Tui {
            return startup::terminal_ui(&root, None, spec, false);
        }
        return session::Session::open(&root, None)?.run();
    };

    dispatch(&root, command)
}

/// One subcommand, run in `root`.
fn dispatch(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::Index { graph, limit } => cmd_index(root, graph, limit),
        Command::Doctor { model } => {
            // Falls back to the repository's configured model.
            //
            // It did not, and the result was `doctor` reporting "not
            // configured" about a repository whose `.trellis/config.toml`
            // names a model and whose TUI shows it in the status bar. The
            // one command whose job is "is this set up" was the one command
            // that did not look.
            let configured = config::RepoConfig::load(root).ok().and_then(|c| c.model());
            let spec = model.as_deref().map(str::to_string).or(configured);
            cmd_doctor(root, spec.as_deref(), model.is_some())
        }
        Command::Check { paths } => cmd_check(root, paths),
        Command::Radius { symbol } => cmd_radius(root, &symbol),
        Command::Run(args) => {
            let exit = run_cmd::cmd_run_args(root, args)?;
            // The one place that decides the process ends, because this is
            // the one caller that *is* the process.
            if exit.code() != 0 {
                std::process::exit(exit.code());
            }
            Ok(())
        }
        Command::Session => session::Session::open(root, None)?.run(),
        Command::Init => cmd_init(root),
        Command::Tui { task, model } => {
            let from_user = model.is_some();
            let spec = model.or_else(|| config::RepoConfig::load(root).ok()?.model());
            startup::terminal_ui(root, task, spec, from_user)
        }
        Command::Resume { run_id } => cmd_resume(root, run_id.as_deref()),
        Command::Log { limit, .. } if output::json() => {
            output::log_json(root, limit, &mut std::io::stdout())
        }
        Command::Log { limit, actions } => cmd_log(root, limit, actions),
        Command::Overview => overview::run(root, output::json(), &mut std::io::stdout().lock()),
        Command::Why { symbol } => cmd_why(root, &symbol),
        // Takes no root at all: models are user state, not repository state.
        // The exemption that makes this reachable outside a repository is in
        // `startup::resolve_root`'s `needs_repo` — this arm alone was never
        // enough.
        Command::Model { action } => cmd_models(action),
        Command::Partitions { symbols } => cmd_partitions(root, &symbols),
        Command::Queue { action } => work::queue(root, action),
        Command::Agents { action } => orchestrate::agents(root, action),
        Command::Routines { action } => orchestrate::routines(root, action),
        Command::Work(args) => {
            // ADR 0025 §4: each task's run brackets itself; these bracket
            // the invocation, so a reader knows when `work` itself is done.
            let frames = output::json().then(output::stdout_sink);
            if let Some(s) = &frames {
                output::frame(s, &serde_json::json!({"type": "work_start", "version": 1}));
            }
            let result = work::work(root, args);
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
        Command::Vault(args) => vault_cmd::run(root, args),
        Command::Trust { show, revoke } => trust_cmd::run(root, show, revoke),
        Command::Install {
            source,
            list,
            remove,
            upgrade,
            dry_run,
        } => install::run(source, list, remove, upgrade, dry_run),
        Command::Rpc => rpc::serve(
            root,
            std::io::stdin().lock(),
            output::stdout_sink(),
            rpc::Mode::Process,
        ),
    }
}
