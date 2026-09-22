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

/// What the interface is told when a run did not fail but *vanished*.
///
/// The same silent pane as above, arriving through the one door
/// `report_outcome` cannot cover: if `cmd_run` panics, it never returns, so
/// nothing reports and `RunFinished` is never sent — leaving an interface
/// that holds input forever and can only be quit.
///
/// The worktree line is not hedging. `cmd_run` rolls back on its own
/// non-converged path and there is no `catch_unwind` and no `Drop` guard
/// around the snapshot, so a panic partway through genuinely leaves the tree
/// as the agent left it. Saying so, and naming the command that shows it, is
/// the difference between a recoverable surprise and a lost afternoon.
fn lost_run_report(join: &tokio::task::JoinError) -> Vec<Message> {
    let what = if join.is_panic() {
        "the run panicked"
    } else {
        "the run was cancelled before it finished"
    };
    vec![
        Message::AgentToken(format!("! {what}\n")),
        Message::AgentToken(
            "! the worktree was NOT rolled back — check `git status` before rerunning\n"
                .to_string(),
        ),
        Message::RunFinished,
    ]
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
        // Parsed once. `classify` needs the runtime's address and weight,
        // and re-parsing per turn would be a second place that decides what
        // a spec means.
        let spec_for_ask: trellis_model::ModelSpec = spec
            .parse()
            .unwrap_or(trellis_model::ModelSpec::Scripted(std::path::PathBuf::new()));
        let driver = tokio::spawn({
            let tx = tx.clone();
            async move {
                while let Some(task) = task_rx.recv().await {
                    // Not everything typed here is work.
                    //
                    // Every input used to become an agent run — a snapshot,
                    // the loop, the repository's checks, a rollback and a
                    // trail — so typing `hello` failed, and failed
                    // expensively. The model is asked once what the input
                    // is; a question is answered here, and only a request to
                    // change the code reaches `cmd_run` untouched.
                    match crate::ask::classify(&spec_for_ask, &task).await {
                        Ok(crate::ask::Intent::Answer(text)) => {
                            for line in text.lines() {
                                let _ = tx
                                    .send(tui::message::Message::AgentToken(format!(
                                        "{line}
"
                                    )))
                                    .await;
                            }
                            let _ = tx.send(tui::message::Message::RunFinished).await;
                            continue;
                        }
                        Ok(crate::ask::Intent::Task) => {}
                        // A runtime that cannot be reached is reported here
                        // rather than silently falling through to a run that
                        // would fail the same way a moment later with a
                        // longer message.
                        Err(e) => {
                            for line in format!("{e:#}").lines() {
                                let _ = tx
                                    .send(tui::message::Message::AgentToken(format!(
                                        "! {line}
"
                                    )))
                                    .await;
                            }
                            let _ = tx.send(tui::message::Message::RunFinished).await;
                            continue;
                        }
                    }

                    let (root, spec) = (root.clone(), spec.clone());
                    let (run_tx, lost_tx) = (tx.clone(), tx.clone());
                    // `spawn_blocking`, because `cmd_run` blocks on its own
                    // runtime and must not sit on a worker of this one.
                    let joined = tokio::task::spawn_blocking(move || {
                        use tracing_subscriber::layer::SubscriberExt;
                        let layer = LoopLayer::new(run_tx.clone());
                        let subscriber = tracing_subscriber::registry().with(layer);
                        let outcome = tracing::subscriber::with_default(subscriber, || {
                            crate::cmd_run(&root, &task, &spec, false, "1", false)
                        });
                        report_outcome(&run_tx, outcome);
                    })
                    .await;
                    // Discarding this `JoinError` reopened the silent pane:
                    // a panicking run reports nothing and never hands input
                    // back. `send` rather than `blocking_send` — this half
                    // is on the runtime, and `blocking_send` panics there.
                    if let Err(join) = joined {
                        for m in lost_run_report(&join) {
                            let _ = lost_tx.send(m).await;
                        }
                    }
                }
            }
        });

        let mut state = tui::app::AppState::with_model(spec_label.clone(), tier_label.clone());
        // A task on the command line starts immediately; without one the
        // interface opens idle and waits for the user to type.
        if let Some(t) = task {
            state.push_line(format!("> {t}"));
            state.running = true;
            let _ = task_tx.send(t).await;
        } else {
            state.push_line("type a task and press enter · esc to quit".into());
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
        let mut state = AppState::default().with_running(true);
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

    #[tokio::test]
    async fn a_run_that_panics_reports_it_and_hands_input_back() {
        // Driven through a real `JoinError` rather than a hand-made one:
        // the bug was that this value was discarded, so the test has to be
        // holding the same value the driver holds.
        let join = tokio::task::spawn_blocking(|| panic!("checker blew up"))
            .await
            .expect_err("the task panicked, so the join has to be an error");

        let mut state = AppState::default().with_running(true);
        for m in lost_run_report(&join) {
            state.update(m);
        }

        let text = state.transcript.join("\n");
        assert!(text.contains("panicked"), "got:\n{text}");
        assert!(
            text.contains("git status"),
            "a panic leaves the tree as the agent left it, and the viewer has \
             to be told how to see that:\n{text}"
        );
        assert!(
            !state.running,
            "a panicking run left the interface holding input forever"
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
