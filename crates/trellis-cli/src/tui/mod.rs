//! The full-screen interface.
//!
//! Three parts, kept apart on purpose:
//!
//! - [`model`] is state. No terminal, no I/O, no drawing — so the behaviour
//!   of the interface is testable without one.
//! - [`view`] draws from `&App`. It cannot mutate, so it cannot disagree with
//!   the controller about what happened.
//! - this module is the controller: it owns the loop, turns events into state
//!   transitions, and starts work on other threads.
//!
//! **The UI thread does no work.** It blocks on one channel receive and
//! nothing else. A task run happens on a worker thread with its own runtime,
//! and reports back the way the rest of TRELLIS reports anything — through
//! `tracing`, which [`events::ChannelLayer`] forwards to the loop. CLAUDE.md
//! forbids a second telemetry path, and a progress channel invented for the
//! TUI would be exactly that.

mod events;
mod model;
mod terminal;
mod view;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use events::{Event, Events, RunReport};
use model::{App, Focus};
use std::path::Path;
use std::time::Duration;

/// How often the screen redraws when nothing else happens.
///
/// Only the spinner needs this. Input arrives on its own thread and wakes the
/// loop immediately, so the tick rate is not keystroke latency — it is how
/// smoothly one character rotates.
const TICK: Duration = Duration::from_millis(120);

pub(crate) fn run(root: &Path, model_spec: Option<String>) -> Result<()> {
    // Refused rather than attempted when there is no terminal to drive.
    // `event::read` waits on console input, which never arrives from a pipe,
    // so the interface would paint one frame and hang — and a hang that has
    // to be killed skips every destructor, which is the one way to leave a
    // terminal broken that no guard can cover.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(concat!(
            "`trellis tui` needs a terminal, and this is a pipe or a redirect.
",
            "Run it directly, or use the line-based session (`trellis`) and the ",
            "one-shot
commands, which work fine without one."
        ));
    }

    let index = trellis_index::build_cached(root);
    let index_line = format!(
        "{} files · {} definitions",
        index.files.len(),
        index.symbols.len()
    );
    // Dropped before the loop starts: the TUI rebuilds it inside the worker
    // after a run, and holding a stale copy across a run that edits the tree
    // is how a sidebar starts describing code that no longer exists.
    drop(index);

    let app = App::new(index_line, model_spec);
    let events = Events::start(TICK);

    // Instrumentation goes to the transcript pane from here on. Installed
    // before the terminal switches modes so that a failure to install is a
    // normal error on a normal terminal.
    install_log_forwarding(events.sender());

    let mut guard = terminal::TerminalGuard::enter()?;
    let outcome = event_loop(&mut guard, app, &events, root);
    // Explicit, so anything printed afterwards lands on a restored terminal
    // rather than inside the alternate screen the user is about to leave.
    guard.restore();
    outcome
}

/// The loop. Blocks on the channel; never on work.
fn event_loop(
    guard: &mut terminal::TerminalGuard,
    mut app: App,
    events: &Events,
    root: &Path,
) -> Result<()> {
    guard.terminal().draw(|f| view::draw(f, &app))?;

    while let Some(event) = events.next() {
        match event {
            Event::Key(k) => on_key(&mut app, k, events, root),
            Event::Tick => app.tick(),
            Event::Mouse(m) => on_mouse(&mut app, m),
            Event::Resize => {}
            Event::Log(line) => app.push_log(line),
            Event::RunFinished(report) => app.finish_run(*report),
        }
        if app.quit {
            break;
        }
        guard.terminal().draw(|f| view::draw(f, &app))?;
    }
    Ok(())
}

/// One place where keys become transitions.
///
/// Keeping this in one function rather than per-pane handlers is deliberate:
/// the duplicated arm — two panes that each implement Ctrl-C slightly
/// differently, and one of them forgets — is the classic way a TUI acquires a
/// key that works everywhere except in the pane you are in.
fn on_key(app: &mut App, key: KeyEvent, events: &Events, root: &Path) {
    // Windows reports press *and* release. Acting on both types every
    // character twice.
    if key.kind != KeyEventKind::Press {
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        app.quit = true;
        return;
    }

    match (app.focus, key.code) {
        (_, KeyCode::Tab) => app.toggle_focus(),
        (_, KeyCode::Esc) => app.quit = true,

        (Focus::Input, KeyCode::Enter) => {
            if let Some(task) = app.start_run() {
                spawn_run(task, root.to_path_buf(), app.model.clone(), events.sender());
            } else if matches!(app.job, model::Job::Running { .. }) {
                app.status = "a run is already in flight".into();
            }
        }
        (Focus::Input, KeyCode::Char(c)) => app.input.push(c),
        (Focus::Input, KeyCode::Backspace) => {
            app.input.pop();
        }

        (Focus::Log, KeyCode::Up) => app.scroll_up(1),
        (Focus::Log, KeyCode::Down) => app.scroll_down(1),
        (Focus::Log, KeyCode::PageUp) => app.scroll_up(20),
        (Focus::Log, KeyCode::PageDown) => app.scroll_down(20),
        (Focus::Log, KeyCode::End) => app.follow_tail(),
        _ => {}
    }
}

/// The wheel scrolls the transcript, wherever the focus is.
///
/// Deliberately not focus-dependent: a wheel is pointed at a pane by the hand
/// holding it, and making it depend on which pane has the keyboard produces
/// the reading where scrolling does nothing and the user concludes it is
/// broken.
fn on_mouse(app: &mut App, m: crossterm::event::MouseEvent) {
    use crossterm::event::MouseEventKind;
    match m.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        _ => {}
    }
}

/// Runs a task on a worker thread.
///
/// The UI thread does not wait on this and holds no handle to it. Progress
/// arrives as ordinary `tracing` events; the terminal report arrives as one
/// [`Event::RunFinished`]. A panic in the worker would otherwise leave the
/// interface spinning forever, so the join is checked and turned into a
/// report either way.
fn spawn_run(
    task: String,
    root: std::path::PathBuf,
    model_spec: Option<String>,
    tx: std::sync::mpsc::Sender<Event>,
) {
    std::thread::Builder::new()
        .name("trellis-tui-run".into())
        .spawn(move || {
            let report = match model_spec {
                None => RunReport {
                    task,
                    converged: false,
                    summary: "no model set; nothing can drive the loop".into(),
                    proposed: Vec::new(),
                },
                Some(spec) => run_task(&root, &task, &spec),
            };
            let _ = tx.send(Event::RunFinished(Box::new(report)));
        })
        .expect("spawning the run thread");
}

/// The run itself, on the worker thread.
///
/// Goes through `cmd_run` rather than reimplementing it: the run path owns
/// the snapshot, the rollback, the governance gate and the trail, and a
/// second copy of that sequence written for the TUI would be a second set of
/// rules about when a change is accepted.
fn run_task(root: &Path, task: &str, spec: &str) -> RunReport {
    let before = trellis_vcs::pending(root).unwrap_or_default();
    let outcome = crate::cmd_run(root, task, spec, false, "1", false);
    let after = trellis_vcs::pending(root).unwrap_or_default();

    let proposed: Vec<String> = after
        .iter()
        .filter(|l| !before.contains(l))
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect();

    match outcome {
        Ok(exit) => RunReport {
            task: task.to_string(),
            converged: exit.converged(),
            summary: if exit.converged() {
                "converged; changes left in the tree for review".into()
            } else {
                format!("did not converge (exit {})", exit.code())
            },
            proposed,
        },
        Err(e) => RunReport {
            task: task.to_string(),
            converged: false,
            summary: e.to_string(),
            proposed,
        },
    }
}

/// Points `tracing` at the transcript pane.
///
/// Best-effort: if a subscriber is already installed — which it is when the
/// TUI is reached through the normal CLI entry point — the existing one keeps
/// its place and the pane simply stays quiet. Failing the whole interface
/// because instrumentation could not be rerouted would be the wrong trade.
fn install_log_forwarding(tx: std::sync::mpsc::Sender<Event>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "trellis=info".into());
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(events::ChannelLayer::new(tx))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn app() -> App {
        App::new("idx".into(), None)
    }

    #[test]
    fn a_key_release_is_not_a_keystroke() {
        // Windows delivers press and release. Acting on both types every
        // character twice, which is invisible in a test that only checks
        // press and obvious the moment anyone runs it.
        let mut a = app();
        let events = Events::start(Duration::from_secs(3600));
        let mut release = key(KeyCode::Char('x'));
        release.kind = KeyEventKind::Release;

        on_key(&mut a, key(KeyCode::Char('x')), &events, Path::new("."));
        on_key(&mut a, release, &events, Path::new("."));
        assert_eq!(a.input, "x");
    }

    #[test]
    fn ctrl_c_quits_from_either_pane() {
        // The duplicated-arm defect: a quit key that works in one pane and
        // not the other traps the user in the one they are in.
        let events = Events::start(Duration::from_secs(3600));
        for focus in [Focus::Input, Focus::Log] {
            let mut a = app();
            a.focus = focus;
            on_key(&mut a, ctrl('c'), &events, Path::new("."));
            assert!(a.quit, "ctrl-c must quit from {focus:?}");
        }
    }

    #[test]
    fn typing_only_reaches_the_input_pane() {
        let events = Events::start(Duration::from_secs(3600));
        let mut a = app();
        a.focus = Focus::Log;
        on_key(&mut a, key(KeyCode::Char('q')), &events, Path::new("."));
        assert!(
            a.input.is_empty(),
            "a character typed at the transcript must not land in the task box"
        );
    }

    #[test]
    fn tab_moves_focus_and_back() {
        let events = Events::start(Duration::from_secs(3600));
        let mut a = app();
        on_key(&mut a, key(KeyCode::Tab), &events, Path::new("."));
        assert_eq!(a.focus, Focus::Log);
        on_key(&mut a, key(KeyCode::Tab), &events, Path::new("."));
        assert_eq!(a.focus, Focus::Input);
    }

    #[test]
    fn enter_with_no_model_still_reports_rather_than_hanging() {
        // The interface must never be left spinning on work that cannot
        // start. With no model the worker reports immediately.
        let events = Events::start(Duration::from_secs(3600));
        let mut a = app();
        a.input = "do something".into();
        on_key(&mut a, key(KeyCode::Enter), &events, Path::new("."));
        assert!(matches!(a.job, model::Job::Running { .. }));

        let report = loop {
            match events.next().expect("the worker reports") {
                Event::RunFinished(r) => break r,
                _ => continue,
            }
        };
        assert!(!report.converged);
        assert!(report.summary.contains("no model"), "{}", report.summary);
    }
}
