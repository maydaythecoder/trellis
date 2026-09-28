//! `trellis rpc`: JSONL commands on stdin, responses, framing and events on
//! stdout (ADR 0025 §5).
//!
//! A `prompt` runs a task through `cmd_run_with`, the path `trellis run`
//! takes, on its own thread. The main thread keeps reading, so `abort`,
//! `approve` and `get_state` are answered while the run is in flight. One
//! run at a time; the clean-worktree rule is `cmd_run`'s and is unchanged.

use crate::output::{self, Sink, frame};
use crate::{Human, RunControl, RunExit, RunSeeding, config};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

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

/// A run in flight.
struct Running {
    thread: std::thread::JoinHandle<()>,
    abort: Arc<output::Abort>,
    /// Set by the run's thread as it finishes.
    done: Arc<AtomicBool>,
    id: Option<String>,
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
}

/// Reads commands until `quit` or end of input.
///
/// End of input waits for a run in flight to finish, so
/// `printf '{"type":"prompt",...}\n' | trellis rpc` runs one task to
/// completion. `quit` aborts it instead.
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
    };
    for line in input.lines() {
        let line = line?;
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
    if let Some(r) = s.running.take() {
        let _ = r.thread.join();
    }
    Ok(())
}

impl Server {
    fn respond(&self, id: Option<&str>, command: &str, result: Result<Value, String>) {
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
        frame(&self.sink, &v);
    }

    fn in_flight(&mut self) -> bool {
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
            "abort" => {
                let r = if self.in_flight() {
                    self.abort_run();
                    Ok(Value::Null)
                } else {
                    Err("no run in progress".to_string())
                };
                self.respond(id, kind, r);
            }
            "get_state" => {
                let running = self.in_flight();
                let pending: Vec<String> = self
                    .approvals
                    .lock()
                    .map(|a| a.keys().cloned().collect())
                    .unwrap_or_default();
                let data = json!({
                    "running": running,
                    "run_request_id": self.running.as_ref().and_then(|r| r.id.clone()),
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
                let prompts: Vec<Value> = crate::templates::list(&self.root)
                    .into_iter()
                    .map(|t| {
                        json!({"name": t.name, "description": t.description,
                               "argument_hint": t.argument_hint})
                    })
                    .collect();
                let skills: Vec<Value> = trellis_loop::SkillCatalog::discover(&self.root)
                    .valid
                    .into_iter()
                    .map(|s| json!({"name": s.name, "description": s.description}))
                    .collect();
                self.respond(
                    id,
                    kind,
                    Ok(json!({"commands": commands, "prompts": prompts, "skills": skills})),
                );
            }
            "quit" => {
                self.abort_run();
                if let Some(r) = self.running.take() {
                    let _ = r.thread.join();
                }
                self.respond(id, kind, Ok(Value::Null));
                return false;
            }
            "compact" => self.respond(
                id,
                kind,
                Err("not built: compaction is W-C (ADR 0024); each prompt is a fresh run".into()),
            ),
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

        if let Ok(mut t) = TURNS.lock() {
            t.clear();
        }
        let abort = Arc::new(output::Abort::default());
        let done = Arc::new(AtomicBool::new(false));
        let run_done = done.clone();
        let request_id = id.map(str::to_string);
        let (root, sink, mode) = (self.root.clone(), self.sink.clone(), self.mode);
        let (approvals, next, last) = (
            self.approvals.clone(),
            self.next_approval.clone(),
            self.last.clone(),
        );
        let run_abort = abort.clone();
        let rid = request_id.clone();
        let thread = std::thread::spawn(move || {
            // Approval questions become records on the stream; answers come
            // back through `approve` and the stored reply channel.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<trellis_loop::ApprovalRequest>(1);
            let fwd = {
                let (sink, approvals, rid) = (sink.clone(), approvals.clone(), rid.clone());
                let aborted = run_abort.clone();
                std::thread::spawn(move || {
                    while let Some(req) = rx.blocking_recv() {
                        let n = next.fetch_add(1, Ordering::SeqCst) + 1;
                        let aid = format!("approval-{n}");
                        if let Ok(mut a) = approvals.lock() {
                            // Checked under the lock `abort_run` clears
                            // under: a question arriving after the run took
                            // the abort path is dropped here, because the
                            // clear may already have happened.
                            if aborted.observed() {
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
            let mut human: Box<dyn std::io::Write + Send> = match mode {
                Mode::Process => output::human(),
                #[cfg(test)]
                Mode::Quiet => Box::new(std::io::sink()),
            };
            let before = crate::output::RunExtras {
                skills,
                request_id: rid.clone(),
                frames: Some(sink),
                abort: Some(run_abort),
            };
            let r = crate::cmd_run_with(
                &root,
                &task,
                &model,
                unconstrained,
                // No agent profile over RPC: the caller is the operator, and
                // profiles are how `work` runs a task as someone (ADR 0026).
                RunSeeding {
                    resume: false,
                    focus: focus.as_deref(),
                    profile: None,
                },
                RunControl {
                    flag,
                    human: Human::Channel(tx),
                },
                before,
                &mut human,
            );
            let _ = fwd.join();
            // Questions still open when the run ended are dropped, which the
            // gate reads as a no; none can be answered now.
            if let Ok(mut a) = approvals.lock() {
                a.clear();
            }
            if let Ok(mut l) = last.lock() {
                *l = Some(summary(&r, rid.as_deref()));
            }
            run_done.store(true, Ordering::SeqCst);
        });
        self.running = Some(Running {
            thread,
            abort,
            done,
            id: request_id,
        });
        Ok(())
    }
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

    /// A reader fed line by line, so a test can answer what the run asks.
    struct Feed {
        rx: mpsc::Receiver<String>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for Feed {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(line) => {
                        self.buf = format!("{line}\n").into_bytes();
                        self.pos = 0;
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
        tx: Option<mpsc::Sender<String>>,
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
            self.tx.as_ref().unwrap().send(v.to_string()).unwrap();
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
        c.tx.as_ref().unwrap().send("{not json".into()).unwrap();
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
        assert!(by("b")["error"].as_str().unwrap().contains("not built"));
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
}
