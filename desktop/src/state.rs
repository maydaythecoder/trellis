//! The desktop's run panes, as a pure reducer over child frames.
//!
//! The TUI's `AppState::update` is synchronous and does no I/O, which is why
//! its tests run without a terminal; `DesktopState::update` is the same for
//! the desktop: a [`Frame`] in, the panes it changed out, and nothing read
//! or written anywhere else. It follows `crates/trellis-cli/src/tui_bridge.rs`
//! for what a turn means, so the desktop and the TUI draw one run the same
//! way.
//!
//! What it shows is what trellis reported (ADR 0048 §8): radius is three
//! counts and never a ratio (CLAUDE.md rule 3); the budget is iterations and
//! prompt bytes, as the TUI's has been since #243, because no rpc event
//! carries a token count; and a figure trellis did not report is `null` in
//! the view, never 0. Every string in the view is trellis- or model-supplied
//! text, which the page writes through `textContent` only (ADR 0007, S10).

use crate::frames::{ApprovalRequest, Frame, RunEnd, Turn};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;

/// The transcript keeps this many lines, as the TUI's does: a run streams
/// for as long as it lasts, and an uncapped log grows with it.
pub const TRANSCRIPT_CAP: usize = 2000;

/// One pane of the view, named as the view's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Transcript,
    Actions,
    Edits,
    Checks,
    Radius,
    Budget,
    Approvals,
    Running,
    Outcome,
}

const PANES: [Pane; 9] = [
    Pane::Transcript,
    Pane::Actions,
    Pane::Edits,
    Pane::Checks,
    Pane::Radius,
    Pane::Budget,
    Pane::Approvals,
    Pane::Running,
    Pane::Outcome,
];

/// The panes a run starts afresh: everything but the transcript, which,
/// as the TUI's, carries on across runs within its cap.
const RUN_PANES: [Pane; 8] = [
    Pane::Actions,
    Pane::Edits,
    Pane::Checks,
    Pane::Radius,
    Pane::Budget,
    Pane::Approvals,
    Pane::Running,
    Pane::Outcome,
];

impl Pane {
    fn key(self) -> &'static str {
        match self {
            Pane::Transcript => "transcript",
            Pane::Actions => "actions",
            Pane::Edits => "edits",
            Pane::Checks => "checks",
            Pane::Radius => "radius",
            Pane::Budget => "budget",
            Pane::Approvals => "approvals",
            Pane::Running => "running",
            Pane::Outcome => "outcome",
        }
    }

    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// Which panes an update changed: what the core sends the page as one
/// `view` event, so a frame that changes nothing sends nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed(u16);

impl Changed {
    fn with(self, p: Pane) -> Self {
        Self(self.0 | p.bit())
    }

    fn of(panes: &[Pane]) -> Self {
        panes.iter().fold(Self::default(), |c, &p| c.with(p))
    }

    pub fn has(self, p: Pane) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One turn, whatever it was and however it went (tui_bridge.rs).
#[derive(Debug, Clone, PartialEq)]
struct Action {
    tool: String,
    target: String,
    /// Whether the turn changed the tree: an applied `edit`, or a
    /// `write_file` that created its file.
    wrote: bool,
}

/// A check that ran: its kind and whether it passed.
#[derive(Debug, Clone, PartialEq)]
struct Check {
    kind: String,
    passed: bool,
}

/// The three counts a `blast_radius` turn reports. No ratio is kept or
/// derived (CLAUDE.md rule 3).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Radius {
    resolved: u64,
    candidates: u64,
    unresolved: u64,
}

/// Iterations and prompt bytes. Each is `None` until trellis reports it.
/// Named `turn` and `max_turns` in the view, not `iteration`: that word
/// contains `ratio`, which no view key may (CLAUDE.md rule 3's test below).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Budget {
    turn: Option<u64>,
    max_turns: Option<u64>,
    /// The latest turn's prompt size; `None` for a turn that reported none
    /// (a scripted run), not 0.
    prompt_bytes: Option<u64>,
}

/// The desktop's run panes.
#[derive(Debug, Default)]
pub struct DesktopState {
    transcript: VecDeque<String>,
    actions: Vec<Action>,
    edits: Vec<String>,
    checks: Vec<Check>,
    radius: Option<Radius>,
    budget: Budget,
    approvals: Vec<ApprovalRequest>,
    /// Whether a run is in flight: from `run_start` to `run_end`.
    running: bool,
    /// The last run's `run_end`, as trellis sent it.
    outcome: Option<RunEnd>,
}

impl DesktopState {
    /// Applies one child frame and returns the panes it changed.
    pub fn update(&mut self, frame: Frame) -> Changed {
        match frame {
            Frame::Turn(t) => self.turn(t),
            Frame::RunBudget { max_iterations } => {
                self.budget.max_turns = max_iterations;
                Changed::of(&[Pane::Budget])
            }
            Frame::SteerDelivered { after_step } => {
                self.push_line(match after_step {
                    Some(n) => format!("· steer delivered after step {n}"),
                    None => "· steer delivered".to_string(),
                });
                Changed::of(&[Pane::Transcript])
            }
            Frame::RunStart(s) => {
                self.reset_run();
                self.running = true;
                let mut c = Changed::of(&RUN_PANES);
                if let Some(task) = s.task {
                    self.push_line(format!("▸ {task}"));
                    c = c.with(Pane::Transcript);
                }
                c
            }
            Frame::RunEnd(e) => {
                self.running = false;
                // Nobody is left to answer a question the run asked.
                self.approvals.clear();
                self.outcome = Some(e);
                Changed::of(&[Pane::Running, Pane::Approvals, Pane::Outcome])
            }
            Frame::Response(r) => {
                // A refused command is said in the transcript; a successful
                // answer changes no run pane.
                if r.success == Some(false) {
                    let command = r.command.as_deref().unwrap_or("command");
                    self.push_line(match r.error {
                        Some(e) => format!("· {command} refused: {e}"),
                        None => format!("· {command} refused"),
                    });
                    Changed::of(&[Pane::Transcript])
                } else {
                    Changed::default()
                }
            }
            Frame::ApprovalRequest(a) => {
                if self.approvals.iter().any(|o| o.id == a.id) {
                    return Changed::default();
                }
                self.approvals.push(a);
                Changed::of(&[Pane::Approvals])
            }
            Frame::Ignored => Changed::default(),
            Frame::Unparsed(line) => {
                self.push_line(line);
                Changed::of(&[Pane::Transcript])
            }
        }
    }

    /// Clears the whole view, transcript included: colibri's New chat and
    /// Clear. Whether a run is in flight is the child's fact, not the
    /// view's, so it is kept. Returns every pane, so the view event that
    /// follows carries all of them.
    pub fn clear(&mut self) -> Changed {
        let running = self.running;
        *self = Self {
            running,
            ..Self::default()
        };
        Changed::of(&PANES)
    }

    /// The panes in `changed`, as the page receives them.
    pub fn view(&self, changed: Changed) -> Value {
        let mut v = Map::new();
        for p in PANES.into_iter().filter(|&p| changed.has(p)) {
            v.insert(p.key().to_string(), self.pane(p));
        }
        Value::Object(v)
    }

    fn pane(&self, p: Pane) -> Value {
        match p {
            Pane::Transcript => json!(self.transcript),
            Pane::Actions => Value::Array(
                self.actions
                    .iter()
                    .map(|a| json!({"tool": a.tool, "target": a.target, "wrote": a.wrote}))
                    .collect(),
            ),
            Pane::Edits => json!(self.edits),
            Pane::Checks => Value::Array(
                self.checks
                    .iter()
                    .map(|c| {
                        json!({"kind": c.kind, "result": if c.passed { "passed" } else { "failed" }})
                    })
                    .collect(),
            ),
            Pane::Radius => match self.radius {
                Some(r) => json!({
                    "resolved": r.resolved,
                    "candidates": r.candidates,
                    "unresolved": r.unresolved,
                }),
                None => Value::Null,
            },
            Pane::Budget => json!({
                "turn": self.budget.turn,
                "max_turns": self.budget.max_turns,
                "prompt_bytes": self.budget.prompt_bytes,
            }),
            Pane::Approvals => Value::Array(
                self.approvals
                    .iter()
                    .map(|a| json!({"id": a.id, "question": a.question, "detail": a.detail}))
                    .collect(),
            ),
            Pane::Running => json!(self.running),
            Pane::Outcome => match &self.outcome {
                Some(e) => json!({
                    "outcome": e.outcome,
                    "exit": e.exit,
                    "run_id": e.run_id,
                    "restored": e.restored,
                    "error": e.error,
                    "id": e.id,
                }),
                None => Value::Null,
            },
        }
    }

    /// A turn, as `tui_bridge.rs` maps it.
    fn turn(&mut self, t: Turn) -> Changed {
        let mut c = Changed::of(&[Pane::Transcript, Pane::Actions, Pane::Budget]);
        // The verdict as the loop renders it; only these two mean a check
        // ran (DESK-07's contract). "None", or any other string, is no
        // check.
        let passed = match t.verdict.as_deref() {
            Some("Some(true)") => Some(true),
            Some("Some(false)") => Some(false),
            _ => None,
        };

        let mut line = format!("· {}", t.call);
        match passed {
            Some(true) => line.push_str("  ✓ check passed"),
            Some(false) => line.push_str("  ✗ check FAILED"),
            None => {}
        }
        self.push_line(line);
        if let Some(obs) = t.observation.as_deref() {
            for l in obs.lines().take(6) {
                self.push_line(format!("    {l}"));
            }
        }

        // Only an applied edit, or a file `write_file` created, changed the
        // tree; the observation is the only place that says so. A refused
        // edit is still an action, but not an edit.
        let obs = t.observation.as_deref().unwrap_or_default();
        let wrote = (t.call.starts_with("edit ") && obs.contains("applied: true"))
            || (t.call.starts_with("write_file ") && obs.starts_with("created "));
        let mut parts = t.call.splitn(2, ' ');
        self.actions.push(Action {
            tool: parts.next().unwrap_or_default().to_string(),
            target: parts.next().unwrap_or_default().to_string(),
            wrote,
        });
        if wrote && let Some(path) = t.call.split_whitespace().nth(1) {
            self.edits.push(path.to_string());
            c = c.with(Pane::Edits);
        }

        if let Some(passed) = passed {
            let kind = t
                .check_kind
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "check".to_string());
            self.checks.push(Check { kind, passed });
            c = c.with(Pane::Checks);
        }

        if let (Some(resolved), Some(candidates), Some(unresolved)) =
            (t.resolved, t.candidates, t.unresolved)
        {
            self.radius = Some(Radius {
                resolved,
                candidates,
                unresolved,
            });
            c = c.with(Pane::Radius);
        }

        self.budget.turn = Some(t.turn);
        self.budget.prompt_bytes = t.prompt_bytes;
        c
    }

    /// Empties the panes a run owns; the transcript is left to its cap.
    fn reset_run(&mut self) {
        self.actions.clear();
        self.edits.clear();
        self.checks.clear();
        self.radius = None;
        self.budget = Budget::default();
        self.approvals.clear();
        self.outcome = None;
    }

    fn push_line(&mut self, line: String) {
        self.transcript.push_back(line);
        while self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::parse_stdout;

    /// The contract the binary's own tests hold trellis to (ADR 0048 §6).
    const CONTRACT: &str = include_str!("../../crates/trellis-cli/tests/client_contract.json");

    /// A loop event as `trellis rpc` writes it: a `tracing` JSON line with
    /// its fields beside `message`.
    fn event(message: &str, fields: Value) -> String {
        let mut f = fields.as_object().cloned().unwrap_or_default();
        f.insert("message".into(), json!(message));
        json!({
            "timestamp": "2026-10-06T00:00:00.000000Z",
            "level": "INFO",
            "fields": f,
            "target": "trellis_loop::agent",
        })
        .to_string()
    }

    fn turn(fields: Value) -> String {
        event("turn", fields)
    }

    fn feed(s: &mut DesktopState, line: &str) -> Changed {
        s.update(parse_stdout(line))
    }

    fn all(s: &DesktopState) -> Value {
        s.view(Changed::of(&PANES))
    }

    fn run_start(id: &str) -> String {
        json!({"type": "run_start", "version": 1, "trellis": "0.0.1", "task": "change f",
               "model": "scripted:x", "root": "/r", "id": id})
        .to_string()
    }

    #[test]
    fn turn_event_becomes_an_action_and_a_transcript_line() {
        // The risk: a turn the loop reported never reaches the panes, so the
        // desktop draws a run that did nothing.
        let mut s = DesktopState::default();
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file app/core.py", "verdict": "None",
                         "check_kind": "", "observation": "def f():\n    return 1"}),
            ),
        );
        assert!(c.has(Pane::Actions) && c.has(Pane::Transcript), "{c:?}");
        let v = all(&s);
        assert_eq!(
            v["actions"],
            json!([{"tool": "read_file", "target": "app/core.py", "wrote": false}])
        );
        assert_eq!(
            v["transcript"],
            json!([
                "· read_file app/core.py",
                "    def f():",
                "        return 1"
            ])
        );
        assert_eq!(v["checks"], json!([]), "verdict None ran no check");
    }

    #[test]
    fn refused_edit_is_an_action_but_not_an_edit() {
        // The risk (tui_bridge.rs): a refused edit listed as pending makes
        // the edits pane contradict the action graph a user checks before
        // accepting.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "error: old text not found"}),
            ),
        );
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: false"}),
            ),
        );
        let v = all(&s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["actions"][0]["wrote"], false);
        assert_eq!(v["edits"], json!([]), "{v}");

        feed(
            &mut s,
            &turn(
                json!({"turn": 3, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: true (+1 -1)"}),
            ),
        );
        let v = all(&s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 3);
        assert_eq!(v["actions"][2]["wrote"], true);
        assert_eq!(v["edits"], json!(["a.py"]));
    }

    #[test]
    fn check_row_comes_only_from_a_some_verdict() {
        // The risk: a check row drawn for a turn that ran no check, or a
        // verdict string read loosely, shows a verdict trellis never gave.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(false)",
                         "check_kind": "Typecheck", "observation": "1 error"}),
            ),
        );
        let v = all(&s);
        assert_eq!(
            v["checks"],
            json!([{"kind": "Typecheck", "result": "failed"}])
        );
        assert!(
            v["transcript"][0]
                .as_str()
                .unwrap()
                .ends_with("check FAILED"),
            "{v}"
        );

        for verdict in [
            json!("None"),
            json!("Some(maybe)"),
            json!("true"),
            json!(true),
        ] {
            let mut s = DesktopState::default();
            feed(
                &mut s,
                &turn(
                    json!({"turn": 1, "call": "run_checks a.py", "verdict": verdict,
                             "check_kind": "Tests", "observation": ""}),
                ),
            );
            assert_eq!(all(&s)["checks"], json!([]), "verdict {verdict}");
        }
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(json!({"turn": 1, "call": "run_checks a.py", "check_kind": "Tests"})),
        );
        assert_eq!(all(&s)["checks"], json!([]), "a missing verdict");

        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(true)",
                         "check_kind": "Tests"}),
            ),
        );
        assert_eq!(
            all(&s)["checks"],
            json!([{"kind": "Tests", "result": "passed"}])
        );
    }

    /// Every key in `v`, at any depth.
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn radius_holds_three_counts_and_no_ratio() {
        // The risk (CLAUDE.md rule 3): a ratio or percentage turns three
        // counts into a confidence figure trellis never computed.
        let mut s = DesktopState::default();
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "blast_radius f", "verdict": "None",
                         "check_kind": "", "observation": "", "resolved": 4,
                         "candidates": 2, "unresolved": 1, "prompt_bytes": 900}),
            ),
        );
        assert!(c.has(Pane::Radius));
        let v = all(&s);
        assert_eq!(
            v["radius"],
            json!({"resolved": 4, "candidates": 2, "unresolved": 1})
        );
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                ["ratio", "percent", "fraction", "total"]
                    .iter()
                    .any(|w| k.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "the view derives a figure: {bad:?} in {v}");

        // Two of three counts is not a radius.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(json!({"turn": 1, "call": "blast_radius f", "resolved": 4, "candidates": 2})),
        );
        assert_eq!(all(&s)["radius"], Value::Null);
    }

    #[test]
    fn budget_reports_prompt_bytes_not_tokens() {
        // The risk (#243): an unreported prompt size drawn as 0, or labelled
        // tokens, reads as a measurement trellis did not make.
        let mut s = DesktopState::default();
        feed(&mut s, &event("run budget", json!({"max_iterations": 40})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": "", "prompt_bytes": 9800}),
            ),
        );
        let v = all(&s);
        assert_eq!(
            v["budget"],
            json!({"turn": 1, "max_turns": 40, "prompt_bytes": 9800})
        );

        // A scripted turn: no prompt was sent, so no size is reported.
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": ""}),
            ),
        );
        let v = all(&s);
        assert_eq!(v["budget"]["turn"], 2);
        assert!(
            v["budget"]["prompt_bytes"].is_null(),
            "unreported, not 0: {v}"
        );
        let mut ks = Vec::new();
        keys(&v["budget"], &mut ks);
        assert!(
            ks.iter().all(|k| !k.contains("token")),
            "no rpc event carries a token count: {ks:?}"
        );
    }

    #[test]
    fn run_start_resets_the_run_panes_and_clear_resets_the_view() {
        // The risk: a second run drawn on top of the first's actions, edits
        // and outcome, so the panes describe neither.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("run budget", json!({"max_iterations": 40})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit a.py", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "applied: true",
                         "resolved": 1, "candidates": 0, "unresolved": 0,
                         "prompt_bytes": 100}),
            ),
        );
        feed(
            &mut s,
            &json!({"type": "approval_request", "id": "approval-1", "question": "q",
                    "detail": "d"})
            .to_string(),
        );
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        let before = all(&s)["transcript"].as_array().unwrap().len();

        let c = feed(&mut s, &run_start("p2"));
        for p in RUN_PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&s);
        assert_eq!(v["actions"], json!([]));
        assert_eq!(v["edits"], json!([]));
        assert_eq!(v["checks"], json!([]));
        assert_eq!(v["radius"], Value::Null);
        assert_eq!(
            v["budget"],
            json!({"turn": null, "max_turns": null, "prompt_bytes": null})
        );
        assert_eq!(v["approvals"], json!([]));
        assert_eq!(v["outcome"], Value::Null);
        assert_eq!(v["running"], true);
        // The transcript runs on across runs, as the TUI's does.
        assert_eq!(v["transcript"].as_array().unwrap().len(), before + 1);
        assert_eq!(v["transcript"][before], "▸ change f");

        let c = s.clear();
        for p in PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&s);
        assert_eq!(v["transcript"], json!([]));
        assert_eq!(
            v["running"], true,
            "clearing the view does not end the run in flight"
        );
    }

    #[test]
    fn run_end_records_outcome_exit_and_run_id() {
        // The risk: the outcome pane shows a run_id, restored flag or error
        // trellis did not send, or loses the one it did.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 2,
                    "run_id": "20261006-abc", "restored": true, "id": "p1"})
            .to_string(),
        );
        assert!(c.has(Pane::Outcome) && c.has(Pane::Running));
        let v = all(&s);
        assert_eq!(v["running"], false);
        assert_eq!(
            v["outcome"],
            json!({"outcome": "not_converged", "exit": 2, "run_id": "20261006-abc",
                   "restored": true, "error": null, "id": "p1"})
        );

        // No trail written: no run_id is made up.
        feed(&mut s, &run_start("p2"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 1, "error": "boom",
                    "id": "p2"})
            .to_string(),
        );
        let v = all(&s);
        assert_eq!(v["outcome"]["run_id"], Value::Null);
        assert_eq!(v["outcome"]["restored"], Value::Null);
        assert_eq!(v["outcome"]["error"], "boom");
        assert_eq!(v["outcome"]["exit"], 1);
    }

    #[test]
    fn approval_request_opens_one_question_and_run_end_drops_it() {
        // The risk: a question shown twice, or left open after its run
        // ended, invites an answer nobody is waiting for.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let ask = json!({"type": "approval_request", "id": "approval-1",
                         "request_id": "p", "question": "edit a.py (+1 -1)",
                         "detail": "<script>alert(1)</script>"})
        .to_string();
        let c = feed(&mut s, &ask);
        assert!(c.has(Pane::Approvals));
        assert!(feed(&mut s, &ask).is_empty(), "a repeat opens nothing");
        let v = all(&s);
        assert_eq!(
            v["approvals"],
            json!([{"id": "approval-1", "question": "edit a.py (+1 -1)",
                    "detail": "<script>alert(1)</script>"}]),
            "one question, its text carried as text"
        );
        // Unanswerable without an id: not opened.
        assert!(
            feed(
                &mut s,
                &json!({"type": "approval_request", "question": "q"}).to_string()
            )
            .is_empty()
        );

        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130,
                    "restored": true, "id": "p"})
            .to_string(),
        );
        assert!(c.has(Pane::Approvals));
        assert_eq!(all(&s)["approvals"], json!([]));
    }

    #[test]
    fn unknown_and_unparsed_lines_change_nothing_but_the_transcript() {
        // The risk: an unknown frame type, a non-object line or a field of
        // the wrong type panics the core or reaches a pane as data.
        let mut s = DesktopState::default();
        for line in [
            r#"{"type":"something_new","x":1}"#,
            r#"{"type":7}"#,
            r#"{"fields":{"message":"index rebuilt","files":3}}"#,
            r#"{"fields":{"message":"turn","call":"edit a.py"}}"#,
            r#"{"fields":{"message":"turn","turn":"one","call":"edit a.py"}}"#,
            r#"{"fields":"turn"}"#,
            r#"{}"#,
        ] {
            assert!(feed(&mut s, line).is_empty(), "{line}");
        }
        let c = feed(&mut s, "<b>not json</b>\r\n");
        assert_eq!(c, Changed::of(&[Pane::Transcript]));
        assert_eq!(all(&s)["transcript"], json!(["<b>not json</b>"]));
        assert_eq!(feed(&mut s, "[1,2]"), Changed::of(&[Pane::Transcript]));
    }

    #[test]
    fn transcript_is_capped_at_the_tuis_bound() {
        // The risk: a long run grows the transcript without bound.
        let mut s = DesktopState::default();
        for i in 0..TRANSCRIPT_CAP + 5 {
            feed(&mut s, &format!("line {i}"));
        }
        let t = all(&s)["transcript"].as_array().unwrap().clone();
        assert_eq!(t.len(), TRANSCRIPT_CAP);
        assert_eq!(t[0], "line 5", "the oldest go first");
    }

    /// A value of the shape `field` has on the wire, distinct from every
    /// other value built: numbers are four digits from 7001 up, strings embed
    /// the record and field name. A field this does not know gets a string,
    /// so a contract field added with another shape fails below by name
    /// rather than being skipped.
    fn sample(field: &str, record: &str, spec: &Value, n: &mut u64) -> Value {
        if let Some(vals) = spec
            .get(format!("{field}_values"))
            .and_then(Value::as_array)
        {
            return vals.last().cloned().expect("a non-empty value list");
        }
        match field {
            "turn" | "resolved" | "candidates" | "unresolved" | "prompt_bytes"
            | "max_iterations" | "after_step" | "version" | "exit" => {
                *n += 1;
                json!(*n)
            }
            "data" => json!({"answer": format!("tok-{record}-{field}-end")}),
            _ => json!(format!("tok-{record}-{field}-end")),
        }
    }

    /// Fields whose value is a boolean on the wire.
    const BOOLEANS: [&str; 2] = ["success", "restored"];

    /// `run_start` as the frame's Debug spells its variant, `RunStart`.
    fn camel(ty: &str) -> String {
        ty.split('_')
            .map(|w| {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect::<String>())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The fields one record carries, from the contract: every list in an
    /// event's spec but a `*_values` one, and a framing type's `always` and
    /// `optional` keys.
    fn fields_of(spec: &Value) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in spec.as_object().expect("a contract record") {
            match v {
                Value::Array(a) if !k.ends_with("_values") => {
                    out.extend(a.iter().map(|f| f.as_str().unwrap().to_string()))
                }
                Value::Object(o) if k == "optional" => out.extend(o.keys().cloned()),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn every_field_in_the_rpc_contract_is_read() {
        // The risk (ADR 0048 §6): trellis keeps a field the contract
        // promises and the desktop's parser drops it, so a pane goes blank
        // while every trellis test passes. This measures what the parser
        // keeps, from its Debug output, for every field the contract lists.
        let contract: Value = serde_json::from_str(CONTRACT).expect("the contract is JSON");
        let rpc = &contract["rpc"];
        let mut n = 7000u64;
        let mut lost: Vec<String> = Vec::new();
        let mut records = 0;

        // (what, the record's fields with their values, how to wrap it)
        let mut build: Vec<(String, Map<String, Value>, bool)> = Vec::new();
        for (name, spec) in rpc["events"].as_object().expect("rpc.events") {
            let mut m = Map::new();
            for f in fields_of(spec) {
                let v = if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, name, spec, &mut n)
                };
                m.insert(f, v);
            }
            build.push((name.clone(), m, true));
        }
        for (ty, spec) in rpc["framing"].as_object().expect("rpc.framing") {
            let mut m = Map::new();
            for f in fields_of(spec) {
                let v = if f == "type" {
                    json!(ty)
                } else if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, ty, spec, &mut n)
                };
                m.insert(f, v);
            }
            build.push((ty.clone(), m, false));
        }

        let line = |name: &str, m: &Map<String, Value>, is_event: bool| {
            if is_event {
                event(name, Value::Object(m.clone()))
            } else {
                Value::Object(m.clone()).to_string()
            }
        };
        for (name, m, is_event) in &build {
            records += 1;
            let debug = format!("{:?}", parse_stdout(&line(name, m, *is_event)));
            for (f, v) in m {
                let kept = if f == "type" {
                    debug.starts_with(&format!("{}(", camel(name)))
                } else if let Some(b) = v.as_bool() {
                    // Present as true; flipped, the parse must differ.
                    let mut flipped = m.clone();
                    flipped.insert(f.clone(), json!(!b));
                    let other = format!("{:?}", parse_stdout(&line(name, &flipped, *is_event)));
                    other != debug
                } else {
                    let needle = match v {
                        Value::String(s) => s.clone(),
                        Value::Object(o) => o["answer"].as_str().unwrap().to_string(),
                        other => other.to_string(),
                    };
                    debug.contains(&needle)
                };
                if !kept {
                    lost.push(format!("{name}.{f}"));
                }
            }
        }
        assert!(
            records >= 7,
            "the contract lists fewer records than expected"
        );
        assert!(
            lost.is_empty(),
            "the parser drops what client_contract.json lists: {lost:?}"
        );
    }
}
