//! One line from the `trellis rpc` child, typed (ADR 0025 §5, ADR 0048 §6).
//!
//! A stderr line is [`Frame::Human`] whatever it holds. The child's stdout
//! carries two shapes. Framing records (`run_start`,
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

use crate::json;
use serde_json::Value;

/// One child line, typed.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// The loop's `turn` event.
    Turn(Turn),
    /// The loop's `model call` event, sent before each call to the model
    /// with the number the `turn` it produces will carry (#614). Without
    /// the turn it names nothing, so one that lacks it is [`Frame::Ignored`].
    ModelCall {
        turn: u64,
        /// The run's wall-clock spend as the budget checked it just before
        /// this call (#799); `None` from a trellis that sends none.
        elapsed_ms: Option<u64>,
    },
    /// The loop's `run budget` event: every ceiling in force, sent as the
    /// loop starts and again after a `set_budget` raise takes effect
    /// (#807), so the latest one replaces the last whole.
    RunBudget(RunBudget),
    /// The loop's `steer delivered` event.
    SteerDelivered {
        after_step: Option<u64>,
    },
    /// The `runtime identified` event: colibri's self-report, once per run
    /// (#627). Without `served` it names no runtime, so one that lacks it
    /// is [`Frame::Ignored`].
    RuntimeIdentified(RuntimeIdentified),
    RunStart(RunStart),
    RunEnd(RunEnd),
    Response(Response),
    ApprovalRequest(ApprovalRequest),
    /// A JSON object of a `type`, or an event, the desktop does not read.
    Ignored,
    /// A stdout line that is not a JSON object, kept as text.
    Unparsed(String),
    /// A stderr line: trellis's human text, shown as text and never read
    /// as data (ADR 0048 section 2, amended by ADR 0052).
    Human(String),
}

/// A `turn` event. `turn` and `call` are what make an event a turn, the
/// test `rpc.rs` applies, so both are required.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub turn: u64,
    pub call: String,
    /// What the trail records for the turn (#800), from the engine's own
    /// reading of the call and its whole observation, so the desktop never
    /// splits `call` or searches `observation` for them.
    pub tool: Option<String>,
    pub target: Option<String>,
    /// True only for an edit that applied or a file `write_file` created.
    pub wrote: Option<bool>,
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
    /// The wall time of the step's model call, queue wait included.
    pub model_ms: Option<u64>,
    /// The output tokens of the replies the step read; absent when any of
    /// them reported none.
    pub completion_tokens: Option<u64>,
    /// colibri's queue waits summed; absent unless every reply carried the
    /// `x-colibri-queue-wait-ms` header.
    pub queue_wait_ms: Option<u64>,
    /// The run's wall-clock spend after the turn, by the budget's own
    /// clock (#799), with waits on a person and pauses taken out.
    pub elapsed_ms: Option<u64>,
    /// The completion (output) tokens over every reply so far, the counter
    /// `run budget`'s `max_tokens` is checked against (#799): never prompt
    /// tokens, and never a sum of the two.
    pub completion_tokens_used: Option<u64>,
}

/// A `run budget` event's ceilings (#799). Each is `None` when the event
/// did not carry it: an older trellis sends `max_iterations` alone, and the
/// scripted driver, which elides nothing, sends no `context_bytes`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RunBudget {
    pub max_iterations: Option<u64>,
    pub max_wall_clock_ms: Option<u64>,
    /// A ceiling on completion tokens only, as `completion_tokens_used`.
    pub max_tokens: Option<u64>,
    /// The ceiling on one prompt, in bytes.
    pub context_bytes: Option<u64>,
}

/// colibri's self-report (client_contract.json's `runtime identified`).
/// Each count is colibri's own and `None` when it did not report it: a
/// liveness-only `/health`, or one that failed, reports no slot and no
/// tier, and the page says so rather than drawing 0 (ADR 0048 §8). The
/// three tier counts stay three; nothing here adds them or takes a share.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeIdentified {
    pub runtime: Option<String>,
    pub served: String,
    pub kv_slots: Option<u64>,
    pub tiers_vram: Option<u64>,
    pub tiers_ram: Option<u64>,
    pub tiers_disk: Option<u64>,
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
    /// The loop's whole wall-clock spend (#799); absent, never a partial
    /// figure, for a run the loop did not finish.
    pub elapsed_ms: Option<u64>,
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
                version: json::u64(&o, "version"),
                trellis: json::string(&o, "trellis"),
                task: json::string(&o, "task"),
                model: json::string(&o, "model"),
                root: json::string(&o, "root"),
                id: json::string(&o, "id"),
            }),
            Some("run_end") => Frame::RunEnd(RunEnd {
                outcome: json::string(&o, "outcome"),
                exit: json::i64(&o, "exit"),
                id: json::string(&o, "id"),
                run_id: json::string(&o, "run_id"),
                restored: json::bool(&o, "restored"),
                error: json::string(&o, "error"),
                elapsed_ms: json::u64(&o, "elapsed_ms"),
            }),
            Some("response") => Frame::Response(Response {
                command: json::string(&o, "command"),
                success: json::bool(&o, "success"),
                id: json::string(&o, "id"),
                data: o.get("data").cloned(),
                error: json::string(&o, "error"),
            }),
            Some("approval_request") => match json::string(&o, "id") {
                Some(id) => Frame::ApprovalRequest(ApprovalRequest {
                    id,
                    question: json::string(&o, "question"),
                    detail: json::string(&o, "detail"),
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
        Some("turn") => match (json::u64(f, "turn"), json::string(f, "call")) {
            (Some(turn), Some(call)) => Frame::Turn(Turn {
                turn,
                call,
                tool: json::string(f, "tool"),
                target: json::string(f, "target"),
                wrote: json::bool(f, "wrote"),
                verdict: json::string(f, "verdict"),
                check_kind: json::string(f, "check_kind"),
                observation: json::string(f, "observation"),
                resolved: json::u64(f, "resolved"),
                candidates: json::u64(f, "candidates"),
                unresolved: json::u64(f, "unresolved"),
                prompt_bytes: json::u64(f, "prompt_bytes"),
                model_ms: json::u64(f, "model_ms"),
                completion_tokens: json::u64(f, "completion_tokens"),
                queue_wait_ms: json::u64(f, "queue_wait_ms"),
                elapsed_ms: json::u64(f, "elapsed_ms"),
                completion_tokens_used: json::u64(f, "completion_tokens_used"),
            }),
            _ => Frame::Ignored,
        },
        Some("model call") => match json::u64(f, "turn") {
            Some(turn) => Frame::ModelCall {
                turn,
                elapsed_ms: json::u64(f, "elapsed_ms"),
            },
            None => Frame::Ignored,
        },
        Some("run budget") => Frame::RunBudget(RunBudget {
            max_iterations: json::u64(f, "max_iterations"),
            max_wall_clock_ms: json::u64(f, "max_wall_clock_ms"),
            max_tokens: json::u64(f, "max_tokens"),
            context_bytes: json::u64(f, "context_bytes"),
        }),
        Some("steer delivered") => Frame::SteerDelivered {
            after_step: json::u64(f, "after_step"),
        },
        Some("runtime identified") => match json::string(f, "served") {
            Some(served) => Frame::RuntimeIdentified(RuntimeIdentified {
                runtime: json::string(f, "runtime"),
                served,
                kv_slots: json::u64(f, "kv_slots"),
                tiers_vram: json::u64(f, "tiers_vram"),
                tiers_ram: json::u64(f, "tiers_ram"),
                tiers_disk: json::u64(f, "tiers_disk"),
            }),
            None => Frame::Ignored,
        },
        _ => Frame::Ignored,
    }
}
