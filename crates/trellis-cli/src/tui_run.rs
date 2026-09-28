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

/// `cmd_run`'s narration, routed into the transcript instead of the terminal.
///
/// `cmd_run` reports a run the way a command-line tool does, by writing lines.
/// Under a full-screen interface those lines land on a terminal ratatui
/// believes it owns, and the frame is destroyed: the rollback notice and the
/// trail path were drawn straight through the panes, mid-border. Nothing was
/// wrong with the run — only with where it spoke.
///
/// So the run writes here, and each completed line becomes a transcript
/// message. Line-buffered because that is the unit the pane displays; a
/// partial line is held until its newline arrives, and `flush_partial` emits
/// whatever is left when the run ends without one.
struct TranscriptSink {
    tx: Sender<Message>,
    partial: Vec<u8>,
}

impl TranscriptSink {
    fn new(tx: Sender<Message>) -> Self {
        Self {
            tx,
            partial: Vec::new(),
        }
    }

    fn emit(&self, line: &str) {
        // Blank lines are `cmd_run` spacing a terminal report. The pane does
        // its own spacing, and forwarding them leaves gaps that read as the
        // run having stalled.
        if line.trim().is_empty() {
            return;
        }
        let _ = self
            .tx
            .blocking_send(Message::AgentToken(format!("{line}\n")));
    }

    /// Emits a trailing line that never got its newline.
    fn flush_partial(&mut self) {
        if !self.partial.is_empty() {
            let rest = String::from_utf8_lossy(&self.partial).into_owned();
            self.emit(&rest);
            self.partial.clear();
        }
    }
}

impl std::io::Write for TranscriptSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.partial.extend_from_slice(buf);
        // `split` on the accumulated bytes rather than on `buf`: a write can
        // end mid-line, and treating each write as a line would break one
        // message into several.
        while let Some(at) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            self.emit(line.trim_end_matches('\r'));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The tier label for the status bar, from what the runtime actually does.
///
/// Three answers, and the third is why this returns a label rather than a
/// tier: a probe that could not run has not established that the schema is
/// ignored, only that nothing is known. Saying `?` is the difference between
/// an unverified claim and a false one — the same distinction `trellis run`
/// draws in its banner, kept in step here deliberately.
async fn tier_from_probe(spec: &trellis_model::ModelSpec) -> String {
    let trellis_model::ModelSpec::Runtime {
        dialect,
        base,
        model,
    } = spec
    else {
        return "scripted".to_string();
    };
    let Ok(client) = trellis_model::RuntimeClient::new(base, *dialect, model.clone()) else {
        return "?".to_string();
    };
    match client.probe_constraint().await {
        Ok(true) => "2".to_string(),
        Ok(false) => "4, no schema".to_string(),
        Err(_) => "?".to_string(),
    }
}

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

    // What the status bar shows before anything has been asked.
    //
    // This used to read `_ => "2"`, under a comment claiming the tier was
    // derived here rather than guessed by the view. It was a guess that had
    // moved: every runtime spec produced "2" regardless of the runtime, so
    // the status bar said `tier 2` while the run beneath it said `tier 4, no
    // schema` — two halves of one screen disagreeing about the same fact.
    //
    // Whether a runtime honours a schema cannot be read off its address. It
    // has to be asked, and the answer arrives from the probe below.
    let spec_label = spec.clone();
    let tier_label = match spec.split(':').next() {
        Some("scripted") => "scripted".to_string(),
        _ => "probing…".to_string(),
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
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
        // Ask the runtime what it actually does with a schema, and label the
        // status bar with the answer.
        //
        // Its own task, so the interface opens immediately and a task typed
        // in the first second is not queued behind a model call. The label
        // settles a moment later; until then it says it is still asking,
        // which is true and is not the same as "2".
        tokio::spawn({
            let tx = tx.clone();
            let spec = spec_for_ask.clone();
            async move {
                let label = tier_from_probe(&spec).await;
                let _ = tx.send(tui::message::Message::TierKnown(label)).await;
            }
        });

        // The inventory the answer path is grounded on.
        //
        // Read once at startup rather than per input: it is a directory walk,
        // and a question typed while a run is writing files should not pay for
        // one. The cost is that a file created during the session is not in the
        // list until the next one - which is the right trade for a list whose
        // job is to stop the model inventing names.
        let inventory = trellis_index::discover_relative(&root);

        let driver = tokio::spawn({
            let tx = tx.clone();
            async move {
                while let Some(task) = task_rx.recv().await {
                    // `/codebase` is a filesystem read, so the interface
                    // declined it and it arrived here. Answered before the
                    // classifier, because it is not a question for a model
                    // and sending it to one would spend a call discovering
                    // that.
                    if task.trim_start().starts_with("/codebase") {
                        let root = root.clone();
                        let limit = task
                            .split_whitespace()
                            .nth(1)
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(20);
                        // On a blocking worker: indexing a large repository
                        // is seconds of parsing, and doing it on this thread
                        // would freeze the interface that is meant to be
                        // showing it.
                        let lines = tokio::task::spawn_blocking(move || {
                            crate::codebase_lines(&root, limit)
                        })
                        .await
                        .unwrap_or_else(|_| {
                            vec!["! reading the index panicked; nothing was changed".into()]
                        });
                        for line in lines {
                            let _ = tx
                                .send(tui::message::Message::AgentToken(format!("{line}\n")))
                                .await;
                        }
                        let _ = tx.send(tui::message::Message::RunFinished).await;
                        continue;
                    }

                    // Not everything typed here is work.
                    //
                    // Every input used to become an agent run — a snapshot,
                    // the loop, the repository's checks, a rollback and a
                    // trail — so typing `hello` failed, and failed
                    // expensively. The model is asked once what the input
                    // is; a question is answered here, and only a request to
                    // change the code reaches `cmd_run` untouched.
                    match crate::ask::classify(&spec_for_ask, &task, &inventory).await {
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
                        let mut narration = TranscriptSink::new(run_tx.clone());
                        let outcome = tracing::subscriber::with_default(subscriber, || {
                            // No resume, and no focus override: `[context]
                            // focus` applies.
                            crate::cmd_run(
                                &root,
                                &task,
                                &spec,
                                false,
                                "1",
                                crate::RunSeeding::default(),
                                &mut narration,
                            )
                        });
                        narration.flush_partial();
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
        result
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

    /// Everything a sink put on the wire, as the pane would show it.
    fn narrated(write: impl FnOnce(&mut TranscriptSink)) -> Vec<String> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let mut sink = TranscriptSink::new(tx);
        write(&mut sink);
        sink.flush_partial();
        drop(sink);
        let mut out = Vec::new();
        while let Ok(Message::AgentToken(t)) = rx.try_recv() {
            out.push(t);
        }
        out
    }

    #[tokio::test]
    async fn a_runtime_that_cannot_be_reached_does_not_claim_a_tier() {
        // The defect: the status bar said `tier 2` for every runtime spec,
        // derived from nothing, while the run beneath it reported `tier 4, no
        // schema`. A probe that could not run has established that nothing is
        // known — not that the schema is honoured.
        let spec: trellis_model::ModelSpec = "vllm:http://127.0.0.1:1"
            .parse()
            .expect("a loopback spec parses");
        assert_eq!(tier_from_probe(&spec).await, "?");
    }

    #[tokio::test]
    async fn a_scripted_model_is_labelled_as_one_without_asking_anything() {
        let spec: trellis_model::ModelSpec = "scripted:/tmp/x.json".parse().unwrap();
        assert_eq!(tier_from_probe(&spec).await, "scripted");
    }

    #[test]
    fn the_status_bar_takes_the_tier_the_probe_reports() {
        let mut s = AppState::with_model("m".into(), "probing…".into());
        s.update(Message::TierKnown("4, no schema".into()));
        assert_eq!(s.tier, "4, no schema");
    }

    #[test]
    fn a_line_split_across_writes_arrives_as_one_message() {
        use std::io::Write;
        // `writeln!` on a formatted line issues several writes. Treating each
        // as a line would chop one sentence into pieces down the pane — the
        // merged-and-split-line failure this interface has had once already.
        let got = narrated(|s| {
            let _ = s.write_all(b"CONVERGED after ");
            let _ = s.write_all(b"4 iterations");
            let _ = s.write_all(b", 2 checks passed.\n");
        });
        assert_eq!(
            got,
            vec!["CONVERGED after 4 iterations, 2 checks passed.\n"]
        );
    }

    #[test]
    fn several_lines_in_one_write_arrive_separately() {
        use std::io::Write;
        let got = narrated(|s| {
            let _ = s.write_all(b"one\ntwo\r\nthree\n");
        });
        // The `\r` goes too: it is a terminal artefact, and left in place it
        // makes the pane redraw the line from column zero.
        assert_eq!(got, vec!["one\n", "two\n", "three\n"]);
    }

    #[test]
    fn a_final_line_without_a_newline_is_not_swallowed() {
        use std::io::Write;
        let got = narrated(|s| {
            let _ = s.write_all(b"trail written");
        });
        assert_eq!(got, vec!["trail written\n"], "the last line still counts");
    }

    #[test]
    fn blank_spacing_lines_do_not_reach_the_pane() {
        use std::io::Write;
        // `cmd_run` spaces a terminal report with blank lines. The pane does
        // its own spacing, and forwarding them reads as a stalled run.
        let got = narrated(|s| {
            let _ = s.write_all(b"\n\nsnapshot at abc123\n\n");
        });
        assert_eq!(got, vec!["snapshot at abc123\n"]);
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
