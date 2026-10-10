//! The desktop's core and the page's four commands (ADR 0048 sections 2, 3
//! and 5).
//!
//! The core owns one `trellis rpc` child per repository and the reducer its
//! lines feed (`state.rs`). What the child prints reaches the page as events:
//!
//! - `rpc-frame`: a stdout line that is a JSON object, parsed;
//! - `rpc-unparsed`: any other stdout line, as a string;
//! - `rpc-human`: a stderr line, as a string, shown and never read as data;
//! - `view`: the panes that line changed, one event per line; after a
//!   forwarded `get_state`, every pane;
//! - `child-started`: `trellis`, `version` and `root`, once per child;
//! - `rpc-exited`: the child's exit status;
//! - `no-child`: why no child is running, at startup;
//! - `overview`: with no root, the overview, once at startup
//!   (`overview-error` says why when it could not be read);
//! - `codebase`: the codebase view's layout (`codebase.rs`), once per
//!   `get_codebase` answer, or `{"error"}` with trellis's reason when it
//!   was refused (#307);
//! - `radius`: a `get_radius` answer as reach and doubt (`radius.rs`), or
//!   `{"error"}`, with the answer's `id` (#308).
//!
//! The first line the core writes to every child is
//! `{"id":"desktop-models","type":"get_available_models"}`. Its answer's
//! `configured` specs are recorded in the view state and read by
//! `rpc_send`'s model rule (ADR 0048 section 4, #305), which names no model
//! until that answer has come. The answer still reaches the page as an
//! `rpc-frame`, as every stdout object does: it is the child's own report,
//! it holds no key, and the model picker needs the user's configured spec
//! to preselect it (section 5, amended). The page may not send a line with
//! that id, so nothing it asks for can be taken as the child's report.
//!
//! Nothing restarts a child that exited, whether on `quit`, a crash or a
//! broken pipe: the page is told, and `rpc_send` refuses until
//! `open_repository` starts another. The page cannot send `quit`; only the
//! core does, and it then waits (ADR 0025 §5).
//!
//! Everything here but the four `#[tauri::command]` functions is plain Rust
//! over a [`Sink`], so the tests drive it without a window.

use crate::child::{Child, Lines, Output, Trellis};
use crate::codebase;
use crate::frames::{self, Frame};
use crate::radius;
use crate::state::{Changed, Configured, DesktopState, MODELS_ID};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The longest line `rpc_send` forwards, in bytes as written.
pub const LINE_CAP: usize = 65_536;

/// The `type`s `rpc_send` forwards: `rpc.rs`'s `COMMANDS` as #296 lists
/// them, without `quit`, which only the core sends, and with each read-only
/// command a desktop view reads through (ADR 0048 section 10) as that view
/// is built: `get_codebase` (#307), `get_radius` (#308).
pub const FORWARDED: [&str; 13] = [
    "prompt",
    "abort",
    "get_state",
    "get_messages",
    "set_model",
    "get_available_models",
    "approve",
    "steer",
    "follow_up",
    "compact",
    "get_commands",
    "get_codebase",
    "get_radius",
];

/// The most trails `get_trails` asks for.
pub const TRAILS_MAX: u32 = 200;

/// Where the core's events go: the window, or a test's list.
pub trait Sink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
}

impl Sink for tauri::AppHandle {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(e) = tauri::Emitter::emit(self, event, payload) {
            eprintln!("trellis-desktop: cannot emit {event}: {e}");
        }
    }
}

/// The core: the window's managed state. Cloned into reader threads.
#[derive(Clone)]
pub struct Core {
    trellis: Trellis,
    sink: Arc<dyn Sink>,
    session: Arc<Mutex<Session>>,
}

struct Session {
    /// The repository the core has open; `None` before one is found.
    root: Option<PathBuf>,
    /// The running child and its generation; `None` while there is none.
    child: Option<(u64, Child)>,
    /// Why there is no child, said when `rpc_send` refuses.
    why_none: String,
    /// Bumped per child, so lines and exits of a child already replaced
    /// touch nothing.
    generation: u64,
    view: DesktopState,
    /// Set by a forwarded `get_state`: the next `view` carries every pane.
    resend: bool,
}

impl Core {
    pub fn new(trellis: Trellis, sink: Arc<dyn Sink>) -> Self {
        Self {
            trellis,
            sink,
            session: Arc::new(Mutex::new(Session {
                root: None,
                child: None,
                why_none: "trellis-desktop is still starting".into(),
                generation: 0,
                view: DesktopState::default(),
                resend: false,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the window does once: opens the startup root, or with none
    /// starts no child and emits the overview.
    pub fn startup(&self, explicit: Option<PathBuf>, cwd: &Path) {
        match startup_root(explicit, cwd) {
            Ok(root) => {
                if let Err(why) = self.start(&root) {
                    self.sink.emit("no-child", json!(why));
                }
            }
            Err(why) => {
                self.lock().why_none = why.clone();
                self.sink.emit("no-child", json!(why));
                match self.trellis.overview(None, cwd) {
                    Ok(v) => self.sink.emit("overview", v),
                    Err(e) => self.sink.emit("overview-error", json!(format!("{e}"))),
                }
            }
        }
    }

    /// Starts a child on `root`, its two readers, and says so with
    /// `child-started`. The view is cleared: its panes were another
    /// repository's.
    fn start(&self, root: &Path) -> Result<(), String> {
        let version = self.trellis.version().ok();
        let (child, out) = match self.trellis.spawn_rpc(root) {
            Ok(started) => started,
            Err(e) => {
                let why = format!(
                    "could not start `{} --root {} rpc`: {e}",
                    self.trellis.bin().display(),
                    root.display()
                );
                self.lock().why_none = why.clone();
                return Err(why);
            }
        };
        // The first line this child reads, written before the child is
        // stored, so no page line can come ahead of it: until it is
        // answered `rpc_send` names no model (#305, ADR 0048 section 4).
        // A child that cannot read it is left to the reader, which sees its
        // stdout end; the guard stays shut meanwhile.
        if let Err(e) = child.send(&models_question()) {
            eprintln!("trellis-desktop: cannot ask trellis rpc for its models: {e}");
        }
        let generation = {
            let mut s = self.lock();
            s.generation += 1;
            s.root = Some(root.to_path_buf());
            s.child = Some((s.generation, child));
            // This child's answer, not the last one's, opens the guard.
            s.view.forget_models();
            let c = s.view.clear();
            self.sink.emit("view", s.view.view(c));
            s.generation
        };
        self.sink.emit(
            "child-started",
            json!({
                "trellis": self.trellis.bin().display().to_string(),
                "version": version,
                "root": root.display().to_string(),
            }),
        );
        self.read(generation, out);
        Ok(())
    }

    /// One thread per stream, until each ends; stdout's end is the child's
    /// exit.
    fn read(&self, generation: u64, out: Output) {
        let Output { stdout, stderr } = out;
        let core = self.clone();
        std::thread::spawn(move || {
            core.drain(stdout, |c, line| c.stdout_line(generation, &line));
            core.stdout_ended(generation);
        });
        let core = self.clone();
        std::thread::spawn(move || {
            core.drain(stderr, |c, line| c.stderr_line(generation, line));
        });
    }

    fn drain<R: io::Read>(&self, mut lines: Lines<R>, each: impl Fn(&Self, String)) {
        loop {
            match lines.next_line() {
                Ok(Some(line)) => each(self, line),
                Ok(None) => return,
                Err(e) => {
                    eprintln!("trellis-desktop: cannot read trellis rpc: {e}");
                    return;
                }
            }
        }
    }

    /// One stdout line: `rpc-frame` or `rpc-unparsed`, then its `view`.
    fn stdout_line(&self, generation: u64, line: &str) {
        let mut s = self.lock();
        if s.generation != generation {
            return;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v @ Value::Object(_)) => self.sink.emit("rpc-frame", v),
            _ => self.sink.emit("rpc-unparsed", json!(line)),
        }
        let frame = frames::parse_stdout(line);
        // The read-only views' answers are typed here, not in the page
        // (ADR 0048 section 5), whichever line asked for them.
        if let Frame::Response(r) = &frame {
            match r.command.as_deref() {
                Some("get_codebase") => self.sink.emit("codebase", codebase::event(r)),
                Some("get_radius") => self.sink.emit("radius", radius::event(r)),
                _ => {}
            }
        }
        self.feed(&mut s, frame);
    }

    /// One stderr line: `rpc-human`, then its `view`.
    fn stderr_line(&self, generation: u64, line: String) {
        let mut s = self.lock();
        if s.generation != generation {
            return;
        }
        self.sink.emit("rpc-human", json!(line));
        self.feed(&mut s, Frame::Human(line));
    }

    /// The reducer, and one `view` event for what it changed: every pane
    /// when a forwarded `get_state` asked for it, nothing when nothing
    /// changed.
    fn feed(&self, s: &mut Session, frame: Frame) {
        let changed = s.view.update(frame);
        let changed = if std::mem::take(&mut s.resend) {
            Changed::everything()
        } else {
            changed
        };
        if !changed.is_empty() {
            self.sink.emit("view", s.view.view(changed));
        }
    }

    /// The child's stdout has ended: it has exited, or is about to. If it is
    /// still this core's child, it is waited for and the page told; if the
    /// core has already taken it (shutdown, a new repository), whoever took
    /// it waits.
    fn stdout_ended(&self, generation: u64) {
        let child = {
            let mut s = self.lock();
            match s.child.take() {
                Some((g, c)) if g == generation => c,
                other => {
                    s.child = other;
                    return;
                }
            }
        };
        let status = child.wait();
        self.exited(generation, status);
    }

    /// Tells the page a child has exited, and stops counting a run in
    /// flight for it.
    fn exited(&self, generation: u64, status: io::Result<ExitStatus>) {
        let status = match status {
            Ok(st) => json!({"code": st.code(), "success": st.success()}),
            Err(e) => json!({"code": null, "success": false, "error": e.to_string()}),
        };
        let mut s = self.lock();
        self.sink.emit("rpc-exited", status.clone());
        if s.generation == generation {
            s.why_none =
                format!("trellis rpc exited ({status}); open a repository to start it again");
            let c = s.view.child_exited();
            self.sink.emit("view", s.view.view(c));
        }
    }

    /// Takes the child out, so it can be shut down outside the lock.
    pub fn take_child(&self, why: &str) -> Option<(u64, Child)> {
        let mut s = self.lock();
        let taken = s.child.take();
        if taken.is_some() {
            s.why_none = why.to_string();
        }
        taken
    }

    /// Sends a child taken out `quit`, waits, and says it exited. Never a
    /// kill (ADR 0048 section 9).
    pub fn shut_down(&self, taken: (u64, Child)) -> io::Result<ExitStatus> {
        let (generation, child) = taken;
        let status = child.shutdown();
        let report = match &status {
            Ok(st) => Ok(*st),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        };
        self.exited(generation, report);
        status
    }

    /// `rpc_send`: checks the line, then the model it names against the
    /// child's answer, then forwards it to the child.
    pub fn send(&self, line: &Value) -> Result<(), String> {
        check_line(line)?;
        let writer = {
            let mut s = self.lock();
            check_model(line, s.view.configured())?;
            let Some((_, child)) = s.child.as_ref() else {
                return Err(format!(
                    "rpc_send: no trellis rpc child is running: {}",
                    s.why_none
                ));
            };
            let w = child.writer();
            // Set before the line is written, so the reply, which a reader
            // thread may feed as soon as it is written, finds it set.
            if line.get("type").and_then(Value::as_str) == Some("get_state") {
                s.resend = true;
            }
            w
        };
        // Outside the lock: the readers take it for every line the child
        // prints, and a child blocked on a full stdout reads no stdin.
        writer
            .send(line)
            .map_err(|e| format!("rpc_send: cannot write to trellis rpc: {e}"))
    }

    /// `get_trails`: the log list once, its stdout lines uninterpreted.
    pub fn trails(&self, limit: u32) -> Result<Vec<String>, String> {
        if !(1..=TRAILS_MAX).contains(&limit) {
            return Err(format!(
                "get_trails: limit must be 1 to {TRAILS_MAX}; got {limit}"
            ));
        }
        let root = self.lock().root.clone();
        let root = root.ok_or("get_trails: no repository is open")?;
        self.trellis
            .log(&root, limit)
            .map_err(|e| format!("get_trails: {e}"))
    }

    /// `get_overview`: the overview list once, `--root` only with a root.
    /// `cwd` is where a root-less overview runs.
    pub fn overview(&self, cwd: &Path) -> Result<Value, String> {
        let root = self.lock().root.clone();
        self.trellis
            .overview(root.as_deref(), cwd)
            .map_err(|e| format!("get_overview: {e}"))
    }

    /// `open_repository`: refused for a path with no `.git` entry and while
    /// a run is in flight; otherwise the current child is shut down and one
    /// started on `path`.
    pub fn open(&self, path: &Path) -> Result<(), String> {
        if !path.join(".git").exists() {
            return Err(format!(
                "open_repository: {} is not a repository: it has no .git entry",
                path.display()
            ));
        }
        if self.lock().view.running() {
            return Err(
                "open_repository: a run is in flight; it must end before another repository \
                 is opened"
                    .into(),
            );
        }
        if let Some(taken) = self.take_child("trellis-desktop is opening another repository")
            && let Err(e) = self.shut_down(taken)
        {
            return Err(format!("open_repository: shutting down trellis rpc: {e}"));
        }
        self.start(path)
    }
}

/// The repository the window opens: `--root` as given, else `cwd` or the
/// nearest parent holding a `.git` entry. `Err` carries what the page shows
/// when there is none.
pub fn startup_root(explicit: Option<PathBuf>, cwd: &Path) -> Result<PathBuf, String> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "No repository was found at {} or above it, so trellis was not started. \
                 Start trellis-desktop inside a repository, or name one with --root <repo>.",
                cwd.display()
            )
        })
}

/// What `rpc_send` checks before it reads the session, each refusal naming
/// its rule: a JSON object, a `type` it forwards, the size cap, and an id
/// that is not the core's own. The model rule, which needs the child's
/// answer, is [`check_model`].
fn check_line(line: &Value) -> Result<(), String> {
    let Some(o) = line.as_object() else {
        return Err("rpc_send: the line is not a JSON object".into());
    };
    match o.get("type").and_then(Value::as_str) {
        Some(t) if FORWARDED.contains(&t) => {}
        Some(t) => {
            return Err(format!(
                "rpc_send: type {t:?} is not one rpc_send forwards ({})",
                FORWARDED.join(", ")
            ));
        }
        None => return Err("rpc_send: the line has no string type".into()),
    }
    let len = serde_json::to_string(line).map_or(usize::MAX, |s| s.len());
    if len > LINE_CAP {
        return Err(format!(
            "rpc_send: the line is {len} bytes, over the {LINE_CAP}-byte cap"
        ));
    }
    // The core's own question: an answer to a page line carrying this id
    // would be read as the child's report of the configured specs.
    if o.get("id").and_then(Value::as_str) == Some(MODELS_ID) {
        return Err(format!(
            "rpc_send: the id {MODELS_ID:?} is the desktop's own; use another"
        ));
    }
    Ok(())
}

/// The core's question to a new child, its first line.
fn models_question() -> Value {
    json!({"id": MODELS_ID, "type": "get_available_models"})
}

/// The schemes `trellis-model`'s `spec.rs` parses as a hosted endpoint that
/// `hosted::resolve_with` gives the protocol's default key once the user
/// chose it. `openai:` is a runtime alias and gets no key.
const HOSTED_SCHEMES: [&str; 2] = ["openai-chat", "anthropic"];

/// The model rule (ADR 0048 section 4, ADR 0024 §3). `rpc.rs` counts a
/// model the client names, a prompt's `model` or `set_model`, as the user's
/// choice, and `resolve_with` then sends a hosted endpoint the user's key.
/// So a hosted spec is forwarded only when it is byte-equal to
/// `configured.user`, the spec in the user's own config. Byte-equal and not
/// parsed: `resolve_with` compares parsed specs, so a reformatted copy of
/// the repository's spec (a trailing slash, an upper-case scheme) would
/// pass any looser check. A `provider:` spec passes, because `resolve_with`
/// holds a repository-defined provider to the same check whoever named it;
/// runtime and `scripted:` specs get no key. Until the child has answered
/// `desktop-models`, no model is named at all.
fn check_model(line: &Value, configured: Option<&Configured>) -> Result<(), String> {
    let ty = line.get("type").and_then(Value::as_str);
    let spec = match (ty, line.get("model")) {
        // A prompt without a model, or with `null`, runs the repository
        // default: rpc reads only a string, and this names nothing.
        (Some("prompt"), None | Some(Value::Null)) => return Ok(()),
        (Some("prompt" | "set_model"), Some(Value::String(m))) => m,
        (Some("prompt"), Some(_)) => {
            return Err(
                "rpc_send: the model rule: a prompt's model is a spec string, or null for the \
                 repository default"
                    .into(),
            );
        }
        (Some("set_model"), _) => {
            return Err("rpc_send: the model rule: set_model needs a model spec string".into());
        }
        _ => return Ok(()),
    };
    let Some(configured) = configured else {
        return Err(
            "rpc_send: the model rule: trellis rpc has not yet answered the desktop's \
             get_available_models, so the user's own configured spec is not known and no \
             set_model or prompt model is sent (ADR 0048 section 4); try again in a moment, \
             or pick \"repository default\""
                .into(),
        );
    };
    // The text before the first `:`, or all of it with none. Trimmed for
    // this test only, which can only widen what counts as hosted; the
    // comparison below is on the bytes as given.
    let scheme = spec.split(':').next().unwrap_or_default().trim();
    let hosted = HOSTED_SCHEMES
        .iter()
        .any(|h| scheme.eq_ignore_ascii_case(h));
    if hosted && configured.user.as_deref() != Some(spec.as_str()) {
        return Err(format!(
            "rpc_send: the model rule: {spec:?} is a hosted spec that is not the one in your own \
             trellis config, and trellis would send it your key as your choice (ADR 0024 §3). \
             Pick \"repository default\" to run the repository's model, or add the spec to your \
             own config ([model] spec in your user config.toml) to choose it yourself"
        ));
    }
    Ok(())
}

/// The page's command: forwards one checked line to the child.
#[tauri::command(async)]
pub fn rpc_send(line: Value, core: tauri::State<'_, Core>) -> Result<(), String> {
    core.send(&line)
}

/// The page's command: the newest `limit` trails, as `log --json` lines.
#[tauri::command(async)]
pub fn get_trails(limit: u32, core: tauri::State<'_, Core>) -> Result<Vec<String>, String> {
    core.trails(limit)
}

/// The page's command: opens another repository.
#[tauri::command(async)]
pub fn open_repository(path: String, core: tauri::State<'_, Core>) -> Result<(), String> {
    core.open(Path::new(&path))
}

/// The page's command: the overview, as one JSON object.
#[tauri::command(async)]
pub fn get_overview(core: tauri::State<'_, Core>) -> Result<Value, String> {
    let cwd = std::env::current_dir()
        .map_err(|e| format!("get_overview: cannot read the current directory: {e}"))?;
    core.overview(&cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::child::testing::{is_empty, scratch, trellis, trellis_bin};
    use std::time::{Duration, Instant};

    /// The events a core emitted, in order, in place of a window.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, Value)>>);

    impl Sink for Recorder {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.to_string(), payload));
        }
    }

    impl Recorder {
        fn take(&self) -> Vec<(String, Value)> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }

        fn named(&self, event: &str) -> Vec<Value> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(e, _)| e == event)
                .map(|(_, p)| p.clone())
                .collect()
        }

        /// Waits up to 60 s for an `event` whose payload passes `ok`.
        fn wait_for(&self, event: &str, ok: impl Fn(&Value) -> bool) -> Value {
            let until = Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(v) = self.named(event).into_iter().find(|v| ok(v)) {
                    return v;
                }
                assert!(
                    Instant::now() < until,
                    "no {event} event came: {:?}",
                    self.0.lock().unwrap()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// A core over a trellis that does not exist, so a test that must start
    /// no child shows it by having nothing to start.
    fn idle_core() -> (Core, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        let core = Core::new(Trellis::new(PathBuf::from("no-such-trellis")), rec.clone());
        (core, rec)
    }

    /// A core over the test trellis, reading the empty config `config`.
    fn core_with(config: &Path) -> (Core, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        (Core::new(trellis(config), rec.clone()), rec)
    }

    fn line(v: Value) -> String {
        v.to_string()
    }

    fn generation(core: &Core) -> u64 {
        core.lock().generation
    }

    fn no_child(err: &str) -> bool {
        err.contains("no trellis rpc child is running")
    }

    /// Shuts down whatever child the core has, as the window does.
    fn finish(core: &Core) -> Option<ExitStatus> {
        core.take_child("test over")
            .map(|t| core.shut_down(t).unwrap())
    }

    #[test]
    fn rpc_send_refuses_a_type_rpc_does_not_list() {
        // The risk: the page drives trellis past what rpc_send forwards: a
        // `quit` only the core may send, or a type rpc does not have that a
        // later trellis might give a meaning.
        let (core, rec) = idle_core();
        for ty in ["quit", "bash", "accept", "revert", "GET_STATE"] {
            let err = core.send(&json!({"id": "x", "type": ty})).unwrap_err();
            assert!(err.contains(&format!("{ty:?}")), "{ty}: {err}");
            assert!(err.contains("is not one rpc_send forwards"), "{ty}: {err}");
        }
        for not_object in [
            json!(["get_state"]),
            json!("get_state"),
            json!(null),
            json!(1),
        ] {
            let err = core.send(&not_object).unwrap_err();
            assert!(err.contains("not a JSON object"), "{not_object}: {err}");
        }
        let err = core.send(&json!({"id": "x", "type": 1})).unwrap_err();
        assert!(err.contains("no string type"), "{err}");
        // Every type it lists passes this rule, and so meets the next one.
        for ty in FORWARDED {
            let err = core.send(&json!({"id": "x", "type": ty})).unwrap_err();
            assert!(
                no_child(&err) || err.contains("the model rule"),
                "{ty}: {err}"
            );
        }
        assert!(rec.take().is_empty(), "a refusal emits nothing");
    }

    #[test]
    fn rpc_send_refuses_a_line_over_the_cap() {
        // The risk: one huge line from the page held in memory and written
        // to trellis whole.
        let (core, _rec) = idle_core();
        let sized = |n: usize| {
            let base = line(json!({"type": "steer", "message": ""}));
            json!({"type": "steer", "message": "m".repeat(n - base.len())})
        };
        assert_eq!(line(sized(LINE_CAP)).len(), LINE_CAP);
        let err = core.send(&sized(LINE_CAP + 1)).unwrap_err();
        assert!(err.contains("over the 65536-byte cap"), "{err}");
        assert!(err.contains(&format!("{}", LINE_CAP + 1)), "{err}");
        let err = core.send(&sized(LINE_CAP)).unwrap_err();
        assert!(
            no_child(&err),
            "at the cap it is not refused for size: {err}"
        );
    }

    #[test]
    fn rpc_send_refuses_while_no_child_is_running() {
        // The risk: a line the page sends after the child has gone is
        // dropped in silence, or something restarts the child behind the
        // page's back.
        let (core, _rec) = idle_core();
        let get_state = json!({"id": "desktop-1", "type": "get_state"});
        let err = core.send(&get_state).unwrap_err();
        assert!(no_child(&err), "{err}");

        let repo = scratch("nochild-repo");
        let config = scratch("nochild-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        assert_eq!(core.send(&get_state), Ok(()));
        finish(&core).unwrap();
        let err = core.send(&get_state).unwrap_err();
        assert!(no_child(&err), "{err}");
        assert!(err.contains("open a repository to start it again"), "{err}");
        assert_eq!(rec.named("child-started").len(), 1, "nothing restarted it");

        // A child that exits on its own, which the page did not ask for (a
        // quit written past rpc_send stands in for a crash): its reader
        // reaps it, the page is told, and nothing restarts it.
        core.start(&repo).unwrap();
        rec.take();
        let writer = core.lock().child.as_ref().unwrap().1.writer();
        writer.send(&json!({"type": "quit", "id": "gone"})).unwrap();
        rec.wait_for("rpc-exited", |_| true);
        let err = core.send(&get_state).unwrap_err();
        assert!(no_child(&err), "{err}");
        assert!(err.contains("open a repository to start it again"), "{err}");
        assert!(core.lock().child.is_none());
        assert!(
            rec.named("child-started").is_empty(),
            "nothing restarted it"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    /// The child's answer to the core's models question, as `rpc.rs`
    /// writes it, with `configured` as given.
    fn models_answer(configured: Value) -> String {
        line(
            json!({"type": "response", "command": "get_available_models",
                    "id": MODELS_ID, "success": true,
                    "data": {"catalog": [], "configured": configured}}),
        )
    }

    /// The two lines that name a model, for `spec`.
    fn naming(spec: Value) -> [Value; 2] {
        [
            json!({"id": "m", "type": "set_model", "model": spec}),
            json!({"id": "p", "type": "prompt", "task": "t", "model": spec}),
        ]
    }

    /// Whether the model rule let `l` through: the idle core then refuses
    /// it for having no child, the next rule.
    fn forwarded(core: &Core, l: &Value) -> bool {
        match core.send(l) {
            Err(e) if no_child(&e) => true,
            Err(e) => {
                assert!(e.contains("the model rule"), "{l}: {e}");
                false
            }
            Ok(()) => panic!("an idle core has no child to write to: {l}"),
        }
    }

    const REPO_SPEC: &str = "anthropic:https://example.invalid#m";
    const USER_SPEC: &str = "openai-chat:https://user.example.invalid/v1#gpt";

    /// An idle core whose child reported `repository` and `user`.
    fn answered(repository: Value, user: Value) -> (Core, Arc<Recorder>) {
        let (core, rec) = idle_core();
        let g = generation(&core);
        core.stdout_line(
            g,
            &models_answer(json!({"repository": repository, "user": user, "session": null})),
        );
        (core, rec)
    }

    #[test]
    fn startup_asks_a_new_child_for_its_models_first() {
        // The risk (#305): the guard opens on a guess, or on another
        // child's answer, because the core never asked this child which
        // spec is the user's, or asked after a page line.
        let repo = scratch("models-repo");
        std::fs::create_dir_all(repo.join(".trellis")).unwrap();
        std::fs::write(
            repo.join(".trellis").join("config.toml"),
            format!("[model]\nspec = \"{REPO_SPEC}\"\n"),
        )
        .unwrap();
        // A user config of the test's own; `scratch` dirs stay apart from
        // the developer's.
        let config = scratch("models-config");
        std::fs::write(
            config.join("config.toml"),
            format!("[model]\nspec = \"{USER_SPEC}\"\n"),
        )
        .unwrap();
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        // A page line sent at once still comes after the core's question.
        core.send(&json!({"id": "desktop-1", "type": "get_state"}))
            .unwrap();
        rec.wait_for("rpc-frame", |f| f["id"] == "desktop-1");
        let responses: Vec<Value> = rec
            .named("rpc-frame")
            .into_iter()
            .filter(|f| f["type"] == "response")
            .collect();
        assert_eq!(responses[0]["id"], MODELS_ID, "{responses:?}");
        assert_eq!(responses[0]["command"], "get_available_models");
        assert_eq!(responses[1]["id"], "desktop-1", "{responses:?}");
        assert_eq!(
            core.lock().view.configured(),
            Some(&Configured {
                repository: Some(REPO_SPEC.into()),
                user: Some(USER_SPEC.into()),
                session: None,
            }),
            "the answer's configured specs are recorded"
        );
        assert_eq!(
            models_question().to_string(),
            r#"{"id":"desktop-models","type":"get_available_models"}"#
        );

        // With the answer in, the user's own spec reaches trellis, and
        // the repository's does not leave the core.
        core.send(&json!({"id": "s", "type": "set_model", "model": USER_SPEC}))
            .unwrap();
        let set = rec.wait_for("rpc-frame", |f| f["id"] == "s");
        assert_eq!(set["success"], true, "{set}");
        let err = core
            .send(&json!({"id": "r", "type": "set_model", "model": REPO_SPEC}))
            .unwrap_err();
        assert!(err.contains("the model rule"), "{err}");

        // A restarted child starts unanswered, and asks again.
        finish(&core).unwrap();
        assert_eq!(core.lock().view.configured(), None, "reset on exit");
        rec.take();
        core.start(&repo).unwrap();
        rec.wait_for("rpc-frame", |f| f["id"] == MODELS_ID);
        finish(&core).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn model_guard_refuses_until_the_models_answer_arrives() {
        // The risk: a model named before the child said which spec is the
        // user's is judged against nothing, and a hosted spec passes.
        let (core, rec) = idle_core();
        let g = generation(&core);
        let waiting = |core: &Core| {
            for spec in [
                USER_SPEC,
                "provider:p#m",
                "llamacpp:http://127.0.0.1:8080",
                "scripted:x",
            ] {
                for l in naming(json!(spec)) {
                    let err = core.send(&l).unwrap_err();
                    assert!(err.contains("has not yet answered"), "{l}: {err}");
                    assert!(err.contains("repository default"), "{l}: {err}");
                }
            }
        };
        waiting(&core);
        // No model, or a null one, is the repository default: it names
        // nothing, so it meets only the child rule.
        for l in [
            json!({"id": "p", "type": "prompt", "task": "t"}),
            json!({"id": "p", "type": "prompt", "task": "t", "model": null}),
        ] {
            assert!(forwarded(&core, &l), "{l}");
        }
        // Answers that are not the core's, or not well formed, open nothing.
        let ok = json!({"repository": null, "user": USER_SPEC, "session": null});
        for bad in [
            line(
                json!({"type": "response", "command": "get_available_models", "id": "page",
                        "success": true, "data": {"configured": ok}}),
            ),
            line(
                json!({"type": "response", "command": "get_available_models", "id": MODELS_ID,
                        "success": false, "error": "no"}),
            ),
            line(
                json!({"type": "response", "command": "get_available_models", "id": MODELS_ID,
                        "success": true, "data": {"catalog": []}}),
            ),
            models_answer(json!({"repository": null, "user": 7, "session": null})),
            models_answer(json!({"repository": null, "user": [USER_SPEC], "session": null})),
            models_answer(json!([USER_SPEC])),
        ] {
            core.stdout_line(g, &bad);
            waiting(&core);
        }
        // The answer opens the guard; a malformed one after it shuts it.
        core.stdout_line(g, &models_answer(ok.clone()));
        assert!(forwarded(&core, &naming(json!(USER_SPEC))[0]));
        core.stdout_line(g, &models_answer(json!({"user": {"spec": USER_SPEC}})));
        waiting(&core);
        // The child's exit shuts it: the next child has not answered.
        core.stdout_line(g, &models_answer(ok));
        assert!(forwarded(&core, &naming(json!(USER_SPEC))[1]));
        core.exited(g, Err(io::Error::other("gone")));
        waiting(&core);
        // The page may not ask under the core's id.
        let err = core
            .send(&json!({"id": MODELS_ID, "type": "get_available_models"}))
            .unwrap_err();
        assert!(err.contains("is the desktop's own"), "{err}");
        assert!(
            rec.take().iter().all(|(e, _)| e != "child-started"),
            "no child was started"
        );
    }

    #[test]
    fn model_guard_forwards_a_hosted_spec_only_when_it_is_the_users_own() {
        // The risk (ADR 0024 §3): rpc counts a model the client names as
        // the user's choice and sends that endpoint the user's key, so a
        // hosted spec the user did not write themselves must not leave.
        let (core, _rec) = answered(Value::Null, json!(USER_SPEC));
        for l in naming(json!(USER_SPEC)) {
            assert!(forwarded(&core, &l), "the user's own spec: {l}");
        }
        for spec in [
            "openai-chat:https://user.example.invalid/v1/#gpt",
            "OPENAI-CHAT:https://user.example.invalid/v1#gpt",
            "Openai-Chat:https://other.invalid/v1#gpt",
            // Byte-equal: no trimming.
            " openai-chat:https://user.example.invalid/v1#gpt",
            "openai-chat:https://user.example.invalid/v1#gpt ",
            "openai-chat:https://user.example.invalid/v1#gpt\n",
            "anthropic:https://user.example.invalid#gpt",
            "aNtHrOpIc:x",
            "anthropic:",
            "anthropic",
        ] {
            for l in naming(json!(spec)) {
                let err = core.send(&l).unwrap_err();
                assert!(err.contains("the model rule"), "{l}: {err}");
                assert!(err.contains("Pick \"repository default\""), "{l}: {err}");
                assert!(err.contains("your own config"), "{l}: {err}");
            }
        }
        // With no user spec, no hosted spec is ever forwarded.
        for user in [Value::Null, json!("")] {
            let (core, _rec) = answered(json!(USER_SPEC), user.clone());
            for spec in [USER_SPEC, REPO_SPEC, ""] {
                for l in naming(json!(spec)) {
                    let hosted = !spec.is_empty();
                    assert_eq!(forwarded(&core, &l), !hosted, "user {user}: {l}");
                }
            }
        }
        // A model that is not a string is not a spec to judge.
        for l in [
            json!({"id": "m", "type": "set_model"}),
            json!({"id": "m", "type": "set_model", "model": null}),
            json!({"id": "p", "type": "prompt", "task": "t", "model": 1}),
            json!({"id": "p", "type": "prompt", "task": "t", "model": {"spec": USER_SPEC}}),
        ] {
            assert!(!forwarded(&core, &l), "{l}");
        }
    }

    #[test]
    fn model_guard_never_forwards_the_repository_spec_as_the_users_choice() {
        // The risk (#305): the page sends the repository's spec back
        // through set_model, rpc counts it as the user's, and the
        // repository's endpoint gets the user's key. A reformatted copy
        // parses to the same spec, so it must not pass either.
        let user = "anthropic:https://user.invalid#u";
        let (core, _rec) = answered(json!(REPO_SPEC), json!(user));
        for spec in [
            REPO_SPEC,
            "ANTHROPIC:https://example.invalid/#m",
            "anthropic:https://example.invalid/#m",
            "Anthropic:https://example.invalid#m",
        ] {
            for l in naming(json!(spec)) {
                assert!(!forwarded(&core, &l), "{l}");
            }
        }
        assert!(forwarded(&core, &naming(json!(user))[0]));
        // The session's spec is not the user's either.
        let (core, _rec) = idle_core();
        core.stdout_line(
            generation(&core),
            &models_answer(json!({"repository": null, "user": null, "session": REPO_SPEC})),
        );
        for l in naming(json!(REPO_SPEC)) {
            assert!(!forwarded(&core, &l), "{l}");
        }
    }

    /// What `start` does to the session for a new child, without a
    /// process: the next generation, with the guard shut.
    fn restart(core: &Core) -> u64 {
        let mut s = core.lock();
        s.generation += 1;
        s.view.forget_models();
        s.generation
    }

    #[test]
    fn model_guard_ignores_a_previous_childs_answer_after_a_restart() {
        // The risk: a late `desktop-models` answer from the child just
        // replaced opens the guard for the new one, whose user spec it may
        // not be, before the new child has said anything.
        let (core, _rec) = answered(Value::Null, json!(USER_SPEC));
        let old = generation(&core);
        assert!(
            forwarded(&core, &naming(json!(USER_SPEC))[0]),
            "open before"
        );
        let new = restart(&core);
        assert_ne!(old, new);
        core.stdout_line(
            old,
            &models_answer(json!({"repository": null, "user": USER_SPEC, "session": null})),
        );
        assert_eq!(core.lock().view.configured(), None);
        for l in naming(json!(USER_SPEC)) {
            let err = core.send(&l).unwrap_err();
            assert!(err.contains("has not yet answered"), "{l}: {err}");
        }

        // The new child's own answer opens it, by its own user spec.
        let other = "anthropic:https://new-user.invalid#n";
        core.stdout_line(
            new,
            &models_answer(json!({"repository": null, "user": other, "session": null})),
        );
        for l in naming(json!(other)) {
            assert!(forwarded(&core, &l), "{l}");
        }
        for l in naming(json!(USER_SPEC)) {
            assert!(!forwarded(&core, &l), "the old child's user spec: {l}");
        }
    }

    #[test]
    fn model_guard_forwards_provider_runtime_and_scripted_specs() {
        // The risk: the guard refuses specs that get no key, or whose key
        // `resolve_with` already holds to the user's own provider, so a
        // user cannot pick a local runtime from the desktop.
        let (core, _rec) = answered(json!(REPO_SPEC), Value::Null);
        for spec in [
            "provider:groq#llama",
            "PROVIDER:groq#llama",
            "llamacpp:http://127.0.0.1:8080",
            "vllm:http://127.0.0.1:8000#qwen",
            "sglang:http://127.0.0.1:30000",
            // A runtime alias, not the hosted `openai-chat`.
            "openai:http://127.0.0.1:8000",
            "scripted:/tmp/script.json",
            // No scheme, or none trellis knows: no key; rpc refuses it.
            "no-colon",
            "",
            "openai-chatty:https://x.invalid#m",
        ] {
            for l in naming(json!(spec)) {
                assert!(forwarded(&core, &l), "{l}");
            }
        }
    }

    #[test]
    fn open_repository_refuses_a_directory_that_is_not_a_repository() {
        // The risk: trellis started on a folder that is not a repository,
        // where a run would have nothing to roll back to.
        let (core, rec) = idle_core();
        let dir = scratch("open-plain");
        let err = core.open(&dir).unwrap_err();
        assert!(err.contains("has no .git entry"), "{err}");
        assert!(err.contains(&dir.display().to_string()), "{err}");
        assert!(rec.take().is_empty(), "nothing started");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_repository_is_refused_while_a_run_is_in_flight() {
        // The risk: opening another repository mid-run shuts the child down
        // under a run the user is watching.
        let repo = scratch("inflight-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let config = scratch("inflight-config");
        let (core, rec) = core_with(&config);
        let start = line(json!({"type": "run_start", "id": "p"}));
        let end = line(json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p"}));

        core.stdout_line(generation(&core), &start);
        let err = core.open(&repo).unwrap_err();
        assert!(err.contains("a run is in flight"), "{err}");
        assert!(rec.named("child-started").is_empty());

        core.stdout_line(generation(&core), &end);
        core.open(&repo).unwrap();
        assert_eq!(rec.named("child-started").len(), 1);

        // The child's exit ends the run too: nobody is left to finish it.
        core.stdout_line(generation(&core), &start);
        assert!(core.open(&repo).unwrap_err().contains("a run is in flight"));
        finish(&core).unwrap();
        core.open(&repo).unwrap();
        assert_eq!(rec.named("child-started").len(), 2);
        finish(&core).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn stdout_lines_become_frames_and_a_bad_line_stays_text() {
        // The risk: a line that is not JSON is dropped, or fails the
        // reader; or a frame reaches the page as a string to parse again.
        let (core, rec) = idle_core();
        let g = generation(&core);
        let frame = json!({"type": "response", "command": "get_state", "success": true,
                           "id": "desktop-1", "data": {"running": false, "tier": null}});
        core.stdout_line(g, &line(frame.clone()));
        core.stdout_line(g, "<b>not json</b>");
        core.stdout_line(g, "[1,2]");
        core.stderr_line(g, "warning: <i>human</i>".into());
        let events: Vec<(String, Value)> = rec
            .take()
            .into_iter()
            .filter(|(e, _)| e != "view")
            .collect();
        assert_eq!(
            events,
            vec![
                ("rpc-frame".to_string(), frame),
                ("rpc-unparsed".to_string(), json!("<b>not json</b>")),
                ("rpc-unparsed".to_string(), json!("[1,2]")),
                ("rpc-human".to_string(), json!("warning: <i>human</i>")),
            ]
        );
        // A line from a child already replaced reaches nothing.
        core.stdout_line(g + 1, "late");
        assert!(rec.take().is_empty());
    }

    #[test]
    fn child_lines_reach_the_page_as_one_view_event() {
        // The risk (#299): the reducer runs, and the page never hears what
        // it changed, or hears every pane on every line.
        let (core, rec) = idle_core();
        let g = generation(&core);
        let turn = line(json!({"fields": {"message": "turn", "turn": 1,
                                           "call": "read_file a.py", "verdict": "None",
                                           "check_kind": "", "observation": "x = 1"}}));
        let views = |rec: &Recorder| -> Vec<Value> {
            rec.take()
                .into_iter()
                .filter(|(e, _)| e == "view")
                .map(|(_, v)| v)
                .collect()
        };

        core.stdout_line(g, &turn);
        let v = views(&rec);
        assert_eq!(v.len(), 1, "{v:?}");
        let mut keys: Vec<&String> = v[0].as_object().unwrap().keys().collect();
        keys.sort();
        // The graph is laid out from the actions, so it changes with them
        // (#303).
        assert_eq!(keys, ["actions", "budget", "graph", "transcript"]);

        core.stdout_line(g, "plain text");
        let v = views(&rec);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].as_object().unwrap().len(), 1, "{v:?}");
        assert_eq!(
            v[0]["transcript"].as_array().unwrap().last().unwrap(),
            "plain text"
        );

        core.stderr_line(g, "trellis: a human line".into());
        let v = views(&rec);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].as_object().unwrap().len(), 1, "{v:?}");
        assert_eq!(
            v[0]["transcript"].as_array().unwrap().last().unwrap(),
            "trellis: a human line"
        );

        // A frame that changes no pane sends no view.
        core.stdout_line(g, r#"{"type":"something_new"}"#);
        assert!(views(&rec).is_empty());
    }

    #[test]
    fn forwarded_get_state_re_emits_the_full_view() {
        // The risk: a page that loads late or reloads sees only the panes
        // that change from then on, and a run already in flight stays
        // blank until it ends.
        let repo = scratch("resend-repo");
        let config = scratch("resend-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        rec.wait_for("child-started", |_| true);
        rec.take();
        core.stderr_line(generation(&core), "before".into());
        let v = rec.named("view");
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0].as_object().unwrap().len(),
            1,
            "only what changed: {v:?}"
        );
        rec.take();

        core.send(&json!({"id": "desktop-1", "type": "get_state"}))
            .unwrap();
        let full = rec.wait_for("view", |v| v.as_object().unwrap().len() > 1);
        let mut keys: Vec<String> = full.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "actions",
                "approvals",
                "budget",
                "checks",
                "edits",
                "graph",
                "outcome",
                "radius",
                "running",
                "tier",
                "transcript"
            ]
        );
        assert!(
            full["transcript"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l == "before"),
            "the run so far, not only what is new: {full}"
        );
        rec.wait_for("rpc-frame", |f| f["id"] == "desktop-1");

        // Once: the line after it is back to what changed.
        rec.take();
        core.stderr_line(generation(&core), "after".into());
        let v = rec.named("view");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].as_object().unwrap().len(), 1, "{v:?}");
        finish(&core).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn version_comes_from_the_version_argument_list() {
        // The risk: `child-started` names a version the binary did not
        // print, a root it was not started on, or another binary.
        let repo = scratch("version-repo");
        let config = scratch("version-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        let started = rec.wait_for("child-started", |_| true);
        let printed = trellis(&config).version().unwrap();
        assert!(printed.starts_with("trellis "), "{printed:?}");
        assert_eq!(
            started,
            json!({"trellis": trellis_bin().display().to_string(), "version": printed,
                   "root": repo.display().to_string()})
        );
        finish(&core).unwrap();
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn shutdown_sends_quit_and_waits_for_the_child_to_exit() {
        // The risk (ADR 0048 section 9): the window kills trellis mid-run,
        // skipping its rollback and trail; or reports it gone before it is.
        let repo = scratch("quit-repo");
        let config = scratch("quit-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        let taken = core.take_child("closing").expect("a child to shut down");
        let status = core.shut_down(taken).unwrap();
        // quit answers with a clean exit; a killed process does not.
        assert!(status.success(), "{status:?}");
        assert_eq!(
            rec.named("rpc-exited"),
            vec![json!({"code": 0, "success": true})]
        );
        assert!(core.lock().child.is_none());
        // Its reader sees the end and finds the child already taken.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(rec.named("rpc-exited").len(), 1, "reported once");
        assert!(is_empty(&repo), "nothing was written to the repository");
        assert!(
            is_empty(&config),
            "nothing was written to the configuration"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn rpc_send_forwards_get_codebase() {
        // The risk (#307): the codebase view asks for a graph the core
        // refuses to forward, or the answer reaches the page as a frame the
        // page must lay out itself (ADR 0048 section 5); or a folder that
        // is not a repository is drawn as one with no modules (#559).
        let repo = scratch("codebase-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("app")).unwrap();
        std::fs::write(repo.join("app/a.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(
            repo.join("app/b.py"),
            "from app.a import f\n\ndef g():\n    return f() + missing()\n",
        )
        .unwrap();
        let config = scratch("codebase-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        rec.wait_for("child-started", |_| true);

        assert_eq!(
            core.send(&json!({"id": "desktop-codebase", "type": "get_codebase"})),
            Ok(())
        );
        let v = rec.wait_for("codebase", |_| true);
        assert_eq!(v.get("error"), None, "{v}");
        assert_eq!(v["modules_indexed"], 2, "{v}");
        let regions = v["overview"]["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 1, "{v}");
        assert_eq!(regions[0]["package"], "app");
        assert_eq!(regions[0]["modules"], 2);
        let modules = v["packages"][0]["modules"].as_array().unwrap();
        let b = modules.iter().find(|m| m["module"] == "app.b").unwrap();
        assert_eq!(b["path"], "app/b.py", "the path #559 carries");
        assert_eq!(b["edges_out"], 1, "{b}");
        assert!(b["unresolved"].as_u64().unwrap() >= 1, "{b}");
        let a = modules.iter().find(|m| m["module"] == "app.a").unwrap();
        assert_eq!(a["edges_in"], 1, "{a}");
        finish(&core).unwrap();

        // A directory with no .git entry: trellis refuses, and the view
        // says so rather than drawing nothing.
        let plain = scratch("codebase-plain");
        rec.take();
        core.start(&plain).unwrap();
        rec.wait_for("child-started", |_| true);
        core.send(&json!({"id": "desktop-codebase", "type": "get_codebase"}))
            .unwrap();
        let v = rec.wait_for("codebase", |_| true);
        let err = v["error"].as_str().unwrap_or_else(|| panic!("{v}"));
        assert!(err.contains("no .git entry"), "{err}");
        assert!(is_empty(&plain), "nothing was written to the directory");
        finish(&core).unwrap();
        assert!(
            !repo.join(".trellis").exists(),
            "get_codebase wrote under the repository"
        );
        for d in [&repo, &plain, &config] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn rpc_send_forwards_get_radius() {
        // The risk (#308): the radius view asks for a radius the core
        // refuses to forward, or the answer reaches the page untyped, so
        // the page decides how reach and doubt are shown; or an unknown
        // symbol's refusal is lost.
        let repo = scratch("radius-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("a.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(
            repo.join("b.py"),
            "from a import f\n\ndef g(x):\n    return f() + f() + x.go()\n",
        )
        .unwrap();
        let config = scratch("radius-config");
        let (core, rec) = core_with(&config);
        core.start(&repo).unwrap();
        rec.wait_for("child-started", |_| true);

        let ask = json!({"id": "desktop-radius-1", "type": "get_radius", "symbol": "f"});
        assert_eq!(core.send(&ask), Ok(()));
        let v = rec.wait_for("radius", |v| v["id"] == "desktop-radius-1");
        assert_eq!(v.get("error"), None, "{v}");
        assert_eq!(v["symbol"], "a.f", "{v}");
        assert_eq!(v["reach"]["resolved"], 2, "{v}");
        assert!(v["doubt"]["unbound_in_touched_files"].is_u64(), "{v}");
        assert!(v["doubt"]["touched_files"].is_u64(), "{v}");

        let ask = json!({"id": "desktop-radius-2", "type": "get_radius", "symbol": "nope"});
        core.send(&ask).unwrap();
        let v = rec.wait_for("radius", |v| v["id"] == "desktop-radius-2");
        let err = v["error"].as_str().unwrap_or_else(|| panic!("{v}"));
        assert!(
            err.contains("no first-party definition matches `nope`"),
            "{err}"
        );
        finish(&core).unwrap();
        assert!(
            !repo.join(".trellis").exists(),
            "get_radius wrote under the repository"
        );
        for d in [&repo, &config] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn get_overview_runs_the_overview_argument_list() {
        // The risk: get_overview reads another repository than the open
        // one, or passes a root the core does not have.
        let plain = scratch("overview-plain");
        let config = scratch("overview-config");
        let (core, _rec) = core_with(&config);
        let v = core.overview(&plain).unwrap();
        assert_eq!(v, json!({"recent": []}), "no root: only recent");

        let repo = scratch("overview-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        core.lock().root = Some(repo.clone());
        let v = core.overview(&plain).unwrap();
        assert_eq!(v["root"], repo.display().to_string(), "{v}");
        assert!(v.get("board").is_some(), "{v}");
        assert!(
            is_empty(&config),
            "nothing was written to the configuration"
        );
        for d in [&plain, &config, &repo] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn startup_without_a_root_emits_the_overview() {
        // The risk: started outside a repository, the window runs trellis
        // rpc on a folder that is not one, or shows nothing at all.
        let plain = scratch("startup-plain");
        assert!(
            !plain.ancestors().any(|d| d.join(".git").exists()),
            "the temp directory {} must not be inside a repository",
            plain.display()
        );
        let config = scratch("startup-config");
        let (core, rec) = core_with(&config);
        core.startup(None, &plain);
        let events = rec.take();
        let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(names, ["no-child", "overview"], "{events:?}");
        assert!(
            events[0]
                .1
                .as_str()
                .unwrap()
                .contains("No repository was found")
        );
        assert_eq!(events[1].1, json!({"recent": []}));
        assert!(core.lock().child.is_none(), "no rpc child");
        let err = core
            .send(&json!({"id": "x", "type": "get_state"}))
            .unwrap_err();
        assert!(err.contains("--root <repo>"), "rpc_send says why: {err}");
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn get_trails_takes_a_limit_from_1_to_200() {
        // The risk: a page asks trellis for every trail there is, or for
        // none, and reads the empty answer as "no runs".
        let (core, _rec) = idle_core();
        for bad in [0, TRAILS_MAX + 1] {
            let err = core.trails(bad).unwrap_err();
            assert!(err.contains("1 to 200"), "{bad}: {err}");
        }
        let err = core.trails(1).unwrap_err();
        assert!(err.contains("no repository is open"), "{err}");
    }

    #[test]
    fn startup_root_is_the_repository_holding_the_current_directory() {
        // The risk: started inside a repository, the window opens a
        // subdirectory as if it were the root, or nothing at all.
        let repo = scratch("root-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(startup_root(None, &repo), Ok(repo.clone()));
        assert_eq!(startup_root(None, &sub), Ok(repo.clone()));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn startup_root_outside_a_repository_says_how_to_name_one() {
        // The risk: started outside a repository, the window runs trellis
        // on a folder that is not one, or shows nothing and says nothing.
        let dir = scratch("root-plain");
        assert!(
            !dir.ancestors().any(|d| d.join(".git").exists()),
            "the temp directory {} must not be inside a repository",
            dir.display()
        );
        let why = startup_root(None, &dir).unwrap_err();
        assert!(why.contains("No repository was found"), "{why}");
        assert!(why.contains("--root <repo>"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_root_named_with_root_is_used_as_given() {
        let dir = scratch("root-named");
        assert_eq!(
            startup_root(Some(dir.clone()), Path::new("/")),
            Ok(dir.clone())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
