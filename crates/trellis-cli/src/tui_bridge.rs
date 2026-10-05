//! The real agent loop, as a stream of `Message`s.
//!
//! **Through `tracing`, not through a second channel wired into the loop.**
//! CLAUDE.md designates `tracing` the one telemetry path and says not to
//! build another; `agent.rs` already emits one event per completed turn for
//! exactly this reason, with a comment saying the event is not a hook the
//! interface owns. This is a subscriber, so `RUST_LOG=trellis=info` still
//! sees the same thing and the loop stays unaware a UI exists.
//!
//! What a turn event carries, after being widened for this: the turn number,
//! a call label, the verdict, the check kind, a tail of the observation, the
//! radius counts when the call produced them, and the prompt size.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Sender;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tui_demo::tui::message::Message;

/// Pulls the fields off a `turn` event.
#[derive(Default)]
struct TurnFields {
    turn: Option<u64>,
    call: Option<String>,
    verdict: Option<String>,
    check_kind: Option<String>,
    observation: Option<String>,
    resolved: Option<u64>,
    candidates: Option<u64>,
    unresolved: Option<u64>,
    prompt_bytes: Option<u64>,
    max_iterations: Option<u64>,
    /// On the loop's "steer delivered" event: turns completed before it.
    after_step: Option<u64>,
    message: Option<String>,
}

impl Visit for TurnFields {
    fn record_u64(&mut self, f: &Field, v: u64) {
        match f.name() {
            "turn" => self.turn = Some(v),
            "resolved" => self.resolved = Some(v),
            "candidates" => self.candidates = Some(v),
            "unresolved" => self.unresolved = Some(v),
            "prompt_bytes" => self.prompt_bytes = Some(v),
            "after_step" => self.after_step = Some(v),
            "max_iterations" => self.max_iterations = Some(v),
            _ => {}
        }
    }

    fn record_str(&mut self, f: &Field, v: &str) {
        match f.name() {
            "call" => self.call = Some(v.to_string()),
            "check_kind" => self.check_kind = Some(v.to_string()),
            "observation" => self.observation = Some(v.to_string()),
            "message" => self.message = Some(v.to_string()),
            _ => {}
        }
    }

    /// Where `%` values arrive as well as `?` ones.
    ///
    /// Both sigils reach a subscriber through `record_debug` — `%` wraps the
    /// value in a type whose `Debug` delegates to `Display`. So a field
    /// handled only in `record_str` is silently dropped when the emitter
    /// writes `field = %value`, and `observation` was: the loop emits
    /// `observation = %tail(...)`, nothing here claimed it, and the action
    /// graph reported "nothing written to the tree" for a run whose trail
    /// recorded the write. The test that should have caught it passed a plain
    /// `&str`, which takes the other path.
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        let s = format!("{v:?}");
        match f.name() {
            "verdict" => self.verdict = Some(s),
            "message" => self.message = Some(s.trim_matches('"').to_string()),
            "call" => self.call = Some(s),
            "observation" => self.observation = Some(s),
            "check_kind" => self.check_kind = Some(s),
            _ => {}
        }
    }
}

/// Converts the loop's own events into interface messages.
///
/// A bounded sender: the loop can outrun a redraw during a token stream, and
/// an unbounded channel turns that into memory growth. `try_send` rather than
/// `send` because a `Layer` is synchronous — dropping a log line under
/// pressure is the right trade against blocking the run to draw a frame.
pub(crate) struct LoopLayer {
    tx: Sender<Message>,
    delivered: SteerDeliveries,
    /// When this run began: one layer per run, so elapsed is measured from
    /// here (#243; it was sent as a constant 0).
    started: std::time::Instant,
}

/// Every steer delivery this layer has seen, as `(nth, turn)`, in order.
///
/// Kept beside the live message because `emit` is lossy: the driver sends
/// these again once the run returns, before `RunFinished`, and a delivery
/// dropped under load would otherwise be reported as a steer the model
/// never saw.
pub(crate) type SteerDeliveries = Arc<Mutex<Vec<(usize, usize)>>>;

impl LoopLayer {
    /// One per run: steers are counted from 0 from here, as the view counts
    /// the ones it queued for the run.
    pub(crate) fn new(tx: Sender<Message>) -> Self {
        Self {
            tx,
            delivered: Arc::default(),
            started: std::time::Instant::now(),
        }
    }

    /// The deliveries this layer records, readable after it is installed.
    pub(crate) fn deliveries(&self) -> SteerDeliveries {
        Arc::clone(&self.delivered)
    }

    /// The loop's own "steer delivered" event, as a message naming the
    /// steer by its position.
    ///
    /// The event carries a byte count and not the text, so the steer is
    /// named by order: the loop drains its channel in the order sent and
    /// delivers a prefix of what it received (a steer that arrives after the
    /// last request is never delivered), so the nth delivery is the nth
    /// steer sent. `turn` is `after_step + 1`: `after_step` counts completed
    /// turns, and the steer is in the request that becomes the next one.
    fn steer_delivered(&self, after_step: u64) {
        let turn = usize::try_from(after_step)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        let nth = {
            // A poisoned lock means a panic elsewhere on this run; the list
            // itself is still whole, and losing it would misreport steers.
            let mut d = self.delivered.lock().unwrap_or_else(|e| e.into_inner());
            let nth = d.len();
            d.push((nth, turn));
            nth
        };
        self.emit(Message::SteerDelivered { nth, turn });
    }

    fn emit(&self, m: Message) {
        let _ = self.tx.try_send(m);
    }
}

impl<S: tracing::Subscriber> Layer<S> for LoopLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut f = TurnFields::default();
        event.record(&mut f);

        // The run's ceiling, from the loop's one "run budget" event.
        if let Some(max) = f.max_iterations
            && f.message.as_deref() == Some("run budget")
        {
            self.emit(Message::BudgetKnown {
                max_iterations: u32::try_from(max).unwrap_or(u32::MAX),
            });
            return;
        }

        // Matched on the message as well as the field, so that no other
        // event which happens to carry an `after_step` can count as one.
        if let Some(after_step) = f.after_step
            && f.message.as_deref() == Some("steer delivered")
        {
            self.steer_delivered(after_step);
            return;
        }

        // A turn is an event with *both* a call and a turn number. A `call`
        // alone is not enough: the loop's "no progress; stopping" warning
        // carried one too, as a `Debug` rendering, and the graph grew a second
        // `Edit` node — spelled with a capital E, because that is how the
        // variant's name is spelled — beside the real one.
        let Some(call) = f.call.clone().filter(|_| f.turn.is_some()) else {
            // Not a turn. Anything the loop says at warn or above is still
            // worth showing — "no progress; stopping" is the explanation for
            // a run that is about to end.
            if *event.metadata().level() <= tracing::Level::WARN
                && let Some(msg) = f.message
            {
                self.emit(Message::AgentToken(format!("{msg}\n")));
            }
            return;
        };

        // The transcript line: what was called, and what came back.
        let mut line = format!("· {call}");
        if let Some(v) = &f.verdict {
            if v.contains("true") {
                line.push_str("  ✓ check passed");
            } else if v.contains("false") {
                line.push_str("  ✗ check FAILED");
            }
        }
        self.emit(Message::AgentToken(format!("{line}\n")));
        if let Some(obs) = f.observation.as_ref().filter(|o| !o.is_empty()) {
            for l in obs.lines().take(6) {
                self.emit(Message::AgentToken(format!("    {l}\n")));
            }
        }

        // Every turn is an action, whatever it was and however it went. The
        // graph is drawn from these, so a refused edit has to reach it too:
        // a run that tried four times and wrote once is the run the graph
        // exists to show, and one built only from successes would draw it as
        // a run that acted once.
        // Only an applied edit or a file `write_file` created changed the
        // tree. The observation is the only place that says so: `edit`
        // reports `applied: true`, and `write_file` begins `created ` on
        // success and `error: ` on every refusal.
        let obs = f.observation.as_deref().unwrap_or_default();
        let wrote = (call.starts_with("edit ") && obs.contains("applied: true"))
            || (call.starts_with("write_file ") && obs.starts_with("created "));

        {
            let mut parts = call.splitn(2, ' ');
            let tool = parts.next().unwrap_or_default().to_string();
            let target = parts.next().unwrap_or_default().to_string();
            self.emit(Message::ActionTaken {
                tool,
                target,
                wrote,
            });
        }

        // An edit is a proposal until the checks and the user agree — but a
        // *refused* edit is not a proposal at all, and listing one made the
        // panes contradict each other: three `~ app/calc.py` rows and
        // "3 edit(s) pending" in the status bar, beside an action graph
        // correctly reporting that one write had happened, for a run that
        // changed one line. The pane that counts is the one a user checks
        // before accepting, so it is the one that has to be right.
        if wrote && let Some(path) = call.split_whitespace().nth(1) {
            self.emit(Message::EditProposed {
                path: path.to_string(),
            });
        }

        // A verdict means a check ran. `check_kind` names which.
        if let Some(v) = &f.verdict
            && (v.contains("true") || v.contains("false"))
        {
            let name = f
                .check_kind
                .clone()
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "check".into());
            self.emit(Message::CheckFinished {
                name,
                passed: v.contains("true"),
            });
        }

        if let (Some(r), Some(c), Some(u)) = (f.resolved, f.candidates, f.unresolved) {
            self.emit(Message::RadiusUpdated {
                resolved: r as usize,
                candidates: c as usize,
                unresolved: u as usize,
            });
        }

        if let Some(turn) = f.turn {
            self.emit(Message::BudgetUpdated {
                iteration: turn as u32,
                prompt_bytes: f.prompt_bytes.unwrap_or(0),
                elapsed_secs: self.started.elapsed().as_secs(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tracing_subscriber::layer::SubscriberExt;

    /// Drives the layer through a real subscriber. The events the tests emit
    /// are written by hand, with field names copied from the turn event in
    /// `agent.rs`: renaming a field there does not fail these tests, so the
    /// two have to be kept in step.
    fn capture(emit: impl FnOnce()) -> Vec<Message> {
        let (tx, mut rx) = mpsc::channel(256);
        let subscriber = tracing_subscriber::registry().with(LoopLayer::new(tx));
        tracing::subscriber::with_default(subscriber, emit);
        let mut out = Vec::new();
        while let Ok(m) = rx.try_recv() {
            out.push(m);
        }
        out
    }

    #[test]
    fn tui_bridge_budget_carries_the_runs_ceiling_and_its_elapsed_time() {
        // The risk (#243): the budget line drew the mock's "/4", a constant
        // "0s elapsed" and prompt bytes labelled as tokens, for every real
        // run. The layer is built two seconds into its run so elapsed is
        // measurably above zero without sleeping.
        let (tx, mut rx) = mpsc::channel(256);
        let layer = LoopLayer {
            started: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(2))
                .expect("the clock is more than two seconds past its origin"),
            ..LoopLayer::new(tx)
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            // As `run_seeded` emits it, then a turn as `agent.rs` emits one.
            tracing::info!(max_iterations = 40u32, "run budget");
            tracing::info!(
                turn = 3u64,
                call = "read_file app/core.py",
                prompt_bytes = 9800u64,
                "turn"
            );
        });
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::BudgetKnown { max_iterations: 40 })),
            "{got:?}"
        );
        let Some(Message::BudgetUpdated {
            iteration,
            prompt_bytes,
            elapsed_secs,
        }) = got
            .iter()
            .find(|m| matches!(m, Message::BudgetUpdated { .. }))
        else {
            panic!("no budget update in {got:?}");
        };
        assert_eq!((*iteration, *prompt_bytes), (3, 9800));
        assert!(*elapsed_secs >= 2, "elapsed from the run's start, not 0");
    }

    #[test]
    fn a_turn_with_a_passing_check_becomes_a_line_and_a_check() {
        let got = capture(|| {
            tracing::info!(
                turn = 3u64,
                call = "run_checks [Typecheck] app/core.py",
                verdict = ?Some(true),
                check_kind = "Typecheck",
                observation = "Typecheck: passed",
                "turn"
            );
        });
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::AgentToken(t) if t.contains("check passed"))),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::CheckFinished { name, passed } if name == "Typecheck" && *passed)),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::BudgetUpdated { iteration, .. } if *iteration == 3)),
            "{got:?}"
        );
    }

    #[test]
    fn a_failing_check_says_failed_and_is_not_recorded_as_a_pass() {
        let got = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "run_checks [Typecheck] (no paths)",
                verdict = ?Some(false),
                check_kind = "Typecheck",
                observation = "Typecheck: FAILED\nfound 1 error",
                "turn"
            );
        });
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::CheckFinished { passed, .. } if !*passed)),
            "{got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::AgentToken(t) if t.contains("found 1 error"))),
            "the checker's own words have to reach the pane: {got:?}"
        );
    }

    #[test]
    fn an_edit_turn_proposes_the_path_it_names() {
        let got = capture(|| {
            tracing::info!(
                turn = 2u64,
                call = "edit src/index/resolve.rs",
                verdict = ?None::<bool>,
                observation = %"edited src/index/resolve.rs (applied: true, bytes 4..9)",
                "turn"
            );
        });
        assert!(
            got.iter().any(
                |m| matches!(m, Message::EditProposed { path } if path == "src/index/resolve.rs")
            ),
            "{got:?}"
        );
        assert!(
            !got.iter()
                .any(|m| matches!(m, Message::CheckFinished { .. })),
            "an edit is not a check: {got:?}"
        );
    }

    #[test]
    fn a_file_write_file_created_is_a_write_and_a_pending_change() {
        // The risk: only `edit` counted as a write, so a run that created a
        // file drew it on the graph as a read and left it out of the edits
        // pane a user checks before accepting.
        let created = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "write_file app/new.py",
                verdict = ?None::<bool>,
                observation = %"created app/new.py (12 bytes, 1 lines)",
                "turn"
            );
        });
        assert!(
            created
                .iter()
                .any(|m| matches!(m, Message::EditProposed { path } if path == "app/new.py")),
            "{created:?}"
        );
        assert!(
            created
                .iter()
                .any(|m| matches!(m, Message::ActionTaken { wrote, .. } if *wrote)),
            "{created:?}"
        );
        let refused = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "write_file app/new.py",
                verdict = ?None::<bool>,
                observation = %"error: write_file refused: app/new.py is ignored by git",
                "turn"
            );
        });
        assert!(
            !refused
                .iter()
                .any(|m| matches!(m, Message::EditProposed { .. })),
            "a refused write_file is not pending: {refused:?}"
        );
        assert!(
            refused
                .iter()
                .any(|m| matches!(m, Message::ActionTaken { wrote, .. } if !*wrote)),
            "{refused:?}"
        );
    }

    #[test]
    fn a_refused_edit_is_not_listed_as_a_pending_change() {
        // The panes used to contradict each other: three `~ app/calc.py` rows
        // and "3 edit(s) pending" in the status bar, beside an action graph
        // correctly reporting one write, for a run that changed one line. The
        // edits pane is the one a user checks before accepting, so it is the
        // one that has to be right.
        let got = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "edit app/calc.py",
                verdict = ?None::<bool>,
                observation = %"error: edit anchor not found in app/calc.py",
                "turn"
            );
        });
        assert!(
            !got.iter()
                .any(|m| matches!(m, Message::EditProposed { .. })),
            "nothing is pending after an edit that was refused: {got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::ActionTaken { wrote, .. } if !*wrote)),
            "but it is still an action, and still on the graph: {got:?}"
        );
    }

    #[test]
    fn a_turn_becomes_an_action_whether_or_not_it_wrote() {
        // The run the graph exists to show: several attempts, one write.
        // Drawn from successes alone it would look like a run that acted once.
        let refused = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "edit app/calc.py",
                verdict = ?None::<bool>,
                observation = "error: edit anchor not found in app/calc.py",
                "turn"
            );
        });
        assert!(
            refused.iter().any(|m| matches!(
                m,
                Message::ActionTaken { tool, target, wrote }
                    if tool == "edit" && target == "app/calc.py" && !*wrote
            )),
            "a refused edit is still an action, and wrote nothing: {refused:?}"
        );

        let applied = capture(|| {
            tracing::info!(
                turn = 2u64,
                call = "edit app/calc.py",
                verdict = ?None::<bool>,
                observation = "edited app/calc.py (applied: true, bytes 59..87)",
                "turn"
            );
        });
        assert!(
            applied
                .iter()
                .any(|m| matches!(m, Message::ActionTaken { wrote, .. } if *wrote)),
            "an applied edit did: {applied:?}"
        );
    }

    #[test]
    fn an_observation_written_with_the_display_sigil_still_arrives() {
        // How the loop actually emits it. `%` reaches a subscriber through
        // `record_debug`, not `record_str`, so a field claimed only in the
        // latter is dropped — and this one was, which made the action graph
        // say "nothing written to the tree" about a run that wrote. The test
        // above passes a plain `&str` and takes the other path, which is why
        // it did not catch this.
        let got = capture(|| {
            tracing::info!(
                turn = 3u64,
                call = %"edit app/calc.py",
                verdict = ?None::<bool>,
                observation = %"edited app/calc.py (applied: true, bytes 59..87)",
                "turn"
            );
        });
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::ActionTaken { wrote, .. } if *wrote)),
            "the write has to survive the sigil the loop actually uses: {got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::AgentToken(t) if t.contains("applied: true"))),
            "and reach the transcript: {got:?}"
        );
    }

    #[test]
    fn the_no_progress_warning_is_not_counted_as_another_turn() {
        // It carries a `call` field, as a `Debug` rendering of the repeated
        // call, and the graph grew a second `Edit` node beside the real one —
        // spelled with a capital E, because that is the variant's name.
        let got = capture(|| {
            tracing::warn!(
                repeated_call = ?"Edit { path: \"app/calc.py\" }",
                repeats = 3,
                "no progress; stopping"
            );
        });
        assert!(
            !got.iter().any(|m| matches!(m, Message::ActionTaken { .. })),
            "a diagnosis of several turns is not one more of them: {got:?}"
        );
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::AgentToken(t) if t.contains("no progress"))),
            "but it still has to reach the pane: {got:?}"
        );
    }

    #[test]
    fn a_radius_turn_carries_its_three_counts() {
        let got = capture(|| {
            tracing::info!(
                turn = 1u64,
                call = "blast_radius app.core.greet",
                verdict = ?None::<bool>,
                resolved = 47u64,
                candidates = 3u64,
                unresolved = 2u64,
                "turn"
            );
        });
        assert!(
            got.iter().any(|m| matches!(
                m,
                Message::RadiusUpdated { resolved, candidates, unresolved }
                    if *resolved == 47 && *candidates == 3 && *unresolved == 2
            )),
            "{got:?}"
        );
    }

    #[test]
    fn a_warning_reaches_the_transcript_even_though_it_is_not_a_turn() {
        // "no progress; stopping" is the explanation for a run about to end,
        // and a viewer that dropped it would show a run stopping for no
        // stated reason.
        let got = capture(|| {
            tracing::warn!(repeats = 3u64, "no progress; stopping");
        });
        assert!(
            got.iter()
                .any(|m| matches!(m, Message::AgentToken(t) if t.contains("no progress"))),
            "{got:?}"
        );
    }

    #[test]
    fn an_ordinary_debug_line_is_not_mistaken_for_a_turn() {
        let got = capture(|| {
            tracing::debug!(prompt_bytes = 512u64, turns = 2u64, "prompt built");
        });
        assert!(got.is_empty(), "only turns and warnings surface: {got:?}");
    }

    #[test]
    fn a_steer_delivery_is_numbered_in_order_and_names_the_turn_it_reached() {
        // The event `SteeringLog::drain` emits, copied by hand as it spells
        // it. A renamed field or reworded message there does not fail this
        // test, so the two have to be kept in step, or the view reports
        // every steer not delivered.
        let (tx, mut rx) = mpsc::channel(8);
        let layer = LoopLayer::new(tx);
        let log = layer.deliveries();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let after_step: usize = 0;
            tracing::info!(after_step, bytes = 9usize, "steer delivered");
            let after_step: usize = 2;
            tracing::info!(after_step, bytes = 4usize, "steer delivered");
        });
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }
        assert!(
            matches!(
                got.as_slice(),
                [
                    Message::SteerDelivered { nth: 0, turn: 1 },
                    Message::SteerDelivered { nth: 1, turn: 3 }
                ]
            ),
            "{got:?}"
        );
        assert_eq!(*log.lock().unwrap(), vec![(0, 1), (1, 3)]);
    }
}
