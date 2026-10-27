//! Where output goes, and the JSONL framing of `--json` and `rpc`
//! (ADR 0025 §4).
//!
//! In JSON mode stdout carries JSONL and nothing else. Each line is either:
//! - a tracing event, in exactly the shape `--log-json` writes, from a layer
//!   on the one subscriber (ADR 0008: one event path), or
//! - a framing record with a `"type"` field: `run_start`, `run_end`,
//!   `work_start`, `work_end`, and in RPC `response` and `approval_request`.
//!
//! Framing is written here directly rather than through tracing, because it
//! is a contract with whatever reads the stream. A `RUST_LOG` setting must
//! not be able to filter out `run_end`, and a human run must not print it.
//!
//! Whether stdout is the JSON channel is a process-wide fact, because stdout
//! is process-wide. That is why it is a global switch here rather than a
//! parameter threaded through callers that other work owns.

use crate::RunExit;
use anyhow::Result;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static JSON: AtomicBool = AtomicBool::new(false);

/// Set once, in `main`, before anything prints.
pub(crate) fn set_json(on: bool) {
    JSON.store(on, Ordering::SeqCst);
}

pub(crate) fn json() -> bool {
    JSON.load(Ordering::SeqCst)
}

/// Where human text goes: stdout normally, stderr in JSON mode. Stderr
/// rather than nowhere, so a person watching a pipeline still sees the run.
pub(crate) fn human() -> Box<dyn Write + Send> {
    if json() {
        Box::new(std::io::stderr())
    } else {
        Box::new(std::io::stdout())
    }
}

/// Whether the human stream may carry colour codes: only when the stream
/// `human()` writes to is a terminal and `NO_COLOR` is unset. A pipe gets
/// none, because what reads it shows text: the desktop's messages pane drew
/// every escape as a glyph (ADR 0048 §2, #642), and so does a CI log.
pub(crate) fn human_colour() -> bool {
    use std::io::IsTerminal;
    let terminal = if json() {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    };
    colour(terminal, std::env::var_os("NO_COLOR").is_some())
}

fn colour(terminal: bool, no_color: bool) -> bool {
    terminal && !no_color
}

/// A place framing records are written, one whole line per lock.
pub(crate) type Sink = Arc<Mutex<dyn Write + Send>>;

/// Stdout, for the binary. Each record is one `write_all` of a whole line
/// under this lock, and the tracing layer's `fmt` writer also writes each
/// event with one `write_all` on `Stdout`, whose own lock covers the call.
/// So lines never interleave.
pub(crate) fn stdout_sink() -> Sink {
    Arc::new(Mutex::new(std::io::stdout()))
}

/// Writes one record as one line.
pub(crate) fn frame(sink: &Sink, record: &Value) {
    let mut line = escape_c1(&record.to_string());
    line.push('\n');
    if let Ok(mut w) = sink.lock() {
        let _ = w.write_all(line.as_bytes());
        let _ = w.flush();
    }
}

/// A string from a trail, made safe to print to a terminal.
///
/// A trail is read back as untrusted data (S10) and then printed straight to a
/// terminal, which interprets what it is given. An escape sequence in a task
/// description or a file path can move the cursor, repaint a line, or clear
/// the screen — so a run that did not converge can be made to *look* like one
/// that did, in the one command a user reads to find out which.
///
/// That matters more here than in most tools. The claim this project makes is
/// that work comes back verified rather than merely written, and a forged
/// terminal defeats the reading of the evidence rather than the evidence
/// itself.
///
/// Control characters become their escaped form rather than vanishing: a path
/// that contains one is worth seeing, and silently dropping it would print a
/// name that does not match the file.
pub(crate) fn plain(s: &str) -> String {
    trellis_core::text::escape_controls(s)
}

/// serde_json escapes C0 controls but writes DEL and the C1 range
/// (U+007F–U+009F) as they are, and some terminals act on C1 controls.
/// Those characters can only occur inside JSON strings, where a `\u` escape
/// is valid, so escaping them keeps the line valid and inert.
pub(crate) fn escape_c1(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ('\u{7f}'..='\u{9f}').contains(&c) {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// What a run is given beyond `cmd_run`'s parameters (ADR 0025).
///
/// Separate from `RunSeeding` and `RunControl` so the callers other work
/// owns (`work.rs`, `rpc.rs`) keep calling `cmd_run` unchanged.
pub(crate) struct RunExtras {
    /// `--skill` / `/skill`: bodies force-loaded into the context.
    pub skills: Vec<String>,
    /// The RPC request that started the run, echoed on its frames.
    pub request_id: Option<String>,
    /// Where `run_start` and `run_end` go. Stdout in JSON mode, the RPC
    /// writer under RPC, nowhere otherwise.
    pub frames: Option<Sink>,
    /// Set by RPC's `abort`; the run takes the Ctrl-C path when it is.
    pub abort: Option<Arc<Abort>>,
    /// RPC's `compact` requests, answered by the driver at its next model
    /// request (ADR 0025 §5). The run closes them for a driver that renders
    /// no prompt; its caller closes them when the run ends.
    pub compact: Option<Arc<trellis_loop::CompactRequests>>,
    /// The session's `/name` label, recorded in the trail as
    /// `Factual::label`. `None` everywhere but a named session.
    pub label: Option<String>,
    /// Where the run leaves the model's `done` summary, when the loop ended
    /// on one. `Outcome` does not carry it, so the run observes it on the
    /// way past and hands it back here; the session's `/copy` reads it.
    /// Left untouched by a run that ended any other way.
    pub done: Option<DoneSlot>,
    /// `trellis run --image`: files for a hosted tier-1 model's first user
    /// message (ADR 0041). Refused, before the snapshot, for any other.
    pub images: Vec<std::path::PathBuf>,
    /// RPC's record of its `steer` and `follow_up` (ADR 0025 §5), which the
    /// run reports each request's delivered steers back to. Watch only: the
    /// steering input itself arrives on `RunControl::steering`, as it does
    /// for `tui` and `session`. It exists because an aborted run drops its
    /// transcript, and with it the loop's record of what was delivered.
    /// `None` everywhere else, and the run only forwards.
    pub steer_watch: Option<Arc<crate::rpc::SteeringPort>>,
    /// `trellis run --agents N --target ...` with N above 1: the plan its
    /// agents run, one per partition (#466). `None` for every other run.
    pub fanout: Option<crate::run_partitioned::Fanout>,
    /// RPC's record of the trails its runs wrote, for `get_session_stats`
    /// (#319). `framed` pushes the run's id before it writes `run_end`, so
    /// a client that has read `run_end` and asks finds the run counted.
    pub run_ids: Option<Arc<Mutex<Vec<String>>>>,
    /// Set by `framed` just before it writes `run_end`, for RPC (#495): a
    /// `prompt` read after that is not refused as a run in progress but
    /// waits out the run thread's tail. Set before, as `run_ids` is pushed
    /// before, so a client that has read `run_end` finds it set.
    pub run_ended: Option<Arc<AtomicBool>>,
    /// Where the run leaves the tier it decided, as the TUI's status bar
    /// labels it, for rpc's `get_state` (#269). Written where the run
    /// settles its tier (after `colibri_tier` for a hosted spec, after the
    /// schema probe for a runtime), and left as it was by a run that stopped
    /// before it did. `None` everywhere but rpc.
    pub tier: Option<TierSlot>,
    /// The agent loop's budget spend when it returned, in whole
    /// milliseconds (`trellis_loop::Seed::elapsed`), which `framed` puts
    /// on `run_end` as `elapsed_ms`. Set only when the loop returned: a run
    /// refused before its loop, an interrupted one and a multi-agent one
    /// (each partition keeps its own clock) leave it empty, and `run_end`
    /// then carries none, never a 0.
    pub elapsed_ms: OnceLock<u64>,
    /// rpc's `pause` and `resume` (ADR 0058 §1), honoured by the agent loop
    /// at the top of each iteration; its caller closes it when the run ends.
    /// `None` everywhere but rpc.
    pub hold: Option<Arc<trellis_loop::Hold>>,
    /// rpc's `set_budget` (ADR 0058 §2): the run's whole ceiling, adopted
    /// by the agent loop and raised from rpc's thread; `None` everywhere
    /// but rpc.
    pub budget: Option<Arc<trellis_loop::LiveBudget>>,
}

/// The shared cell `RunExtras::done` writes into. `Arc<Mutex<_>>` because
/// `rpc` builds its extras on a thread of its own.
pub(crate) type DoneSlot = Arc<Mutex<Option<String>>>;

/// The shared cell `RunExtras::tier` writes into, for the same reason.
pub(crate) type TierSlot = Arc<Mutex<Option<String>>>;

impl Default for RunExtras {
    fn default() -> Self {
        Self {
            skills: Vec::new(),
            request_id: None,
            frames: json().then(stdout_sink),
            abort: None,
            compact: None,
            label: None,
            done: None,
            images: Vec::new(),
            steer_watch: None,
            fanout: None,
            run_ids: None,
            run_ended: None,
            tier: None,
            elapsed_ms: OnceLock::new(),
            hold: None,
            budget: None,
        }
    }
}

/// Runs `body`, bracketed by `run_start` and `run_end` when framing is on.
pub(crate) fn framed(
    root: &Path,
    task: &str,
    model: &str,
    extras: &RunExtras,
    body: impl FnOnce() -> Result<RunExit>,
) -> Result<RunExit> {
    let Some(sink) = &extras.frames else {
        return body();
    };
    let mut start = json!({
        "type": "run_start",
        "version": 1,
        "trellis": env!("CARGO_PKG_VERSION"),
        "task": task,
        "model": model,
        "root": root.display().to_string(),
    });
    if let Some(id) = &extras.request_id {
        start["id"] = json!(id);
    }
    frame(sink, &start);
    let (result, run_id) = crate::work::with_run_id(root, body);
    if let (Some(ids), Some(id)) = (&extras.run_ids, &run_id) {
        ids.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(id.clone());
    }
    if let Some(ended) = &extras.run_ended {
        ended.store(true, Ordering::SeqCst);
    }
    frame(
        sink,
        &run_end(
            &result,
            run_id,
            extras.elapsed_ms.get().copied(),
            extras.request_id.as_deref(),
        ),
    );
    result
}

/// The `run_end` record for a result.
pub(crate) fn run_end(
    result: &Result<RunExit>,
    run_id: Option<String>,
    elapsed_ms: Option<u64>,
    request_id: Option<&str>,
) -> Value {
    let mut v = match result {
        Ok(exit) => {
            let (outcome, restored) = match exit {
                RunExit::Converged => ("converged", None),
                RunExit::NotConverged { restored } => ("not_converged", Some(*restored)),
                // The same frame as any unconverged, restored run, so the
                // protocol (ADR 0025) does not grow; the question is the
                // task's, in the queue.
                RunExit::Asked { .. } => ("not_converged", Some(true)),
                RunExit::Interrupted { restored } => ("interrupted", Some(*restored)),
            };
            let mut v = json!({"type": "run_end", "outcome": outcome, "exit": exit.code()});
            if let Some(r) = restored {
                v["restored"] = json!(r);
            }
            v
        }
        // Exit 1 is what `trellis run` exits with when `main` returns an
        // error, which is where this error goes next.
        Err(e) => {
            json!({"type": "run_end", "outcome": "error", "exit": 1, "error": format!("{e:#}")})
        }
    };
    if let Some(id) = run_id {
        v["run_id"] = json!(id);
    }
    if let Some(ms) = elapsed_ms {
        v["elapsed_ms"] = json!(ms);
    }
    if let Some(id) = request_id {
        v["id"] = json!(id);
    }
    v
}

// ---------------------------------------------------------------------------
// Abort, for RPC. `cmd_run` races its run against Ctrl-C; this is a third
// arm beside it, taking the same interrupted path: rollback, then a trail.
// ---------------------------------------------------------------------------

/// One run's abort: asked for by RPC, then observed by the run.
///
/// Per run rather than per process, so an abort aimed at one run cannot end
/// another (tests run many at once in one process). `observed` exists for
/// ordering: RPC must not drop an open approval question until the run has
/// taken the abort path, or the dropped question reads as a "no", the loop
/// carries on to its next step, and the run ends as not converged instead
/// of interrupted.
#[derive(Default)]
pub(crate) struct Abort {
    requested: AtomicBool,
    observed: AtomicBool,
}

impl Abort {
    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }
    pub(crate) fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
    pub(crate) fn observed(&self) -> bool {
        self.observed.load(Ordering::SeqCst)
    }
}

/// Resolves once an abort is requested; never, when there is none.
///
/// Polled rather than notified: a notification sent before the run reached
/// its select would be lost, and a 50 ms poll on a run measured in seconds
/// costs nothing.
pub(crate) async fn aborted(abort: Option<Arc<Abort>>) {
    let Some(abort) = abort else {
        return std::future::pending().await;
    };
    while !abort.requested() {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    abort.observed.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// `trellis log --json`
// ---------------------------------------------------------------------------

/// One JSON line per trail, newest first: `run_id`, `integrity`, `outcome`
/// and the file's JSON as written. A trail is untrusted data (ADR 0007); it
/// is exported as data and never interpreted. A file that stops parsing
/// between the listing and this read is skipped, as `trellis log` skips one.
pub(crate) fn log_json(root: &Path, limit: usize, out: &mut dyn Write) -> Result<()> {
    for e in trellis_trail::query::all(root).iter().take(limit) {
        let Ok(text) = std::fs::read_to_string(&e.path) else {
            continue;
        };
        let Ok(trail) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let record = json!({
            "run_id": e.run_id(),
            "integrity": match e.integrity {
                trellis_trail::Integrity::Intact => "intact",
                trellis_trail::Integrity::Altered => "altered",
                trellis_trail::Integrity::Unrecorded => "unrecorded",
            },
            "outcome": e.outcome(),
            "trail": trail,
        });
        writeln!(out, "{}", escape_c1(&record.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_keeps_colour_and_a_pipe_or_no_color_drops_it() {
        // The pipe half is held through the binary in tests/modes.rs; a pty
        // is not available there, so the terminal half is held here: the fix
        // for #642 must not turn colour off for a person at a terminal.
        assert!(colour(true, false));
        assert!(!colour(false, false));
        assert!(!colour(true, true));
        assert!(!colour(false, true));
    }

    #[test]
    fn c1_controls_are_escaped_and_the_line_stays_valid_json() {
        // The risk: a trail's task holding U+009B (CSI), which some
        // terminals treat like ESC [, reaching a terminal through the export.
        let v = json!({"task": "fix\u{9b}2Jthis\u{7f}"});
        let s = escape_c1(&v.to_string());
        assert!(!s.contains('\u{9b}') && !s.contains('\u{7f}'), "{s}");
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v, "escaping must not change the data");
    }

    #[test]
    fn run_end_says_how_the_run_ended_in_words_and_as_an_exit_code() {
        let v = run_end(
            &Ok(RunExit::NotConverged { restored: false }),
            Some("r1".into()),
            Some(1234),
            Some("req"),
        );
        assert_eq!(v["outcome"], "not_converged");
        assert_eq!(
            v["exit"], 2,
            "an unrestored tree is not an ordinary failure"
        );
        assert_eq!(v["restored"], false);
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["elapsed_ms"], 1234);
        assert_eq!(v["id"], "req");
        let e = run_end(&Err(anyhow::anyhow!("dirty worktree")), None, None, None);
        assert_eq!(e["outcome"], "error");
        assert!(e["error"].as_str().unwrap().contains("dirty worktree"));
        // Refused before any loop ran: no spend was measured, so none is
        // written, rather than a 0 that reads as an instant run.
        assert!(e.get("elapsed_ms").is_none(), "{e}");
    }

    #[test]
    fn framing_brackets_the_run_with_start_and_end_records() {
        let d = tempfile::tempdir().unwrap();
        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = buf.clone();
        let extras = RunExtras {
            request_id: Some("7".into()),
            frames: Some(sink),
            ..RunExtras::default()
        };
        // The body stands in for a run whose loop returned and left its
        // spend, which `run_end` must carry.
        let r = framed(d.path(), "t", "scripted:x", &extras, || {
            let _ = extras.elapsed_ms.set(42);
            Ok(RunExit::Converged)
        });
        assert!(r.is_ok());
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["type"], "run_start");
        assert_eq!(lines[0]["id"], "7");
        assert_eq!(lines[1]["type"], "run_end");
        assert_eq!(lines[1]["outcome"], "converged");
        assert_eq!(lines[1]["elapsed_ms"], 42);
    }
}
