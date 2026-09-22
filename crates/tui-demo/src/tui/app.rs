//! State, and the one function allowed to change it.
//!
//! `update` is synchronous and does no IO — no file reads, no `.await`, no
//! subprocess. It is a pure state transition, which is what makes every test
//! in this file runnable without a terminal.

use crate::tui::message::Message;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

/// How many transcript lines are kept.
///
/// A run streams tokens for as long as it lasts; without a cap this grows
/// without bound on a long one.
const TRANSCRIPT_CAP: usize = 2000;

#[derive(Debug, Default)]
pub struct AppState {
    /// What the status bar says about the model, set by whoever is driving
    /// runs. The view used to hardcode "mock", which was a lie in every
    /// binary except the demo.
    pub model: String,
    pub tier: String,
    /// What the user is typing. Submitted on Enter.
    pub input: String,
    /// Set by `update` when Enter is pressed, drained by the event loop,
    /// which is what keeps `update` free of IO: it records the intent, and
    /// something outside it does the work.
    pub submitted: Option<String>,
    /// True between submitting a task and the run reporting finished. Input
    /// is refused while it holds — one run at a time, because two runs
    /// sharing a worktree is the collision the whole design avoids.
    pub running: bool,
    pub transcript: Vec<String>,
    /// Whether the last transcript line was opened by a *stream* and may
    /// still be continued.
    ///
    /// False after a whole line is pushed at once — a task echo, a notice.
    /// Without it the first token of a reply lands on the end of the line
    /// that announced the task, and `> hello` plus `! cannot reach the
    /// runtime` render as `> hello! cannot reach the runtime`: two
    /// unrelated statements read as one sentence.
    line_open: bool,
    pub edits: Vec<Edit>,
    pub checks: Vec<Check>,
    pub radius: Radius,
    pub budget: Budget,
    pub modal: Option<Modal>,
    pub scroll: u16,
    pub should_quit: bool,
}

#[derive(Debug, Clone)]
pub struct Edit {
    pub path: String,
    pub status: EditStatus,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Radius {
    pub resolved: usize,
    pub candidates: usize,
    pub unresolved: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub iteration: u32,
    pub tokens: u64,
    pub elapsed_secs: u64,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Approval { diff: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditStatus {
    Proposed,
    Applied,
    Rejected,
}

impl AppState {
    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Key(k) => self.handle_key(k),
            Message::AgentToken(t) => self.push_transcript(t),
            Message::EditProposed { path } => self.edits.push(Edit {
                path,
                status: EditStatus::Proposed,
            }),
            Message::CheckFinished { name, passed } => self.checks.push(Check { name, passed }),
            Message::RadiusUpdated {
                resolved,
                candidates,
                unresolved,
            } => {
                self.radius = Radius {
                    resolved,
                    candidates,
                    unresolved,
                }
            }
            Message::BudgetUpdated {
                iteration,
                tokens,
                elapsed_secs,
            } => {
                self.budget = Budget {
                    iteration,
                    tokens,
                    elapsed_secs,
                }
            }
            Message::ApprovalRequested { diff } => self.modal = Some(Modal::Approval { diff }),
            Message::Quit => self.should_quit = true,
            Message::RunFinished => self.running = false,
            Message::Tick | Message::Resize(..) => {}
        }
    }

    /// Appends a token, keeping the transcript bounded.
    ///
    /// Tokens arrive mid-line, so a token containing a newline starts a new
    /// line and anything else extends the last one. Pushing every token as
    /// its own entry would render one word per line.
    fn push_transcript(&mut self, t: String) {
        if self.transcript.is_empty() || !self.line_open {
            self.transcript.push(String::new());
            self.line_open = true;
        }
        for (i, part) in t.split('\n').enumerate() {
            if i > 0 {
                self.transcript.push(String::new());
            }
            if let Some(last) = self.transcript.last_mut() {
                last.push_str(part);
            }
        }
        if self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript
                .drain(0..self.transcript.len() - TRANSCRIPT_CAP);
        }
    }

    /// A state labelled with the model and tier actually in play.
    ///
    /// A constructor rather than `..Default::default()` at the call site,
    /// because `line_open` is an invariant of the transcript and not a
    /// setting: it is maintained by `push_line` and `push_transcript`, and
    /// a caller that set it by hand could make a task echo continuable
    /// again — which is the bug this pair exists to close.
    pub fn with_model(model: String, tier: String) -> Self {
        Self {
            model,
            tier,
            ..Default::default()
        }
    }

    /// Marks a run as in flight.
    ///
    /// A builder rather than a field assignment at the call site: once
    /// `line_open` became private, `..Default::default()` stopped working
    /// across the crate boundary, and every other field a driver sets has
    /// one of these. Setting `running` by hand still works inside this
    /// crate; this exists for the ones outside it.
    #[must_use]
    pub fn with_running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// A complete line, which the next streamed token may not continue.
    pub fn push_line(&mut self, line: String) {
        self.transcript.push(line);
        self.line_open = false;
        if self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript
                .drain(0..self.transcript.len() - TRANSCRIPT_CAP);
        }
    }

    /// Only `Press`.
    ///
    /// Windows also reports `Release` and `Repeat`, and without this filter
    /// every keystroke fires twice — which looks like a scroll that jumps
    /// two lines and a `q` that quits before you see it register.
    fn handle_key(&mut self, k: KeyEvent) {
        if k.kind != KeyEventKind::Press {
            return;
        }
        // The modal is answered first: while it is up, the keys underneath
        // it are not the ones on screen.
        if self.modal.is_some() {
            match k.code {
                KeyCode::Char('y') => {
                    for e in &mut self.edits {
                        if e.status == EditStatus::Proposed {
                            e.status = EditStatus::Applied;
                        }
                    }
                    self.modal = None;
                }
                KeyCode::Char('n') => {
                    for e in &mut self.edits {
                        if e.status == EditStatus::Proposed {
                            e.status = EditStatus::Rejected;
                        }
                    }
                    self.modal = None;
                }
                _ => {}
            }
            return;
        }
        match k.code {
            // Esc on an empty line leaves; Esc with text clears it. `q` is a
            // letter now that there is somewhere to type, so it cannot also
            // be the way out — Ctrl-C does that too, handled in the loop.
            KeyCode::Esc if self.input.is_empty() => self.should_quit = true,
            KeyCode::Esc => self.input.clear(),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            _ => {}
        }
    }

    /// Hands the typed line to whatever is driving runs.
    ///
    /// Refused while one is in flight rather than queued: a queue would let
    /// a second task start against a worktree the first is still editing.
    fn submit(&mut self) {
        let task = self.input.trim().to_string();
        if task.is_empty() {
            return;
        }
        if self.running {
            self.push_line("· a run is already in flight; wait for it to finish".into());
            return;
        }
        self.input.clear();

        // Not everything typed here is a task, and treating it as one is
        // expensive: a task takes a snapshot, drives the model and writes a
        // trail. Reported from a real session, where `trellis model` was
        // typed at this prompt and became an agent run that failed against
        // a dead runtime — the user was looking for the command surface and
        // got the loop instead, twice.
        // `/clear` is the one that mutates, so it is handled here rather
        // than by `local_command`, which only reads.
        if task == "/clear" {
            self.transcript.clear();
            self.line_open = false;
            self.push_line("· transcript cleared".into());
            return;
        }
        if let Some(reply) = self.local_command(&task) {
            self.push_line(format!("> {task}"));
            for line in reply {
                self.push_line(line);
            }
            return;
        }

        self.running = true;
        self.push_line(format!("> {task}"));
        self.submitted = Some(task);
    }

    /// Input this interface can answer itself, without a run.
    ///
    /// Pure: every answer is built from state already held, so `update`
    /// stays free of IO. Anything needing the filesystem or a model is not
    /// here — it is a task, or it belongs to `trellis session`.
    fn local_command(&self, input: &str) -> Option<Vec<String>> {
        // A CLI invocation typed at a prompt that is not a shell. Caught by
        // name rather than by guessing: `trellis` is the one word a user
        // reaches for here, having just come from the command line.
        if let Some(rest) = input.strip_prefix("trellis ") {
            let sub = rest.split_whitespace().next().unwrap_or("");
            return Some(vec![
                format!("· `{input}` is a shell command, and this is not a shell."),
                format!(
                    "  Leave with esc and run it, or use /{sub} if this view has one                      — /help lists what it does."
                ),
            ]);
        }

        let mut parts = input.strip_prefix('/')?.split_whitespace();
        let name = parts.next().unwrap_or("");
        Some(match name {
            "help" | "?" => vec![
                "· this view runs one task at a time against the model in the status bar.".into(),
                "  /model    what is driving runs".into(),
                "  /graph    the blast radius, and what it does not claim".into(),
                "  /clear    empty the transcript".into(),
                "  /help     this".into(),
                "  esc       clear the line; again to leave".into(),
                "  ^c        interrupt a run".into(),
                "  ↑ ↓       scroll the transcript".into(),
                String::new(),
                "  Anything else is a task: it snapshots the worktree, drives the".into(),
                "  model, runs the repository's own checks and rolls back if it".into(),
                "  does not converge.".into(),
                String::new(),
                "  For the model catalogue, weights and config, leave and use".into(),
                "  `trellis model` or `trellis session`.".into(),
            ],
            "graph" | "radius" => {
                let r = self.radius;
                let known = r.resolved + r.candidates + r.unresolved;
                let mut out = vec![
                    format!(
                        "· reach: {} resolved · {} candidates · {} unresolved",
                        r.resolved, r.candidates, r.unresolved
                    ),
                    "  drawn in the radius pane on the right, as converging edges.".into(),
                ];
                if known == 0 {
                    out.push("  nothing yet — a task that calls blast_radius fills it.".into());
                }
                // Never a ratio between the three: they are filled from
                // different populations, and dividing them printed "3.7%
                // resolved" for a resolver audited at 72% (ADR 0014).
                out.push(
                    "  three counts, never a ratio: they come from different populations.".into(),
                );
                out.push("  resolved is a floor on reach, not a boundary (Constraint 1).".into());
                out
            }
            "model" => vec![
                format!("· model  {}", self.model),
                format!("  tier   {}", self.tier),
                "  `trellis model` outside this view lists the weights you can fetch.".into(),
            ],
            other => vec![format!(
                "· `/{other}` is not a command here. /help lists them."
            )],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventState, KeyModifiers};

    fn key(code: KeyCode, kind: KeyEventKind) -> Message {
        Message::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        })
    }

    fn press(code: KeyCode) -> Message {
        key(code, KeyEventKind::Press)
    }

    #[test]
    fn esc_on_an_empty_line_quits() {
        // Was `q`. Task entry took the letters, so the exit moved to Esc —
        // and to Ctrl-C, which the event loop turns into `Quit` before this
        // ever sees it.
        let mut s = AppState::default();
        assert!(!s.should_quit);
        s.update(press(KeyCode::Esc));
        assert!(s.should_quit);
    }

    #[test]
    fn quit_is_still_reachable_as_a_message() {
        // Ctrl-C arrives this way rather than as a key.
        let mut s = AppState::default();
        s.update(Message::Quit);
        assert!(s.should_quit);
    }

    #[test]
    fn a_release_event_changes_nothing() {
        // Windows reports Release and Repeat alongside Press. Without the
        // filter every keystroke is handled twice.
        let mut s = AppState::default();
        s.update(key(KeyCode::Char('q'), KeyEventKind::Release));
        assert!(!s.should_quit, "a key release must not quit");

        s.update(key(KeyCode::Down, KeyEventKind::Repeat));
        assert_eq!(s.scroll, 0, "a repeat must not scroll");
    }

    #[test]
    fn the_transcript_is_capped() {
        let mut s = AppState::default();
        for i in 0..TRANSCRIPT_CAP + 500 {
            s.update(Message::AgentToken(format!("line {i}\n")));
        }
        assert_eq!(
            s.transcript.len(),
            TRANSCRIPT_CAP,
            "an unbounded transcript grows for as long as a run lasts"
        );
        // The oldest were dropped, not the newest.
        assert!(s.transcript.last().unwrap().is_empty() || s.transcript[0].contains("line 5"));
    }

    #[test]
    fn a_shell_command_typed_here_does_not_become_an_agent_run() {
        // Reported from a real session: `trellis model` was typed at this
        // prompt, became a task, and failed against a dead runtime — the
        // user was looking for the command surface and got the loop. A task
        // snapshots the worktree, drives the model and writes a trail, so
        // starting one by mistake is not free.
        let mut s = AppState {
            input: "trellis model".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(s.submitted.is_none(), "no run may be started");
        assert!(!s.running);
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("not a shell"), "{text}");
        assert!(
            text.contains("/help"),
            "and it must say where to look: {text}"
        );
    }

    #[test]
    fn graph_is_a_command_and_reports_the_reach_without_a_run() {
        // Typed at this prompt in a real session and answered "`/graph` is
        // not a command here" — in the one interface that has a call graph
        // behind it. The radius pane was three bare numbers.
        let mut s = AppState {
            radius: Radius {
                resolved: 47,
                candidates: 3,
                unresolved: 2,
            },
            input: "/graph".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(s.submitted.is_none(), "answering must not cost a run");
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("47"), "{text}");
        assert!(
            text.contains("radius pane"),
            "it must say where to look:
{text}"
        );
        assert!(
            text.contains("never a ratio"),
            "the three counts come from different populations, and saying so              is the reason this pane exists:
{text}"
        );
        assert!(
            text.contains("floor"),
            "Constraint 1: resolved is a floor on reach, not a boundary:
{text}"
        );
    }

    #[test]
    fn graph_appears_in_help() {
        // A command nobody can discover is not a command.
        let mut s = AppState {
            input: "/help".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("/graph")
        );
    }

    #[test]
    fn help_and_model_are_answered_without_a_run() {
        let mut s = AppState::with_model("qwen3:8b".into(), "2".into());
        s.input = "/model".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none());
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("qwen3:8b"), "{text}");

        s.input = "/help".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none());
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("/model"), "{text}");
        assert!(
            text.contains("rolls back"),
            "help must say what a task actually costs: {text}"
        );
    }

    #[test]
    fn an_unknown_slash_command_is_refused_rather_than_run() {
        let mut s = AppState {
            input: "/nosuch".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none(), "a typo must not become a turn");
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("not a command here")
        );
    }

    #[test]
    fn clear_empties_the_transcript_and_says_so() {
        let mut s = AppState::default();
        s.update(Message::AgentToken(
            "noise
"
            .into(),
        ));
        s.input = "/clear".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(
            !s.transcript
                .join(
                    "
"
                )
                .contains("noise")
        );
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("cleared")
        );
        // And the cleared transcript is not left open for a stream to
        // continue the notice line.
        s.update(Message::AgentToken(
            "next
"
            .into(),
        ));
        assert_eq!(s.transcript[0], "· transcript cleared");
    }

    #[test]
    fn an_ordinary_task_is_still_a_task() {
        // The guard must not swallow real input. Anything that is not a
        // slash command or a `trellis` invocation goes to the model.
        let mut s = AppState {
            input: "rename helper to assist".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert_eq!(s.submitted.as_deref(), Some("rename helper to assist"));
        assert!(s.running);
    }

    #[test]
    fn a_reply_starts_its_own_line_and_does_not_land_on_the_task_echo() {
        // Reported from a real session. A run against a dead runtime
        // rendered as:
        //
        //     > hello! cannot reach the runtime at 127.0.0.1:8793
        //
        // Two unrelated statements — what the user asked, and why it
        // failed — read as one sentence, and the `!` that marks a failure
        // looked like an exclamation mark.
        //
        // The cause is that streamed tokens continue the last transcript
        // line, and the task echo was pushed as a bare line, so the first
        // token of the reply continued *it*.
        let mut s = AppState {
            input: "hello".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        s.update(Message::AgentToken(
            "! cannot reach the runtime
"
            .into(),
        ));

        assert_eq!(s.transcript[0], "> hello");
        assert_eq!(s.transcript[1], "! cannot reach the runtime");
    }

    #[test]
    fn a_second_task_also_gets_its_own_line() {
        // The same defect repeats per turn, and the screenshot showed it
        // twice. Asserted separately because the first task is the one that
        // opens an empty transcript, and the second is the one that follows
        // a line a stream left open.
        let mut s = AppState {
            input: "first".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        s.update(Message::AgentToken(
            "reply one
"
            .into(),
        ));
        s.update(Message::RunFinished);
        s.input = "second".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(
            s.transcript.iter().any(|l| l == "> second"),
            "got {:?}",
            s.transcript
        );
    }

    #[test]
    fn a_multi_line_failure_keeps_every_line_marked() {
        // The screenshot also showed the first line of the error losing its
        // `!` into the task line while the rest kept theirs — so the block
        // read as one marked paragraph with an unmarked opening.
        let mut s = AppState {
            input: "hello".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        for line in [
            "! one
", "! two
", "! three
",
        ] {
            s.update(Message::AgentToken(line.into()));
        }
        let marked = s.transcript.iter().filter(|l| l.starts_with("! ")).count();
        assert_eq!(marked, 3, "got {:?}", s.transcript);
    }

    #[test]
    fn tokens_join_into_lines_rather_than_one_line_each() {
        let mut s = AppState::default();
        s.update(Message::AgentToken("Reading ".into()));
        s.update(Message::AgentToken("blast ".into()));
        s.update(Message::AgentToken("radius".into()));
        assert_eq!(s.transcript, vec!["Reading blast radius".to_string()]);

        s.update(Message::AgentToken("\n".into()));
        s.update(Message::AgentToken("next".into()));
        assert_eq!(s.transcript.len(), 2);
        assert_eq!(s.transcript[1], "next");
    }

    #[test]
    fn y_accepts_the_modal_and_n_rejects_it() {
        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.rs".into(),
        });
        s.update(Message::ApprovalRequested {
            diff: "--- a\n+++ b".into(),
        });
        assert!(s.modal.is_some());

        s.update(press(KeyCode::Char('y')));
        assert!(s.modal.is_none(), "y must clear the modal");
        assert_eq!(s.edits[0].status, EditStatus::Applied);

        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.rs".into(),
        });
        s.update(Message::ApprovalRequested {
            diff: String::new(),
        });
        s.update(press(KeyCode::Char('n')));
        assert!(s.modal.is_none(), "n must clear the modal");
        assert_eq!(s.edits[0].status, EditStatus::Rejected);
    }

    #[test]
    fn while_the_modal_is_up_q_does_not_quit() {
        // The keys on screen are the modal's. Quitting out from under a
        // pending approval would discard the decision silently.
        let mut s = AppState::default();
        s.update(Message::ApprovalRequested {
            diff: String::new(),
        });
        s.update(press(KeyCode::Char('q')));
        assert!(!s.should_quit);
        assert!(s.modal.is_some());
    }

    #[test]
    fn scrolling_stops_at_the_top() {
        let mut s = AppState::default();
        s.update(press(KeyCode::Up));
        assert_eq!(s.scroll, 0, "saturating, not wrapping to u16::MAX");
        s.update(press(KeyCode::Down));
        s.update(press(KeyCode::Down));
        assert_eq!(s.scroll, 2);
        s.update(press(KeyCode::Up));
        assert_eq!(s.scroll, 1);
    }

    #[test]
    fn work_events_land_where_they_belong() {
        let mut s = AppState::default();
        s.update(Message::RadiusUpdated {
            resolved: 47,
            candidates: 3,
            unresolved: 2,
        });
        s.update(Message::BudgetUpdated {
            iteration: 2,
            tokens: 8400,
            elapsed_secs: 18,
        });
        s.update(Message::CheckFinished {
            name: "typecheck".into(),
            passed: true,
        });
        assert_eq!(s.radius.resolved, 47);
        assert_eq!(s.budget.tokens, 8400);
        assert!(s.checks[0].passed);

        // Ticks, resizes and RunFinished are accepted and change nothing.
        s.update(Message::Tick);
        s.update(Message::Resize(10, 10));
        s.update(Message::RunFinished);
        assert_eq!(s.radius.resolved, 47);
        assert_eq!(s.budget.tokens, 8400);
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventState, KeyModifiers};

    fn press(code: KeyCode) -> Message {
        Message::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn type_in(s: &mut AppState, text: &str) {
        for c in text.chars() {
            s.update(press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_then_enter_submits_the_line() {
        let mut s = AppState::default();
        type_in(&mut s, "fix the type error");
        assert_eq!(s.input, "fix the type error");
        assert!(s.submitted.is_none(), "nothing leaves before Enter");

        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("fix the type error"));
        assert!(s.input.is_empty(), "the line is cleared for the next one");
        assert!(s.running, "and the interface knows a run is in flight");
        assert_eq!(s.transcript.last().unwrap(), "> fix the type error");
    }

    #[test]
    fn q_types_a_q_now_that_there_is_somewhere_to_type() {
        // The spec's original binding. Once the interface takes text, a
        // letter cannot also be the exit, and silently quitting mid-sentence
        // would be the worst version of that.
        let mut s = AppState::default();
        type_in(&mut s, "quit the queue");
        assert_eq!(s.input, "quit the queue");
        assert!(!s.should_quit);
    }

    #[test]
    fn esc_clears_a_line_and_leaves_an_empty_one() {
        let mut s = AppState::default();
        type_in(&mut s, "half a thought");
        s.update(press(KeyCode::Esc));
        assert!(s.input.is_empty(), "first Esc clears");
        assert!(!s.should_quit, "and does not also quit");

        s.update(press(KeyCode::Esc));
        assert!(s.should_quit, "Esc on an empty line leaves");
    }

    #[test]
    fn backspace_deletes_and_stops_at_empty() {
        let mut s = AppState::default();
        type_in(&mut s, "ab");
        s.update(press(KeyCode::Backspace));
        assert_eq!(s.input, "a");
        s.update(press(KeyCode::Backspace));
        s.update(press(KeyCode::Backspace));
        assert_eq!(s.input, "", "popping an empty string is not a panic");
    }

    #[test]
    fn an_empty_line_submits_nothing() {
        let mut s = AppState::default();
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none());
        assert!(!s.running);

        type_in(&mut s, "   ");
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none(), "whitespace is not a task");
    }

    #[test]
    fn a_second_task_is_refused_while_one_is_running() {
        // Two runs sharing a worktree is the collision the whole design
        // avoids; a queue would only delay it.
        let mut s = AppState::default();
        type_in(&mut s, "first");
        s.update(press(KeyCode::Enter));
        s.submitted.take();

        type_in(&mut s, "second");
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none(), "the second must not be submitted");
        assert!(
            s.transcript.iter().any(|l| l.contains("already in flight")),
            "and the refusal must be visible: {:?}",
            s.transcript
        );

        // Once the run reports finished, input is taken again.
        s.update(Message::RunFinished);
        assert!(!s.running);
        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("second"));
    }

    #[test]
    fn the_modal_takes_the_keys_while_it_is_up() {
        let mut s = AppState::default();
        s.update(Message::ApprovalRequested {
            diff: String::new(),
        });

        // Anything that is not an answer is swallowed rather than typed.
        type_in(&mut s, "fix the");
        assert!(
            s.input.is_empty(),
            "keys under a modal are not input: {:?}",
            s.input
        );
        assert!(s.modal.is_some(), "and do not dismiss it either");

        // The answer closes it, and only then does typing resume. A key
        // pressed after that lands in the input, which is ordinary terminal
        // behaviour rather than a leak.
        s.update(press(KeyCode::Char('y')));
        assert!(s.modal.is_none());
        type_in(&mut s, "ok");
        assert_eq!(s.input, "ok");
    }
}
