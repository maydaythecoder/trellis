//! One line from the `trellis rpc` child, typed (ADR 0025 §5, ADR 0048 §6).
//!
//! The child's stdout carries two shapes. Framing records (`run_start`,
//! `run_end`, `response`, `approval_request`) have a top-level `type`. The
//! loop's own events are `tracing` JSON lines, `{"fields": {"message": ..},
//! ..}`, told apart by `fields.message`, as the contract tests in
//! `crates/trellis-cli/tests/modes.rs` read them.
//!
//! Everything here is untrusted: it is model or server text. A value of the
//! wrong JSON type is treated as absent, an absent field stays `None` (no 0,
//! no empty string stands in for something trellis did not report), and a
//! `type` or event this file does not know is [`Frame::Ignored`]. Only the
//! fields `crates/trellis-cli/tests/client_contract.json` lists are read;
//! the rest are unversioned (ADR 0048 §6).

use serde_json::{Map, Value};

/// One child line, typed.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// The loop's `turn` event.
    Turn(Turn),
    /// The loop's one `run budget` event per run.
    RunBudget {
        max_iterations: Option<u64>,
    },
    /// The loop's `steer delivered` event.
    SteerDelivered {
        after_step: Option<u64>,
    },
    RunStart(RunStart),
    RunEnd(RunEnd),
    Response(Response),
    ApprovalRequest(ApprovalRequest),
    /// A JSON object of a `type`, or an event, the desktop does not read.
    Ignored,
    /// A stdout line that is not a JSON object, kept as text.
    Unparsed(String),
}

/// A `turn` event. `turn` and `call` are what make an event a turn, the
/// test `tui_bridge.rs` and `rpc.rs` apply, so both are required.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub turn: u64,
    pub call: String,
    /// The loop's Debug rendering: `"Some(true)"`, `"Some(false)"` or
    /// `"None"`, kept as sent.
    pub verdict: Option<String>,
    /// `""` on a turn that ran no check.
    pub check_kind: Option<String>,
    pub observation: Option<String>,
    /// Present only on a `blast_radius` turn.
    pub resolved: Option<u64>,
    pub candidates: Option<u64>,
    pub unresolved: Option<u64>,
    /// Absent when the driver sent no prompt (a scripted run).
    pub prompt_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunStart {
    pub version: Option<u64>,
    pub trellis: Option<String>,
    pub task: Option<String>,
    pub model: Option<String>,
    pub root: Option<String>,
    /// The id of the prompt that started the run.
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunEnd {
    pub outcome: Option<String>,
    pub exit: Option<i64>,
    pub id: Option<String>,
    /// Present when a trail was written.
    pub run_id: Option<String>,
    /// Present when the outcome is `not_converged` or `interrupted`.
    pub restored: Option<bool>,
    /// Present when the outcome is `error`.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub command: Option<String>,
    pub success: Option<bool>,
    pub id: Option<String>,
    pub data: Option<Value>,
    pub error: Option<String>,
}

/// An approval question. Without an `id` it could not be answered, so a
/// request that lacks one is [`Frame::Ignored`].
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    pub question: Option<String>,
    pub detail: Option<String>,
}

/// Types one stdout line. Never fails: a line that is not a JSON object is
/// [`Frame::Unparsed`], shown as text and read as nothing else.
pub fn parse_stdout(line: &str) -> Frame {
    let line = line.trim_end_matches(['\r', '\n']);
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(line) else {
        return Frame::Unparsed(line.to_string());
    };
    if let Some(ty) = o.get("type") {
        return match ty.as_str() {
            Some("run_start") => Frame::RunStart(RunStart {
                version: u(&o, "version"),
                trellis: s(&o, "trellis"),
                task: s(&o, "task"),
                model: s(&o, "model"),
                root: s(&o, "root"),
                id: s(&o, "id"),
            }),
            Some("run_end") => Frame::RunEnd(RunEnd {
                outcome: s(&o, "outcome"),
                exit: o.get("exit").and_then(Value::as_i64),
                id: s(&o, "id"),
                run_id: s(&o, "run_id"),
                restored: b(&o, "restored"),
                error: s(&o, "error"),
            }),
            Some("response") => Frame::Response(Response {
                command: s(&o, "command"),
                success: b(&o, "success"),
                id: s(&o, "id"),
                data: o.get("data").cloned(),
                error: s(&o, "error"),
            }),
            Some("approval_request") => match s(&o, "id") {
                Some(id) => Frame::ApprovalRequest(ApprovalRequest {
                    id,
                    question: s(&o, "question"),
                    detail: s(&o, "detail"),
                }),
                None => Frame::Ignored,
            },
            _ => Frame::Ignored,
        };
    }
    let Some(f) = o.get("fields").and_then(Value::as_object) else {
        return Frame::Ignored;
    };
    match f.get("message").and_then(Value::as_str) {
        Some("turn") => match (u(f, "turn"), s(f, "call")) {
            (Some(turn), Some(call)) => Frame::Turn(Turn {
                turn,
                call,
                verdict: s(f, "verdict"),
                check_kind: s(f, "check_kind"),
                observation: s(f, "observation"),
                resolved: u(f, "resolved"),
                candidates: u(f, "candidates"),
                unresolved: u(f, "unresolved"),
                prompt_bytes: u(f, "prompt_bytes"),
            }),
            _ => Frame::Ignored,
        },
        Some("run budget") => Frame::RunBudget {
            max_iterations: u(f, "max_iterations"),
        },
        Some("steer delivered") => Frame::SteerDelivered {
            after_step: u(f, "after_step"),
        },
        _ => Frame::Ignored,
    }
}

fn s(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).map(str::to_string)
}

fn u(o: &Map<String, Value>, k: &str) -> Option<u64> {
    o.get(k).and_then(Value::as_u64)
}

fn b(o: &Map<String, Value>, k: &str) -> Option<bool> {
    o.get(k).and_then(Value::as_bool)
}
