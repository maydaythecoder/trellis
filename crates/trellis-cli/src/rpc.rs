//! `trellis rpc`: JSONL commands on stdin, responses, framing and events on
//! stdout (ADR 0025 §5).
//!
//! A `prompt` runs a task through `cmd_run_with`, the path `trellis run`
//! takes, on its own thread. The main thread keeps reading, so `abort`,
//! `approve`, `get_state`, `steer` and `follow_up` are answered while the
//! run is in flight. One run at a time; the clean-worktree rule is
//! `cmd_run`'s and is unchanged. A follow-up is the one way a run starts
//! other than `prompt`: after the run it was sent during ends.

use crate::output::{self, Sink, frame};
use crate::{Human, RunControl, RunExit, RunSeeding, config};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use trellis_loop::Steering;

/// Where the runs' human text goes.
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// The binary: stderr, since stdout is the protocol.
    Process,
    /// Tests: nowhere.
    #[cfg(test)]
    Quiet,
}

/// The commands, with one line each, for `get_commands`.
const COMMANDS: &[(&str, &str)] = &[
    (
        "prompt",
        "run a task as `trellis run` does: {task, focus?, prompt?, args?, skill?, model?, control?}",
    ),
    ("abort", "interrupt the run in flight; it rolls back"),
    (
        "get_state",
        "whether a run is in flight, the model, the last run",
    ),
    ("get_messages", "the current or last run's turns"),
    ("set_model", "the model for later prompts: {model}"),
    (
        "get_available_models",
        "the catalog and the configured specs",
    ),
    ("approve", "answer an approval request: {id, yes}"),
    (
        "steer",
        "user text for the run in flight's next model request: {message}; \
         answered when that request carries it",
    ),
    (
        "follow_up",
        "a task to run as a new prompt once the run in flight ends: {message}",
    ),
    (
        "compact",
        "elide every observation but the newest from the run's next model request; \
         answers with what was folded",
    ),
    (
        "get_codebase",
        "the module graph `trellis index --graph` draws, as data, read-only: {limit?} \
         (default 40)",
    ),
    (
        "get_radius",
        "a symbol's blast radius `trellis radius` prints, as data, read-only: {symbol}; \
         reach and doubt as separate counts, never a ratio",
    ),
    (
        "get_queue",
        "the tasks `trellis queue list` prints, as data, read-only; claims nothing",
    ),
    (
        "vault_search",
        "the hits `trellis vault search` prints, as data, read-only: {query, limit?} \
         (default 12)",
    ),
    (
        "vault_graph",
        "a note's lineage, children and links `trellis vault graph` prints, as data, \
         read-only: {note}",
    ),
    (
        "get_commands",
        "this list, the prompt templates and the skills",
    ),
    ("quit", "abort any run, wait for its rollback, exit"),
];

/// Turns captured from the loop's `turn` events, for `get_messages`.
static TURNS: LazyLock<Arc<Mutex<Vec<Value>>>> = LazyLock::new(Arc::default);

/// A layer on the one subscriber that keeps each `turn` event's fields. The
/// same event path `tui_bridge` reads; not a second telemetry channel.
pub(crate) struct TurnCapture {
    store: Arc<Mutex<Vec<Value>>>,
}

impl Default for TurnCapture {
    fn default() -> Self {
        Self {
            store: TURNS.clone(),
        }
    }
}

#[derive(Default)]
struct Fields(Map<String, Value>);

impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), json!(format!("{v:?}")));
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.0.insert(f.name().into(), json!(v));
    }
}

impl<S: tracing::Subscriber> Layer<S> for TurnCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        // A turn has both a `turn` number and a `call`, the same test
        // `tui_bridge` applies so a diagnosis event is not taken for one.
        if f.0.get("message") == Some(&json!("turn"))
            && f.0.contains_key("turn")
            && f.0.contains_key("call")
            && let Ok(mut s) = self.store.lock()
        {
            f.0.remove("message");
            s.push(Value::Object(f.0));
        }
    }
}

/// What a `steer` still waiting is answered once its run has ended: the
/// loop will read the channel no more.
const STEER_ENDED: &str = "the run ended before its next model request; not delivered";

/// One run's steering channel, and the server's own record of it.
///
/// The record exists because an aborted run returns no transcript: `abort`
/// drops the run's future, and the loop's [`trellis_loop::SteeringLog`]
/// goes with it. So the server keeps every message it sent, in order, with
/// the command id that sent it, and the run reports each request's
/// delivered steers here as it makes it (through [`Watched`]). What was
/// sent and not delivered is then known however the run ended.
///
/// Sending, recording and closing happen under one lock, so a message that
/// races the run's end is either recorded (and handed back undelivered) or
/// refused; never taken and lost.
pub(crate) struct SteeringPort {
    state: Mutex<PortState>,
    receiver: Mutex<Option<trellis_loop::SteeringRx>>,
}

struct PortState {
    /// `None` once the run has ended.
    tx: Option<tokio::sync::mpsc::UnboundedSender<Steering>>,
    sent: Vec<Sent>,
    /// Steers that have reached a model request. The channel is FIFO and
    /// the loop delivers every steer it takes before a request, so these
    /// are the first `delivered` steers in `sent`.
    delivered: usize,
}

/// A steer's answer: the step it was delivered after, or why it was not.
type SteerAnswer = Result<usize, String>;

struct Sent {
    id: Option<String>,
    what: Steering,
    /// Set for a steer until it is answered.
    reply: Option<tokio::sync::oneshot::Sender<SteerAnswer>>,
}

/// A message that was sent and never delivered, as `abort` reports it.
struct Unsent {
    id: Option<String>,
    what: Steering,
}

impl Unsent {
    fn json(&self) -> Value {
        let (kind, message) = match &self.what {
            Steering::Steer(m) => ("steer", m),
            Steering::FollowUp(m) => ("follow_up", m),
        };
        json!({"id": self.id, "type": kind, "message": message})
    }
}

impl SteeringPort {
    fn new() -> Arc<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Self {
            state: Mutex::new(PortState {
                tx: Some(tx),
                sent: Vec::new(),
                delivered: 0,
            }),
            receiver: Mutex::new(Some(rx)),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PortState> {
        // No holder can panic between two writes it depends on, so a
        // poisoned lock still guards a consistent record.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The run's end of the channel, taken once by the run that uses it.
    pub(crate) fn take_receiver(&self) -> Option<trellis_loop::SteeringRx> {
        self.receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Records and sends one message. A steer comes back with the receiver
    /// its answer arrives on; a follow-up with none.
    fn send(
        &self,
        id: Option<&str>,
        what: Steering,
    ) -> Result<Option<tokio::sync::oneshot::Receiver<SteerAnswer>>, String> {
        let mut st = self.state();
        let Some(tx) = &st.tx else {
            // Only between a run's end and the thread moving on (the next
            // follow-up, or finishing), when a `prompt` is still refused,
            // so neither message tells the client to send one now.
            return Err(match what {
                Steering::Steer(_) => "the run has just ended; nothing was delivered".into(),
                Steering::FollowUp(_) => {
                    "the run has just ended; nothing was queued. Send it again once \
                     `get_state` shows no run in flight"
                        .into()
                }
            });
        };
        // A send fails only once the loop has dropped its receiver, at the
        // run's end. The record below still holds the message, and the
        // close that follows hands it back.
        let _ = tx.send(what.clone());
        let (reply, answer) = match what {
            Steering::Steer(_) => {
                let (t, r) = tokio::sync::oneshot::channel();
                (Some(t), Some(r))
            }
            Steering::FollowUp(_) => (None, None),
        };
        st.sent.push(Sent {
            id: id.map(str::to_string),
            what,
            reply,
        });
        Ok(answer)
    }

    /// Called as each model request is made, with the steers it carries.
    /// Answers each steer the first time a request carries it.
    fn observe(&self, delivered: &[trellis_loop::Steer]) {
        let mut st = self.state();
        let from = st.delivered;
        if delivered.len() <= from {
            return;
        }
        let steers = st
            .sent
            .iter_mut()
            .filter(|s| matches!(s.what, Steering::Steer(_)));
        for (s, d) in steers.skip(from).zip(&delivered[from..]) {
            if let Some(r) = s.reply.take() {
                let _ = r.send(Ok(d.after_step));
            }
        }
        st.delivered = delivered.len();
    }

    /// Ends the channel and hands back everything sent and not delivered,
    /// in the order it was received. A steer among them is answered as not
    /// delivered.
    fn close(&self) -> Vec<Unsent> {
        let mut st = self.state();
        st.tx = None;
        let delivered = st.delivered;
        let mut steers = 0;
        let mut unsent = Vec::new();
        for mut s in std::mem::take(&mut st.sent) {
            if matches!(s.what, Steering::Steer(_)) {
                steers += 1;
                if steers <= delivered {
                    continue;
                }
            }
            if let Some(r) = s.reply.take() {
                let _ = r.send(Err(STEER_ENDED.into()));
            }
            unsent.push(Unsent {
                id: s.id,
                what: s.what,
            });
        }
        unsent
    }
}

/// A model whose requests are reported to a [`SteeringPort`].
///
/// The loop drains the steering channel immediately before each request,
/// so the steers a request carries are the transcript's delivered ones at
/// the moment it is made. Reading them here, rather than from the
/// transcript the run returns, is what makes delivery known on an aborted
/// run too. With no port it forwards and does nothing else.
///
/// It forwards every method but `prompt_tokens`, which only the trail reads,
/// from the concrete driver after the run (#487).
pub(crate) struct Watched<'a, M> {
    model: &'a mut M,
    port: Option<&'a SteeringPort>,
}

impl<'a, M> Watched<'a, M> {
    pub(crate) fn new(model: &'a mut M, port: Option<&'a SteeringPort>) -> Self {
        Self { model, port }
    }
}

impl<M: trellis_loop::Model> trellis_loop::Model for Watched<'_, M> {
    fn next_step(
        &mut self,
        t: &trellis_loop::Transcript,
    ) -> impl std::future::Future<Output = Result<trellis_loop::Step, String>> + Send {
        if let Some(p) = self.port {
            p.observe(&t.steering.delivered);
        }
        self.model.next_step(t)
    }

    fn tokens_used(&self) -> u64 {
        self.model.tokens_used()
    }

    fn prompt_bytes(&self) -> Option<usize> {
        self.model.prompt_bytes()
    }

    fn compactions(&self) -> &[trellis_loop::Compacted] {
        self.model.compactions()
    }

    fn requested_compactions(&self) -> &[trellis_loop::RequestedCompaction] {
        self.model.requested_compactions()
    }

    fn step_usage(&self) -> trellis_loop::StepUsage {
        self.model.step_usage()
    }
}

/// The run of a chain in progress. A follow-up replaces it when it starts.
#[derive(Clone)]
struct Current {
    /// The command that started it: the `prompt`, or the `follow_up`.
    id: Option<String>,
    /// `compact` requests, answered by the driver at its next request.
    compact: Arc<trellis_loop::CompactRequests>,
    steering: Arc<SteeringPort>,
}

impl Current {
    fn new(id: Option<String>) -> Self {
        Self {
            id,
            compact: Arc::new(trellis_loop::CompactRequests::default()),
            steering: SteeringPort::new(),
        }
    }
}

/// A run in flight, and the follow-ups queued behind it.
struct Running {
    thread: std::thread::JoinHandle<()>,
    abort: Arc<output::Abort>,
    /// Set by the run's thread as it finishes, after `leftover`.
    done: Arc<AtomicBool>,
    current: Arc<Mutex<Current>>,
    /// What was sent and never delivered when the chain stopped on an
    /// abort, in the order it was received, for `abort`'s answer.
    leftover: Arc<Mutex<Vec<Value>>>,
}

impl Running {
    fn current(&self) -> Current {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

struct Server {
    root: PathBuf,
    sink: Sink,
    mode: Mode,
    /// Set by `set_model`; falls back to `[model] spec`.
    model: Option<String>,
    running: Option<Running>,
    last: Arc<Mutex<Option<Value>>>,
    /// Open approval questions, by id, each holding its reply channel.
    approvals: Arc<Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    next_approval: Arc<AtomicU64>,
    /// Set, under the `approvals` lock, when input ends: no `approve` can
    /// arrive after it, so a question asked from then on is declined.
    input_closed: Arc<AtomicBool>,
    /// Threads waiting to answer a `compact`. Joined before `serve`
    /// returns, so no answer is lost to the process exiting.
    waiters: Vec<std::thread::JoinHandle<()>>,
}

/// Reads commands until `quit` or end of input.
///
/// End of input waits for a run in flight to finish, so
/// `printf '{"type":"prompt",...}\n' | trellis rpc` runs one task to
/// completion. `quit` aborts it instead.
///
/// End of input also means no `approve` can come: every question open
/// then, and every one the run asks after, is declined (a dropped reply
/// channel is a no, ADR 0018), and the run goes on to its end. Joining
/// with a question still open would wait forever on an answer nobody can
/// send.
pub(crate) fn serve(root: &Path, input: impl BufRead, sink: Sink, mode: Mode) -> Result<()> {
    let mut s = Server {
        root: root.to_path_buf(),
        sink,
        mode,
        model: None,
        running: None,
        last: Arc::default(),
        approvals: Arc::default(),
        next_approval: Arc::default(),
        input_closed: Arc::default(),
        waiters: Vec::new(),
    };
    // A read error is the end of input, not a return: `?` here left `serve`
    // with a run in flight, and the process exited with no rollback and no
    // trail. It is handled as end of input is, and reported after the run
    // has ended.
    let mut broke: Option<std::io::Error> = None;
    for line in input.lines() {
        let line = match line {
            Ok(l) => l,
            // A line that is not UTF-8 was read whole and is gone from the
            // stream: bad input, answered as bad JSON is.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                s.respond(None, "parse", Err(format!("not UTF-8: {e}")));
                continue;
            }
            Err(e) => {
                broke = Some(e);
                break;
            }
        };
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let cmd: Value = match serde_json::from_str(line) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                s.respond(None, "parse", Err("a command is a JSON object".into()));
                continue;
            }
            Err(e) => {
                s.respond(None, "parse", Err(format!("not JSON: {e}")));
                continue;
            }
        };
        if !s.handle(&cmd) {
            return Ok(());
        }
    }
    // Under the lock the forwarder inserts under, so a question it is
    // about to record is either cleared here or sees the flag and is
    // dropped: none is left open.
    //
    // Not after an `abort`: `abort_run` already drops every question once
    // the run has taken the abort path, and a "no" delivered before that
    // would let the run end as not converged instead of interrupted.
    let aborting = s.running.as_ref().is_some_and(|r| r.abort.requested());
    if !aborting && let Ok(mut a) = s.approvals.lock() {
        s.input_closed.store(true, Ordering::SeqCst);
        a.clear();
    }
    if let Some(r) = s.running.take() {
        let _ = r.thread.join();
    }
    s.join_waiters();
    match broke {
        Some(e) => Err(anyhow::Error::new(e).context("reading commands from stdin failed")),
        None => Ok(()),
    }
}

impl Server {
    fn respond(&self, id: Option<&str>, command: &str, result: Result<Value, String>) {
        respond_on(&self.sink, id, command, result);
    }

    fn join_waiters(&mut self) {
        for w in self.waiters.drain(..) {
            let _ = w.join();
        }
    }

    /// `compact`: refused with no run in flight; otherwise answered from
    /// another thread once the run's next model request has been compacted,
    /// or once the run has closed the requests without one.
    fn compact(&mut self, id: Option<&str>) {
        if !self.in_flight() {
            self.respond(id, "compact", Err("no run in flight".into()));
            return;
        }
        let Some(r) = &self.running else {
            return;
        };
        let requests = r.current().compact;
        let rx = requests.request();
        let (sink, id) = (self.sink.clone(), id.map(str::to_string));
        self.waiters.push(std::thread::spawn(move || {
            let result = match rx.blocking_recv() {
                Ok(c) => Ok(json!({
                    "turn": c.turn,
                    "turns_folded": c.turns_folded,
                    "elided_turns": c.elided_turns,
                    "elided_bytes": c.elided_bytes,
                    "bytes_before": c.bytes_before,
                    "bytes_after": c.bytes_after,
                })),
                Err(_) => Err(requests
                    .closed()
                    .unwrap_or_else(|| "the run ended before its next model request".into())),
            };
            respond_on(&sink, id.as_deref(), "compact", result);
        }));
    }

    /// `get_codebase`: the module graph as data, built in memory as the
    /// interface's `/codebase` builds it; nothing is written.
    ///
    /// Answered from another thread, as `compact` is: indexing a large
    /// repository takes seconds, and on this thread it would hold up the
    /// `approve` or `abort` a waiting run needs.
    fn get_codebase(&mut self, cmd: &Value, id: Option<&str>) {
        let limit = match cmd.get("limit") {
            None => 40,
            Some(v) => match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                Some(n) => n,
                None => {
                    self.respond(
                        id,
                        "get_codebase",
                        Err(format!("`limit` is a count of modules, not {v}")),
                    );
                    return;
                }
            },
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let data = crate::doctor_cmd::codebase_json(&root, limit);
            respond_on(&sink, id.as_deref(), "get_codebase", Ok(data));
        }));
    }

    /// `get_radius`: a symbol's blast radius as data, reach and doubt as
    /// separate counts (ADR 0014); nothing is written.
    ///
    /// Built with `build_from_sources`, never `build_cached`: the cached path
    /// writes `.trellis/index.bin` when the cache is stale, and a read must
    /// not. Answered from another thread, as `get_codebase` is, so a waiting
    /// run's `approve` or `abort` is still read at once.
    fn get_radius(&mut self, cmd: &Value, id: Option<&str>) {
        let Some(symbol) = cmd.get("symbol").and_then(Value::as_str) else {
            self.respond(
                id,
                "get_radius",
                Err("`symbol` is required: the name `trellis radius` takes".into()),
            );
            return;
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        let symbol = symbol.to_string();
        self.waiters.push(std::thread::spawn(move || {
            let index = trellis_index::build_from_sources(&trellis_index::read_sources(&root));
            let result = crate::index_cmd::radius_json(&index, &symbol).map_err(|e| e.to_string());
            respond_on(&sink, id.as_deref(), "get_radius", result);
        }));
    }

    /// `get_queue`: every queued task as data; nothing is written, claimed
    /// or moved. The queue's writes are `trellis queue`'s and `trellis
    /// work`'s (ADR 0020 §5). Answered from another thread, as
    /// `get_codebase` is, so a waiting run's `approve` or `abort` is still
    /// read at once.
    fn get_queue(&mut self, id: Option<&str>) {
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::work::queue_json(&root).map_err(|e| e.to_string());
            respond_on(&sink, id.as_deref(), "get_queue", result);
        }));
    }

    /// `vault_search`: the hits `trellis vault search` prints, from the same
    /// function, as data; nothing is written. Every vault write stays a
    /// `trellis vault` command (#70). Answered from another thread, as
    /// `get_codebase` is, so a waiting run's `approve` or `abort` is still
    /// read at once.
    fn vault_search(&mut self, cmd: &Value, id: Option<&str>) {
        let Some(query) = cmd.get("query").and_then(Value::as_str) else {
            self.respond(
                id,
                "vault_search",
                Err("`query` is required: the text `trellis vault search` takes".into()),
            );
            return;
        };
        let limit = match cmd.get("limit") {
            None => 12,
            Some(v) => match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                Some(n) => n,
                None => {
                    self.respond(
                        id,
                        "vault_search",
                        Err(format!("`limit` is a count of hits, not {v}")),
                    );
                    return;
                }
            },
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        let query = query.to_string();
        self.waiters.push(std::thread::spawn(move || {
            let result =
                crate::vault_cmd::search_json(&root, &query, limit).map_err(|e| format!("{e:#}"));
            respond_on(&sink, id.as_deref(), "vault_search", result);
        }));
    }

    /// `vault_graph`: what `trellis vault graph` prints about a note, from
    /// the same function, as data; nothing is written. Answered from
    /// another thread, as `vault_search` is.
    fn vault_graph(&mut self, cmd: &Value, id: Option<&str>) {
        let Some(note) = cmd.get("note").and_then(Value::as_str) else {
            self.respond(
                id,
                "vault_graph",
                Err("`note` is required: the note `trellis vault graph` takes".into()),
            );
            return;
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        let note = note.to_string();
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::vault_cmd::graph_json(&root, &note).map_err(|e| format!("{e:#}"));
            respond_on(&sink, id.as_deref(), "vault_graph", result);
        }));
    }

    /// `steer` and `follow_up`: refused with no run in flight; otherwise
    /// sent on the run's steering channel, never the approval channel.
    ///
    /// A steer is answered from another thread when a model request carries
    /// it, with the step it followed, or, if the run ends first, with
    /// `success: false`. A follow-up is answered at once: accepted, as a
    /// `prompt` is, and its own run's `run_start` and `run_end` carry its id.
    fn steer(&mut self, cmd: &Value, id: Option<&str>, kind: &str) {
        if !self.in_flight() {
            self.respond(id, kind, Err("no run in flight".into()));
            return;
        }
        let message = match cmd.get("message").and_then(Value::as_str) {
            Some(m) if !m.trim().is_empty() => m.to_string(),
            _ => {
                let e = format!("{kind} needs {{\"message\": \"<text>\"}}, and not an empty one");
                self.respond(id, kind, Err(e));
                return;
            }
        };
        let Some(r) = &self.running else {
            return;
        };
        let what = match kind {
            "steer" => Steering::Steer(message),
            _ => Steering::FollowUp(message),
        };
        match r.current().steering.send(id, what) {
            Err(e) => self.respond(id, kind, Err(e)),
            Ok(None) => self.respond(id, kind, Ok(Value::Null)),
            Ok(Some(answer)) => {
                let (sink, id) = (self.sink.clone(), id.map(str::to_string));
                self.waiters.push(std::thread::spawn(move || {
                    let result = match answer.blocking_recv() {
                        Ok(Ok(after_step)) => Ok(json!({"after_step": after_step})),
                        Ok(Err(e)) => Err(e),
                        // The port answers every steer it holds before it is
                        // dropped; this is the port gone some other way.
                        Err(_) => Err(STEER_ENDED.into()),
                    };
                    respond_on(&sink, id.as_deref(), "steer", result);
                }));
            }
        }
    }

    /// `abort`: answered once the run has ended, with everything sent on
    /// the steering channel and never delivered, in the order received.
    ///
    /// After, not at once, because until the run has taken the abort path
    /// it can still deliver a steer to a request, and an answer given before
    /// then could name as undelivered a steer the model was sent.
    fn abort(&mut self, id: Option<&str>) {
        if !self.in_flight() {
            self.respond(id, "abort", Err("no run in progress".into()));
            return;
        }
        self.abort_run();
        let Some(r) = &self.running else {
            return;
        };
        let (done, leftover) = (r.done.clone(), r.leftover.clone());
        let (sink, id) = (self.sink.clone(), id.map(str::to_string));
        self.waiters.push(std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let undelivered = leftover.lock().map(|l| l.clone()).unwrap_or_default();
            respond_on(
                &sink,
                id.as_deref(),
                "abort",
                Ok(json!({"undelivered": undelivered})),
            );
        }));
    }

    fn in_flight(&mut self) -> bool {
        self.waiters.retain(|w| !w.is_finished());
        match &self.running {
            Some(r) if !r.thread.is_finished() => true,
            Some(_) => {
                if let Some(r) = self.running.take() {
                    let _ = r.thread.join();
                }
                false
            }
            None => false,
        }
    }

    /// Returns false to stop serving.
    fn handle(&mut self, cmd: &Value) -> bool {
        let id = cmd.get("id").and_then(Value::as_str).map(str::to_string);
        let id = id.as_deref();
        let kind = cmd.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "prompt" => {
                let r = self.prompt(cmd, id);
                self.respond(id, kind, r.map(|()| Value::Null));
            }
            "abort" => self.abort(id),
            "steer" | "follow_up" => self.steer(cmd, id, kind),
            "get_state" => {
                let running = self.in_flight();
                let pending: Vec<String> = self
                    .approvals
                    .lock()
                    .map(|a| a.keys().cloned().collect())
                    .unwrap_or_default();
                let data = json!({
                    "running": running,
                    "run_request_id": self.running.as_ref().and_then(|r| r.current().id),
                    "model": self.model(None).ok(),
                    "root": self.root.display().to_string(),
                    "last_run": self.last.lock().ok().and_then(|l| l.clone()),
                    "pending_approvals": pending,
                });
                self.respond(id, kind, Ok(data));
            }
            "get_messages" => {
                let turns = TURNS.lock().map(|t| t.clone()).unwrap_or_default();
                self.respond(id, kind, Ok(json!({"messages": turns})));
            }
            "set_model" => {
                let r = match cmd.get("model").and_then(Value::as_str) {
                    Some(m) => check_spec(m).map(|()| {
                        self.model = Some(m.to_string());
                        json!({"model": m})
                    }),
                    None => Err("set_model needs {\"model\": \"<spec>\"}".into()),
                };
                self.respond(id, kind, r);
            }
            "get_available_models" => {
                let data = crate::session_extra::models_json(&self.root, self.model.as_deref());
                self.respond(id, kind, Ok(data));
            }
            "approve" => {
                let r = self.approve(cmd, id);
                self.respond(id, kind, r);
            }
            "get_commands" => {
                let commands: Vec<Value> = COMMANDS
                    .iter()
                    .map(|(n, d)| json!({"name": n, "description": d}))
                    .collect();
                // A package manifest that cannot be read is the answer, not
                // an empty list of what it would have added.
                let listed = (|| -> anyhow::Result<Value> {
                    let prompts: Vec<Value> = crate::templates::list(&self.root)?
                        .into_iter()
                        .map(|t| {
                            json!({"name": t.name, "description": t.description,
                                   "argument_hint": t.argument_hint})
                        })
                        .collect();
                    // The same catalog a run lists, user-level and installed
                    // skills included, since `prompt {skill}` can force any.
                    let skills: Vec<Value> = trellis_loop::SkillCatalog::discover_with(
                        &self.root,
                        &crate::user_config::skill_dirs()?,
                    )
                    .valid
                    .into_iter()
                    .map(|s| json!({"name": s.name, "description": s.description}))
                    .collect();
                    Ok(json!({"commands": commands, "prompts": prompts, "skills": skills}))
                })();
                self.respond(id, kind, listed.map_err(|e| format!("{e:#}")));
            }
            "quit" => {
                self.abort_run();
                if let Some(r) = self.running.take() {
                    let _ = r.thread.join();
                }
                // After the join: the run closed its requests as it ended,
                // so every waiter has its answer to write.
                self.join_waiters();
                self.respond(id, kind, Ok(Value::Null));
                return false;
            }
            "compact" => self.compact(id),
            "get_codebase" => self.get_codebase(cmd, id),
            "get_radius" => self.get_radius(cmd, id),
            "get_queue" => self.get_queue(id),
            "vault_search" => self.vault_search(cmd, id),
            "vault_graph" => self.vault_graph(cmd, id),
            other => self.respond(
                id,
                if other.is_empty() { "unknown" } else { other },
                Err(format!(
                    "unknown command `{other}`; `get_commands` lists the {} there are \
                     (ADR 0025 §5 says why the rest of pi's are not built)",
                    COMMANDS.len()
                )),
            ),
        }
        true
    }

    /// Asks the run to abort, then drops every open question once it has.
    ///
    /// Dropping matters as much as the request. After the abort the run asks
    /// the orchestrator what it touched, and an orchestrator still waiting
    /// on an unanswered approval would never reply. A dropped reply channel
    /// is a no (ADR 0018), which frees it. The order matters too: dropped
    /// before the run has taken the abort path, the "no" lets the loop go on
    /// to its next step, and the run can end as not converged rather than
    /// interrupted. So the drop waits for `observed`.
    fn abort_run(&self) {
        let Some(r) = &self.running else {
            return;
        };
        r.abort.request();
        let (abort, done, approvals) = (r.abort.clone(), r.done.clone(), self.approvals.clone());
        std::thread::spawn(move || {
            while !abort.observed() && !done.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if let Ok(mut a) = approvals.lock() {
                a.clear();
            }
        });
    }

    /// The model for a run: the command's, then `set_model`'s, then config.
    fn model(&self, given: Option<&str>) -> Result<String, String> {
        given
            .map(str::to_string)
            .or_else(|| self.model.clone())
            .or_else(|| config::RepoConfig::load(&self.root).ok()?.model())
            .ok_or_else(|| "no model: pass \"model\", use set_model, or set [model] spec".into())
    }

    fn approve(&self, cmd: &Value, id: Option<&str>) -> Result<Value, String> {
        let id = id.ok_or("approve needs the approval request's \"id\"")?;
        // Only JSON `true` approves, as only `y` or `yes` does on a terminal
        // (ADR 0018). `"yes"`, `1` or a missing field is a no.
        let yes = cmd.get("yes") == Some(&Value::Bool(true));
        let reply = self
            .approvals
            .lock()
            .map_err(|_| "approval state poisoned".to_string())?
            .remove(id)
            .ok_or_else(|| format!("no open approval request `{id}`"))?;
        let _ = reply.send(yes);
        Ok(json!({"approved": yes}))
    }

    /// Validates the command and starts the run. Everything cheap is checked
    /// here, so a bad request is refused rather than accepted and failed; the
    /// worktree is `cmd_run`'s to check, and its refusal arrives as `run_end`.
    fn prompt(&mut self, cmd: &Value, id: Option<&str>) -> Result<(), String> {
        if self.in_flight() {
            return Err("a run is in progress; one at a time (abort it, or wait)".into());
        }
        let text = |k: &str| cmd.get(k).and_then(Value::as_str).map(str::to_string);
        let task = text("task").unwrap_or_default();
        let task = match text("prompt") {
            Some(name) => {
                let t = crate::templates::render(&self.root, &name, text("args").as_deref())
                    .map_err(|e| format!("{e:#}"))?;
                if task.trim().is_empty() {
                    t
                } else {
                    format!("{t}\n\n{task}")
                }
            }
            // As `run --args` requires `--prompt`: arguments with no
            // template to receive them would be dropped without a word.
            None if cmd.get("args").is_some() => {
                return Err("\"args\" needs \"prompt\", the template they fill".into());
            }
            None if task.trim().is_empty() => {
                return Err("prompt needs \"task\", \"prompt\", or both".into());
            }
            None => task,
        };
        // The operator's program chose it (the request or `set_model`), or the
        // repository did (`[model] spec`); ADR 0024 §3 treats them differently.
        let model_from_user = text("model").is_some() || self.model.is_some();
        let model = self.model(text("model").as_deref())?;
        check_spec(&model)?;
        let flag = match text("control").as_deref() {
            None => None,
            Some("auto") => Some(config::ControlMode::Auto),
            Some("approve") => Some(config::ControlMode::Approve),
            Some(o) => return Err(format!("control \"{o}\" is not \"auto\" or \"approve\"")),
        };
        let skills: Vec<String> = match cmd.get("skill") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(xs)) => xs
                .iter()
                .map(|x| x.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or("skill is a name or a list of names")?,
            Some(_) => return Err("skill is a name or a list of names".into()),
        };
        let focus = text("focus");
        let unconstrained = config::RepoConfig::load(&self.root)
            .map(|c| c.unconstrained())
            .unwrap_or(false);

        let abort = Arc::new(output::Abort::default());
        let done = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(Current::new(id.map(str::to_string))));
        let leftover: Arc<Mutex<Vec<Value>>> = Arc::default();
        let chain = Chain {
            root: self.root.clone(),
            sink: self.sink.clone(),
            mode: self.mode,
            approvals: self.approvals.clone(),
            next_approval: self.next_approval.clone(),
            last: self.last.clone(),
            input_closed: self.input_closed.clone(),
            abort: abort.clone(),
            current: current.clone(),
            model,
            model_from_user,
            unconstrained,
            flag,
            skills,
            focus,
        };
        let (run_done, run_leftover) = (done.clone(), leftover.clone());
        let thread = std::thread::spawn(move || {
            let unsent = chain.run(task);
            if let Ok(mut l) = run_leftover.lock() {
                *l = unsent;
            }
            run_done.store(true, Ordering::SeqCst);
        });
        self.running = Some(Running {
            thread,
            abort,
            done,
            current,
            leftover,
        });
        Ok(())
    }
}

/// A `prompt`'s run and the follow-ups sent during it, run one after
/// another on one thread.
///
/// On the run's thread rather than the reader's because the reader is
/// blocked on stdin and cannot see a run end. Every run in the chain takes
/// the prompt's model, control mode, focus and skills: a follow-up is the
/// same operator continuing the same session, and carries only its text.
struct Chain {
    root: PathBuf,
    sink: Sink,
    mode: Mode,
    approvals: Arc<Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    next_approval: Arc<AtomicU64>,
    last: Arc<Mutex<Option<Value>>>,
    input_closed: Arc<AtomicBool>,
    /// One for the chain: an abort stops the run in flight and every
    /// follow-up behind it.
    abort: Arc<output::Abort>,
    current: Arc<Mutex<Current>>,
    model: String,
    model_from_user: bool,
    unconstrained: bool,
    flag: Option<config::ControlMode>,
    skills: Vec<String>,
    focus: Option<String>,
}

impl Chain {
    /// Runs the prompt, then each follow-up in the order received, until
    /// none is left or an abort is requested. Returns, for `abort`, what was
    /// sent and never delivered when it stopped on one; otherwise nothing,
    /// since every follow-up ran and every stranded steer was answered.
    fn run(&self, task: String) -> Vec<Value> {
        let first = self.current().id;
        let mut queue: VecDeque<(Option<String>, String)> = VecDeque::from([(first, task)]);
        let mut started = false;
        while let Some((id, task)) = queue.pop_front() {
            if self.abort.requested() {
                queue.push_front((id, task));
                return queue
                    .into_iter()
                    .map(|(id, task)| {
                        Unsent {
                            id,
                            what: Steering::FollowUp(task),
                        }
                        .json()
                    })
                    .collect();
            }
            // The first run's `Current` was made with the `prompt`, so a
            // `steer` read before this thread got here still reaches it.
            if started {
                *self.current.lock().unwrap_or_else(PoisonError::into_inner) =
                    Current::new(id.clone());
            }
            started = true;
            let cur = self.current();
            self.one(&task, &cur);
            let unsent = cur.steering.close();
            if self.abort.requested() {
                // Queued before this run's own, so received before them.
                let mut all: Vec<Value> = queue
                    .into_iter()
                    .map(|(id, task)| {
                        Unsent {
                            id,
                            what: Steering::FollowUp(task),
                        }
                        .json()
                    })
                    .collect();
                all.extend(unsent.iter().map(Unsent::json));
                return all;
            }
            // A stranded steer was answered as not delivered by `close`. It
            // is not run as a task: it was written to redirect this run, and
            // starting work from it would do something nobody asked for.
            for u in unsent {
                if let Steering::FollowUp(text) = u.what {
                    queue.push_back((u.id, text));
                }
            }
        }
        Vec::new()
    }

    fn current(&self) -> Current {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// One run, as `trellis run` runs it.
    fn one(&self, task: &str, cur: &Current) {
        if let Ok(mut t) = TURNS.lock() {
            t.clear();
        }
        let rid = cur.id.clone();
        // Approval questions become records on the stream; answers come
        // back through `approve` and the stored reply channel.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<trellis_loop::ApprovalRequest>(1);
        let fwd = {
            let (sink, approvals, rid) = (self.sink.clone(), self.approvals.clone(), rid.clone());
            let (aborted, closed, next) = (
                self.abort.clone(),
                self.input_closed.clone(),
                self.next_approval.clone(),
            );
            std::thread::spawn(move || {
                while let Some(req) = rx.blocking_recv() {
                    let n = next.fetch_add(1, Ordering::SeqCst) + 1;
                    let aid = format!("approval-{n}");
                    if let Ok(mut a) = approvals.lock() {
                        // Checked under the lock `abort_run` clears
                        // under: a question arriving after the run took
                        // the abort path is dropped here, because the
                        // clear may already have happened.
                        // The same for end of input: nobody is left
                        // to answer, and the drop is the no.
                        if aborted.observed() || closed.load(Ordering::SeqCst) {
                            continue;
                        }
                        a.insert(aid.clone(), req.reply);
                    }
                    frame(
                        &sink,
                        &json!({"type": "approval_request", "id": aid, "request_id": rid,
                                "question": req.question, "detail": req.detail}),
                    );
                }
            })
        };
        let mut human: Box<dyn std::io::Write + Send> = match self.mode {
            Mode::Process => output::human(),
            #[cfg(test)]
            Mode::Quiet => Box::new(std::io::sink()),
        };
        let before = crate::output::RunExtras {
            skills: self.skills.clone(),
            request_id: rid.clone(),
            frames: Some(self.sink.clone()),
            abort: Some(self.abort.clone()),
            compact: Some(cur.compact.clone()),
            label: None,
            done: None,
            // `--image` is `trellis run`'s alone (ADR 0041).
            images: Vec::new(),
            steer_watch: Some(cur.steering.clone()),
        };
        let r = crate::cmd_run_with(
            &self.root,
            task,
            crate::ModelChoice {
                spec: &self.model,
                unconstrained: self.unconstrained,
            },
            // No agent profile over RPC: the caller is the operator, and
            // profiles are how `work` runs a task as someone (ADR 0026).
            RunSeeding {
                resume: false,
                focus: self.focus.as_deref(),
                profile: None,
                model_from_user: self.model_from_user,
                model_source: crate::hosted::SpecSource::Repository,
                queued: false,
                services: &[],
            },
            RunControl {
                flag: self.flag,
                human: Human::Channel(tx),
                // The one carrier every caller hands a run its steering on
                // (#115, #116); the port keeps the sending end.
                steering: cur.steering.take_receiver(),
            },
            before,
            &mut human,
        );
        let _ = fwd.join();
        // A `compact` still waiting will get no model request to act on.
        cur.compact
            .close("the run ended before its next model request; nothing was compacted");
        // Questions still open when the run ended are dropped, which the
        // gate reads as a no; none can be answered now.
        if let Ok(mut a) = self.approvals.lock() {
            a.clear();
        }
        if let Ok(mut l) = self.last.lock() {
            *l = Some(summary(&r, rid.as_deref()));
        }
    }
}

fn respond_on(sink: &Sink, id: Option<&str>, command: &str, result: Result<Value, String>) {
    let mut v = json!({"type": "response", "command": command});
    if let Some(id) = id {
        v["id"] = json!(id);
    }
    match result {
        Ok(data) => {
            v["success"] = json!(true);
            if !data.is_null() {
                v["data"] = data;
            }
        }
        Err(e) => {
            v["success"] = json!(false);
            v["error"] = json!(e);
        }
    }
    frame(sink, &v);
}

/// `get_state`'s view of how the last run ended.
fn summary(r: &Result<RunExit>, id: Option<&str>) -> Value {
    let mut v = output::run_end(r, None, id);
    if let Some(o) = v.as_object_mut() {
        o.remove("type");
    }
    v
}

/// A spec that parses, and for a script, a file that exists.
fn check_spec(spec: &str) -> Result<(), String> {
    let parsed: trellis_model::ModelSpec = spec.parse().map_err(|e| format!("{e}"))?;
    if let trellis_model::ModelSpec::Scripted(p) = &parsed
        && !p.is_file()
    {
        return Err(format!("{} is not a readable file", p.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// What a test feeds the server: a line's bytes (a newline is added),
    /// or a read that fails.
    enum Chunk {
        Line(Vec<u8>),
        Fail,
    }

    /// A reader fed line by line, so a test can answer what the run asks.
    struct Feed {
        rx: mpsc::Receiver<Chunk>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for Feed {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(Chunk::Line(mut line)) => {
                        line.push(b'\n');
                        self.buf = line;
                        self.pos = 0;
                    }
                    Ok(Chunk::Fail) => {
                        return Err(std::io::Error::other("the client's pipe broke"));
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    struct Client {
        tx: Option<mpsc::Sender<Chunk>>,
        out: Arc<Mutex<Vec<u8>>>,
        server: Option<std::thread::JoinHandle<Result<()>>>,
    }

    impl Client {
        fn start(root: &Path) -> Self {
            let (tx, rx) = mpsc::channel();
            let out: Arc<Mutex<Vec<u8>>> = Arc::default();
            let sink: Sink = out.clone();
            let root = root.to_path_buf();
            let server = std::thread::spawn(move || {
                let input = std::io::BufReader::new(Feed {
                    rx,
                    buf: Vec::new(),
                    pos: 0,
                });
                serve(&root, input, sink, Mode::Quiet)
            });
            Self {
                tx: Some(tx),
                out,
                server: Some(server),
            }
        }

        fn send(&self, v: Value) {
            self.raw(v.to_string().into_bytes());
        }

        fn raw(&self, line: Vec<u8>) {
            self.tx.as_ref().unwrap().send(Chunk::Line(line)).unwrap();
        }

        fn records(&self) -> Vec<Value> {
            let text = String::from_utf8(self.out.lock().unwrap().clone()).unwrap();
            text.lines()
                .map(|l| serde_json::from_str(l).expect("every line is JSON"))
                .collect()
        }

        /// Waits for a record matching `pred`.
        fn wait_for(&self, pred: impl Fn(&Value) -> bool) -> Value {
            let until = Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(v) = self.records().into_iter().find(|v| pred(v)) {
                    return v;
                }
                assert!(Instant::now() < until, "timed out: {:?}", self.records());
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Closes input and waits for the server, which waits for the run.
        fn finish(mut self) -> Vec<Value> {
            drop(self.tx.take());
            self.server.take().unwrap().join().unwrap().unwrap();
            self.records()
        }
    }

    fn fixture(name: &str, config: &str) -> (PathBuf, String) {
        let dir = crate::testutil::scratch(&format!("rpc-{name}"));
        crate::testutil::git_init(&dir);
        std::fs::create_dir_all(dir.join(".trellis")).unwrap();
        std::fs::write(dir.join(".trellis/config.toml"), config).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        let scripts = crate::testutil::scratch(&format!("rpc-{name}-script"));
        let path = scripts.join("script.json");
        std::fs::write(
            &path,
            r#"{"steps":[
                {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
                {"tool":"done","summary":"changed it"}
            ]}"#,
        )
        .unwrap();
        (dir, format!("scripted:{}", path.display()))
    }

    fn of_type<'a>(rs: &'a [Value], t: &str) -> Vec<&'a Value> {
        rs.iter().filter(|r| r["type"] == t).collect()
    }

    #[test]
    fn a_prompt_runs_like_trellis_run_and_is_bracketed_by_start_and_end() {
        let (dir, spec) = fixture("prompt", "");
        let c = Client::start(&dir);
        c.send(json!({"id": "1", "type": "prompt", "task": "change f", "model": spec}));
        let rs = c.finish();
        let resp = of_type(&rs, "response");
        assert_eq!(resp[0]["id"], "1");
        assert_eq!(resp[0]["success"], true, "{rs:?}");
        let start = rs.iter().position(|r| r["type"] == "run_start").unwrap();
        let end = rs.iter().position(|r| r["type"] == "run_end").unwrap();
        assert!(start < end);
        // Edited, then `done` with no check: the grounding contract says
        // that does not converge, and the tree is rolled back.
        assert_eq!(rs[end]["outcome"], "not_converged", "{rs:?}");
        assert_eq!(rs[end]["exit"], 1);
        assert_eq!(rs[end]["id"], "1");
        assert!(rs[end]["run_id"].is_string(), "a trail was written");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    /// Runs one approve-mode prompt, answers its question with `yes`, and
    /// returns the `approve` response and the approval outcomes the trail
    /// recorded.
    fn approval_round(name: &str, yes: Value) -> (Value, Vec<String>) {
        let (dir, spec) = fixture(name, "[control]\nmode = \"approve\"\n");
        let c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        let q = c.wait_for(|r| r["type"] == "approval_request");
        assert_eq!(q["id"], "approval-1");
        assert_eq!(q["request_id"], "p");
        assert!(q["question"].as_str().unwrap().contains("edit a.py"), "{q}");

        // One run at a time, even while this one waits on a person.
        c.send(json!({"id": "second", "type": "prompt", "task": "again", "model": spec}));
        let r2 = c.wait_for(|r| r["id"] == "second");
        assert_eq!(r2["success"], false);

        c.send(json!({"id": "approval-1", "type": "approve", "yes": yes}));
        let a = c.wait_for(|r| r["type"] == "response" && r["command"] == "approve");
        let rs = c.finish();
        assert!(!of_type(&rs, "run_end").is_empty());

        let runs = dir.join(".trellis/runs");
        let file = std::fs::read_dir(&runs)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == "json"))
            .unwrap();
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let t = trellis_trail::UntrustedTrail::from_json(&raw).unwrap();
        let outcomes = t
            .factual
            .actions
            .iter()
            .filter(|a| a.tool == "approval")
            .map(|a| a.outcome.clone())
            .collect();
        (a, outcomes)
    }

    #[test]
    fn approve_mode_asks_over_rpc_and_the_trail_records_the_clients_yes() {
        // The risk: approve mode configured, a client driving trellis, and
        // edits landing with nobody asked. The question must arrive as a
        // record and the answer must be the client's.
        let (a, outcomes) = approval_round("approve-yes", json!(true));
        assert_eq!(a["success"], true);
        assert_eq!(a["data"]["approved"], true);
        assert_eq!(outcomes, ["approved"]);
    }

    #[test]
    fn a_yes_that_is_not_json_true_is_a_no() {
        // ADR 0018: only an explicit yes counts. A client that sends the
        // string "true" has not said yes in this protocol's terms.
        let (a, outcomes) = approval_round("approve-string", json!("true"));
        assert_eq!(a["data"]["approved"], false);
        assert_eq!(outcomes, ["declined"]);
    }

    #[test]
    fn a_hosted_model_named_in_the_prompt_is_the_client_s_choice() {
        // ADR 0024 §3. The risk: only `set_model` counted as the client's
        // choice, so a hosted spec sent with the prompt itself was refused
        // as an endpoint the repository chose. The host is `.invalid`, which
        // never resolves, so no request is made whether or not this machine
        // has OPENAI_API_KEY set; how the run ends depends on that variable,
        // so only the absence of the refusal is checked.
        let (dir, _) = fixture("prompt-model", "");
        let c = Client::start(&dir);
        c.send(json!({
            "id": "p",
            "type": "prompt",
            "task": "change f",
            "model": "openai-chat:https://collector.invalid/v1#m",
        }));
        let rs = c.finish();
        assert!(!of_type(&rs, "run_end").is_empty(), "{rs:?}");
        let all = serde_json::to_string(&rs).unwrap();
        assert!(!all.contains("chose"), "{all}");
    }

    #[test]
    fn in_flight_forgets_answered_waiters_and_keeps_the_ones_still_waiting() {
        // The risk: the pruning inverted, so a waiter that has yet to answer
        // is dropped (detached) and `serve`'s last join no longer waits for
        // it: its answer can be lost to the process exiting.
        let out: Arc<Mutex<Vec<u8>>> = Arc::default();
        let mut s = Server {
            root: PathBuf::new(),
            sink: out,
            mode: Mode::Quiet,
            model: None,
            running: None,
            last: Arc::default(),
            approvals: Arc::default(),
            next_approval: Arc::default(),
            input_closed: Arc::default(),
            waiters: Vec::new(),
        };
        let answered = std::thread::spawn(|| {});
        while !answered.is_finished() {
            std::thread::yield_now();
        }
        let (release, wait) = mpsc::channel::<()>();
        let waiting = std::thread::spawn(move || {
            let _ = wait.recv();
        });
        s.waiters = vec![answered, waiting];
        assert!(!s.in_flight());
        assert_eq!(s.waiters.len(), 1);
        assert!(!s.waiters[0].is_finished(), "the one still waiting is kept");
        release.send(()).unwrap();
        s.join_waiters();
    }

    #[test]
    fn abort_interrupts_the_run_and_it_rolls_back() {
        let (dir, spec) = fixture("abort", "[control]\nmode = \"approve\"\n");
        let c = Client::start(&dir);
        c.send(json!({"type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.send(json!({"id": "x", "type": "abort"}));
        let rs = c.finish();
        let end = of_type(&rs, "run_end")[0];
        assert_eq!(end["outcome"], "interrupted", "{rs:?}");
        assert_eq!(end["exit"], 130);
        assert_eq!(end["restored"], true);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    /// A shell argv for `script`, as TOML, with literal strings so a Windows
    /// path's backslashes are not escapes.
    fn sh(script: &str) -> String {
        if cfg!(windows) {
            format!("['cmd', '/C', '{script}']")
        } else {
            format!("['sh', '-c', '{script}']")
        }
    }

    /// Where `fixture(name, ..)` puts its repository, known before it is
    /// made so a hook in its configuration can name a file in it.
    fn fixture_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("trellis-rpc-{name}-{}", std::process::id()))
    }

    #[test]
    fn after_run_interrupted_by_abort_runs_before_the_rollback_and_its_write_is_undone() {
        // The risk: an aborted run rolled back and wrote its trail, and a
        // hook relying on `after_run` (to release a lock, post a note) never
        // learned the run had ended (#138). And once it runs, it must run
        // before the rollback, or what it wrote in the worktree outlives the
        // run. The hook runs in a throwaway clone (ADR 0032, "Hooks"), so
        // it names the repository's file by absolute path: the one writer
        // whose bytes reach the worktree. Its stdin goes outside the
        // repository, where the rollback cannot touch it.
        let name = "after-run-abort";
        let hooked = fixture_dir(name).join("hooked.py");
        let payload = crate::testutil::scratch("rpc-after-run-abort-payload").join("payload.json");
        let script = if cfg!(windows) {
            format!(
                "more > {} & echo hooked> {}",
                payload.display(),
                hooked.display()
            )
        } else {
            format!(
                "cat > {}; echo hooked > {}",
                payload.display(),
                hooked.display()
            )
        };
        let (dir, spec) = fixture(
            name,
            &format!(
                "[control]\nmode = \"approve\"\n[hooks]\nafter_run = {}\n",
                sh(&script)
            ),
        );
        assert_eq!(
            dir.join("hooked.py"),
            hooked,
            "the hook names the fixture's file"
        );
        crate::testutil::trust(&dir);
        let c = Client::start(&dir);
        c.send(json!({"type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.send(json!({"id": "x", "type": "abort"}));
        let rs = c.finish();
        let end = of_type(&rs, "run_end")[0];
        assert_eq!(end["outcome"], "interrupted", "{rs:?}");
        assert_eq!(end["restored"], true, "{rs:?}");

        let raw = std::fs::read_to_string(&payload)
            .unwrap_or_else(|e| panic!("after_run did not run on the abort ({e}): {rs:?}"));
        let given: Value = serde_json::from_str(raw.trim())
            .unwrap_or_else(|e| panic!("after_run's stdin is not JSON ({e}): {raw:?}"));
        assert_eq!(given["reason"], "interrupted", "{given}");
        assert_eq!(given["converged"], false, "{given}");
        assert!(
            given["iterations"].is_null(),
            "not known, so not stated: {given}"
        );
        assert_eq!(given["run_id"], end["run_id"], "the trail's id: {given}");
        assert!(given["touched"].is_array(), "{given}");

        assert!(
            !hooked.exists(),
            "the hook's write outlived the run: it ran after the rollback, or the rollback missed it"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn after_run_interrupted_by_abort_is_bounded_by_the_hook_timeout() {
        // The risk: a hook that hangs holds an aborted run, and its
        // rollback, forever. `rpc` has no second Ctrl-C to stop it (a second
        // `abort` asks for what was already asked), so `[hooks] timeout_s`
        // is the bound.
        //
        // What the timeout controls is the hook's lifetime, so that is what
        // is asserted: the hook marks its start, sleeps 30 s, then marks
        // that the sleep finished. Without the timeout the run waits for
        // the hook, so the second marker exists by the time the run ends,
        // however fast or slow the host is. With it, the hook is killed
        // mid-sleep and the marker is never written. A wall-clock bound on
        // the run measured host load as much as the timeout (#207). The
        // markers live outside the repository, where the rollback cannot
        // touch them; the hook runs in a throwaway clone (ADR 0032), so it
        // names them by absolute path.
        let marks = crate::testutil::scratch("rpc-after-run-timeout-marks");
        let (started, slept) = (marks.join("started"), marks.join("slept"));
        let script = if cfg!(windows) {
            format!(
                "echo started> {} & ping -n 31 127.0.0.1 >NUL & echo slept> {}",
                started.display(),
                slept.display()
            )
        } else {
            format!(
                "echo started > {}; sleep 30; echo slept > {}",
                started.display(),
                slept.display()
            )
        };
        let (dir, spec) = fixture(
            "after-run-timeout",
            &format!(
                "[control]\nmode = \"approve\"\n[hooks]\ntimeout_s = 1\nafter_run = {}\n",
                sh(&script)
            ),
        );
        crate::testutil::trust(&dir);
        let c = Client::start(&dir);
        c.send(json!({"type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.send(json!({"id": "x", "type": "abort"}));
        let rs = c.finish();
        let end = of_type(&rs, "run_end")[0];
        assert_eq!(end["outcome"], "interrupted", "{rs:?}");
        assert_eq!(end["restored"], true, "{rs:?}");
        assert!(
            started.exists(),
            "after_run never started, so nothing here tested its timeout: {rs:?}"
        );
        assert!(
            !slept.exists(),
            "the run waited out the 30 s hook: it was not stopped at its 1 s timeout"
        );
    }

    #[test]
    fn end_of_input_with_a_question_open_declines_it_and_the_process_exits() {
        // The risk: the client closed stdin while an approve-mode run
        // waited on a person. The server joined the run, the run waited on
        // a reply channel the server still held, and nothing ever ended.
        let (dir, spec) = fixture("eof-approval", "[control]\nmode = \"approve\"\n");
        let mut c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        drop(c.tx.take());
        let server = c.server.take().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(server.join());
        });
        done_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the server returns at end of input")
            .unwrap()
            .unwrap();
        let rs = c.records();
        let end = of_type(&rs, "run_end")[0];
        // Declined, then `done` with nothing checked: not converged, and
        // the edit never landed.
        assert_eq!(end["outcome"], "not_converged", "{rs:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
    }

    #[test]
    fn a_line_that_is_not_utf8_mid_run_is_a_parse_error_and_the_run_goes_on() {
        // The risk: `line?` returned from `serve` with a run in flight, and
        // the process exited with no rollback and no trail (review round 8,
        // finding 4). A line that is not UTF-8 is bad input, answered as
        // bad JSON is; the question is still open and still answerable.
        let (dir, spec) = fixture("not-utf8", "[control]\nmode = \"approve\"\n");
        let c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.raw(vec![b'{', 0xff, 0xfe, b'}']);
        let parse = c.wait_for(|r| r["command"] == "parse");
        assert_eq!(parse["success"], false);
        c.send(json!({"id": "approval-1", "type": "approve", "yes": false}));
        let rs = c.finish();
        let end = of_type(&rs, "run_end")[0];
        assert_eq!(end["outcome"], "not_converged", "{rs:?}");
        assert!(end["run_id"].is_string(), "a trail was written: {rs:?}");
    }

    #[test]
    fn a_read_error_mid_run_ends_input_and_the_run_still_ends_and_rolls_back() {
        // Any other read error is the end of input, handled as end of input
        // is: open questions declined, the run joined, then the error.
        let (dir, spec) = fixture("read-error", "[control]\nmode = \"approve\"\n");
        let mut c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.tx.as_ref().unwrap().send(Chunk::Fail).unwrap();
        let server = c.server.take().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(server.join());
        });
        let r = done_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the server returns once the run has ended")
            .unwrap();
        let rs = c.records();
        let ends = of_type(&rs, "run_end");
        assert!(
            !ends.is_empty(),
            "the server returned with the run still in flight: {rs:?}"
        );
        assert_eq!(ends[0]["outcome"], "not_converged", "{rs:?}");
        assert!(ends[0]["run_id"].is_string(), "a trail was written");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        let e = r.expect_err("the read error is still reported");
        assert!(format!("{e:#}").contains("pipe broke"), "{e:#}");
    }

    #[test]
    fn a_dirty_worktree_is_refused_as_trellis_run_refuses_it() {
        let (dir, spec) = fixture("dirty", "");
        std::fs::write(dir.join("a.py"), "def f():\n    return 9\n").unwrap();
        let c = Client::start(&dir);
        c.send(json!({"type": "prompt", "task": "change f", "model": spec}));
        let rs = c.finish();
        let end = of_type(&rs, "run_end")[0];
        assert_eq!(end["outcome"], "error");
        assert!(
            end["error"]
                .as_str()
                .unwrap()
                .contains("clean git worktree"),
            "{end}"
        );
    }

    #[test]
    fn bad_input_gets_a_response_and_the_server_keeps_going() {
        let d = tempfile::tempdir().unwrap();
        let c = Client::start(d.path());
        c.raw(b"{not json".to_vec());
        c.send(json!({"id": "a", "type": "steer"}));
        c.send(json!({"id": "b", "type": "compact"}));
        c.send(json!({"id": "c", "type": "approve", "yes": true}));
        c.send(json!({"id": "d", "type": "set_model", "model": "nonsense"}));
        c.send(json!({"id": "e", "type": "get_commands"}));
        c.send(json!({"id": "f", "type": "abort"}));
        let rs = c.finish();
        let by = |id: &str| rs.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(rs[0]["command"], "parse");
        assert_eq!(by("a")["success"], false);
        assert_eq!(by("b")["error"], "no run in flight");
        assert_eq!(by("c")["success"], false, "no such approval is open");
        assert_eq!(by("d")["success"], false);
        let cmds = by("e");
        assert_eq!(
            cmds["data"]["commands"].as_array().unwrap().len(),
            COMMANDS.len()
        );
        assert_eq!(by("f")["success"], false, "nothing to abort");
    }

    #[test]
    fn get_queue_outside_a_repository_answers_the_queue_s_own_error() {
        // The risk: a root with no git directory answered as an empty queue,
        // which a client would show as "nothing queued".
        let d = tempfile::tempdir().unwrap();
        let c = Client::start(d.path());
        c.send(json!({"id": "q", "type": "get_queue"}));
        let rs = c.finish();
        let r = rs.iter().find(|r| r["id"] == "q").unwrap();
        assert_eq!(r["success"], false, "{r}");
        let want = trellis_queue::Queue::for_repo(d.path())
            .unwrap_err()
            .to_string();
        assert_eq!(r["error"], want.as_str());
    }

    #[test]
    fn compact_during_a_scripted_run_is_refused_rather_than_left_waiting() {
        // The risk: a scripted driver renders no request, so a `compact`
        // handed to it would wait for one that never comes, and the client
        // with it. It is answered while the run is still paused.
        let (dir, spec) = fixture("compact-scripted", "[control]\nmode = \"approve\"\n");
        let c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.send(json!({"id": "c", "type": "compact"}));
        let r = c.wait_for(|r| r["id"] == "c");
        assert_eq!(r["success"], false);
        assert!(r["error"].as_str().unwrap().contains("scripted"), "{r}");
        c.send(json!({"id": "approval-1", "type": "approve", "yes": false}));
        let rs = c.finish();
        assert!(!of_type(&rs, "run_end").is_empty());
    }

    #[test]
    fn rpc_steer_abort_answers_with_the_undelivered_text_in_order() {
        // The risk: text a person typed lost with the aborted run. The run's
        // transcript is dropped on abort, so the answer has to come from the
        // server's own record, and a steer that never reached a request must
        // be told so rather than left waiting.
        let (dir, spec) = fixture("steer-abort", "[control]\nmode = \"approve\"\n");
        let c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "approval_request");
        c.send(json!({"id": "u1", "type": "follow_up", "message": "first"}));
        c.send(json!({"id": "s1", "type": "steer", "message": "second"}));
        c.send(json!({"id": "u2", "type": "follow_up", "message": "third"}));
        c.send(json!({"id": "bad", "type": "steer", "message": "  "}));
        c.send(json!({"id": "x", "type": "abort"}));
        let rs = c.finish();
        let by = |id: &str| {
            rs.iter()
                .find(|r| r["type"] == "response" && r["id"] == id)
                .unwrap_or_else(|| panic!("no answer to {id}: {rs:?}"))
                .clone()
        };
        let x = by("x");
        assert_eq!(x["success"], true, "{x}");
        assert_eq!(
            x["data"]["undelivered"],
            json!([
                {"id": "u1", "type": "follow_up", "message": "first"},
                {"id": "s1", "type": "steer", "message": "second"},
                {"id": "u2", "type": "follow_up", "message": "third"},
            ]),
            "{x}"
        );
        assert_eq!(by("s1")["success"], false);
        assert_eq!(by("s1")["error"], STEER_ENDED);
        assert_eq!(by("bad")["success"], false, "an empty steer is refused");
        // Stopped: no follow-up ran after the abort.
        let starts = of_type(&rs, "run_start");
        assert_eq!(starts.len(), 1, "{rs:?}");
        assert_eq!(of_type(&rs, "run_end")[0]["outcome"], "interrupted");
        let abort_at = rs.iter().position(|r| r["id"] == "x").unwrap();
        let end_at = rs.iter().position(|r| r["type"] == "run_end").unwrap();
        assert!(
            end_at < abort_at,
            "abort is answered once the run has ended"
        );
    }

    #[test]
    fn quit_ends_the_server_even_with_input_left() {
        let d = tempfile::tempdir().unwrap();
        let c = Client::start(d.path());
        c.send(json!({"id": "q", "type": "quit"}));
        c.wait_for(|r| r["id"] == "q");
        // Never closed: `quit` alone must end `serve`.
        let server = {
            let mut c = c;
            c.server.take().unwrap()
        };
        server.join().unwrap().unwrap();
    }

    #[test]
    fn turn_events_are_captured_for_get_messages_and_other_events_are_not() {
        use tracing_subscriber::layer::SubscriberExt;
        let store: Arc<Mutex<Vec<Value>>> = Arc::default();
        let layer = TurnCapture {
            store: store.clone(),
        };
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(turn = 1u64, call = "edit a.py", verdict = "None", "turn");
            tracing::warn!(repeated_call = "x", repeats = 3u64, "no progress; stopping");
        });
        let got = store.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["call"], "edit a.py");
        assert_eq!(got[0]["turn"], 1);
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn a_turn_s_signed_and_boolean_fields_reach_get_messages() {
        // The risk: a field of a type the visitor dropped is missing from
        // `get_messages` with nothing to say it was there.
        use tracing_subscriber::layer::SubscriberExt;
        let store: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sub = tracing_subscriber::registry().with(TurnCapture {
            store: store.clone(),
        });
        tracing::subscriber::with_default(sub, || {
            tracing::info!(turn = 2u64, call = "x", delta = -3i64, wrote = true, "turn");
        });
        let got = store.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["delta"], -3);
        assert_eq!(got[0]["wrote"], true);
    }

    #[test]
    fn watched_passes_the_driver_s_figures_through() {
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let w = Watched::new(&mut inner, None);
        assert_eq!(w.tokens_used(), 7);
        assert_eq!(w.prompt_bytes(), Some(4321));
        assert_eq!(w.compactions().len(), 1);
        assert_eq!(w.compactions()[0].elided_bytes, 50);
        assert_eq!(w.requested_compactions().len(), 1);
        assert_eq!(w.requested_compactions()[0].elided_bytes, 60);
    }

    #[test]
    fn watched_forwards_step_usage() {
        // The risk (#256): every RPC run drives its model through
        // `Watched`, so a default answer here would drop the step's figures
        // from every `turn` event a client is sent.
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let w = Watched::new(&mut inner, None);
        assert_eq!(
            w.step_usage(),
            trellis_loop::StepUsage {
                completion_tokens: Some(13),
                queue_wait_ms: Some(250),
            }
        );
    }

    #[test]
    fn a_prompt_without_a_task_or_with_args_and_no_template_says_which() {
        let (dir, spec) = fixture("prompt-shape", "");
        let c = Client::start(&dir);
        c.send(json!({"id": "a", "type": "prompt", "task": "t", "args": "x", "model": spec}));
        c.send(json!({"id": "b", "type": "prompt", "task": "  ", "model": spec}));
        let rs = c.finish();
        let by = |id: &str| rs.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(
            by("a")["error"],
            "\"args\" needs \"prompt\", the template they fill"
        );
        assert_eq!(
            by("b")["error"],
            "prompt needs \"task\", \"prompt\", or both"
        );
        assert!(of_type(&rs, "run_start").is_empty(), "{rs:?}");
    }

    #[test]
    fn get_state_after_a_run_says_it_is_over_and_how_it_ended() {
        // The risk: `last_run` empty, or the finished run still reported
        // as running, so a client waits on a run that will never end.
        let (dir, spec) = fixture("state", "");
        let c = Client::start(&dir);
        c.send(json!({"id": "p", "type": "prompt", "task": "change f", "model": spec}));
        c.wait_for(|r| r["type"] == "run_end");
        let until = Instant::now() + Duration::from_secs(60);
        let state = (0..)
            .map(|i| {
                let id = format!("s{i}");
                c.send(json!({"id": id, "type": "get_state"}));
                c.wait_for(|r| r["id"] == id.as_str())
            })
            .find(|s| {
                assert!(Instant::now() < until, "still running: {s}");
                s["data"]["running"] == false || {
                    std::thread::sleep(Duration::from_millis(20));
                    false
                }
            })
            .unwrap();
        let last = &state["data"]["last_run"];
        assert_eq!(last["outcome"], "not_converged", "{state}");
        assert_eq!(last["id"], "p", "{state}");
        assert!(last.get("type").is_none(), "{state}");
        c.finish();
    }
}
