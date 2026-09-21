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
    pub transcript: Vec<String>,
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
            Message::Tick | Message::Resize(..) | Message::RunFinished => {}
        }
    }

    /// Appends a token, keeping the transcript bounded.
    ///
    /// Tokens arrive mid-line, so a token containing a newline starts a new
    /// line and anything else extends the last one. Pushing every token as
    /// its own entry would render one word per line.
    fn push_transcript(&mut self, t: String) {
        if self.transcript.is_empty() {
            self.transcript.push(String::new());
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
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            _ => {}
        }
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
    fn q_quits() {
        let mut s = AppState::default();
        assert!(!s.should_quit);
        s.update(press(KeyCode::Char('q')));
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
