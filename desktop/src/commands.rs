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
//!   (`overview-error` says why when it could not be read).
//!
//! Nothing restarts a child that exited, whether on `quit`, a crash or a
//! broken pipe: the page is told, and `rpc_send` refuses until
//! `open_repository` starts another. The page cannot send `quit`; only the
//! core does, and it then waits (ADR 0025 §5).
//!
//! Everything here but the four `#[tauri::command]` functions is plain Rust
//! over a [`Sink`], so the tests drive it without a window.

use crate::child::{Child, Lines, Output, Trellis};
use crate::frames::{self, Frame};
use crate::state::{Changed, DesktopState};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The longest line `rpc_send` forwards, in bytes as written.
pub const LINE_CAP: usize = 65_536;

/// The `type`s `rpc_send` forwards: `rpc.rs`'s `COMMANDS` as #296 lists
/// them, without `quit`, which only the core sends.
pub const FORWARDED: [&str; 11] = [
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
        let generation = {
            let mut s = self.lock();
            s.generation += 1;
            s.root = Some(root.to_path_buf());
            s.child = Some((s.generation, child));
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
        self.feed(&mut s, frames::parse_stdout(line));
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

    /// `rpc_send`: checks the line, then forwards it to the child.
    pub fn send(&self, line: &Value) -> Result<(), String> {
        check_line(line)?;
        let writer = {
            let mut s = self.lock();
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

/// What `rpc_send` checks before it looks for a child, each refusal naming
/// its rule: a JSON object, a `type` it forwards, the size cap, and the
/// model rule.
fn check_line(line: &Value) -> Result<(), String> {
    let Some(o) = line.as_object() else {
        return Err("rpc_send: the line is not a JSON object".into());
    };
    let ty = match o.get("type").and_then(Value::as_str) {
        Some(t) if FORWARDED.contains(&t) => t,
        Some(t) => {
            return Err(format!(
                "rpc_send: type {t:?} is not one rpc_send forwards ({})",
                FORWARDED.join(", ")
            ));
        }
        None => return Err("rpc_send: the line has no string type".into()),
    };
    let len = serde_json::to_string(line).map_or(usize::MAX, |s| s.len());
    if len > LINE_CAP {
        return Err(format!(
            "rpc_send: the line is {len} bytes, over the {LINE_CAP}-byte cap"
        ));
    }
    // rpc.rs counts a model the client names as the user's choice, which
    // skips the hosted endpoint check (ADR 0024 §3), so none is named until
    // DESK-12's guard: whatever the spec.
    if ty == "set_model" || (ty == "prompt" && o.contains_key("model")) {
        return Err(
            "rpc_send: the model rule: the desktop sends no set_model and no prompt carrying \
             model until its model guard (DESK-12) lands, since rpc counts a model the client \
             names as the user's choice (ADR 0024 §3)"
                .into(),
        );
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

    #[test]
    fn rpc_send_refuses_set_model_and_a_prompt_model() {
        // The risk (ADR 0024 §3): rpc counts a model the client names as
        // the user's choice, so a spec the repository chose, forwarded,
        // would carry the user's key to the repository's endpoint.
        let (core, rec) = idle_core();
        for spec in [
            "anthropic:https://example.invalid#m",
            "provider:anthropic/claude",
            "llamacpp:",
            "scripted:x",
        ] {
            for l in [
                json!({"id": "m", "type": "set_model", "model": spec}),
                json!({"id": "p", "type": "prompt", "task": "t", "model": spec}),
            ] {
                let err = core.send(&l).unwrap_err();
                assert!(err.contains("the model rule"), "{l}: {err}");
                assert!(!no_child(&err), "{l}: refused for the model, not the child");
            }
        }
        let err = core
            .send(&json!({"id": "p", "type": "prompt", "task": "t"}))
            .unwrap_err();
        assert!(
            no_child(&err),
            "a prompt without model meets only the child rule: {err}"
        );
        assert!(rec.take().is_empty(), "no child was started");
        assert!(core.lock().child.is_none());
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
        assert_eq!(keys, ["actions", "budget", "transcript"]);

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
