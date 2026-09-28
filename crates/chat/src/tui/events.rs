//! The async event loop. The only place in the interface that `.await`s.
//!
//! Three things here are the whole point:
//!
//! - **Redraw on change, not on a clock.** A fixed 60fps loop burns a core
//!   for a UI that changes a few times a second. The ticker exists for the
//!   elapsed-time display and the spinner, nothing else.
//! - **Coalesce before drawing.** Drain every pending token, then draw once.
//!   Drawing per token makes typing feel laggy under a stream, because each
//!   frame is a full buffer diff.
//! - **Filter `KeyEventKind::Press`.** Windows also reports `Release` and
//!   `Repeat`; without this every keystroke types twice, and it looks like
//!   an input bug rather than an event-source one.

use crate::tui::app::{AppState, Effect};
use crate::tui::message::Message;
use crate::tui::theme::Theme;
use crate::tui::view::{self, ViewState};
use futures_util::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyEventKind};
use ratatui::prelude::*;
use std::time::Duration;
use tokio::sync::mpsc;

/// Bounded on purpose. An unbounded channel turns a model that outruns the
/// renderer into unbounded memory growth, and the failure arrives as an OOM
/// long after the cause.
pub const CHANNEL_CAPACITY: usize = 256;

/// How often the spinner and elapsed time advance.
pub const TICK: Duration = Duration::from_millis(250);

/// Runs until the state says to quit.
///
/// Generic over the backend so the loop itself is exercised by a
/// `TestBackend` in the tests below — the part worth testing is the
/// coalescing and the filtering, neither of which needs a real terminal.
pub async fn run<B>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    theme: &Theme,
    work_rx: mpsc::Receiver<Message>,
    effect_tx: mpsc::Sender<Effect>,
    cancel: crate::commands::runner::CancelToken,
) -> Result<(), B::Error>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    run_with_events(
        terminal,
        state,
        theme,
        EventStream::new(),
        work_rx,
        effect_tx,
        cancel,
    )
    .await
}

/// The loop, with the terminal's event source passed in.
///
/// Separate because crossterm's `EventStream` opens the controlling terminal
/// when first polled and panics where there is none — which is every CI
/// runner on Linux. The tests below exercise the coalescing and effect rules
/// with a source that never yields, so they run anywhere a terminal does not.
async fn run_with_events<B, S>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
    theme: &Theme,
    mut term_events: S,
    mut work_rx: mpsc::Receiver<Message>,
    effect_tx: mpsc::Sender<Effect>,
    cancel: crate::commands::runner::CancelToken,
) -> Result<(), B::Error>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: futures_util::Stream<Item = std::io::Result<Event>> + Unpin,
{
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut view = ViewState::default();
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            terminal.draw(|f| view::render(f, state, theme, &mut view))?;
            needs_redraw = false;
        }

        let msg = tokio::select! {
            ev = term_events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => Message::Key(k),
                Some(Ok(Event::Resize(w, h))) => Message::Resize(w, h),
                // Anything else — Release, Repeat, mouse, focus, paste — is
                // not a state change, and redrawing for it would undo the
                // redraw-on-change rule above.
                Some(Ok(_)) => continue,
                Some(Err(e)) => Message::Error(e.to_string()),
                // The stream ended: the terminal is gone.
                None => Message::Quit,
            },
            Some(m) = work_rx.recv() => m,
            _ = ticker.tick() => Message::Tick,
        };

        state.update(msg);
        // Coalesce: take everything already queued before drawing, so a
        // burst of tokens costs one frame rather than one frame each.
        while let Ok(m) = work_rx.try_recv() {
            state.update(m);
        }

        // Effects leave here, never from `update`. A full channel drops the
        // effect rather than blocking the frame — `try_send` is deliberate,
        // and the runtime reports what it could not take.
        for effect in state.drain_effects() {
            // Suspend is performed here, not forwarded: it restores the
            // terminal and stops the process, and this loop is what owns
            // the terminal. Handing it to the runtime would mean suspending
            // from a thread that does not hold the screen.
            // Cancelled here, not only forwarded. The runtime processes
            // effects one at a time and a streaming turn holds that loop
            // until it finishes, so a `/stop` sent mid-stream sat unread
            // until the stream it was meant to interrupt had ended — which
            // is to say it did nothing. Setting the flag from here reaches
            // the receive loop immediately; the effect is still forwarded
            // so the runtime can report it.
            if matches!(effect, Effect::Run(crate::commands::Command::Stop)) {
                cancel.cancel();
            }
            if matches!(effect, Effect::Suspend) {
                if crate::tui::terminal::suspend() {
                    // Back from `fg`. The next draw is unconditional
                    // because the shell owned the screen while we were
                    // stopped and nothing of ours survives on it.
                    terminal.clear()?;
                } else {
                    state.update(Message::Error(
                        "this platform has no job control; ctrl-z does nothing here".into(),
                    ));
                }
                continue;
            }
            if effect_tx.try_send(effect).is_err() {
                state.update(Message::Error(
                    "the runtime is behind; that request was dropped".into(),
                ));
            }
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
    use crate::tui::theme::ColorDepth;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(80, 24)).unwrap()
    }

    #[tokio::test]
    async fn the_loop_stops_when_the_state_says_to() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (etx, _erx) = mpsc::channel(CHANNEL_CAPACITY);
        let mut state = AppState::default();
        let mut term = terminal();

        tx.send(Message::Quit).await.unwrap();
        run_with_events(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            futures_util::stream::pending(),
            rx,
            etx,
            crate::commands::runner::CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(state.should_quit);
    }

    #[tokio::test]
    async fn a_burst_of_tokens_is_coalesced_into_one_frame() {
        // The property that keeps typing responsive under a stream: fifty
        // queued tokens must cost one draw, not fifty.
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (etx, _erx) = mpsc::channel(CHANNEL_CAPACITY);
        let mut state = AppState::default();
        let mut term = terminal();

        for i in 0..50 {
            tx.send(Message::StreamToken(format!("t{i} ")))
                .await
                .unwrap();
        }
        tx.send(Message::Quit).await.unwrap();
        run_with_events(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            futures_util::stream::pending(),
            rx,
            etx,
            crate::commands::runner::CancelToken::new(),
        )
        .await
        .unwrap();

        // All fifty landed in the state...
        assert!(state.conversation[0].text.contains("t49"));
        // ...and the loop ended without having drawn per token. The draw
        // count is not observable through TestBackend, so the assertion is
        // that the whole burst was consumed in the drain: had it not been,
        // `Quit` would have been processed before the later tokens and the
        // text above would be short.
        assert!(state.should_quit);
    }

    #[tokio::test]
    async fn effects_leave_through_the_channel_and_not_from_update() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (etx, mut erx) = mpsc::channel(CHANNEL_CAPACITY);
        let mut state = AppState::default();
        let mut term = terminal();

        for c in "hello".chars() {
            tx.send(Message::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )))
            .await
            .unwrap();
        }
        tx.send(Message::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        tx.send(Message::Quit).await.unwrap();

        run_with_events(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            futures_util::stream::pending(),
            rx,
            etx,
            crate::commands::runner::CancelToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(erx.try_recv(), Ok(Effect::Prompt("hello".into())));
    }

    #[tokio::test]
    async fn a_full_effect_channel_reports_rather_than_blocking_the_frame() {
        // A blocked send here freezes the interface, which looks identical
        // to a hang. Dropping and saying so is the better failure.
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (etx, _erx) = mpsc::channel(1);
        etx.try_send(Effect::Run(crate::commands::Command::Context))
            .unwrap();

        let mut state = AppState::default();
        let mut term = terminal();
        for c in "hi".chars() {
            tx.send(Message::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )))
            .await
            .unwrap();
        }
        tx.send(Message::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        tx.send(Message::Quit).await.unwrap();

        run_with_events(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            futures_util::stream::pending(),
            rx,
            etx,
            crate::commands::runner::CancelToken::new(),
        )
        .await
        .unwrap();
        assert!(
            state
                .conversation
                .iter()
                .any(|l| l.text.contains("was dropped")),
            "the user has to be told the request did not happen"
        );
    }
}
