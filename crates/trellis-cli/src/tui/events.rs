//! Everything that can wake the application loop, on one channel.
//!
//! The loop selects over a single `Receiver`, so input, the redraw tick and
//! background work arrive through the same door and are handled in one place.
//! A loop that polled input here and checked a job handle there is how the
//! two drift into different ideas of when to redraw.
//!
//! **Nothing in here does work.** Producers are threads that read or wait;
//! the consumer is the application loop, which never blocks on I/O.

use crossterm::event::{self, Event as CtEvent, KeyEvent, MouseEvent};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// A message to the application loop.
pub(crate) enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    /// Redraw timer. Carries nothing: the model is the only source of truth
    /// about what a frame should contain.
    Tick,
    /// A line of instrumentation from a background task.
    ///
    /// Rides the `tracing` path the rest of TRELLIS already uses. CLAUDE.md:
    /// "tracing for instrumentation, and it doubles as the budget accounting
    /// substrate. Do not build a second telemetry path." A TUI that invented
    /// its own progress channel would be exactly that second path, and the
    /// two would disagree the first time one of them was updated.
    Log(LogLine),
    /// A run finished. Terminal for that job.
    RunFinished(Box<RunReport>),
}

pub(crate) struct LogLine {
    pub(crate) level: tracing::Level,
    pub(crate) text: String,
}

/// What a completed run leaves for the user to read.
pub(crate) struct RunReport {
    pub(crate) task: String,
    pub(crate) converged: bool,
    /// Already rendered: the view allocates nothing.
    pub(crate) summary: String,
    pub(crate) proposed: Vec<String>,
}

/// The receiving end, plus the sender background work clones.
pub(crate) struct Events {
    rx: Receiver<Event>,
    tx: Sender<Event>,
}

impl Events {
    /// Starts the input reader and the tick timer.
    ///
    /// Two threads rather than one polling loop with a timeout: a poll-based
    /// reader ties input latency to the tick interval, so a 250 ms tick makes
    /// every keystroke feel 125 ms late on average. Blocking reads deliver a
    /// key the moment it arrives.
    pub(crate) fn start(tick: Duration) -> Self {
        let (tx, rx) = channel();

        let input_tx = tx.clone();
        std::thread::Builder::new()
            .name("trellis-tui-input".into())
            .spawn(move || {
                loop {
                    // A read error means the terminal is gone. Ending the
                    // thread drops its sender; the loop sees the disconnect
                    // and exits rather than spinning on a dead handle.
                    let Ok(ev) = event::read() else { return };
                    let msg = match ev {
                        CtEvent::Key(k) => Event::Key(k),
                        CtEvent::Mouse(m) => Event::Mouse(m),
                        CtEvent::Resize(_, _) => Event::Resize,
                        _ => continue,
                    };
                    if input_tx.send(msg).is_err() {
                        return;
                    }
                }
            })
            .expect("spawning the input thread");

        let tick_tx = tx.clone();
        std::thread::Builder::new()
            .name("trellis-tui-tick".into())
            .spawn(move || {
                while tick_tx.send(Event::Tick).is_ok() {
                    std::thread::sleep(tick);
                }
            })
            .expect("spawning the tick thread");

        Self { rx, tx }
    }

    /// Blocks until something happens.
    ///
    /// The application loop is allowed to block *here* and nowhere else: this
    /// is the one wait that is not work, and spinning instead would burn a
    /// core to redraw a screen nobody changed.
    pub(crate) fn next(&self) -> Option<Event> {
        self.rx.recv().ok()
    }

    /// A sender for background work to report through.
    pub(crate) fn sender(&self) -> Sender<Event> {
        self.tx.clone()
    }
}

/// A `tracing` subscriber layer that forwards events to the application loop.
///
/// This is how a run reports progress without the UI thread ever touching the
/// work: the agent loop emits its ordinary instrumentation on a worker
/// thread, and this turns each event into an [`Event::Log`].
pub(crate) struct ChannelLayer {
    tx: Sender<Event>,
}

impl ChannelLayer {
    pub(crate) fn new(tx: Sender<Event>) -> Self {
        Self { tx }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ChannelLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut text = String::new();
        event.record(&mut MessageVisitor(&mut text));
        if text.is_empty() {
            return;
        }
        // A closed channel means the UI has gone. Dropping the line is
        // correct: instrumentation must never block or fail the work it
        // describes.
        let _ = self.tx.send(Event::Log(LogLine {
            level: *event.metadata().level(),
            text,
        }));
    }
}

/// Renders an event's fields into one line.
///
/// `message` is the sentence; every other field is appended as `key=value`,
/// which is what makes `budget exhausted; escalating once` arrive with the
/// exhaustion attached rather than as prose with the numbers missing.
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            // The message leads, so a truncated line still says what happened.
            let rendered = format!("{value:?}");
            self.0.insert_str(0, rendered.trim_matches('"'));
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}
