//! Starting the process, in the order `main` calls these: the tracing
//! subscriber, the repository root, and where bare `trellis` goes when no
//! subcommand was given. Apart from `main.rs` so that file is the command
//! line and its dispatch.

use crate::{Cli, Command, config, first_run, launch, output, overview, rpc, start_screen};
use anyhow::{Context, Result};
use clap::CommandFactory;
use std::io::Write as _;
use std::path::{Path, PathBuf};

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

/// The tracing subscriber: the human stream, `--log-json`'s file and, with
/// `--json` or `rpc`, the stdout event layer. `--json` on a command it would
/// do nothing for is refused here, before anything prints.
pub(crate) fn init_tracing(cli: &Cli) -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = log_filter(std::env::var("RUST_LOG"));
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
                // After `set_json`, so it asks about the stream `human`
                // actually writes to.
                .with_ansi(output::human_colour())
                .with_writer(output::human)
                .with_filter(filter.clone()),
        )
        .with(json.map(|l| l.with_filter(filter)))
        .with(stdout_json)
        .with(is_rpc.then(rpc::TurnCapture::default))
        .init();
    Ok(())
}

/// The repository root: `--root`, else the nearest `.git` at or above the
/// working directory, else the working directory for a command that needs
/// no repository. `None` means bare `trellis` found no root and the desktop
/// or the start screen has already answered, so the caller is done.
/// `terminal_only` is set once a fallback to the terminal has been said.
pub(crate) fn resolve_root(
    cli: &Cli,
    ttys: (bool, bool, bool),
    terminal_only: &mut bool,
) -> Result<Option<PathBuf>> {
    // Two commands are exempt, and both for the same reason: they are how
    // someone gets ready to use a repository, so requiring one inverts the
    // order. `init` scaffolds `.trellis/` in a directory that is not set up
    // yet; `model` fetches weights, which are user state and live outside any
    // repository — the dispatch arm in `main.rs` says so and, until this list
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
    let root = match &cli.root {
        Some(r) => r.clone(),
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
                if !*terminal_only
                    && bare_desktop(Route::StartScreen, None, &std::env::current_dir()?)?
                {
                    return Ok(None);
                }
                *terminal_only = true;
                // The repository the start screen picks is handed to
                // `trellis-tui` (#516). With none beside this binary that
                // handoff refuses, so refuse before asking a question whose
                // answer would go nowhere.
                let exe = std::env::current_exe()
                    .context("cannot find the running trellis executable")?;
                if let Err(tried) = launch::find_tui(&exe) {
                    anyhow::bail!(launch::tui_not_found(&tried));
                }
                match start_screen::offer_start_screen()? {
                    Some(r) => r,
                    None => return Ok(None),
                }
            }
            Err(e) => return Err(e.context("cannot determine the repository root")),
        },
    };
    Ok(Some(root))
}

/// `args` with the subcommand's name lowered when its lowercase form is a
/// subcommand (ADR 0050 section 4): `trellis RUN` is `trellis run`.
///
/// Only the first argument that is not a flag, or the value of a global flag
/// that takes one, is compared, and only a name clap knows is changed, so a
/// task text, a path or a symbol is never touched.
pub(crate) fn subcommand_lowercased(mut args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
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
pub(crate) enum Route {
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
pub(crate) fn bare_route(
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

/// The terminal interface (ADR 0052 decision 2.2, ADR 0050 §1, #514, #516).
///
/// With `trellis-tui` beside this executable, first run keeps its turn in
/// Rust (ADR 0050 §2) and then the terminal is handed to it. Without one,
/// this refuses, naming where `trellis-tui` was looked for and the line
/// interface that still runs: the Rust TUI fallback shipped for one release
/// after the handoff and is retired (#516). `spec` is the model to use,
/// `--model`'s when `from_user`, otherwise the configured one.
pub(crate) fn terminal_ui(
    root: &Path,
    task: Option<String>,
    spec: Option<String>,
    from_user: bool,
) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the running trellis executable")?;
    let tui =
        launch::find_tui(&exe).map_err(|tried| anyhow::anyhow!(launch::tui_not_found(&tried)))?;
    if spec.is_none() {
        // First run: the picker, in this terminal, before the handoff. It
        // needs a terminal on both ends: it waits on console input, which
        // never arrives from a pipe, so a pipe is refused rather than hung
        // on. A config that does not load says why rather than reading as
        // "no model"; the picker's choice is saved to the user's config,
        // which `trellis-tui` confirms; quitting it starts nothing.
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(concat!(
                "`trellis tui` needs a terminal, and this is a pipe or a redirect.\n",
                "Run it directly, or use `trellis run` which works fine without one."
            ));
        }
        config::RepoConfig::load(root)?;
        if first_run::offer_model_choice()?.is_none() {
            return Ok(());
        }
    }
    let user_model = if from_user { spec.as_deref() } else { None };
    Err(launch::hand_off(
        &tui,
        &launch::tui_args(root, &exe, user_model, task.as_deref()),
    ))
}

/// Bare `trellis` at a terminal: starts the desktop on `root` and prints
/// the overview of `here`, returning true when the caller is done. On a
/// fallback, says why on stderr, prints the overview and returns false for
/// the terminal path to run.
pub(crate) fn bare_desktop(route: Route, root: Option<&Path>, here: &Path) -> Result<bool> {
    let exe = std::env::current_exe().context("cannot find the running trellis executable")?;
    let found = launch::find_desktop(&exe);
    let display = launch::display();
    let started = match launch_route(route, found.as_deref().ok(), display) {
        Launch::Unchanged(_) => return Ok(false),
        Launch::Desktop { root: rooted } => {
            // ADR 0050 §2: with no model, the picker runs here, in the
            // terminal, before the window opens. A config that does not load
            // says why rather than reading as "no model", and quitting the
            // picker starts nothing, both as in `terminal_ui`.
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
pub(crate) fn root_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
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
        // `launch_route_tui_flag_*` in tests/cli/bare.rs holds that end.
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
