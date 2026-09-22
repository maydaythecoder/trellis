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
}

impl LoopLayer {
    pub(crate) fn new(tx: Sender<Message>) -> Self {
        Self { tx }
    }

    fn emit(&self, m: Message) {
        let _ = self.tx.try_send(m);
    }
}

impl<S: tracing::Subscriber> Layer<S> for LoopLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut f = TurnFields::default();
        event.record(&mut f);

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
        // Only an applied edit changed the tree. The observation is the only
        // place that says so.
        let wrote = call.starts_with("edit ")
            && f.observation
                .as_deref()
                .is_some_and(|o| o.contains("applied: true"));

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
        if call.starts_with("edit ")
            && wrote
            && let Some(path) = call.split_whitespace().nth(1)
        {
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
                tokens: f.prompt_bytes.unwrap_or(0),
                elapsed_secs: 0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tracing_subscriber::layer::SubscriberExt;

    /// Drives the layer with the event `agent.rs` actually emits, rather than
    /// with a hand-built one: if the loop's field names change, this fails.
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
}
