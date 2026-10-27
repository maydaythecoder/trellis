//! `trellis rpc`: JSONL commands on stdin, responses, framing and events on
//! stdout (ADR 0025 §5).
//!
//! A `prompt` runs a task through `cmd_run_with`, the path `trellis run`
//! takes, on its own thread. The main thread keeps reading, so `abort`,
//! `approve`, `get_state`, `steer`, `follow_up`, `pause`, `resume`, `set_budget` and `withdraw` are
//! answered while the run is in flight. One run at a time; the clean-worktree rule is
//! `cmd_run`'s and is unchanged. A follow-up is the one way a run starts
//! other than `prompt`: after the run it was sent during ends.
//!
//! Stdout carries two layers. The framing goes through one `output::Sink`:
//! this file writes `response` and `approval_request`, and hands the sink to
//! `output::framed` in `RunExtras`, which writes each run's `run_start` and
//! `run_end` around it. The run's events (`turn`, `model call`, `steer
//! delivered`, …) are `tracing` events that `startup.rs`'s stdout JSON layer
//! writes, one per line; `TurnCapture`, a layer beside it (`rpc/turns.rs`), keeps
//! the `turn` events for `get_messages`. The fields a client may rely on, in
//! both layers and in the answers it names, are listed in
//! `crates/trellis-cli/tests/client_contract.json` (ADR 0048 section 6),
//! which the tests hold this file to. Where each program starts and where
//! the flow goes next: docs/ENTRY_POINTS.md.

use crate::output::{self, Sink, frame};
use crate::{Human, RunControl, RunExit, RunSeeding, config};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use trellis_loop::Steering;

mod steering;
mod turns;

pub(crate) use steering::SteeringPort;
use steering::{Unsent, Withdrawal};
use turns::TURNS;
pub(crate) use turns::TurnCapture;

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
    (
        "abort",
        "interrupt the run in flight; it rolls back. Or drop a pending classify, unanswered",
    ),
    (
        "get_state",
        "whether a run is in flight or a classify pending, the model, the last run",
    ),
    ("get_messages", "the current or last run's turns"),
    (
        "set_model",
        "this session's model, which `prompt` needs first: {model, save?} names one, and \
         save writes it to your user config; {configured: true} confirms the configured one",
    ),
    (
        "get_available_models",
        "the catalog and the configured specs",
    ),
    (
        "list_models",
        "the configured spec, the specs local runtimes on their default ports report, and \
         your user config's providers by name and key variable, read-only",
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
        "pause",
        "hold the run in flight before its next model request; answered once it holds: \
         {turn}, the model call it holds before. Held time is not charged to the wall clock",
    ),
    (
        "resume",
        "let a paused run go on, or withdraw a pause it has not reached yet",
    ),
    (
        "set_budget",
        "raise the run in flight's ceilings, never lower one: any of {max_iterations?, \
         max_wall_clock_ms?, max_completion_tokens?}; answered with every ceiling after it, in force \
         from the loop's next budget check, where `run budget` is sent again",
    ),
    (
        "withdraw",
        "take back a steer or follow_up not yet delivered: {id}, the id it was sent with; \
         answered {id, type}, refused once a model request carries the steer or the \
         follow_up's run has started",
    ),
    (
        "compact",
        "elide every observation but the newest from the run's next model request; \
         answers with what was folded",
    ),
    (
        "get_codebase",
        "the module graph `trellis index --graph` draws, as data, read-only: {limit?} \
         (default 40), {edges?: \"all\"} for every edge, {ref?} for the graph at a commit",
    ),
    (
        "get_radius",
        "a symbol's blast radius `trellis radius` prints, as data, read-only: {symbol}; \
         reach and doubt as separate counts, never a ratio",
    ),
    (
        "get_symbols",
        "every definition the index holds, or one module's: {module?}; each with its \
         lines and its call sites in, ambiguously in and out as separate counts, read-only",
    ),
    (
        "get_history",
        "HEAD's newest first-parent commits with their paths, the tags in its history and \
         each path's churn in the 30, 90 and 365 days before HEAD's date, from git, \
         read-only: {commits?, tags?} (default 20 and 12, at most 200)",
    ),
    (
        "get_queue",
        "the tasks `trellis queue list` prints, as data, read-only; claims nothing",
    ),
    (
        "list_paths",
        "the paths `@` completion offers that start with {prefix}, read-only: at most 10, \
         shortest first; tracked or untracked, never ignored",
    ),
    (
        "read_file",
        "one file's text, read-only: {path}, a path `list_paths` offers; refused outside \
         the root, into .git or not UTF-8; {path, text, bytes, cut}, cut past 256 KiB",
    ),
    (
        "vault_search",
        "the hits `trellis vault search` prints, as data, read-only: {query, limit?} \
         (default 12)",
    ),
    (
        "get_session_stats",
        "prompt and completion tokens of this process's runs, per model, from their \
         trails, read-only; never summed, null when a run did not report one",
    ),
    (
        "get_last_assistant_text",
        "the `done` summary of this process's last run that ended on `done`, read-only: \
         {text}, null before one",
    ),
    (
        "vault_graph",
        "a note's lineage, children and links `trellis vault graph` prints, as data, \
         read-only: {note}",
    ),
    (
        "vault_check",
        "the report line, issues, warnings and run-gate answer `trellis vault check` \
         prints, as data, read-only; `--fix` has no rpc form, and a `fix` field is refused",
    ),
    (
        "classify",
        "whether {text} is a question or a task, by the rule the TUI routes on; starts no \
         run: {intent}, and {answer} when it is a question",
    ),
    (
        "get_commands",
        "this list, the prompt templates and the skills",
    ),
    (
        "quit",
        "abort any run, wait for its rollback, drop any pending classify, exit",
    ),
];

/// The ids of the trails this process's runs wrote, in order, for
/// `get_session_stats`. Pushed by `output::framed` before each `run_end`.
type Ran = Arc<Mutex<Vec<String>>>;

/// `get_session_stats`: `/status`'s per-model totals (#318), from the one
/// function that counts them, over the trails of this process's runs.
///
/// Each trail is read when asked, as untrusted data (S10), as `trellis
/// session` reads it for `/status`. One that does not read back is counted
/// in `runs_not_counted` rather than left out unsaid, so the totals are not
/// taken for whole. The model name is trail text; it leaves as a JSON
/// string through `frame`, which escapes C1 as well.
fn session_stats(root: &Path, ids: &[String]) -> Value {
    let mut trails = Vec::new();
    let mut unread = 0usize;
    for id in ids {
        let path = root
            .join(".trellis")
            .join("runs")
            .join(format!("{id}.json"));
        let read = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                trellis_trail::UntrustedTrail::from_json(&text).map_err(|e| e.to_string())
            });
        match read {
            Ok(t) => trails.push(t),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "an rpc run's trail did not read back");
                unread += 1;
            }
        }
    }
    let models: Vec<Value> = crate::session_extra::tokens_by_model(&trails)
        .into_iter()
        .map(|m| {
            json!({"model": m.model, "prompt_tokens": m.prompt,
                   "completion_tokens": m.completion})
        })
        .collect();
    json!({"models": models, "runs_not_counted": unread})
}

/// The paths `@` completion may offer: what git lists as tracked, plus
/// untracked files that no ignore rule covers. The TUI and `list_paths`
/// both ask this, so a client never lists paths itself.
///
/// Asked of git rather than walked from disk, so a `.gitignore`d file — a
/// `.env`, a build directory, a virtualenv — is never offered to be typed
/// into a task that a model will read. `-z` because without it git quotes a
/// path with unusual bytes, and a quoted path inserted into a task names a
/// file that does not exist.
pub(crate) fn at_file_paths(root: &Path) -> Result<Vec<String>> {
    let out = trellis_vcs::command()
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "listing repository paths exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut paths: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        // A path that is not UTF-8 cannot be typed into a task faithfully,
        // so it is left out rather than offered with its bytes replaced.
        .filter_map(|p| std::str::from_utf8(p).ok().map(str::to_string))
        .collect();
    // `--cached` lists a file once per conflict stage during a merge.
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// How many paths `list_paths` answers with: the TUI's `@` list length.
const LIST_PATHS_LIMIT: usize = 10;

/// How much of a file `read_file` answers with, in bytes (ADR 0058 §4). A
/// display limit for a client's whole-file view, not a measurement: past
/// it the file is cut, and the answer says so.
const READ_FILE_CAP: usize = 256 * 1024;

/// `list_paths`'s `prefix`, refused unless it can only be the start of a
/// repository-relative path. It only filters git's listing, so no prefix
/// could read a file; it is refused anyway, by the rule it breaks, so a
/// client learns its input was wrong rather than seeing an empty list.
fn path_prefix(cmd: &Value) -> Result<&str, String> {
    let Some(prefix) = cmd.get("prefix").and_then(Value::as_str) else {
        return Err(
            "`prefix` is required, as a string: the start of a repository-relative \
                    path, \"\" for any"
                .into(),
        );
    };
    relative_path_rule("prefix", prefix, "the start of a repository-relative path")?;
    Ok(prefix)
}

/// The rule `list_paths`'s `prefix` and `read_file`'s `path` are both held
/// to, named by `field` in the refusal: no control character, not absolute,
/// no `..` component. Both separators are checked, whatever the platform,
/// since the client may not share it.
fn relative_path_rule(field: &str, value: &str, what: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!(
            "`{field}` holds a control character; a repository path does not"
        ));
    }
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if value.starts_with(['/', '\\']) || drive {
        return Err(format!("`{field}` is absolute; it is {what}"));
    }
    if value.split(['/', '\\']).any(|c| c == "..") {
        return Err(format!(
            "`{field}` holds a `..` component; it names paths inside the repository only"
        ));
    }
    Ok(())
}

/// `set_budget`'s ceilings, under the names and units `run budget` sends
/// them in. Each is refused by the rule it breaks: a client that sent
/// `max_wall_clock_s`, `"5"` or `-1` learns its request was wrong rather than
/// seeing a ceiling it did not mean, and nothing is raised.
fn budget_raise(cmd: &Value) -> Result<trellis_loop::Raise, String> {
    let Some(fields) = cmd.as_object() else {
        return Err("a command is a JSON object".into());
    };
    let mut raise = trellis_loop::Raise::default();
    for (name, value) in fields {
        match name.as_str() {
            "id" | "type" => {}
            "max_iterations" => {
                let v = positive(name, value)?;
                raise.max_iterations = Some(u32::try_from(v).map_err(|_| {
                    format!(
                        "`max_iterations` {v} is past the most a run can count, {}",
                        u32::MAX
                    )
                })?);
            }
            "max_wall_clock_ms" => {
                raise.max_wall_clock =
                    Some(std::time::Duration::from_millis(positive(name, value)?));
            }
            "max_completion_tokens" => raise.max_tokens = Some(positive(name, value)?),
            // `run budget` carries it, but it is the driver's prompt size,
            // fixed when the driver is built, and no budget check reads it.
            "max_context_bytes" => {
                return Err(format!(
                    "`max_context_bytes` is the driver's prompt size, fixed for the run; \
                     set_budget raises {SET_BUDGET_FIELDS}"
                ));
            }
            other => {
                return Err(format!(
                    "`{other}` is not a set_budget field; it raises {SET_BUDGET_FIELDS}"
                ));
            }
        }
    }
    if raise == trellis_loop::Raise::default() {
        return Err(format!("name a ceiling to raise: {SET_BUDGET_FIELDS}"));
    }
    Ok(raise)
}

/// A ceiling's value: a whole number above zero, as JSON writes one.
fn positive(name: &str, value: &Value) -> Result<u64, String> {
    match value.as_u64() {
        Some(0) => Err(format!(
            "`{name}` is 0; a ceiling of 0 ends the run, and set_budget only raises"
        )),
        Some(v) => Ok(v),
        None => Err(format!(
            "`{name}` must be a whole number above 0, at most {}; got {value}",
            u64::MAX
        )),
    }
}

/// `read_file`'s `path`, refused by the rule it breaks, as `list_paths`
/// refuses a prefix.
fn read_file_path(cmd: &Value) -> Result<&str, String> {
    let Some(path) = cmd.get("path").and_then(Value::as_str) else {
        return Err("`path` is required, as a string: a repository-relative file path".into());
    };
    if path.is_empty() {
        return Err("`path` is empty; it names one repository file".into());
    }
    relative_path_rule("path", path, "a repository-relative file path")?;
    Ok(path)
}

/// `read_file`'s answer for `path`, or why it is refused (ADR 0058 §4).
///
/// The order is the point. Membership in `at_file_paths` comes first, so a
/// path `@` completion would not offer — an ignored `.env`, anything in
/// `.git` — is refused with one message whether or not it exists, and is
/// never opened. Membership is byte-exact against git's `/`-spelled
/// listing: `a\b` is refused, not normalised. Then the path is resolved
/// through `resolve_repo_input` (ADR 0025), so a tracked link that leads
/// outside the root or into `.git` is refused, and what was checked is what
/// is read.
///
/// At most `READ_FILE_CAP` bytes are read, so a large file costs no more
/// than the cap. A file past it is cut at the last whole character before
/// the cap. The bytes read must be UTF-8, or the file is refused as not
/// text; bytes past the cap are not read, so they are not checked.
fn read_repo_file(root: &Path, path: &str) -> Result<Value, String> {
    use std::io::Read;
    let offered = at_file_paths(root).map_err(|e| format!("{e:#}"))?;
    if offered.binary_search_by(|p| p.as_str().cmp(path)).is_err() {
        return Err(format!(
            "`path` {path:?} is not a path `list_paths` offers (tracked, or untracked and \
             not ignored), so it is not read"
        ));
    }
    let real = match trellis_core::paths::resolve_repo_input(root, &root.join(path)) {
        Ok(Some(real)) => real,
        Ok(None) => return Err(format!("`path` {path:?}: no file is there now")),
        Err(e) => return Err(format!("`path` {path:?}: {e}")),
    };
    let file = std::fs::File::open(&real).map_err(|e| format!("`path` {path:?}: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("`path` {path:?}: {e}"))?;
    if !meta.is_file() {
        return Err(format!(
            "`path` {path:?} is not a file (a directory or a submodule); read_file reads one file"
        ));
    }
    let bytes = meta.len();
    let mut read = Vec::new();
    file.take(READ_FILE_CAP as u64)
        .read_to_end(&mut read)
        .map_err(|e| format!("`path` {path:?}: {e}"))?;
    // The length read, not `bytes`: the file may have grown or shrunk since
    // its size was taken, and only a cap actually reached cuts.
    let cut = read.len() == READ_FILE_CAP && bytes > READ_FILE_CAP as u64;
    let text = match String::from_utf8(read) {
        Ok(t) => t,
        // A character the cap split in two: keep what comes before it.
        Err(e) if cut && e.utf8_error().error_len().is_none() => {
            let whole = e.utf8_error().valid_up_to();
            let mut v = e.into_bytes();
            v.truncate(whole);
            String::from_utf8(v).map_err(|e| format!("`path` {path:?}: {e}"))?
        }
        Err(_) => {
            return Err(format!(
                "`path` {path:?} is not UTF-8 text, so it is not read"
            ));
        }
    };
    Ok(json!({"path": path, "text": text, "bytes": bytes, "cut": cut}))
}

/// The first `LIST_PATHS_LIMIT` of `paths` that start with `prefix`,
/// shortest first and then by bytes, the order the TUI offers them in.
fn starting_with(paths: Vec<String>, prefix: &str) -> Vec<String> {
    let mut hits: Vec<String> = paths
        .into_iter()
        .filter(|p| p.starts_with(prefix))
        .collect();
    hits.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    hits.truncate(LIST_PATHS_LIMIT);
    hits
}

/// What a `steer` still waiting is answered once its run has ended: the
/// loop will read the channel no more.
const STEER_ENDED: &str = "the run ended before its next model request; not delivered";

/// What a `steer` is answered when a `withdraw` takes it back first.
const STEER_WITHDRAWN: &str = "withdrawn before a model request carried it; not delivered";

/// What a `pause` is answered with, and a held run released by, when input
/// ends: nobody is left to send `resume`.
const INPUT_ENDED: &str = "input ended; nothing could resume the run, so it did not hold";

/// What a `pause` the run never reached is answered with once it has ended.
const HOLD_ENDED: &str = "the run ended before it reached the hold; it did not pause";

/// What a `set_budget` is refused with once the run has ended.
const BUDGET_ENDED: &str = "the run has ended; nothing changed";

/// The fields `set_budget` reads, as `run budget` names them.
const SET_BUDGET_FIELDS: &str = "max_iterations, max_wall_clock_ms or max_completion_tokens";

/// What `prompt` and `classify` are refused with while a run is in flight.
const ONE_AT_A_TIME: &str = "a run is in progress; one at a time (abort it, or wait)";

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

    fn tokens_used(&self) -> Option<u64> {
        self.model.tokens_used()
    }

    fn prompt_bytes(&self) -> Option<usize> {
        self.model.prompt_bytes()
    }

    fn context_bytes(&self) -> Option<usize> {
        self.model.context_bytes()
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
    /// Started by a `follow_up`, so `withdraw` names its id delivered: its
    /// run has started. The `prompt`'s id names no queued message.
    follow_up: bool,
    /// `compact` requests, answered by the driver at its next request.
    compact: Arc<trellis_loop::CompactRequests>,
    steering: Arc<SteeringPort>,
    /// `pause` and `resume` (ADR 0058 §1), honoured by the loop at the top
    /// of its next iteration. Closed when the run ends.
    hold: Arc<trellis_loop::Hold>,
    /// `set_budget` (ADR 0058 §2): the run's whole ceiling, enforced by the
    /// loop from its next budget check. Closed when the run ends.
    budget: Arc<trellis_loop::LiveBudget>,
}

impl Current {
    fn new(id: Option<String>) -> Self {
        Self {
            id,
            follow_up: false,
            compact: Arc::new(trellis_loop::CompactRequests::default()),
            steering: SteeringPort::new(),
            hold: Arc::default(),
            budget: Arc::default(),
        }
    }
}

/// A run in flight, and the follow-ups queued behind it.
struct Running {
    thread: std::thread::JoinHandle<()>,
    abort: Arc<output::Abort>,
    /// Set by the run's thread as it finishes, after `leftover`.
    done: Arc<AtomicBool>,
    /// Set just before a run in the chain writes `run_end`, cleared before
    /// a follow-up starts: a `prompt` read while it is set waits for the
    /// thread instead of being refused (#495).
    ended: Arc<AtomicBool>,
    current: Arc<Mutex<Current>>,
    /// `Chain::queue`.
    queue: FollowUps,
    /// What was sent and never delivered when the chain stopped on an
    /// abort, in the order it was received, for `abort`'s answer.
    leftover: Arc<Mutex<Vec<Value>>>,
}

/// Follow-ups whose run has not started, as `(id, text)` in the order
/// received. Shared, not the chain's own, so `get_state` can name them.
type FollowUps = Arc<Mutex<VecDeque<(Option<String>, String)>>>;

/// A follow-up that never started, as `abort` and `get_state` name it.
fn queued_json(id: Option<String>, text: String) -> Value {
    Unsent {
        id,
        what: Steering::FollowUp(text),
    }
    .json()
}

impl Running {
    /// What `get_state` reports as `pending` (#801): the follow-ups queued
    /// behind the run in flight, then what was sent to that run and not yet
    /// delivered. The queue's were sent during earlier runs of the chain,
    /// so this is oldest first. The queue's lock is held while the port is
    /// read, as the chain holds it from closing a port to starting the next
    /// run, so a follow-up moving from one to the other is seen once.
    fn pending(&self) -> Vec<Value> {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<Value> = queue
            .iter()
            .map(|(id, text)| queued_json(id.clone(), text.clone()))
            .collect();
        out.extend(self.current().steering.pending());
        out
    }

    fn current(&self) -> Current {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `withdraw` (ADR 0058 §3): takes back the first waiting steer or
    /// follow-up sent with `id`, answering with its type. Under the queue's
    /// lock, as `pending` reads, which the chain holds from closing a run's
    /// port to starting the next run: a follow-up is then in the queue, in
    /// the port's record, or started, and never between two of them.
    /// Queued follow-ups come first, since they were sent earlier.
    fn withdraw(&self, id: &str) -> Result<&'static str, String> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = queue.iter().position(|(q, _)| q.as_deref() == Some(id)) {
            queue.remove(i);
            return Ok("follow_up");
        }
        let cur = self.current();
        match cur.steering.withdraw(id) {
            Withdrawal::Withdrawn(Steering::Steer(_)) => Ok("steer"),
            Withdrawal::Withdrawn(Steering::FollowUp(_)) => Ok("follow_up"),
            Withdrawal::Delivered => Err(ALREADY_DELIVERED.into()),
            Withdrawal::NotHere if cur.follow_up && cur.id.as_deref() == Some(id) => {
                Err(ALREADY_DELIVERED.into())
            }
            Withdrawal::NotHere => Err(format!(
                "unknown id: no steer or follow_up waiting to be delivered was sent with `{id}`"
            )),
        }
    }
}

/// What `withdraw` is refused with once the message is in a model request,
/// or its follow-up's run has started.
const ALREADY_DELIVERED: &str = "already delivered";

/// A `classify` being asked, on its own thread (#518).
struct Classifying {
    thread: std::thread::JoinHandle<()>,
    /// Set by the thread once the model has answered, before it writes the
    /// response, so a command read after that response is not refused while
    /// the thread finishes the write.
    answered: Arc<AtomicBool>,
    /// Sent by `quit` or `abort`: the request is dropped and nothing is
    /// written.
    cancel: tokio::sync::oneshot::Sender<()>,
}

/// One `trellis rpc` session: the state every command reads or changes
/// between lines, kept on the main thread. `serve_with` reads a line, and
/// `dispatch_command` answers it from here; a run in flight is `running`,
/// its own thread, and the model it uses is the `prompt`'s own when it names
/// one, else `model` or `confirmed` (`Server::model`).
struct Server {
    root: PathBuf,
    sink: Sink,
    mode: Mode,
    /// Set by `set_model {model}`: the client's choice, which ADR 0024 §3
    /// counts as the user's.
    model: Option<String>,
    /// Set by `set_model {configured: true}`: the configured spec, as it
    /// read when the client confirmed it. Not the user's choice: a
    /// repository's spec confirmed is still the repository's, so a hosted
    /// endpoint it names gets a key only where `hosted` already allows one.
    confirmed: Option<String>,
    running: Option<Running>,
    /// Holds the slot a run holds while it is pending: no run starts, and
    /// no second model request is made, until it is answered or dropped.
    classifying: Option<Classifying>,
    last: Arc<Mutex<Option<Value>>>,
    ran: Ran,
    /// One slot for every run: a run writes it only when it ended on
    /// `done`, so it holds the most recent such summary, as `/copy`'s does.
    done: output::DoneSlot,
    /// The tier the last run decided, as the TUI's status bar labels it,
    /// for `get_state` (#269). `None` until a run has decided one: nothing
    /// here guesses a tier from the spec.
    tier: output::TierSlot,
    /// Open approval questions, by id, each holding its reply channel.
    approvals: Arc<Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<bool>>>>,
    next_approval: Arc<AtomicU64>,
    /// Set, under the `approvals` lock, when input ends: no `approve` can
    /// arrive after it, so a question asked from then on is declined.
    input_closed: Arc<AtomicBool>,
    /// Threads waiting to answer a `compact`. Joined before `serve`
    /// returns, so no answer is lost to the process exiting.
    waiters: Vec<std::thread::JoinHandle<()>>,
    /// Where `list_models` looks and `set_model {save}` writes.
    sources: ModelSources,
    /// The `provider:colibri#<id>` specs `list_models` has offered, each
    /// with the base its `[[providers]]` entry names (#625), so `set_model
    /// {save: true}` on one writes the entry the spec needs beside the spec.
    /// Shared with `list_models`'s thread, which fills it.
    colibri_offers: Arc<Mutex<BTreeMap<String, String>>>,
}

/// Where `list_models` probes for local runtimes and which user config file
/// it reads and `set_model {save: true}` writes: `first_run`'s defaults and
/// the user's own file, so the desktop's picker and bare `trellis`'s
/// first-run list offer the same specs and save to the same place (ADR 0050
/// §2). A parameter so a test can point both at its own stub and directory,
/// since `user_config::user_config_path()` is `None` under test.
pub(crate) struct ModelSources {
    pub(crate) runtimes: Vec<crate::first_run::Runtime>,
    pub(crate) user_file: Option<PathBuf>,
}

impl ModelSources {
    /// The defaults `first_run` probes, and the user's own config file.
    pub(crate) fn user() -> Self {
        Self {
            runtimes: crate::first_run::defaults().to_vec(),
            user_file: crate::user_config::user_config_path(),
        }
    }
}

/// What `prompt` is refused with until this session has a model (#564; ADR
/// 0048 section 5, amended): the owner's condition for starting the desktop
/// with its environment unchanged is that no prompt runs before the user has
/// said which model it runs on. A prompt's own `model` does not count, so a
/// client cannot pass the choice by naming one in the same line.
const CHOOSE_FIRST: &str = "choose a model first: send set_model {\"model\": \"<spec>\"} \
     to choose one, or {\"configured\": true} to confirm the configured one; list_models \
     lists what there is";

/// Reads commands until `quit` or end of input.
///
/// End of input waits for a run in flight to finish, so
/// `printf '{"type":"set_model",...}\n{"type":"prompt",...}\n' | trellis rpc`
/// runs one task to completion. `quit` aborts it instead. A `prompt` is
/// refused until `set_model` has given the session a model (#564).
///
/// End of input also means no `approve` can come: every question open
/// then, and every one the run asks after, is declined (a dropped reply
/// channel is a no, ADR 0018), and the run goes on to its end. Joining
/// with a question still open would wait forever on an answer nobody can
/// send.
pub(crate) fn serve(root: &Path, input: impl BufRead, sink: Sink, mode: Mode) -> Result<()> {
    serve_with(root, input, sink, mode, ModelSources::user())
}

/// [`serve`], with where models are looked for and saved given.
fn serve_with(
    root: &Path,
    input: impl BufRead,
    sink: Sink,
    mode: Mode,
    sources: ModelSources,
) -> Result<()> {
    let mut s = Server {
        root: root.to_path_buf(),
        sink,
        mode,
        model: None,
        confirmed: None,
        running: None,
        classifying: None,
        last: Arc::default(),
        ran: Arc::default(),
        done: output::DoneSlot::default(),
        tier: output::TierSlot::default(),
        approvals: Arc::default(),
        next_approval: Arc::default(),
        input_closed: Arc::default(),
        waiters: Vec::new(),
        sources,
        colibri_offers: Arc::default(),
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
        if !s.dispatch_command(&cmd) {
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
    // No `resume` can arrive either, and the join below would wait on a
    // held run forever. Released as a question is declined: the run goes on
    // to its end. A follow-up's run after it gets a fresh hold, which
    // nothing can ask to hold. Not after an `abort`, for the reason above:
    // released before the run takes the abort path, the loop would make a
    // model request the person had stopped, and dropping its future ends a
    // held run without a release.
    if !aborting && let Some(r) = &s.running {
        r.current().hold.close(INPUT_ENDED);
    }
    if let Some(r) = s.running.take() {
        let _ = r.thread.join();
    }
    // Answered, as a run is finished: `cancel` lives until after the join.
    if let Some(c) = s.classifying.take() {
        let _ = c.thread.join();
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

    /// `get_codebase`: the module graph as data, built as the interface's
    /// `/codebase` builds it. The worktree's index is then saved to
    /// `.trellis/index.bin` through the writer `trellis index` uses, so the
    /// overview reads the index this answer drew (#654, the owner's
    /// decision of 2026-10-12); nothing else is written.
    ///
    /// Answered from another thread, as `compact` is: indexing a large
    /// repository takes seconds, and on this thread it would hold up the
    /// `approve` or `abort` a waiting run needs. `edges` and `ref` (the
    /// Atlas, owner request of 2026-10-11) are checked here, before that
    /// thread or any git starts.
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
        let edges = match crate::atlas::edges(cmd.get("edges")) {
            Ok(e) => e,
            Err(e) => {
                self.respond(id, "get_codebase", Err(e));
                return;
            }
        };
        let rev = match cmd.get("ref").map(crate::atlas::revision).transpose() {
            Ok(r) => r,
            Err(e) => {
                self.respond(id, "get_codebase", Err(e));
                return;
            }
        };
        if let Err(e) = repository_root(&self.root, "get_codebase") {
            self.respond(id, "get_codebase", Err(e));
            return;
        }
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::atlas::codebase(&root, limit, edges, rev.as_deref());
            respond_on(&sink, id.as_deref(), "get_codebase", result);
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
        if let Err(e) = repository_root(&self.root, "get_radius") {
            self.respond(id, "get_radius", Err(e));
            return;
        }
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        let symbol = symbol.to_string();
        self.waiters.push(std::thread::spawn(move || {
            let sources = trellis_index::read_sources(&root);
            let index = trellis_index::build_from_sources(&sources);
            let result =
                crate::index_cmd::radius_json(&index, &sources, &symbol).map_err(|e| e.to_string());
            respond_on(&sink, id.as_deref(), "get_radius", result);
        }));
    }

    /// `get_symbols`: every definition the index holds, or one module's,
    /// with its lines and three call-site counts (the Atlas, owner request
    /// of 2026-10-11); nothing is written. The index is built as
    /// `get_codebase` builds it, on its own thread for the same reason, and
    /// a root with no `.git` is refused first, as there (#559).
    fn get_symbols(&mut self, cmd: &Value, id: Option<&str>) {
        let module = match crate::atlas::module_arg(cmd.get("module")) {
            Ok(m) => m,
            Err(e) => {
                self.respond(id, "get_symbols", Err(e));
                return;
            }
        };
        if let Err(e) = repository_root(&self.root, "get_symbols") {
            self.respond(id, "get_symbols", Err(e));
            return;
        }
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::atlas::symbols(&root, module.as_deref());
            respond_on(&sink, id.as_deref(), "get_symbols", result);
        }));
    }

    /// `get_history`: HEAD's first-parent commits, the tags in its history
    /// and each path's churn, from git through `trellis_vcs` (the Atlas,
    /// owner request of 2026-10-11); nothing is written. Its own thread, as
    /// `get_codebase`'s, since a long history takes a while to walk; a
    /// root with no `.git` is refused first, as there (#559).
    fn get_history(&mut self, cmd: &Value, id: Option<&str>) {
        let counts = crate::atlas::count_arg(cmd, "commits", 20)
            .and_then(|c| Ok((c, crate::atlas::count_arg(cmd, "tags", 12)?)));
        let (commits, tags) = match counts {
            Ok(n) => n,
            Err(e) => {
                self.respond(id, "get_history", Err(e));
                return;
            }
        };
        if let Err(e) = repository_root(&self.root, "get_history") {
            self.respond(id, "get_history", Err(e));
            return;
        }
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::atlas::history(&root, commits, tags);
            respond_on(&sink, id.as_deref(), "get_history", result);
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

    /// `list_paths`: the paths `@` completion offers that start with
    /// `prefix`, from `at_file_paths`, so ignored files are never listed and
    /// git is spawned only through `trellis_vcs` (ADR 0023); nothing is
    /// written. Answered from another thread, as `get_queue` is, so a
    /// waiting run's `approve` or `abort` is still read at once.
    fn list_paths(&mut self, cmd: &Value, id: Option<&str>) {
        let prefix = match path_prefix(cmd) {
            Ok(p) => p.to_string(),
            Err(e) => {
                self.respond(id, "list_paths", Err(e));
                return;
            }
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = at_file_paths(&root)
                .map(|paths| json!({"paths": starting_with(paths, &prefix)}))
                .map_err(|e| format!("{e:#}"));
            respond_on(&sink, id.as_deref(), "list_paths", result);
        }));
    }

    /// `read_file`: one file's text for a client's whole-file view, refused
    /// for anything `list_paths` would not offer or that resolves outside
    /// the root or into `.git` (ADR 0058 §4, `read_repo_file`). The client's
    /// read, not the model's: nothing it answers enters a prompt, and
    /// nothing is written. It reads the working tree as it is when asked,
    /// from another thread, as `list_paths` does.
    fn read_file(&mut self, cmd: &Value, id: Option<&str>) {
        let path = match read_file_path(cmd) {
            Ok(p) => p.to_string(),
            Err(e) => {
                self.respond(id, "read_file", Err(e));
                return;
            }
        };
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = read_repo_file(&root, &path);
            respond_on(&sink, id.as_deref(), "read_file", result);
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

    /// `vault_check`: what `trellis vault check` prints, from the same
    /// report, as data; nothing is written. `--fix` is a vault write, and
    /// every vault write stays a `trellis vault` command (#70), so a `fix`
    /// field is refused by name rather than ignored: a client that sent it
    /// would otherwise read the unrepaired report as the repaired one. The
    /// CLI's path exits the process on a refusing gate and is not reused.
    /// Answered from another thread, as `vault_search` is.
    fn vault_check(&mut self, cmd: &Value, id: Option<&str>) {
        if cmd.get("fix").is_some() {
            self.respond(
                id,
                "vault_check",
                Err(
                    "`fix` is not an rpc field: rpc reads the vault and never repairs it; \
                     `trellis vault check --fix` does (#70)"
                        .into(),
                ),
            );
            return;
        }
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        self.waiters.push(std::thread::spawn(move || {
            let result = crate::vault_cmd::check_json(&root).map_err(|e| format!("{e:#}"));
            respond_on(&sink, id.as_deref(), "vault_check", result);
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
        // A steer is user text for a model request, and a pending classify
        // holds the slot one would go to: refused as a `prompt` is (#518).
        if kind == "steer" && self.classify_pending() {
            self.respond(id, kind, Err(ONE_AT_A_TIME.into()));
            return;
        }
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

    /// `pause`: answered from another thread once the loop holds, with the
    /// turn of the model call it holds before, or, if the run ends first or
    /// a `resume` withdraws it, with why it did not. On this thread only the
    /// request is made, as `abort`'s is, so `approve` and `abort` are still
    /// read while the loop finishes its turn (ADR 0058 §1).
    fn pause(&mut self, id: Option<&str>) {
        if !self.in_flight() {
            self.respond(id, "pause", Err("no run in flight".into()));
            return;
        }
        let Some(r) = &self.running else {
            return;
        };
        let answer = match r.current().hold.pause() {
            Ok(a) => a,
            Err(e) => {
                self.respond(id, "pause", Err(e.to_string()));
                return;
            }
        };
        let (sink, id) = (self.sink.clone(), id.map(str::to_string));
        self.waiters.push(std::thread::spawn(move || {
            let result = match answer.blocking_recv() {
                Ok(Ok(turn)) => Ok(json!({"turn": turn})),
                Ok(Err(e)) => Err(e),
                // The hold answers every pause it holds before it is
                // dropped; this is the hold gone some other way.
                Err(_) => Err(HOLD_ENDED.into()),
            };
            respond_on(&sink, id.as_deref(), "pause", result);
        }));
    }

    /// `resume`: answered at once. The loop says how long it was held in
    /// the `resumed` event it sends as it goes on.
    fn resume(&mut self) -> Result<Value, String> {
        if !self.in_flight() {
            return Err("no run in flight".into());
        }
        let Some(r) = &self.running else {
            return Err("no run in flight".into());
        };
        r.current()
            .hold
            .resume()
            .map(|()| Value::Null)
            .map_err(|e| e.to_string())
    }

    /// `set_budget` (ADR 0058 §2): answered at once with every ceiling after
    /// the raise, which the loop enforces from its next budget check and
    /// announces there with `run budget`. Refused whole, changing nothing,
    /// when any value is not a raise or not a ceiling's.
    fn set_budget(&mut self, cmd: &Value) -> Result<Value, String> {
        if !self.in_flight() {
            return Err("no run in flight".into());
        }
        let Some(r) = &self.running else {
            return Err("no run in flight".into());
        };
        let raise = budget_raise(cmd)?;
        let b = r
            .current()
            .budget
            .raise(&raise)
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "max_iterations": b.max_iterations,
            "max_wall_clock_ms": u64::try_from(b.max_wall_clock.as_millis()).unwrap_or(u64::MAX),
            "max_completion_tokens": b.max_tokens,
        }))
    }

    /// `withdraw` (ADR 0058 §3): answered at once with `{id, type}` once the
    /// steer or follow-up sent with the command's `id` is taken back, as
    /// `approve` names its question by its `id`. The withdrawn message is
    /// gone from `get_state`'s `pending` and `abort`'s `undelivered`.
    fn withdraw(&mut self, cmd: &Value, id: Option<&str>) -> Result<Value, String> {
        let id = match (id, cmd.get("id")) {
            (Some(id), _) if !id.is_empty() => id,
            (_, None) => {
                return Err(
                    "withdraw needs the \"id\" the steer or follow_up was sent with".into(),
                );
            }
            (_, Some(v)) => {
                return Err(format!(
                    "withdraw's \"id\" is the id a steer or follow_up was sent with, \
                     a non-empty string; got {v}"
                ));
            }
        };
        if !self.in_flight() {
            return Err("no run in flight".into());
        }
        let Some(r) = &self.running else {
            return Err("no run in flight".into());
        };
        let kind = r.withdraw(id)?;
        Ok(json!({"id": id, "type": kind}))
    }

    /// `abort`: answered once the run has ended, with everything sent on
    /// the steering channel and never delivered, in the order received.
    ///
    /// After, not at once, because until the run has taken the abort path
    /// it can still deliver a steer to a request, and an answer given before
    /// then could name as undelivered a steer the model was sent.
    fn abort(&mut self, id: Option<&str>) {
        // A pending classify holds the slot `prompt` is refused for, in
        // words that offer `abort` (#522). It is dropped as `quit` drops
        // it; no steer can have reached it, so none is undelivered.
        if self.classify_pending()
            && let Some(c) = self.classifying.take()
        {
            let _ = c.cancel.send(());
            let _ = c.thread.join();
            // The model answered before the cancel was seen, and that
            // answer was written: there was nothing left to abort.
            if c.answered.load(Ordering::SeqCst) {
                self.respond(id, "abort", Err("no run in progress".into()));
            } else {
                self.respond(id, "abort", Ok(json!({"undelivered": []})));
            }
            return;
        }
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

    /// For a `prompt` read while `in_flight` was true: whether the slot is
    /// free once the run's thread has finished its tail.
    ///
    /// Before `run_end` it is not, at once. After it the thread has only
    /// its cleanup and the follow-up check left, none of which waits on
    /// this reader, so it waits for one of two outcomes: the thread
    /// finished with nothing queued (free; it has been joined, so the new
    /// run is the only one), or a follow-up started (not free: it was
    /// queued first, and runs first).
    fn free_after_run_end(&mut self) -> bool {
        loop {
            match &self.running {
                None => return true,
                Some(r) if !r.ended.load(Ordering::SeqCst) => return false,
                Some(_) => {}
            }
            if !self.in_flight() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Whether a `classify` is waiting on its model. Once it has its answer
    /// it is not: the thread has only the response to write, and is joined.
    fn classify_pending(&mut self) -> bool {
        match &self.classifying {
            None => false,
            Some(c) if !c.thread.is_finished() && !c.answered.load(Ordering::SeqCst) => true,
            Some(_) => {
                if let Some(c) = self.classifying.take() {
                    let _ = c.thread.join();
                }
                false
            }
        }
    }

    /// The one gate a `prompt` and a `classify` pass: refused while a run
    /// is in flight (after #495's wait past `run_end`) or a classify is
    /// pending, so one model user holds the slot at a time.
    fn one_at_a_time(&mut self) -> Result<(), String> {
        if self.classify_pending() || (self.in_flight() && !self.free_after_run_end()) {
            return Err(ONE_AT_A_TIME.into());
        }
        Ok(())
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
    fn dispatch_command(&mut self, cmd: &Value) -> bool {
        let id = cmd.get("id").and_then(Value::as_str).map(str::to_string);
        let id = id.as_deref();
        let kind = cmd.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "prompt" => {
                let r = self.prompt(cmd, id);
                self.respond(id, kind, r.map(|()| Value::Null));
            }
            "abort" => self.abort(id),
            "pause" => self.pause(id),
            "resume" => {
                let r = self.resume();
                self.respond(id, kind, r);
            }
            "set_budget" => {
                let r = self.set_budget(cmd);
                self.respond(id, kind, r);
            }
            "withdraw" => {
                let r = self.withdraw(cmd, id);
                self.respond(id, kind, r);
            }
            "steer" | "follow_up" => self.steer(cmd, id, kind),
            "get_state" => {
                let running = self.in_flight();
                let pending: Vec<String> = self
                    .approvals
                    .lock()
                    .map(|a| a.keys().cloned().collect())
                    .unwrap_or_default();
                // After `in_flight`, which drops a finished run: with none in
                // flight nothing is waiting to be delivered.
                let waiting = self
                    .running
                    .as_ref()
                    .map(Running::pending)
                    .unwrap_or_default();
                let data = json!({
                    "running": running,
                    // Each steer and follow-up not yet delivered (#801).
                    "pending": waiting,
                    // Not a run, but it holds the slot `prompt` needs (#522).
                    "classifying": self.classify_pending(),
                    "run_request_id": self.running.as_ref().and_then(|r| r.current().id),
                    "model": self.model(None).ok(),
                    "root": self.root.display().to_string(),
                    "last_run": self.last.lock().ok().and_then(|l| l.clone()),
                    "pending_approvals": pending,
                    // The run's own decision, null before one (#269).
                    "tier": self.tier.lock().ok().and_then(|t| t.clone()),
                });
                self.respond(id, kind, Ok(data));
            }
            "get_messages" => {
                let turns = TURNS.lock().map(|t| t.clone()).unwrap_or_default();
                self.respond(id, kind, Ok(json!({"messages": turns})));
            }
            // Inline, as `get_state` is: a few small files, and nothing
            // that could keep a waiting run's `approve` unread.
            "get_session_stats" => {
                let ids = self
                    .ran
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                self.respond(id, kind, Ok(session_stats(&self.root, &ids)));
            }
            // Model output, sent as a string and never read as anything
            // else (rule 10).
            "get_last_assistant_text" => {
                let text = self
                    .done
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                self.respond(id, kind, Ok(json!({"text": text})));
            }
            "set_model" => {
                let r = self.set_model(cmd);
                self.respond(id, kind, r);
            }
            "list_models" => self.list_models(id),
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
                // Dropped, not waited for: the model may take `[model]
                // timeout_s` to answer, and a client that sends `quit` does
                // not kill (ADR 0048 §2). The join is the thread leaving its
                // `select`; it writes nothing.
                if let Some(c) = self.classifying.take() {
                    let _ = c.cancel.send(());
                    let _ = c.thread.join();
                }
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
            "get_symbols" => self.get_symbols(cmd, id),
            "get_history" => self.get_history(cmd, id),
            "get_queue" => self.get_queue(id),
            "list_paths" => self.list_paths(cmd, id),
            "read_file" => self.read_file(cmd, id),
            "vault_search" => self.vault_search(cmd, id),
            "vault_graph" => self.vault_graph(cmd, id),
            "vault_check" => self.vault_check(cmd, id),
            "classify" => {
                if let Err(e) = self.classify(cmd, id) {
                    self.respond(id, kind, Err(e));
                }
            }
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

    /// The model for a run: the command's, then `set_model`'s, then the
    /// configured spec as it was confirmed, then config.
    fn model(&self, given: Option<&str>) -> Result<String, String> {
        given
            .map(str::to_string)
            .or_else(|| self.model.clone())
            .or_else(|| self.confirmed.clone())
            .or_else(|| config::RepoConfig::load(&self.root).ok()?.model())
            .ok_or_else(|| "no model: pass \"model\", use set_model, or set [model] spec".into())
    }

    /// Whether this session has a model `prompt` may run on: one named by
    /// `set_model {model}`, or the configured one confirmed (#564).
    fn has_model(&self) -> bool {
        self.model.is_some() || self.confirmed.is_some()
    }

    /// The repository's configuration over the user file `sources` names:
    /// what `list_models` reports and `set_model {configured}` confirms.
    fn config(&self) -> Result<config::RepoConfig, String> {
        config::RepoConfig::load_with_user(&self.root, self.sources.user_file.as_deref())
            .map_err(|e| format!("{e:#}"))
    }

    /// `set_model`: this session's model (#564, ADR 0048 section 5 as
    /// amended), in one of two forms.
    ///
    /// `{model, save?}` names a spec, validated as `--model` is, and counts
    /// as the user's choice (ADR 0024 §3). With `save: true` it is also
    /// written to the user's own config file by `first_run`'s writer, never
    /// to `.trellis/config.toml`; a write that fails sets nothing, so the
    /// client is never told a model is in force that was not saved as asked.
    ///
    /// `{configured: true}` confirms the configured spec, as it reads now,
    /// and pins it for this session. It names no model, so a repository's
    /// hosted spec confirmed this way is still the repository's choice and
    /// meets `hosted`'s key rule as one: the desktop forwards it for that
    /// reason, where it refuses a repository's hosted spec sent as `model`.
    fn set_model(&mut self, cmd: &Value) -> Result<Value, String> {
        let flag = |k: &str| match cmd.get(k) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(v) => Err(format!("`{k}` is true or false, not {v}")),
        };
        let (save, configured) = (flag("save")?, flag("configured")?);
        match (cmd.get("model"), configured) {
            (Some(_), true) => Err(
                "set_model takes {\"model\": \"<spec>\"} or {\"configured\": true}, not both"
                    .into(),
            ),
            (None, true) => {
                if save {
                    return Err("the configured spec is already in a config file; `save` \
                         takes a model you name with {\"model\": \"<spec>\"}"
                        .into());
                }
                let spec = self.config()?.model().ok_or_else(|| {
                    "nothing is configured to confirm: neither the repository's config nor \
                     yours sets [model] spec; choose one with {\"model\": \"<spec>\"}"
                        .to_string()
                })?;
                validate_model_spec(&spec)?;
                self.model = None;
                self.confirmed = Some(spec.clone());
                Ok(json!({"model": spec, "configured": true, "saved_to": null}))
            }
            (Some(Value::String(m)), false) => {
                validate_model_spec(m)?;
                let saved_to = if save {
                    Some(self.save_model(m)?)
                } else {
                    None
                };
                self.model = Some(m.clone());
                self.confirmed = None;
                Ok(json!({"model": m, "configured": false,
                          "saved_to": saved_to.map(|f| f.display().to_string())}))
            }
            (_, false) => Err("set_model needs {\"model\": \"<spec>\"}, or \
                 {\"configured\": true} to confirm the configured one"
                .into()),
        }
    }

    /// Writes `spec` to the user's config file as `first_run` does, and says
    /// where. Refused with no user config directory, and refused when that
    /// file is the repository's own `.trellis/config.toml` (a
    /// `TRELLIS_CONFIG_DIR` pointed into the repository), since where a model
    /// is, is the user's to say and never the repository's (ADR 0024 §3).
    fn save_model(&self, spec: &str) -> Result<PathBuf, String> {
        let Some(file) = &self.sources.user_file else {
            return Err("no user config directory to save the model in: set \
                 TRELLIS_CONFIG_DIR; nothing was written and no model was set"
                .into());
        };
        let repo = self.root.join(".trellis").join("config.toml");
        if same_place(file, &repo) {
            return Err(format!(
                "your user config file {} is the repository's own .trellis/config.toml; a \
                 model you choose is saved to your config, never the repository's (ADR 0024 \
                 §3), so nothing was written and no model was set",
                file.display()
            ));
        }
        // A colibri `list_models` offered needs its `[[providers]]` entry
        // beside the spec (#625), written first for the reason `first_run`
        // writes it first: an entry refused (one named `colibri` already,
        // at another address) leaves no spec naming a provider that points
        // elsewhere. Only for a spec this session offered: the base is the
        // one its probe read, and nothing else names one.
        let offered = self
            .colibri_offers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(spec)
            .cloned();
        if let Some(base) = offered {
            crate::user_config::set_colibri_provider(file, &base)
                .map_err(|e| format!("{e:#}; no model was set"))?;
        }
        crate::user_config::set_model_spec(file, spec)
            .map_err(|e| format!("{e:#}; no model was set"))?;
        Ok(file.clone())
    }

    /// `list_models`: what a client offers the user to choose from (#564).
    /// The configured spec; the specs `first_run`'s probes find, bounded and
    /// checked as `first_run` bounds and checks them, with every address
    /// tried and what it answered; and the user config's `[[providers]]` by
    /// name and the variable a key is read from. No variable is read, so no
    /// key value can be in the answer. The probes take up to
    /// `first_run::BOUND`, so they run on another thread, as `get_queue`
    /// does, and a waiting run's `approve` or `abort` is still read at once.
    fn list_models(&mut self, id: Option<&str>) {
        let cfg = match self.config() {
            Ok(c) => c,
            Err(e) => {
                self.respond(id, "list_models", Err(e));
                return;
            }
        };
        let configured = cfg.model();
        // `key_variable`, not `key_env`: the desktop holds no `key_env`
        // anywhere in its source (desktop-guards.sh), and this is the name
        // of a variable, never its value. Null for a protocol trellis does
        // not know, which a run would refuse by name.
        let providers: Vec<Value> = cfg
            .provenance
            .providers
            .iter()
            .map(|p| {
                let variable = if p.key_env.is_empty() {
                    trellis_model::Protocol::parse_config(&p.protocol)
                        .map(|pr| pr.default_key_env().to_string())
                } else {
                    Some(p.key_env.clone())
                };
                json!({"name": p.name, "key_variable": variable})
            })
            .collect();
        let runtimes = self.sources.runtimes.clone();
        let (sink, id) = (self.sink.clone(), id.map(str::to_string));
        let colibri_offers = self.colibri_offers.clone();
        self.waiters.push(std::thread::spawn(move || {
            let probed = crate::first_run::probe(&runtimes, crate::first_run::BOUND);
            {
                let mut known = colibri_offers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                for o in crate::first_run::offers(&probed) {
                    if let Some(base) = o.colibri_base {
                        known.insert(o.spec, base);
                    }
                }
            }
            let tried: Vec<Value> = probed
                .iter()
                .map(|p| {
                    json!({
                        "runtime": p.runtime.name,
                        "address": format!("http://{}{}", p.runtime.addr, p.runtime.api.path()),
                        "error": p.answer.as_ref().err(),
                    })
                })
                .collect();
            let data = json!({
                "configured": configured,
                "local": crate::first_run::specs(&probed),
                "probed": tried,
                "providers": providers,
            });
            respond_on(&sink, id.as_deref(), "list_models", Ok(data));
        }));
    }

    /// `classify`: whether `text` is a question or a task, by `ask::classify`,
    /// the rule the TUI routes on, so every client routes by one rule (ADR
    /// 0052 §2.3). It starts no run and writes nothing; a task is only named,
    /// and starting it stays `prompt`'s.
    ///
    /// The model is resolved as `prompt` resolves one with no `model` field:
    /// `set_model`'s, else `[model] spec`, and it is the user's choice only
    /// when `set_model` named it, so a key goes only where `prompt` would
    /// send it (ADR 0024 §3; `ask::classify` resolves through `hosted`).
    ///
    /// Refused while a run is in flight, by `prompt`'s gate and in its words:
    /// an answer then could be read as one about the run. Asked on its own
    /// thread (#518), so the reader still reads `quit`; while it is pending
    /// it holds that same gate, so no `prompt`, `steer` or second `classify`
    /// can put a model request beside it. `abort` and `quit` drop it
    /// unanswered (#522); end of input waits for its answer. The request is
    /// bounded by `[model] timeout_s`. `text` is the client's and goes to
    /// the model as data; the answer is model text and goes back as a JSON
    /// string through `frame`, never as a record.
    ///
    /// Everything cheap is checked here, as `prompt` checks it, and an
    /// error is returned for the caller to answer; once the thread starts,
    /// it answers.
    fn classify(&mut self, cmd: &Value, id: Option<&str>) -> Result<(), String> {
        self.one_at_a_time()?;
        let text = match cmd.get("text").and_then(Value::as_str) {
            Some(t) if !t.trim().is_empty() => t.to_string(),
            _ => {
                return Err(
                    "classify needs {\"text\": \"<text>\"}, a string and not an empty one".into(),
                );
            }
        };
        let model_from_user = self.model.is_some();
        let model = self.model(None)?;
        validate_model_spec(&model)?;
        let spec: trellis_model::ModelSpec = model.parse().map_err(|e| format!("{e}"))?;
        // With `?`, as the TUI loads it: a default in its place would refuse
        // the user's own provider as the repository's choice.
        let cfg = config::RepoConfig::load(&self.root).map_err(|e| format!("{e:#}"))?;
        let answered = Arc::new(AtomicBool::new(false));
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        let (sink, id, root) = (self.sink.clone(), id.map(str::to_string), self.root.clone());
        let done = answered.clone();
        let thread = std::thread::spawn(move || {
            let files = trellis_index::discover_relative(&root);
            let asked = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("starting the runtime to ask the model: {e}"))
                .map(|rt| {
                    let ask = crate::ask::classify(&spec, &text, &files, &cfg, model_from_user);
                    let asked = rt.block_on(async {
                        tokio::select! {
                            r = ask => Some(r),
                            // `quit`; or the sender dropped, which only
                            // follows a join, so never while it is asked.
                            _ = cancelled => None,
                        }
                    });
                    // Nothing the dropped request left behind is waited on.
                    rt.shutdown_background();
                    asked
                });
            let result = match asked {
                // Dropped by `quit`: no answer, whole or partial, is written.
                Ok(None) => return,
                Ok(Some(Ok(crate::ask::Intent::Answer(answer)))) => {
                    Ok(json!({"intent": "question", "answer": answer}))
                }
                Ok(Some(Ok(crate::ask::Intent::Task))) => Ok(json!({"intent": "task"})),
                Ok(Some(Err(e))) => Err(format!("{e:#}")),
                Err(e) => Err(e),
            };
            done.store(true, Ordering::SeqCst);
            respond_on(&sink, id.as_deref(), "classify", result);
        });
        self.classifying = Some(Classifying {
            thread,
            answered,
            cancel,
        });
        Ok(())
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
        self.one_at_a_time()?;
        if !self.has_model() {
            return Err(CHOOSE_FIRST.into());
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
        validate_model_spec(&model)?;
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
        let ended = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(Current::new(id.map(str::to_string))));
        let leftover: Arc<Mutex<Vec<Value>>> = Arc::default();
        let queue: FollowUps = Arc::default();
        let chain = Chain {
            root: self.root.clone(),
            sink: self.sink.clone(),
            mode: self.mode,
            approvals: self.approvals.clone(),
            next_approval: self.next_approval.clone(),
            last: self.last.clone(),
            ran: self.ran.clone(),
            done: self.done.clone(),
            tier: self.tier.clone(),
            input_closed: self.input_closed.clone(),
            abort: abort.clone(),
            ended: ended.clone(),
            current: current.clone(),
            queue: queue.clone(),
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
            ended,
            current,
            queue,
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
    ran: Ran,
    done: output::DoneSlot,
    tier: output::TierSlot,
    input_closed: Arc<AtomicBool>,
    /// One for the chain: an abort stops the run in flight and every
    /// follow-up behind it.
    abort: Arc<output::Abort>,
    /// `Running::ended`.
    ended: Arc<AtomicBool>,
    current: Arc<Mutex<Current>>,
    /// Follow-ups waiting for the run they were sent during to end and the
    /// ones before them to run. Held from a port's close to the next run's
    /// start (#801).
    queue: FollowUps,
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
        // The first run's `Current` was made with the `prompt`, so a
        // `steer` read before this thread got here still reaches it.
        if self.abort.requested() {
            return vec![queued_json(self.current().id, task)];
        }
        let mut task = task;
        loop {
            let cur = self.current();
            self.run_one_task(&task, &cur);
            // Held until the next run is current, so a follow-up leaving
            // the port's record is in the queue whenever `get_state` reads
            // either, and is gone from both only once its run starts.
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            let unsent = cur.steering.close();
            if self.abort.requested() {
                // Queued before this run's own, so received before them.
                let mut all: Vec<Value> = queue
                    .drain(..)
                    .map(|(id, text)| queued_json(id, text))
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
            let Some((id, next)) = queue.pop_front() else {
                return Vec::new();
            };
            // Before the run is replaced, so a `prompt` waiting on this
            // thread sees a run in flight from here on.
            self.ended.store(false, Ordering::SeqCst);
            *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Current {
                follow_up: true,
                ..Current::new(id)
            };
            task = next;
        }
    }

    fn current(&self) -> Current {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// One run, as `trellis run` runs it.
    fn run_one_task(&self, task: &str, cur: &Current) {
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
            // For `get_last_assistant_text`; the run leaves it as it was
            // unless it ends on `done`.
            done: Some(self.done.clone()),
            // `--image` is `trellis run`'s alone (ADR 0041).
            images: Vec::new(),
            steer_watch: Some(cur.steering.clone()),
            // `--agents` above 1 is `trellis run`'s alone (#466).
            fanout: None,
            run_ids: Some(self.ran.clone()),
            run_ended: Some(self.ended.clone()),
            tier: Some(self.tier.clone()),
            // This run's own, for its `run_end`.
            elapsed_ms: std::sync::OnceLock::new(),
            hold: Some(cur.hold.clone()),
            budget: Some(cur.budget.clone()),
        };
        // Cleared first, so a run that stops before it decides a tier
        // leaves none, rather than the last run's for another model.
        if let Ok(mut t) = self.tier.lock() {
            *t = None;
        }
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
                trust_root: None,
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
        cleanup_delay();
        let _ = fwd.join();
        // A `compact` still waiting will get no model request to act on.
        cur.compact
            .close("the run ended before its next model request; nothing was compacted");
        // After `run_end`, which `cmd_run_with` framed: a pause the run never
        // reached is answered after the frame that says how it ended.
        cur.hold.close(HOLD_ENDED);
        // The loop closes it as it returns; this is for a run whose loop
        // never started or never returned (an error before it, an abort).
        cur.budget.close(BUDGET_ENDED);
        // Questions still open when the run ended are dropped, which the
        // gate reads as a no; none can be answered now.
        if let Ok(mut a) = self.approvals.lock() {
            a.clear();
        }
        if let Ok(mut l) = self.last.lock() {
            *l = Some(run_end_response(&r, rid.as_deref()));
        }
    }
}

/// A test seam (#495): widens the window between a run's `run_end` and
/// its thread finishing, as a loaded machine does, so `tests/modes.rs`
/// can send a `prompt` inside it through the binary. `cfg(test)` is not
/// set on the binary those tests spawn, hence `debug_assertions`; a
/// release build has no such read.
#[cfg(debug_assertions)]
fn cleanup_delay() {
    let ms = std::env::var("TRELLIS_TEST_RPC_CLEANUP_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(ms) = ms {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(not(debug_assertions))]
fn cleanup_delay() {}

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

/// `Ok` when `root` is a repository, by the test every other root check
/// in trellis makes: a `.git` entry, a directory or the file a worktree or
/// submodule has. Otherwise the refusal `command` answers with.
///
/// `get_codebase` and `get_radius` index whatever is under the root, and
/// `rpc` started outside a repository falls back to the working directory:
/// from a home folder that walk read every file under it before anyone
/// could tell no repository had been found (ADR 0048 §10, amended for
/// #557). Checked before the walk's thread is started, so nothing under the
/// root is read.
fn repository_root(root: &Path, command: &str) -> Result<(), String> {
    if root.join(".git").exists() {
        return Ok(());
    }
    Err(format!(
        "{command} refused: {} has no .git entry, and {command} reads only a repository \
         (ADR 0048 section 10), so nothing was indexed; open trellis on a repository's root",
        root.display()
    ))
}

/// Whether two file paths name one file, by their directories resolved: the
/// files themselves may not exist yet. Unresolvable, they are compared as
/// written.
fn same_place(a: &Path, b: &Path) -> bool {
    let resolved = |p: &Path| -> Option<PathBuf> {
        let dir = std::fs::canonicalize(p.parent()?).ok()?;
        Some(dir.join(p.file_name()?))
    };
    match (resolved(a), resolved(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// `get_state`'s view of how the last run ended: `run_end`'s outcome
/// fields, without its trail id or the loop's spend.
fn run_end_response(r: &Result<RunExit>, id: Option<&str>) -> Value {
    let mut v = output::run_end(r, None, None, id);
    if let Some(o) = v.as_object_mut() {
        o.remove("type");
    }
    v
}

/// A spec that parses, and for a script, a file that exists.
fn validate_model_spec(spec: &str) -> Result<(), String> {
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
    use crate::testutil::sh;
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
        /// A server that probes no runtime and has no user config file.
        fn start(root: &Path) -> Self {
            Self::start_with(
                root,
                ModelSources {
                    runtimes: Vec::new(),
                    user_file: None,
                },
            )
        }

        fn start_with(root: &Path, sources: ModelSources) -> Self {
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
                serve_with(&root, input, sink, Mode::Quiet, sources)
            });
            Self {
                tx: Some(tx),
                out,
                server: Some(server),
            }
        }

        /// [`Client::start`], with `spec` chosen as the session's model, as
        /// a client must before its first `prompt` (#564).
        fn chosen(root: &Path, spec: &str) -> Self {
            let c = Self::start(root);
            c.choose(spec);
            c
        }

        /// Sends `set_model {model: spec}` and waits for it to be taken.
        fn choose(&self, spec: &str) {
            self.send(json!({"id": "choose", "type": "set_model", "model": spec}));
            let r = self.wait_for(|r| r["id"] == "choose");
            assert_eq!(r["success"], true, "{r}");
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
        let c = Client::chosen(&dir, &spec);
        c.send(json!({"id": "1", "type": "prompt", "task": "change f", "model": spec}));
        let rs = c.finish();
        // The prompt's answer is the one after the model choice's.
        let resp = of_type(&rs, "response");
        assert_eq!(resp.len(), 2, "{rs:?}");
        assert_eq!(resp[0]["id"], "choose");
        assert_eq!(resp[1]["id"], "1");
        assert_eq!(resp[1]["success"], true, "{rs:?}");
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
        let c = Client::chosen(&dir, &spec);
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
        //
        // The session's model is the configured one confirmed (#564), not
        // one named with `set_model {model}`: that would make every later
        // prompt the client's choice, and this would pass whether or not
        // the prompt's own `model` counted.
        let (dir, spec) = fixture("prompt-model", "");
        std::fs::write(
            dir.join(".trellis/config.toml"),
            format!("[model]\nspec = {}\n", toml::Value::String(spec)),
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "configure the script");
        let c = Client::start(&dir);
        c.send(json!({"id": "c", "type": "set_model", "configured": true}));
        let confirmed = c.wait_for(|r| r["id"] == "c");
        assert_eq!(confirmed["success"], true, "{confirmed}");
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
            confirmed: None,
            running: None,
            classifying: None,
            last: Arc::default(),
            ran: Arc::default(),
            done: output::DoneSlot::default(),
            tier: output::TierSlot::default(),
            approvals: Arc::default(),
            next_approval: Arc::default(),
            input_closed: Arc::default(),
            waiters: Vec::new(),
            sources: ModelSources {
                runtimes: Vec::new(),
                user_file: None,
            },
            colibri_offers: Arc::default(),
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
        let c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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
        let mut c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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
        let mut c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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
    fn get_session_stats_names_a_trail_that_did_not_read_back() {
        // The risk: a run whose trail is gone or edited into nonsense left
        // out of the totals unsaid, so they read as the whole session's.
        let d = tempfile::tempdir().unwrap();
        let runs = d.path().join(".trellis/runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("edited.json"), "{ not a trail").unwrap();
        let ids = ["edited".to_string(), "gone".to_string()];
        assert_eq!(
            session_stats(d.path(), &ids),
            json!({"models": [], "runs_not_counted": 2})
        );
    }

    #[test]
    fn rpc_contract_refusal_starts_no_indexing_thread() {
        // The risk: a root with no `.git` (the `current_dir()` fallback
        // from a home folder) refused only after `read_sources` had walked
        // it. The refusal is answered before any thread is started, so
        // nothing under the root is read.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("loose.py"), "def f():\n    return 1\n").unwrap();
        let out: Arc<Mutex<Vec<u8>>> = Arc::default();
        let mut s = Server {
            root: d.path().to_path_buf(),
            sink: out.clone(),
            mode: Mode::Quiet,
            model: None,
            confirmed: None,
            running: None,
            classifying: None,
            last: Arc::default(),
            ran: Arc::default(),
            done: output::DoneSlot::default(),
            tier: output::TierSlot::default(),
            approvals: Arc::default(),
            next_approval: Arc::default(),
            input_closed: Arc::default(),
            waiters: Vec::new(),
            sources: ModelSources {
                runtimes: Vec::new(),
                user_file: None,
            },
            colibri_offers: Arc::default(),
        };
        for cmd in [
            json!({"id": "g", "type": "get_codebase"}),
            json!({"id": "r", "type": "get_radius", "symbol": "f"}),
        ] {
            s.dispatch_command(&cmd);
            assert!(s.waiters.is_empty(), "{cmd} started a thread");
        }
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let answers: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(answers.len(), 2, "{text}");
        for a in &answers {
            assert_eq!(a["success"], false, "{a}");
            assert!(
                a["error"].as_str().is_some_and(|e| e.contains("no .git")),
                "{a}"
            );
        }
    }

    #[test]
    fn rpc_atlas_refusals_start_no_thread() {
        // The risk: the Atlas's reads (owner request of 2026-10-11) walking
        // a root with no `.git` before refusing it (#559), or starting git
        // on an argument they then refuse.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("loose.py"), "def f():\n    return 1\n").unwrap();
        let out: Arc<Mutex<Vec<u8>>> = Arc::default();
        let mut s = Server {
            root: d.path().to_path_buf(),
            sink: out.clone(),
            mode: Mode::Quiet,
            model: None,
            confirmed: None,
            running: None,
            classifying: None,
            last: Arc::default(),
            ran: Arc::default(),
            done: output::DoneSlot::default(),
            tier: output::TierSlot::default(),
            approvals: Arc::default(),
            next_approval: Arc::default(),
            input_closed: Arc::default(),
            waiters: Vec::new(),
            sources: ModelSources {
                runtimes: Vec::new(),
                user_file: None,
            },
            colibri_offers: Arc::default(),
        };
        let cmds = [
            json!({"id": "s", "type": "get_symbols"}),
            json!({"id": "m", "type": "get_symbols", "module": ["a"]}),
            json!({"id": "r", "type": "get_codebase", "ref": "HEAD"}),
            json!({"id": "o", "type": "get_codebase", "ref": "--output=x"}),
            json!({"id": "e", "type": "get_codebase", "edges": "most"}),
            json!({"id": "h", "type": "get_history"}),
            json!({"id": "c", "type": "get_history", "commits": "all"}),
        ];
        for cmd in &cmds {
            s.dispatch_command(cmd);
            assert!(s.waiters.is_empty(), "{cmd} started a thread");
        }
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let answers: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(answers.len(), cmds.len(), "{text}");
        for a in &answers {
            assert_eq!(a["success"], false, "{a}");
            assert!(
                a["error"].as_str().is_some_and(|e| e.contains(" refused")),
                "{a}"
            );
        }
        assert!(!d.path().join("x").exists());
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
        let c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn watched_passes_the_driver_s_figures_through() {
        use trellis_loop::Model;
        let mut inner = crate::testutil::figures();
        let w = Watched::new(&mut inner, None);
        assert_eq!(w.tokens_used(), Some(7));
        assert_eq!(w.prompt_bytes(), Some(4321));
        assert_eq!(w.context_bytes(), Some(9_000));
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
        let c = Client::chosen(&dir, &spec);
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
        let c = Client::chosen(&dir, &spec);
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

    /// One connection on a loopback port, answered as Ollama's `/api/tags`
    /// answers with `body`: what `list_models` probes stands in for.
    fn ollama_stub(body: &'static str) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap() == 1 {
                req.push(b[0]);
            }
            assert!(req.starts_with(b"GET /api/tags "), "{req:?}");
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (addr, h)
    }

    #[test]
    fn rpc_model_select_list_models_against_an_ollama_stub_lists_its_specs_and_the_configured_one()
    {
        // The risk (#564): the desktop's model pane offers nothing a local
        // runtime serves, so the user cannot choose one and prompt input
        // never opens; or it offers a spec first_run would not (an id
        // carrying an escape sequence), or a name in place of the configured
        // spec the pane preselects.
        let (dir, spec) = fixture("list-models", "");
        std::fs::write(
            dir.join(".trellis/config.toml"),
            format!("[model]\nspec = {}\n", toml::Value::String(spec.clone())),
        )
        .unwrap();
        let (addr, stub) = ollama_stub(
            r#"{"models":[{"name":"qwen3:8b"},{"name":"bad\u001b[2J"},{"name":"qwen2.5:3b"}]}"#,
        );
        let user = crate::testutil::scratch("rpc-list-models-user");
        std::fs::write(
            user.join("config.toml"),
            "[[providers]]\nname = \"mine\"\nprotocol = \"anthropic\"\n\
             base_url = \"https://mine.invalid\"\n",
        )
        .unwrap();
        let ollama = crate::first_run::Runtime {
            name: "Ollama",
            addr,
            api: crate::first_run::Api::OllamaTags,
        };
        let c = Client::start_with(
            &dir,
            ModelSources {
                runtimes: vec![ollama],
                user_file: Some(user.join("config.toml")),
            },
        );
        c.send(json!({"id": "l", "type": "list_models"}));
        let r = c.wait_for(|r| r["id"] == "l");
        stub.join().unwrap();
        c.finish();
        assert_eq!(r["success"], true, "{r}");
        let d = &r["data"];
        assert_eq!(d["configured"], json!(spec), "{r}");
        assert_eq!(
            d["local"],
            json!([
                format!("vllm:http://{addr}#qwen3:8b"),
                format!("vllm:http://{addr}#qwen2.5:3b")
            ]),
            "first_run's specs, the unusable id dropped: {r}"
        );
        assert_eq!(
            d["probed"],
            json!([{"runtime": "Ollama", "address": format!("http://{addr}/api/tags"),
                    "error": null}])
        );
        // The protocol's own variable, named and never read.
        assert_eq!(
            d["providers"],
            json!([{"name": "mine", "key_variable": "ANTHROPIC_API_KEY"}])
        );
        let _ = std::fs::remove_dir_all(&user);
    }

    /// A loopback stub answering `GET /v1/models` as colibri 2.0.0 does
    /// (bf244291, c/openai_server.py:5997: `owned_by: "colibri"`).
    fn colibri_stub() -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        const BODY: &str = r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap() == 1 {
                req.push(b[0]);
            }
            assert!(req.starts_with(b"GET /v1/models "), "{req:?}");
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{BODY}",
                BODY.len()
            )
            .unwrap();
        });
        (addr, h)
    }

    #[test]
    fn first_run_colibri_rpc_list_models_offers_the_provider_spec_and_save_writes_its_entry() {
        // The risk (#625): the desktop's picker offers colibri as a `vllm:`
        // spec (tier 4, no chat template), or offers `provider:colibri#<id>`
        // and a save writes the spec without the `[[providers]]` entry it
        // resolves through, so the saved model names nothing.
        let (dir, _) = fixture("list-models-colibri", "");
        let (addr, stub) = colibri_stub();
        let user = crate::testutil::scratch("rpc-list-models-colibri-user");
        let file = user.join("config.toml");
        std::fs::write(&file, "# mine\n").unwrap();
        let c = Client::start_with(
            &dir,
            ModelSources {
                runtimes: vec![crate::first_run::Runtime {
                    name: "vLLM",
                    addr,
                    api: crate::first_run::Api::OpenAiModels,
                }],
                user_file: Some(file.clone()),
            },
        );
        c.send(json!({"id": "l", "type": "list_models"}));
        let r = c.wait_for(|r| r["id"] == "l");
        stub.join().unwrap();
        assert_eq!(r["success"], true, "{r}");
        assert_eq!(
            r["data"]["local"],
            json!(["provider:colibri#glm-5.2"]),
            "the provider spec and no vllm: one: {r}"
        );
        c.send(
            json!({"id": "s", "type": "set_model", "model": "provider:colibri#glm-5.2",
                      "save": true}),
        );
        let r = c.wait_for(|r| r["id"] == "s");
        c.finish();
        assert_eq!(r["success"], true, "{r}");
        assert_eq!(
            r["data"]["saved_to"],
            json!(file.display().to_string()),
            "{r}"
        );
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with("# mine\n"), "{written}");
        let cfg = crate::config::RepoConfig::load_with_user(&dir, Some(&file)).unwrap();
        assert_eq!(cfg.model().as_deref(), Some("provider:colibri#glm-5.2"));
        let p = cfg.provider("colibri").unwrap();
        assert_eq!(p.base_url, format!("http://{addr}/v1"));
        assert_eq!(p.key_env, "COLI_API_KEY");
        assert!(
            !dir.join(".trellis/config.toml").exists()
                || !std::fs::read_to_string(dir.join(".trellis/config.toml"))
                    .unwrap()
                    .contains("colibri"),
            "nothing under .trellis/ names the provider"
        );
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn rpc_model_select_save_refuses_a_user_file_that_is_the_repository_s() {
        // The risk (ADR 0024 §3): a user config directory pointed into the
        // repository's own `.trellis/` makes "save to your config" write the
        // repository's file, where a model chosen for this user would then
        // be the repository's to change.
        let (dir, spec) = fixture("save-into-repo", "");
        let before = std::fs::read_to_string(dir.join(".trellis/config.toml")).unwrap();
        let c = Client::start_with(
            &dir,
            ModelSources {
                runtimes: Vec::new(),
                user_file: Some(dir.join(".trellis").join("config.toml")),
            },
        );
        c.send(json!({"id": "s", "type": "set_model", "model": spec, "save": true}));
        c.send(json!({"id": "p", "type": "prompt", "task": "change f"}));
        let rs = c.finish();
        let by = |id: &str| rs.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(by("s")["success"], false, "{rs:?}");
        assert!(
            by("s")["error"]
                .as_str()
                .unwrap()
                .contains("never the repository's"),
            "{rs:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(".trellis/config.toml")).unwrap(),
            before
        );
        assert!(
            by("p")["error"]
                .as_str()
                .unwrap()
                .starts_with("choose a model first"),
            "a refused save sets no model: {rs:?}"
        );
    }

    #[test]
    fn at_file_paths_lists_tracked_and_untracked_but_never_ignored() {
        // An ignored file is the one most likely to hold something that
        // should not be typed into a task a model reads: `.env`, a build
        // output, a virtualenv.
        let d = crate::testutil::scratch("at-file-paths");
        crate::testutil::git_init(&d);
        std::fs::write(
            d.join(".gitignore"),
            "secret.env
build/
",
        )
        .unwrap();
        std::fs::write(
            d.join("tracked.py"),
            "x = 1
",
        )
        .unwrap();
        crate::testutil::git_commit_all(&d, "base");
        std::fs::write(
            d.join("untracked.py"),
            "y = 2
",
        )
        .unwrap();
        std::fs::write(
            d.join("secret.env"),
            "not for a prompt
",
        )
        .unwrap();
        std::fs::create_dir_all(d.join("build")).unwrap();
        std::fs::write(
            d.join("build/out.py"),
            "z = 3
",
        )
        .unwrap();

        let paths = at_file_paths(&d).expect("a repository lists");
        assert!(paths.iter().any(|p| p == "tracked.py"), "{paths:?}");
        assert!(paths.iter().any(|p| p == "untracked.py"), "{paths:?}");
        assert!(!paths.iter().any(|p| p == "secret.env"), "{paths:?}");
        assert!(!paths.iter().any(|p| p.starts_with("build/")), "{paths:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
