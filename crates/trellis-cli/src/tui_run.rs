//! `trellis tui <task>` — the run, watched while it happens.
//!
//! The interface itself lives in the `tui-demo` crate, which knows nothing
//! about this one: `AppState`, `render` and the event loop are the same code
//! the standalone demo runs, so the thing being wired here is the *source*
//! of events and nothing else.
//!
//! The run goes through `cmd_run`, not around it. That path owns the
//! snapshot, the reachability gate, the governance rules, the rollback and
//! the trail, and a second copy of that sequence written for a UI would be a
//! second set of rules about when a change is accepted.

use crate::tui_bridge::LoopLayer;
use anyhow::Result;
use std::path::Path;
use tokio::sync::mpsc::Sender;
use tui_demo::tui;
use tui_demo::tui::message::Message;

/// What the interface is told when a run ends, whatever way it ended.
///
/// Separate from the driver so it can be exercised without a terminal. The
/// event loop refuses a pipe on purpose (see below), which makes the failing
/// path unreachable from a shell — so the only honest way to show that a
/// failed run reaches the pane is to call this directly from a test.
///
/// The `Err` arm is the point. A run can fail *before its first turn* — an
/// unreachable runtime, a dirty worktree — and then no tracing event is ever
/// emitted, so the bridge has nothing to forward and the pane shows a task
/// that produced nothing at all. That is the exact failure this interface
/// was built to end, and without this it happened inside it.
fn report_outcome(tx: &Sender<Message>, outcome: Result<crate::RunExit>) {
    match outcome {
        Err(e) => {
            // `{:#}` so the causes travel with the error; a bare `{}` would
            // report "run failed" and drop the reason it failed.
            let rendered = format!("{e:#}");
            let mut said_something = false;
            for line in rendered.lines().filter(|l| !l.trim().is_empty()) {
                let _ = tx.blocking_send(Message::AgentToken(format!("! {line}\n")));
                said_something = true;
            }
            // An error that renders to nothing would otherwise be silence
            // wearing an error's clothes.
            if !said_something {
                let _ = tx.blocking_send(Message::AgentToken(
                    "! the run failed and reported no reason\n".to_string(),
                ));
            }
        }
        Ok(exit) => {
            let _ = tx.blocking_send(Message::AgentToken(format!(
                "· run finished, exit {}\n",
                exit.code()
            )));
        }
    }
    // Whatever happened, the interface takes input again.
    let _ = tx.blocking_send(Message::RunFinished);
}

pub(crate) fn run(root: &Path, task: Option<String>, model_spec: Option<String>) -> Result<()> {
    // Refused rather than attempted when there is no terminal to drive.
    // `EventStream` waits on console input, which never arrives from a pipe,
    // so the interface would paint one frame and hang — and a hang that has
    // to be killed skips every destructor, which is the one way to leave a
    // terminal broken that no guard can cover.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(concat!(
            "`trellis tui` needs a terminal, and this is a pipe or a redirect.\n",
            "Run it directly, or use `trellis run` which works fine without one."
        ));
    }

    let Some(spec) = model_spec else {
        anyhow::bail!(concat!(
            "no model configured, and a run would have nothing to drive it.\n",
            "Set one with `trellis model`, or in .trellis/config.toml, or pass\n",
            "`--model <spec>` to `trellis run`."
        ));
    };

    // What the status bar shows. Derived once, here, rather than guessed by
    // the view.
    let spec_label = spec.clone();
    let tier_label = match spec.split(':').next() {
        Some("scripted") => "scripted".to_string(),
        _ => "2".to_string(),
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        // Bounded. The loop can outrun a redraw during a turn burst, and an
        // unbounded channel turns that into memory growth.
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let (task_tx, mut task_rx) = tokio::sync::mpsc::channel::<String>(8);

        // One driver, serving every task the user types. It owns the
        // subscriber install and the worker thread so the interface does
        // not have to know a run is a thread at all.
        let root = root.to_path_buf();
        let driver = tokio::spawn({
            let tx = tx.clone();
            async move {
                while let Some(task) = task_rx.recv().await {
                    let (root, spec, tx) = (root.clone(), spec.clone(), tx.clone());
                    // `spawn_blocking`, because `cmd_run` blocks on its own
                    // runtime and must not sit on a worker of this one.
                    let _ = tokio::task::spawn_blocking(move || {
                        use tracing_subscriber::layer::SubscriberExt;
                        let layer = LoopLayer::new(tx.clone());
                        let subscriber = tracing_subscriber::registry().with(layer);
                        let outcome = tracing::subscriber::with_default(subscriber, || {
                            crate::cmd_run(&root, &task, &spec, false, "1", false)
                        });
                        report_outcome(&tx, outcome);
                    })
                    .await;
                }
            }
        });

        let mut state = tui::app::AppState {
            model: spec_label.clone(),
            tier: tier_label.clone(),
            ..Default::default()
        };
        // A task on the command line starts immediately; without one the
        // interface opens idle and waits for the user to type.
        if let Some(t) = task {
            state.transcript.push(format!("> {t}"));
            state.running = true;
            let _ = task_tx.send(t).await;
        } else {
            state
                .transcript
                .push("type a task and press enter · esc to quit".into());
        }

        let mut terminal = tui::terminal::init();
        let result = tui::events::run(&mut terminal, &mut state, rx, task_tx).await;
        tui::terminal::restore();

        // Aborted after the terminal is handed back, so a panic inside a run
        // surfaces onto a readable screen.
        driver.abort();
        result.map_err(|e| anyhow::anyhow!("{e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_demo::tui::app::AppState;

    /// Drains a channel through the same `update` the running interface
    /// uses, so what is asserted is what a viewer would have on screen —
    /// not what was put on the wire.
    fn screen(outcome: Result<crate::RunExit>) -> AppState {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        report_outcome(&tx, outcome);
        drop(tx);
        let mut state = AppState {
            running: true,
            ..Default::default()
        };
        while let Ok(m) = rx.try_recv() {
            state.update(m);
        }
        state
    }

    #[test]
    fn a_run_that_fails_before_its_first_turn_still_says_why() {
        // The unreachable-runtime shape, which emits no turn event at all:
        // before this path existed the pane showed the typed task and then
        // nothing, which is the failure this interface exists to end.
        let err = anyhow::anyhow!("cannot reach the runtime at 127.0.0.1:8793")
            .context("the run could not start");
        let state = screen(Err(err));

        let text = state.transcript.join("\n");
        assert!(
            text.contains("cannot reach the runtime at 127.0.0.1:8793"),
            "the reason has to survive to the pane, not just the exit code:\n{text}"
        );
        assert!(
            text.contains("the run could not start"),
            "and so does the context it was raised with:\n{text}"
        );
        assert!(
            state.transcript.iter().any(|l| l.starts_with("! ")),
            "a failure is marked as one:\n{text}"
        );
    }

    #[test]
    fn a_multi_line_failure_arrives_as_lines_rather_than_one_run_on() {
        let state = screen(Err(anyhow::anyhow!(
            "the worktree has uncommitted changes.\ncommit or stash them, then run again."
        )));
        let marked = state
            .transcript
            .iter()
            .filter(|l| l.starts_with("! "))
            .count();
        assert_eq!(marked, 2, "got:\n{:#?}", state.transcript);
    }

    #[test]
    fn a_failure_that_renders_to_nothing_is_still_not_silence() {
        // An error whose Display is empty would otherwise reproduce the bug
        // exactly — a task, then nothing.
        let state = screen(Err(anyhow::anyhow!("")));
        assert!(
            state.transcript.iter().any(|l| l.contains("no reason")),
            "got:\n{:#?}",
            state.transcript
        );
    }

    #[test]
    fn a_run_that_ends_reports_its_exit_code_rather_than_a_verdict() {
        // 1 is "did not converge, tree restored". The interface reports the
        // code; it does not translate it into a claim about correctness,
        // which is not its to make.
        let state = screen(Ok(crate::RunExit::NotConverged { restored: true }));
        let text = state.transcript.join("\n");
        assert!(text.contains("exit 1"), "got:\n{text}");
        assert!(
            !text.to_lowercase().contains("fail"),
            "a non-converged run is not the interface's verdict to give:\n{text}"
        );
    }

    #[test]
    fn every_ending_hands_input_back() {
        // A run that ended without clearing `running` leaves an interface
        // that never takes another task — recoverable only by quitting.
        for outcome in [
            Ok(crate::RunExit::Converged),
            Ok(crate::RunExit::NotConverged { restored: false }),
            Ok(crate::RunExit::Interrupted { restored: true }),
            Err(anyhow::anyhow!("boom")),
        ] {
            let state = screen(outcome);
            assert!(!state.running, "input stayed held after a run ended");
        }
    }
}
