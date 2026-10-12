//! The loop: one merged stream, redraw on change.

use crate::tui::app::AppState;
use crate::tui::message::{Message, SteerOut};
use crate::tui::view;
use futures_util::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;
use tokio::sync::mpsc;

/// Generic over the backend so the loop itself is testable.
///
/// `DefaultTerminal` is `Terminal<CrosstermBackend<Stdout>>`, so the binary
/// is unaffected; a test can pass a `TestBackend` and drive the same code
/// that runs in front of a user. Without this the exit path — the one thing
/// that decides whether the terminal is left usable — could only be checked
/// by hand.
/// `task_tx` carries submitted tasks out to whatever performs runs.
///
/// The loop does not run anything itself: `update` records the intent on
/// `AppState::submitted` and this drains it. That is what keeps the state
/// transition pure and lets the same interface drive a mock in one binary
/// and the real agent loop in another.
pub async fn run<B>(
    terminal: &mut ratatui::Terminal<B>,
    state: &mut AppState,
    work_rx: mpsc::Receiver<Message>,
    task_tx: mpsc::Sender<String>,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    // `?` on `terminal.draw` needs the backend's error to cross into
    // `anyhow`, which the concrete crossterm backend satisfies and a
    // bare `Backend` does not state.
    B::Error: std::error::Error + Send + Sync + 'static,
{
    run_with_answers(terminal, state, work_rx, task_tx, None).await
}

/// As [`run`], and each answer to the approval modal goes out on `answer_tx`
/// to whatever asked (ADR 0018). The mock has nobody to answer, so it uses
/// [`run`]; a real run in approve mode is waiting on this channel.
pub async fn run_with_answers<B>(
    terminal: &mut ratatui::Terminal<B>,
    state: &mut AppState,
    work_rx: mpsc::Receiver<Message>,
    task_tx: mpsc::Sender<String>,
    answer_tx: Option<mpsc::Sender<bool>>,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    run_with_steering(terminal, state, work_rx, task_tx, answer_tx, None).await
}

/// As [`run_with_answers`], and each line typed while a run is in flight
/// goes out on `steer_tx` as a steer for that run (#115).
///
/// Unbounded for the reason the loop's own steering channel is: the sender
/// is a person typing, and the driver does not read this while it is still
/// deciding what a task is. A bounded channel that filled then would block
/// this loop, and the interface the person is typing into would freeze.
/// Each task is preceded on `steer_tx` by [`SteerOut::TaskFollows`], in the
/// order `update` recorded them, so the driver can tell which run a steer
/// was typed for. Without a `steer_tx` a
/// queued steer goes nowhere, and the view says so when the run ends.
pub async fn run_with_steering<B>(
    terminal: &mut ratatui::Terminal<B>,
    state: &mut AppState,
    work_rx: mpsc::Receiver<Message>,
    task_tx: mpsc::Sender<String>,
    answer_tx: Option<mpsc::Sender<bool>>,
    steer_tx: Option<mpsc::UnboundedSender<SteerOut>>,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    run_with_events(
        terminal,
        state,
        EventStream::new(),
        work_rx,
        Outlets {
            task_tx,
            answer_tx,
            steer_tx,
        },
    )
    .await
}

/// Where the loop sends what `update` recorded.
struct Outlets {
    task_tx: mpsc::Sender<String>,
    answer_tx: Option<mpsc::Sender<bool>>,
    steer_tx: Option<mpsc::UnboundedSender<SteerOut>>,
}

/// The loop, with the terminal's event source passed in.
///
/// Separate because crossterm's `EventStream` opens the controlling terminal
/// when first polled and panics ("reader source not set") where there is
/// none — every Linux CI runner. The tests pass a source that never yields.
async fn run_with_events<B, S>(
    terminal: &mut ratatui::Terminal<B>,
    state: &mut AppState,
    mut term_events: S,
    mut work_rx: mpsc::Receiver<Message>,
    out: Outlets,
) -> anyhow::Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: futures_util::Stream<Item = std::io::Result<Event>> + Unpin,
{
    // For the elapsed-time display, not to drive rendering.
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            terminal.draw(|frame| view::render(state, frame))?;
            needs_redraw = false;
        }

        let msg = tokio::select! {
            Some(Ok(ev)) = term_events.next() => match ev {
                // Ctrl-C reaches us as a key rather than a signal in raw
                // mode, and it has to leave the terminal as it found it —
                // the same exit path as `q`, not a process kill. Mid-run,
                // `update` turns it into a stop first (#634).
                Event::Key(k)
                    if k.kind == KeyEventKind::Press
                        && k.code == KeyCode::Char('c')
                        && k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    Message::Quit
                }
                Event::Key(k) if k.kind == KeyEventKind::Press => Message::Key(k),
                Event::Resize(w, h) => Message::Resize(w, h),
                _ => continue,
            },
            Some(m) = work_rx.recv() => m,
            _ = ticker.tick() => Message::Tick,
        };

        state.update(msg);

        // Coalesce: drain everything else pending, then draw once. Drawing
        // per token makes input feel laggy under a stream.
        while let Ok(m) = work_rx.try_recv() {
            state.update(m);
        }

        // Before the task, so that a driver holding the task can already see
        // its boundary on the steer channel. A steer with nowhere to go is
        // dropped here and reported not delivered when the run ends.
        let mut stop_went_nowhere = false;
        for steer in state.steered.drain(..) {
            let stop = steer == SteerOut::Stop;
            let sent = out
                .steer_tx
                .as_ref()
                .is_some_and(|tx| tx.send(steer).is_ok());
            stop_went_nowhere |= stop && !sent;
        }
        // A stop nobody can carry out leaves at once, as Ctrl-C did before
        // #634: the view would otherwise wait for a `RunFinished` that only
        // a stopped run sends. The mock has no driver to stop, and a driver
        // that has gone has no run.
        if stop_went_nowhere {
            state.should_quit = true;
        }

        // Drained after every batch: a task typed during a burst of work
        // messages must not wait for the next keystroke to be noticed.
        if let Some(task) = state.submitted.take()
            && out.task_tx.send(task).await.is_err()
        {
            // Nothing is listening for tasks any more, so nothing can run.
            // Saying so beats an input box that silently does nothing.
            state.update(Message::AgentToken(
                "· nothing is listening for tasks; the run driver is gone
"
                .into(),
            ));
            state.update(Message::RunFinished);
        }

        // Drained the same way, and before quitting is considered: an
        // answer given in the same batch as a quit still reaches the run.
        if let Some(yes) = state.answered.take()
            && let Some(tx) = &out.answer_tx
        {
            let _ = tx.send(yes).await;
        }

        if state.should_quit {
            break;
        }
        needs_redraw = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn headless() -> futures_util::stream::Pending<std::io::Result<Event>> {
        futures_util::stream::pending()
    }

    fn outlets(task_tx: mpsc::Sender<String>) -> Outlets {
        Outlets {
            task_tx,
            answer_tx: None,
            steer_tx: None,
        }
    }

    /// The exit path, driven through the real loop.
    ///
    /// The terminal source here never yields (crossterm's own panics with no
    /// tty, as on Linux CI): the `select!` still takes work messages, and a
    /// `Quit` has to end the loop and return `Ok` — because that return is
    /// what lets `main` call `restore()` and hand the terminal back.
    #[tokio::test]
    async fn quit_ends_the_loop_and_returns_ok() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default();
        let (tx, rx) = mpsc::channel(16);
        let (task_tx, _task_rx) = mpsc::channel(8);

        tx.send(Message::AgentToken("working\n".into()))
            .await
            .unwrap();
        tx.send(Message::Quit).await.unwrap();

        let out = tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(&mut terminal, &mut state, headless(), rx, outlets(task_tx)),
        )
        .await
        .expect("the loop must not hang on Quit");

        assert!(out.is_ok(), "the loop must return Ok so main can restore");
        assert!(state.should_quit);
        assert_eq!(state.transcript, vec!["working".to_string(), String::new()]);
    }

    /// Everything pending is applied before the next draw.
    #[tokio::test]
    async fn pending_messages_are_coalesced_into_one_draw() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default();
        let (tx, rx) = mpsc::channel(64);
        let (task_tx, _task_rx) = mpsc::channel(8);

        for i in 0..20 {
            tx.send(Message::AgentToken(format!("tok{i} ")))
                .await
                .unwrap();
        }
        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(&mut terminal, &mut state, headless(), rx, outlets(task_tx)),
        )
        .await
        .expect("must not hang")
        .unwrap();

        let line = state.transcript.join("");
        assert!(line.contains("tok0"), "the first token survived: {line}");
        assert!(line.contains("tok19"), "and so did the last: {line}");
        // The initial draw only: the whole batch, Quit included, is applied
        // before the next one. A draw per message would count 21 or more.
        assert_eq!(
            terminal.get_frame().count(),
            1,
            "the pending batch was drawn more than once"
        );
    }

    /// The answer to the approval modal leaves the loop for whoever asked.
    ///
    /// A modal whose answer went nowhere would leave a real run waiting on
    /// its gate forever while the screen said the question was settled.
    #[tokio::test]
    async fn a_modal_answer_is_sent_to_the_asker() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default();
        let (tx, rx) = mpsc::channel(16);
        let (task_tx, _task_rx) = mpsc::channel(8);
        let (answer_tx, mut answer_rx) = mpsc::channel(1);

        tx.send(Message::ApprovalRequested {
            diff: "- a\n+ b".into(),
        })
        .await
        .unwrap();
        tx.send(Message::Key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    answer_tx: Some(answer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("must not hang")
        .unwrap();
        assert_eq!(answer_rx.try_recv().ok(), Some(true));
    }

    fn typed(text: &str) -> Vec<Message> {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        text.chars()
            .map(|c| Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            .chain(std::iter::once(Message::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))))
            .collect()
    }

    /// A line typed mid-run leaves on the steer channel, and not as a task.
    ///
    /// Queued on screen but never sent, a steer would read as taken while
    /// the run went on without it; sent as a task, it would start a second
    /// run against the worktree the first is editing.
    #[tokio::test]
    async fn a_steer_typed_mid_run_leaves_on_the_steer_channel_and_not_as_a_task() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default().with_running(true);
        let (tx, rx) = mpsc::channel(64);
        let (task_tx, mut task_rx) = mpsc::channel(8);
        let (steer_tx, mut steer_rx) = mpsc::unbounded_channel();

        for m in typed("prefer the smaller diff") {
            tx.send(m).await.unwrap();
        }
        // The run ends before the quit: a quit mid-run is a stop (#634),
        // which is not what this test is about.
        tx.send(Message::RunFinished).await.unwrap();
        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    steer_tx: Some(steer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("must not hang")
        .unwrap();

        assert_eq!(
            steer_rx.try_recv().ok(),
            Some(SteerOut::Steer("prefer the smaller diff".into()))
        );
        assert!(task_rx.try_recv().is_err(), "a steer is not a task");
        assert!(state.steered.is_empty(), "the loop drained it");
    }

    /// What the person sees: the queued line, then its delivery, drawn.
    #[tokio::test]
    async fn a_steer_is_drawn_queued_then_delivered() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default().with_running(true);
        let (tx, rx) = mpsc::channel(64);
        let (task_tx, _task_rx) = mpsc::channel(8);
        let (steer_tx, _steer_rx) = mpsc::unbounded_channel();

        for m in typed("use b.py") {
            tx.send(m).await.unwrap();
        }
        tx.send(Message::SteerDelivered { nth: 0, turn: 2 })
            .await
            .unwrap();
        tx.send(Message::RunFinished).await.unwrap();
        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    steer_tx: Some(steer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("must not hang")
        .unwrap();
        // The loop leaves on `Quit` without drawing the batch that carried
        // it, so draw the state it left once more, as the next frame would.
        terminal.draw(|frame| view::render(&state, frame)).unwrap();

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("queued: use b.py"), "{screen}");
        assert!(screen.contains("delivered at turn 2: use b.py"), "{screen}");
    }

    /// The run a steer was typed for is recoverable from the wire alone.
    ///
    /// A steer typed during one run and a task typed straight after it can
    /// both be sent before the driver next runs; without the boundary the
    /// driver could only guess which run the steer belonged to, and a guess
    /// that carried it into the next run would steer work it was not about.
    #[tokio::test]
    async fn a_steer_before_a_task_is_ahead_of_that_tasks_boundary() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default().with_running(true);
        let (tx, rx) = mpsc::channel(128);
        let (task_tx, mut task_rx) = mpsc::channel(8);
        let (steer_tx, mut steer_rx) = mpsc::unbounded_channel();

        for m in typed("old steer") {
            tx.send(m).await.unwrap();
        }
        tx.send(Message::RunFinished).await.unwrap();
        for m in typed("next task") {
            tx.send(m).await.unwrap();
        }
        for m in typed("new steer") {
            tx.send(m).await.unwrap();
        }
        tx.send(Message::RunFinished).await.unwrap();
        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    steer_tx: Some(steer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("must not hang")
        .unwrap();

        let mut wire = Vec::new();
        while let Ok(m) = steer_rx.try_recv() {
            wire.push(m);
        }
        assert_eq!(
            wire,
            vec![
                SteerOut::Steer("old steer".into()),
                SteerOut::TaskFollows,
                SteerOut::Steer("new steer".into()),
            ]
        );
        assert_eq!(task_rx.try_recv().ok().as_deref(), Some("next task"));
    }

    fn screen_of(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// The driver hears one stop, and the view stays until the run ends.
    ///
    /// The risk (#634): Ctrl-C ended the view while the run went on editing
    /// the tree with nobody watching, its rollback and trail unseen. A
    /// second press must not ask again, and the loop must still end, once
    /// the stopped run reports finished.
    #[tokio::test]
    async fn ctrl_c_mid_run_emits_one_stop_before_the_loop_ends() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default().with_running(true);
        let (tx, rx) = mpsc::channel(16);
        let (task_tx, _task_rx) = mpsc::channel(8);
        let (steer_tx, mut steer_rx) = mpsc::unbounded_channel();

        tx.send(Message::Quit).await.unwrap();
        tx.send(Message::Quit).await.unwrap();
        tx.send(Message::RunFinished).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    steer_tx: Some(steer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("the loop ends once the stopped run has finished")
        .unwrap();

        let mut wire = Vec::new();
        while let Ok(m) = steer_rx.try_recv() {
            wire.push(m);
        }
        assert_eq!(wire, vec![SteerOut::Stop], "exactly one stop");
        assert!(state.should_quit);
    }

    /// With nobody to stop the run, Ctrl-C leaves at once, as it always did.
    ///
    /// The mock binary has no driver, and its runs never report finished;
    /// waiting for one would leave a view that can only be killed, and a
    /// killed view skips the restore that hands the terminal back.
    #[tokio::test]
    async fn ctrl_c_mid_run_with_nothing_to_stop_it_leaves_at_once() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = AppState::default().with_running(true);
        let (tx, rx) = mpsc::channel(16);
        let (task_tx, _task_rx) = mpsc::channel(8);

        tx.send(Message::Quit).await.unwrap();

        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(&mut terminal, &mut state, headless(), rx, outlets(task_tx)),
        )
        .await
        .expect("a stop with nowhere to go must not hang the view")
        .unwrap();
        assert!(state.should_quit);
    }

    /// What the person sees: the stop said while the run winds down, and
    /// /help describing the same thing.
    #[tokio::test]
    async fn ctrl_c_mid_run_draws_the_stopping_line_and_help_says_so() {
        // Wide and tall enough that /help's lines are drawn whole.
        let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
        let mut state = AppState::default();
        for m in typed("/help") {
            state.update(m);
        }
        terminal.draw(|frame| view::render(&state, frame)).unwrap();
        let help = screen_of(&terminal);
        assert!(
            help.contains("^c        leave; a run in flight is stopped and rolled back first"),
            "{help}"
        );

        let (tx, rx) = mpsc::channel(16);
        let (task_tx, _task_rx) = mpsc::channel(8);
        let (steer_tx, _steer_rx) = mpsc::unbounded_channel();
        state.running = true;
        tx.send(Message::Quit).await.unwrap();
        // The view is still up while the run winds down: a frame is drawn
        // after the stop, and the loop leaves only on `RunFinished`.
        let stopped = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tx.send(Message::RunFinished).await.unwrap();
        });
        tokio::time::timeout(
            Duration::from_secs(5),
            run_with_events(
                &mut terminal,
                &mut state,
                headless(),
                rx,
                Outlets {
                    steer_tx: Some(steer_tx),
                    ..outlets(task_tx)
                },
            ),
        )
        .await
        .expect("the loop ends once the run has finished")
        .unwrap();
        stopped.await.unwrap();
        let screen = screen_of(&terminal);
        assert!(
            screen.contains("stopping: trellis rolls the run back"),
            "{screen}"
        );
    }
}
