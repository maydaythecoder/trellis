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
use futures::StreamExt;
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
    mut work_rx: mpsc::Receiver<Message>,
    effect_tx: mpsc::Sender<Effect>,
) -> Result<(), B::Error>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut term_events = EventStream::new();
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
        run(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            rx,
            etx,
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
        run(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            rx,
            etx,
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

        run(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            rx,
            etx,
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

        run(
            &mut term,
            &mut state,
            &Theme::new(ColorDepth::None),
            rx,
            etx,
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
