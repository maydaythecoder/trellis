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
use tokio::sync::mpsc::{Sender, UnboundedReceiver, UnboundedSender};
use tui_demo::tui;
use tui_demo::tui::message::{Message, SteerOut};

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
    // Tier 1 is the protocol; nothing to probe, and "scripted" would be
    // untrue (ADR 0024).
    if matches!(
        spec,
        trellis_model::ModelSpec::Hosted { .. } | trellis_model::ModelSpec::Provider { .. }
    ) {
        return "tier 1, native tool calls".to_string();
    }
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
/// The worktree line is hedged on purpose. A panic between the snapshot and
/// the outcome's dispatch rolls back through `RollbackOnPanic`'s `Drop`, but
/// that reports on stderr, which this interface draws over, and a panic
/// before the snapshot or in the rollback itself does not restore anything.
/// Naming the command that shows the truth is the difference between a
/// recoverable surprise and a lost afternoon.
fn lost_run_report(join: &tokio::task::JoinError) -> Vec<Message> {
    let what = if join.is_panic() {
        "the run panicked"
    } else {
        "the run was cancelled before it finished"
    };
    vec![
        Message::AgentToken(format!("! {what}\n")),
        Message::AgentToken(
            "! a panic mid-run rolls the worktree back to the run's snapshot; whether \
             that succeeded is not known here — check `git status` before rerunning\n"
                .to_string(),
        ),
        Message::RunFinished,
    ]
}

/// Carries each approval question to the modal and its answer back.
///
/// A question whose answer never comes — the interface quit, the channel
/// closed — is dropped, and a dropped reply is a no to the gate. Nothing here
/// can turn silence into a yes.
async fn relay_approvals(
    mut questions: tokio::sync::mpsc::Receiver<trellis_loop::ApprovalRequest>,
    mut answers: tokio::sync::mpsc::Receiver<bool>,
    screen: Sender<Message>,
) {
    while let Some(req) = questions.recv().await {
        // Answers left over from a modal the mock or a stale key raised
        // belong to no question, and must not answer this one.
        while answers.try_recv().is_ok() {}
        let text = if req.detail.is_empty() {
            format!("{}?", req.question)
        } else {
            format!(
                "{}
{}?",
                req.detail.trim_end(),
                req.question
            )
        };
        if screen
            .send(Message::ApprovalRequested { diff: text })
            .await
            .is_err()
        {
            return;
        }
        let Some(yes) = answers.recv().await else {
            return;
        };
        let _ = screen
            .send(Message::AgentToken(format!(
                "· {} {}
",
                req.question,
                if yes { "approved" } else { "declined" }
            )))
            .await;
        let _ = req.reply.send(yes);
    }
}

/// Waits for `run`, passing each line typed meanwhile to it as a steer.
///
/// Concurrent with the run rather than before it: a steer is typed while
/// the run works, and the loop takes whatever is waiting on its channel
/// before each request (#113). `to_run` is dropped on return, which closes
/// the run's channel.
///
/// Stops reading at the next task's boundary, and says so in the `bool`:
/// what follows it was typed for that task. That happens only when the run
/// has already reported finished and the next task was typed before the
/// run was joined.
async fn forward_steers<T>(
    mut run: tokio::task::JoinHandle<T>,
    typed: &mut UnboundedReceiver<SteerOut>,
    to_run: UnboundedSender<trellis_loop::Steering>,
) -> (Result<T, tokio::task::JoinError>, bool) {
    loop {
        tokio::select! {
            joined = &mut run => return (joined, false),
            Some(line) = typed.recv() => match line {
                // A run that has returned has dropped its receiver, and
                // this line is then undelivered, which the view says when
                // `RunFinished` arrives.
                SteerOut::Steer(text) => {
                    let _ = to_run.send(trellis_loop::Steering::Steer(text));
                }
                SteerOut::TaskFollows => {
                    drop(to_run);
                    return (run.await, true);
                }
            },
        }
    }
}

/// Discards steers typed for tasks that have ended, up to the boundary
/// sent ahead of the task now starting.
///
/// Those steers were typed after their run stopped taking them, or while
/// their task turned out to be a question and no run happened; the view has
/// already reported them not delivered. Carried into this run, they would
/// steer work they were not written about. `None` (the interface has gone)
/// ends the wait as the boundary would.
async fn skip_to_task_boundary(typed: &mut UnboundedReceiver<SteerOut>) {
    while let Some(SteerOut::Steer(_)) = typed.recv().await {}
}

/// One run, as the interface drives it: the loop's events and `cmd_run`'s
/// narration go to `tx`, and so does how it ended.
///
/// Blocking: call it from a blocking worker, never from the runtime, since
/// `cmd_run` blocks on its own runtime and the sends here wait for room.
fn run_for_tui(
    root: &Path,
    task: &str,
    spec: &str,
    model_from_user: bool,
    control: crate::RunControl,
    tx: &Sender<Message>,
) {
    use tracing_subscriber::layer::SubscriberExt;
    let layer = LoopLayer::new(tx.clone());
    let deliveries = layer.deliveries();
    let subscriber = tracing_subscriber::registry().with(layer);
    let mut narration = TranscriptSink::new(tx.clone());
    let outcome = tracing::subscriber::with_default(subscriber, || {
        // No resume, and no focus override: `[context] focus` applies.
        crate::cmd_run(
            root,
            task,
            spec,
            false,
            crate::RunSeeding {
                model_from_user,
                ..crate::RunSeeding::default()
            },
            control,
            &mut narration,
        )
    });
    narration.flush_partial();
    // Every delivery again, and this time without dropping any. The layer
    // sends each as it happens, but with `try_send`, and one lost under load
    // would reach `RunFinished` looking undelivered: the view would tell the
    // person the model never saw a steer that it did. The view ignores a
    // repeat, so the ones already shown cost nothing.
    let seen = deliveries.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (nth, turn) in seen {
        let _ = tx.blocking_send(Message::SteerDelivered { nth, turn });
    }
    report_outcome(tx, outcome);
}

/// `model_from_user`: the spec came from `--model`, not `[model] spec`
/// (ADR 0024 §3).
/// The paths `@` completion may offer: what git lists as tracked, plus
/// untracked files that no ignore rule covers.
///
/// Asked of git rather than walked from disk, so a `.gitignore`d file — a
/// `.env`, a build directory, a virtualenv — is never offered to be typed
/// into a task that a model will read. `-z` because without it git quotes a
/// path with unusual bytes, and a quoted path inserted into a task names a
/// file that does not exist.
fn at_file_paths(root: &Path) -> Result<Vec<String>> {
    let out = trellis_vcs::command()
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "listing repository paths exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut paths: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        // A path that is not UTF-8 cannot be typed into a task faithfully,
        // so it is left out rather than offered with its bytes replaced.
        .filter_map(|p| std::str::from_utf8(p).ok().map(str::to_string))
        .collect();
    // `--cached` lists a file once per conflict stage during a merge.
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn run(
    root: &Path,
    task: Option<String>,
    model_spec: Option<String>,
    model_from_user: bool,
) -> Result<()> {
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

    // Once, at startup. A file created during the session is not offered
    // until the next one; re-listing per keystroke would put a process spawn
    // behind every letter typed.
    let at_paths = at_file_paths(root);

    // What a hosted spec is resolved against before the classifier sends the
    // key anywhere (ADR 0024 §3). Loaded with `?`, as `Session::open` loads
    // it: a default in its place would refuse the user's own provider as the
    // repository's choice and name the wrong cause.
    let ask_cfg = crate::config::RepoConfig::load(root)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        // Bounded. The loop can outrun a redraw during a turn burst, and an
        // unbounded channel turns that into memory growth.
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let (task_tx, mut task_rx) = tokio::sync::mpsc::channel::<String>(8);

        // Approve mode (ADR 0018). The run's approval gate asks here; the
        // question becomes `Modal::Approval`, and the key pressed on it comes
        // back on `answer_rx` and is sent to the gate's `oneshot`. One
        // question at a time, because the gate waits for each answer.
        let (approve_tx, approve_rx) = tokio::sync::mpsc::channel(1);
        let (answer_tx, answer_rx) = tokio::sync::mpsc::channel::<bool>(1);
        tokio::spawn(relay_approvals(approve_rx, answer_rx, tx.clone()));

        // Lines typed while a run is in flight (#115). Unbounded, as the
        // loop's own steering channel is: the driver does not read this
        // while it is classifying a task, and a bounded channel that filled
        // then would freeze the interface the person is typing into.
        let (steer_tx, mut steer_rx) = tokio::sync::mpsc::unbounded_channel::<SteerOut>();

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
                // Whether the last run's forwarder already read this task's
                // boundary; it cannot be read twice.
                let mut at_boundary = false;
                while let Some(task) = task_rx.recv().await {
                    if !std::mem::take(&mut at_boundary) {
                        skip_to_task_boundary(&mut steer_rx).await;
                    }
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
                    match crate::ask::classify(
                        &spec_for_ask,
                        &task,
                        &inventory,
                        &ask_cfg,
                        model_from_user,
                    )
                    .await
                    {
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
                    // One steering channel per run, so a run cannot be
                    // handed a steer typed for another.
                    let (to_run, steering) = tokio::sync::mpsc::unbounded_channel();
                    let control = crate::RunControl {
                        // `[control] mode` applies. In auto mode no gate is
                        // installed and nothing is asked.
                        flag: None,
                        human: crate::Human::Channel(approve_tx.clone()),
                        steering: Some(steering),
                    };
                    // `spawn_blocking`, because `cmd_run` blocks on its own
                    // runtime and must not sit on a worker of this one.
                    let run = tokio::task::spawn_blocking(move || {
                        run_for_tui(&root, &task, &spec, model_from_user, control, &run_tx);
                    });
                    let joined;
                    (joined, at_boundary) = forward_steers(run, &mut steer_rx, to_run).await;
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
        match at_paths {
            Ok(paths) => state.paths = paths,
            Err(e) => state.push_line(format!("· @ path completion is off: {e}")),
        }
        // A task on the command line starts immediately; without one the
        // interface opens idle and waits for the user to type.
        if let Some(t) = task {
            state.push_line(format!("> {t}"));
            state.running = true;
            // The boundary the driver waits for before any task, sent here
            // because this task does not come through the event loop.
            let _ = steer_tx.send(SteerOut::TaskFollows);
            let _ = task_tx.send(t).await;
        } else {
            state.push_line("type a task and press enter · esc to quit".into());
        }

        let mut terminal = tui::terminal::init();
        let result = tui::events::run_with_steering(
            &mut terminal,
            &mut state,
            rx,
            task_tx,
            Some(answer_tx),
            Some(steer_tx),
        )
        .await;
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

    #[test]
    fn at_file_paths_lists_tracked_and_untracked_but_never_ignored() {
        // An ignored file is the one most likely to hold something that
        // should not be typed into a task a model reads: `.env`, a build
        // output, a virtualenv.
        let d = crate::testutil::scratch("at-file-paths");
        crate::testutil::git_init(&d);
        std::fs::write(
            d.join(".gitignore"),
            "secret.env
build/
",
        )
        .unwrap();
        std::fs::write(
            d.join("tracked.py"),
            "x = 1
",
        )
        .unwrap();
        crate::testutil::git_commit_all(&d, "base");
        std::fs::write(
            d.join("untracked.py"),
            "y = 2
",
        )
        .unwrap();
        std::fs::write(
            d.join("secret.env"),
            "not for a prompt
",
        )
        .unwrap();
        std::fs::create_dir_all(d.join("build")).unwrap();
        std::fs::write(
            d.join("build/out.py"),
            "z = 3
",
        )
        .unwrap();

        let paths = at_file_paths(&d).expect("a repository lists");
        assert!(paths.iter().any(|p| p == "tracked.py"), "{paths:?}");
        assert!(paths.iter().any(|p| p == "untracked.py"), "{paths:?}");
        assert!(!paths.iter().any(|p| p == "secret.env"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("build/")), "{paths:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

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
            "a panic may leave the tree as the agent left it, and the viewer has \
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

    #[tokio::test]
    async fn an_approval_question_becomes_the_modal_and_its_key_goes_back() {
        // ADR 0018: the modal used to be driven only by the mock. Here the
        // question comes from the gate's channel and the answer returns on
        // the gate's `oneshot`.
        let (q_tx, q_rx) = tokio::sync::mpsc::channel(1);
        let (a_tx, a_rx) = tokio::sync::mpsc::channel(1);
        let (screen_tx, mut screen_rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(relay_approvals(q_rx, a_rx, screen_tx));

        let (reply, answer) = tokio::sync::oneshot::channel();
        q_tx.send(trellis_loop::ApprovalRequest {
            question: "edit a.py (+1 -1)".into(),
            detail: "- x\n+ y\n".into(),
            reply,
        })
        .await
        .unwrap();

        let mut state = AppState::default();
        state.update(screen_rx.recv().await.unwrap());
        match &state.modal {
            Some(tui_demo::tui::app::Modal::Approval { diff }) => {
                assert!(
                    diff.contains("- x") && diff.contains("edit a.py (+1 -1)?"),
                    "{diff}"
                );
            }
            other => panic!("expected the approval modal, got {other:?}"),
        }
        // The key itself becomes `answered` in `AppState::update` and is sent
        // by the event loop; both are tested in `tui-demo`. This is the
        // relay's half: the answer reaches the gate.
        a_tx.send(false).await.unwrap();
        assert!(!answer.await.unwrap(), "n is a no");
        let said = screen_rx.recv().await.unwrap();
        assert!(
            matches!(&said, Message::AgentToken(t) if t.contains("edit a.py (+1 -1) declined")),
            "the decision is also left in the transcript: {said:?}"
        );
    }

    #[tokio::test]
    async fn an_interface_that_goes_away_mid_question_has_not_said_yes() {
        let (q_tx, q_rx) = tokio::sync::mpsc::channel(1);
        let (a_tx, a_rx) = tokio::sync::mpsc::channel::<bool>(1);
        let (screen_tx, _screen_rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(relay_approvals(q_rx, a_rx, screen_tx));
        let (reply, answer) = tokio::sync::oneshot::channel();
        q_tx.send(trellis_loop::ApprovalRequest {
            question: "edit a.py (+1 -1)".into(),
            detail: String::new(),
            reply,
        })
        .await
        .unwrap();
        drop(a_tx);
        assert!(
            answer.await.is_err(),
            "the reply is dropped, which the gate reads as a no"
        );
    }

    /// A repository with one file and a script outside it, in auto mode.
    fn steer_fixture(name: &str, script: &str) -> (std::path::PathBuf, String) {
        let dir = crate::testutil::scratch(&format!("tui-steer-{name}"));
        crate::testutil::git_init(&dir);
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        // Outside the repository: an untracked script would make the
        // worktree dirty, and a run refuses that.
        let scripts = crate::testutil::scratch(&format!("tui-steer-{name}-script"));
        let path = scripts.join("script.json");
        std::fs::write(&path, script).unwrap();
        (dir, format!("scripted:{}", path.display()))
    }

    #[test]
    fn tui_steer_reaches_the_loop_and_the_trail_and_is_reported_before_the_run_ends() {
        // The path a steer takes once the view has queued it: sent on the
        // run's steering channel, delivered by the loop, recorded in the
        // trail (#117), and reported back towards the view by the loop's own
        // event. A steer that reached the channel but was
        // never reported would read as ignored; one reported but not in the
        // trail would be a person's words shaping a run with no record.
        let (dir, spec) = steer_fixture(
            "e2e",
            r#"{"steps":[
                {"tool":"read_file","path":"a.py"},
                {"tool":"done","summary":"looked"}
            ]}"#,
        );

        // Sent before the run starts, so the loop's first drain, before
        // its first request, takes it: deterministic, and turn 1.
        let (to_run, steering) = tokio::sync::mpsc::unbounded_channel();
        to_run
            .send(trellis_loop::Steering::Steer(
                "prefer the smaller diff".into(),
            ))
            .expect("the run's receiver is open");

        // Room for everything a short run says: the sink waits for room,
        // and nothing reads this until the run has returned.
        let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
        run_for_tui(
            &dir,
            "look at f",
            &spec,
            false,
            crate::RunControl {
                flag: Some(crate::config::ControlMode::Auto),
                human: crate::Human::Absent,
                steering: Some(steering),
            },
            &tx,
        );
        drop(tx);
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }

        // Before `RunFinished`, or the view would already have called it
        // not delivered by the time it heard otherwise. What the view then
        // prints is `tui-demo`'s to test; this is what it is given.
        let finished = got
            .iter()
            .position(|m| matches!(m, Message::RunFinished))
            .expect("the run handed input back");
        let delivered = got
            .iter()
            .position(|m| matches!(m, Message::SteerDelivered { nth: 0, turn: 1 }))
            .unwrap_or_else(|| panic!("the delivery was not reported: {got:#?}"));
        assert!(delivered < finished, "{got:#?}");
        assert_eq!(finished, got.len() - 1, "nothing follows RunFinished");
        assert!(
            !got.iter()
                .any(|m| matches!(m, Message::SteerDelivered { nth, .. } if *nth > 0)),
            "one steer sent, one delivery: {got:#?}"
        );

        let runs = dir.join(".trellis").join("runs");
        let file = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == "json"))
            .expect("a trail was written");
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let trail = trellis_trail::UntrustedTrail::from_json(&raw).unwrap();
        // The first writer to fill `steering`: a field the digest missed
        // would make this trail read as altered.
        assert_eq!(trail.integrity(&raw), trellis_trail::Integrity::Intact);
        let steering = &trail.factual.steering;
        assert_eq!(steering.len(), 1, "{steering:?}");
        assert_eq!(steering[0].text, "prefer the smaller diff");
        assert_eq!(steering[0].turn, 1, "the trail and the view agree on N");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tui_steer_typed_while_a_run_works_is_forwarded_to_it() {
        // The driver's half: a line arriving while the run is in flight
        // reaches that run's channel, as a steer and not as a follow-up.
        let (typed_tx, mut typed_rx) = tokio::sync::mpsc::unbounded_channel();
        let (to_run, mut steering) = tokio::sync::mpsc::unbounded_channel();
        let run = tokio::task::spawn_blocking(move || steering.blocking_recv());
        typed_tx.send(SteerOut::Steer("use b.py".into())).unwrap();
        let (got, at_boundary) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            forward_steers(run, &mut typed_rx, to_run),
        )
        .await
        .expect("the forwarder returns once the run does");
        assert_eq!(
            got.expect("the run did not panic"),
            Some(trellis_loop::Steering::Steer("use b.py".into()))
        );
        assert!(!at_boundary);
    }

    #[tokio::test]
    async fn tui_steer_for_the_next_task_is_not_forwarded_to_the_run_ending() {
        // The run has reported finished, and the next task and a steer for
        // it were typed before the run was joined. The forwarder stops at
        // the boundary: the steer is left for the next run and not sent to
        // this one, and the boundary is reported, or the next task would
        // wait for one that is never sent again.
        let (typed_tx, mut typed_rx) = tokio::sync::mpsc::unbounded_channel();
        let (to_run, mut steering) = tokio::sync::mpsc::unbounded_channel();
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let run = tokio::task::spawn_blocking(move || {
            let _ = hold.recv();
        });
        typed_tx.send(SteerOut::TaskFollows).unwrap();
        typed_tx
            .send(SteerOut::Steer("for the next".into()))
            .unwrap();
        release.send(()).unwrap();
        let (joined, at_boundary) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            forward_steers(run, &mut typed_rx, to_run),
        )
        .await
        .expect("the forwarder returns once the run does");
        joined.expect("the run did not panic");
        assert!(at_boundary, "the boundary was read, and has to be said so");
        assert!(steering.try_recv().is_err(), "nothing reached this run");
        assert_eq!(
            typed_rx.try_recv().ok(),
            Some(SteerOut::Steer("for the next".into()))
        );
    }

    #[tokio::test]
    async fn tui_steer_left_from_an_ended_task_is_skipped_up_to_the_boundary() {
        let (typed_tx, mut typed_rx) = tokio::sync::mpsc::unbounded_channel();
        typed_tx.send(SteerOut::Steer("stale".into())).unwrap();
        typed_tx.send(SteerOut::TaskFollows).unwrap();
        typed_tx.send(SteerOut::Steer("fresh".into())).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            skip_to_task_boundary(&mut typed_rx),
        )
        .await
        .expect("the boundary is already there");
        assert_eq!(
            typed_rx.try_recv().ok(),
            Some(SteerOut::Steer("fresh".into())),
            "only what was typed for the ended task is dropped"
        );
    }

    #[test]
    fn tui_steer_that_arrives_too_late_is_still_in_the_trail() {
        // A steer typed as the run ended reached no request, and the view
        // says so; the trail keeps it too, at the turn the run had reached,
        // so no text a person sent into a run is missing from its record.
        let log = trellis_loop::SteeringLog {
            delivered: vec![trellis_loop::Steer {
                text: "early".into(),
                after_step: 2,
            }],
            undelivered: vec![trellis_loop::Steering::Steer("late".into())],
        };
        let got = crate::steering_entries(&log, 4);
        assert_eq!(
            got,
            vec![
                trellis_trail::Steer::new(3, trellis_trail::SteerKind::Steer, "early"),
                trellis_trail::Steer::new(4, trellis_trail::SteerKind::Steer, "late"),
            ]
        );
    }
}
