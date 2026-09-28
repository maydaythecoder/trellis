//! The loop: one merged stream, redraw on change.

use crate::tui::app::AppState;
use crate::tui::message::Message;
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
    run_with_events(
        terminal,
        state,
        EventStream::new(),
        work_rx,
        task_tx,
        answer_tx,
    )
    .await
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
    task_tx: mpsc::Sender<String>,
    answer_tx: Option<mpsc::Sender<bool>>,
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
                // the same exit path as `q`, not a process kill.
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

        // Drained after every batch: a task typed during a burst of work
        // messages must not wait for the next keystroke to be noticed.
        if let Some(task) = state.submitted.take()
            && task_tx.send(task).await.is_err()
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
            && let Some(tx) = &answer_tx
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
            run_with_events(&mut terminal, &mut state, headless(), rx, task_tx, None),
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
            run_with_events(&mut terminal, &mut state, headless(), rx, task_tx, None),
        )
        .await
        .expect("must not hang")
        .unwrap();

        let line = state.transcript.join("");
        assert!(line.contains("tok0"), "the first token survived: {line}");
        assert!(line.contains("tok19"), "and so did the last: {line}");
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
                task_tx,
                Some(answer_tx),
            ),
        )
        .await
        .expect("must not hang")
        .unwrap();
        assert_eq!(answer_rx.try_recv().ok(), Some(true));
    }
}
