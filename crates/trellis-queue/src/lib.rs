//! A local, file-backed work queue and the heartbeat that drains it (ADR 0020).
//!
//! One task is one JSON file. Its state is the directory it sits in:
//! `pending/`, `claimed/`, `done/` or `failed/`, all under
//! `<git-dir>/trellis/queue/`. Every file is written whole under a temporary
//! name and hard-linked into place, which fails if the name exists; a claim
//! is exactly that link into `claimed/`, so one claimer wins. A move is a
//! publish at the new name and then a removal of the old. There is no
//! database to keep consistent with the files.
//!
//! The queue lives in the git directory because anything in the worktree
//! makes it dirty, which blocks every run (ADR 0009), and rollback's
//! `git clean -fd` would delete it.
//!
//! [`work`] is the heartbeat. It is generic over a [`Runner`] so that what it
//! decides — claim, continue, stop — is tested against every way a run can
//! end without a model, a runtime or a repository being involved.

mod stamp;

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub use stamp::iso_now;

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("{0} is not inside a git repository: {1}")]
    NotARepo(PathBuf, String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a task file: {error}")]
    Unreadable { path: PathBuf, error: String },
    #[error("no task `{0}` in claimed/ or failed/")]
    NoSuchTask(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> QueueError + '_ {
    move |source| QueueError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Which directory a task file is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Pending,
    Claimed,
    Done,
    Failed,
}

impl State {
    pub const ALL: [State; 4] = [State::Pending, State::Claimed, State::Done, State::Failed];

    pub fn dir_name(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Claimed => "claimed",
            State::Done => "done",
            State::Failed => "failed",
        }
    }
}

/// A task file's contents.
///
/// `focus` and `prompt` serialise as `null` rather than being omitted, so a
/// file read by hand says what was *not* asked for. The fields after
/// `attempts` are written by the queue as the task moves, and are absent
/// until then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub task: String,
    pub focus: Option<String>,
    pub prompt: Option<String>,
    pub added_at: String,
    /// How many runs this task has started. A run that was interrupted and
    /// put back counts, so a task that keeps being interrupted says so.
    pub attempts: u32,
    /// When and by which process it was claimed. What `queue list` uses to
    /// tell a live claim from one a crashed process left behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

/// How the run for a task ended, appended when it leaves `claimed/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The trail's run id. `None` when the run wrote no trail, which is what
    /// a run that failed before its snapshot does.
    pub run_id: Option<String>,
    /// Why it ended, in words.
    pub exit: String,
    pub finished_at: String,
}

/// One task file as `queue list` sees it.
#[derive(Debug)]
pub struct Entry {
    pub id: String,
    pub state: State,
    /// `Err` for a file that is not valid task JSON. Listed rather than
    /// dropped: a file the queue cannot read is one the heartbeat will never
    /// run, and the person who wrote it should be able to see that.
    pub task: Result<Task, String>,
    /// Time since the file was last written: the add for a pending task, the
    /// claim for a claimed one, the outcome for a finished one.
    pub age: Option<Duration>,
}

/// The queue for one git directory.
#[derive(Debug, Clone)]
pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    /// The queue for the repository containing `root`.
    ///
    /// `Repository::path()` rather than `root/.git`: in a linked worktree
    /// `.git` is a file naming `<main>/.git/worktrees/<name>/`, and that
    /// per-worktree directory is what this returns. Each worktree therefore
    /// has its own queue, which is what ADR 0009's one-run-per-worktree
    /// requires of a serial heartbeat.
    pub fn for_repo(root: &Path) -> Result<Self, QueueError> {
        let repo = git2::Repository::open(root)
            .map_err(|e| QueueError::NotARepo(root.to_path_buf(), e.message().to_string()))?;
        Ok(Queue::at(repo.path().join("trellis").join("queue")))
    }

    /// A queue at an explicit directory. Nothing is created until written.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Queue { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn state_dir(&self, s: State) -> PathBuf {
        self.dir.join(s.dir_name())
    }

    fn path(&self, s: State, id: &str) -> PathBuf {
        self.state_dir(s).join(format!("{id}.json"))
    }

    fn ensure(&self) -> Result<(), QueueError> {
        for s in State::ALL {
            let d = self.state_dir(s);
            std::fs::create_dir_all(&d).map_err(io(&d))?;
        }
        Ok(())
    }

    /// Adds a task to `pending/` and returns its id.
    ///
    /// Written under a dot-name and linked into place, so a heartbeat
    /// listing `pending/` at the same moment never reads half a file.
    pub fn add(
        &self,
        task: &str,
        focus: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<String, QueueError> {
        self.ensure()?;
        let t = Task {
            task: task.to_string(),
            focus: focus.map(str::to_string),
            prompt: prompt.map(str::to_string),
            added_at: iso_now(),
            attempts: 0,
            claimed_at: None,
            claimed_by_pid: None,
            outcome: None,
        };
        let slug = slugify(task);
        // The stamp has nanosecond digits, but a coarse clock can repeat
        // one; `publish` refuses an existing name, so a collision is a retry
        // rather than an overwrite.
        loop {
            let id = format!("{}-{slug}", stamp::sortable_now());
            if publish(&self.path(State::Pending, &id), &t)? {
                return Ok(id);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Every task in every state: pending oldest first, then claimed, done
    /// and failed, each in id order.
    pub fn list(&self) -> Result<Vec<Entry>, QueueError> {
        let mut all = Vec::new();
        for s in State::ALL {
            for id in self.ids(s)? {
                let path = self.path(s, &id);
                let task = read_task(&path).map_err(|e| e.to_string());
                let age = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| SystemTime::now().duration_since(m).ok());
                all.push(Entry {
                    id,
                    state: s,
                    task,
                    age,
                });
            }
        }
        Ok(all)
    }

    /// Ids in one state, sorted. Ids begin with a fixed-width UTC stamp, so
    /// lexical order is age order and "oldest first" is deterministic.
    pub fn ids(&self, s: State) -> Result<Vec<String>, QueueError> {
        let d = self.state_dir(s);
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&d)(e)),
        };
        let mut ids: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                if name.starts_with('.') {
                    return None;
                }
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Claims one specific pending task. `Ok(None)` means someone else got
    /// it first, which is not an error for the caller: it moves on.
    pub fn claim(&self, id: &str) -> Result<Option<Claim>, QueueError> {
        let from = self.path(State::Pending, id);
        let to = self.path(State::Claimed, id);
        let mut task = match read_task(&from) {
            Ok(t) => t,
            // Someone else claimed it between the listing and now.
            Err(QueueError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e @ QueueError::Unreadable { .. }) => {
                // Not runnable and not retryable. Moved to failed/ so it
                // stops blocking the head of the queue, and reported.
                let _ = std::fs::hard_link(&from, self.path(State::Failed, id));
                remove(&from)?;
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        task.claimed_at = Some(iso_now());
        task.claimed_by_pid = Some(std::process::id());

        // The atomic checkout: `claimed/<id>.json` is created by a hard link
        // that fails when the name exists, so exactly one claimer creates it.
        //
        // Not `rename`, which ADR 0020 first specified. On Windows
        // `std::fs::rename` opens the source and renames *by handle*, so two
        // claimers that both opened the file before either moved it both
        // succeed — the second "renames" the file to where it already is.
        // `two_claimers_one_file_exactly_one_wins` found it: [true, true].
        if !publish(&to, &task)? {
            return Ok(None);
        }
        // A claimer that read `pending/` long ago can create the claimed
        // name after the real winner has finished and moved on. The winner
        // removed the pending file first, so its absence means this claim is
        // stale. Nobody else can remove it while this process holds the
        // claimed name.
        if !from.exists() {
            remove(&to)?;
            return Ok(None);
        }
        remove(&from)?;
        Ok(Some(Claim {
            queue: self.clone(),
            id: id.to_string(),
            task,
        }))
    }

    /// Claims the oldest pending task anyone else has not claimed first.
    pub fn claim_next(&self) -> Result<Option<Claim>, QueueError> {
        self.ensure()?;
        for id in self.ids(State::Pending)? {
            if let Some(c) = self.claim(&id)? {
                return Ok(Some(c));
            }
        }
        Ok(None)
    }

    /// Puts a task from `claimed/` or `failed/` back in `pending/`.
    ///
    /// The way out for a claim a crashed process left behind: the heartbeat
    /// never re-runs one on its own, because it cannot tell a dead claimant
    /// from a live one in another terminal. `attempts` is kept.
    pub fn requeue(&self, id: &str) -> Result<State, QueueError> {
        let id = id.strip_suffix(".json").unwrap_or(id);
        for s in [State::Claimed, State::Failed] {
            let from = self.path(s, id);
            if !from.exists() {
                continue;
            }
            let mut task = read_task(&from)?;
            task.claimed_at = None;
            task.claimed_by_pid = None;
            // A pending task has not ended; an old outcome on it would be
            // read as this attempt's.
            task.outcome = None;
            move_to(&from, &self.path(State::Pending, id), &task)?;
            return Ok(s);
        }
        Err(QueueError::NoSuchTask(id.to_string()))
    }
}

/// A task this process holds. Consumed by exactly one of [`Claim::finish`]
/// or [`Claim::release`], so a claim cannot be both recorded and returned.
#[derive(Debug)]
pub struct Claim {
    queue: Queue,
    pub id: String,
    pub task: Task,
}

impl Claim {
    /// Records the outcome and moves the task to `done/` or `failed/`.
    pub fn finish(mut self, to: State, outcome: Outcome) -> Result<PathBuf, QueueError> {
        debug_assert!(matches!(to, State::Done | State::Failed));
        self.task.attempts += 1;
        self.task.outcome = Some(outcome);
        let from = self.queue.path(State::Claimed, &self.id);
        let dest = self.queue.path(to, &self.id);
        move_to(&from, &dest, &self.task)?;
        Ok(dest)
    }

    /// Returns the task to `pending/`. `attempted` counts the run that was
    /// started and cut short; a task released before its run started is
    /// put back exactly as it was.
    pub fn release(mut self, attempted: bool) -> Result<(), QueueError> {
        if attempted {
            self.task.attempts += 1;
        }
        self.task.claimed_at = None;
        self.task.claimed_by_pid = None;
        let from = self.queue.path(State::Claimed, &self.id);
        move_to(
            &from,
            &self.queue.path(State::Pending, &self.id),
            &self.task,
        )
    }
}

fn read_task(path: &Path) -> Result<Task, QueueError> {
    let text = std::fs::read_to_string(path).map_err(io(path))?;
    serde_json::from_str(&text).map_err(|e| QueueError::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    })
}

fn write_json(path: &Path, t: &Task) -> Result<(), QueueError> {
    let mut json = serde_json::to_string_pretty(t).map_err(|e| QueueError::Unreadable {
        path: path.to_path_buf(),
        error: e.to_string(),
    })?;
    json.push('\n');
    std::fs::write(path, json).map_err(io(path))
}

/// Makes `dest` appear with `t` as its complete contents, or returns false
/// if `dest` already exists. Never overwrites.
///
/// Written under a unique dot-name, then hard-linked to `dest`: a hard link
/// fails when the name exists, and the file appears whole in one step, so a
/// reader never sees half of it. This is the queue's only way of writing a
/// task file, and there is deliberately no `rename` anywhere. Renaming a
/// freshly written file over an existing one failed on Windows with "Access
/// is denied" in about one run in five of this crate's tests, and waiting
/// five seconds did not clear it.
fn publish(dest: &Path, t: &Task) -> Result<bool, QueueError> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("task");
    // Unique per writer: two claimers racing for one id must not share a
    // temp file.
    let tmp = dest.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    write_json(&tmp, t)?;
    let linked = patiently(|| std::fs::hard_link(&tmp, dest));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(io(dest)(e)),
    }
}

/// Removes a file; one already gone is not an error.
fn remove(path: &Path) -> Result<(), QueueError> {
    match patiently(|| std::fs::remove_file(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(path)(e)),
    }
}

/// Moves a task this process holds: publish at `to`, then remove `from`.
///
/// A crash between the two leaves the task complete in both places, which
/// `queue list` shows; it never leaves it in neither.
fn move_to(from: &Path, to: &Path, t: &Task) -> Result<(), QueueError> {
    if !publish(to, t)? {
        return Err(io(to)(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "a task with this id is already there",
        )));
    }
    remove(from)
}

/// A file operation, retried for about five seconds on PermissionDenied.
///
/// On Windows a file another process has open without delete sharing cannot
/// be linked to, moved or removed for as long as it is open, and "open"
/// includes a virus scanner inspecting a file just written. Transient, so it
/// is waited out; anything longer is a real error and reported as one.
fn patiently<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut waited = Duration::ZERO;
    let mut step = Duration::from_millis(10);
    loop {
        match op() {
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && waited < Duration::from_secs(5) =>
            {
                std::thread::sleep(step);
                waited += step;
                step = (step * 2).min(Duration::from_millis(200));
            }
            r => return r,
        }
    }
}

/// Lowercase ASCII alphanumerics and single dashes, at most 40 characters.
pub fn slugify(task: &str) -> String {
    let mut s = String::new();
    for c in task.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 40 {
            break;
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "task".to_string() } else { s }
}

/// Control characters shown as escapes. Task text is typed by a person, but
/// it reaches a terminal, and an escape sequence in it could repaint the
/// line that says how a run ended.
pub fn plain(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The heartbeat
// ---------------------------------------------------------------------------

/// How one run ended, as the heartbeat needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The change is in the worktree, uncommitted, awaiting review.
    Converged { run_id: Option<String> },
    /// Did not converge, and the tree is back as it was.
    RolledBack { run_id: Option<String> },
    /// Did not converge, and the tree is NOT back as it was.
    NotRestored { run_id: Option<String> },
    /// Ctrl-C during the run. The run's own rollback has already applied.
    Interrupted {
        run_id: Option<String>,
        restored: bool,
    },
    /// The run returned an error rather than an outcome: a vault that would
    /// not compile, a missing prompt template, an unreachable runtime.
    Error {
        run_id: Option<String>,
        error: String,
    },
}

/// What the heartbeat needs from the binary.
pub trait Runner {
    /// Why a run cannot start right now, or `None`. Asked before claiming,
    /// because a task claimed and then not started is a task stranded.
    fn blocker(&mut self) -> Option<String>;
    /// Runs one task to its end.
    fn run(&mut self, task: &Task) -> RunEnd;
    /// Whether the person has asked the heartbeat to stop.
    fn stop_requested(&self) -> bool;
    /// Waits `d` or until a stop is requested. Returns false on a stop.
    fn idle(&mut self, d: Duration) -> bool;
}

/// Why [`work`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Nothing pending, and no `every` to wait for more.
    Empty,
    /// `max_tasks` runs happened.
    MaxTasks,
    /// A run converged; its change awaits review.
    Converged { id: String },
    /// The worktree is not clean; nothing was claimed.
    Blocked(String),
    /// A run left the tree in a state it could not restore.
    NotRestored { id: String },
    /// Ctrl-C, during a run or between them.
    Interrupted,
    /// A run returned an error.
    Error { id: String },
}

impl Stop {
    /// Mirrors `trellis run`'s codes: 0 for a stop that is the heartbeat
    /// working as intended, 1 for one that needs attention, 2 for a tree
    /// not restored, 130 for Ctrl-C.
    pub fn code(&self) -> i32 {
        match self {
            Stop::Empty | Stop::MaxTasks | Stop::Converged { .. } => 0,
            Stop::Blocked(_) | Stop::Error { .. } => 1,
            Stop::NotRestored { .. } => 2,
            Stop::Interrupted => 130,
        }
    }
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Empty => write!(f, "queue empty"),
            Stop::MaxTasks => write!(f, "task limit for this invocation reached"),
            Stop::Converged { id } => write!(f, "{id} converged; its change awaits review"),
            Stop::Blocked(_) => write!(f, "worktree not clean; nothing claimed"),
            Stop::NotRestored { id } => write!(f, "{id} left the tree NOT restored"),
            Stop::Interrupted => write!(f, "interrupted"),
            Stop::Error { id } => write!(f, "{id} ended in an error"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkOptions {
    /// At most this many runs in this invocation.
    pub max_tasks: Option<u32>,
    /// When the queue is empty, wait this long and look again, rather than
    /// returning. Tasks run back to back while there are any.
    pub every: Option<Duration>,
}

/// Drains the queue, one task at a time, until something says stop.
///
/// Returns how many runs happened and why it stopped. Every path out leaves
/// no task in `claimed/` that this process put there.
pub fn work(
    q: &Queue,
    runner: &mut dyn Runner,
    opts: WorkOptions,
    out: &mut dyn Write,
) -> Result<(u32, Stop), QueueError> {
    let stale = q.ids(State::Claimed)?;
    if !stale.is_empty() {
        let _ = writeln!(
            out,
            "queue:    {} task(s) already in claimed/ were not started by this process \
             and will not be re-run: {}",
            stale.len(),
            stale.join(", ")
        );
        let _ = writeln!(
            out,
            "          if the process that claimed them is gone, \
             `trellis queue requeue <id>` puts one back in pending/"
        );
    }

    let mut ran = 0u32;
    loop {
        if runner.stop_requested() {
            return Ok((ran, Stop::Interrupted));
        }
        if opts.max_tasks.is_some_and(|m| ran >= m) {
            return Ok((ran, Stop::MaxTasks));
        }
        if let Some(why) = runner.blocker() {
            let _ = writeln!(out, "work:     not claiming a task: {why}");
            return Ok((ran, Stop::Blocked(why)));
        }
        let claim = match q.claim_next() {
            Ok(Some(c)) => c,
            Ok(None) => match opts.every {
                Some(d) => {
                    if !runner.idle(d) {
                        return Ok((ran, Stop::Interrupted));
                    }
                    continue;
                }
                None => {
                    let _ = writeln!(out, "work:     queue empty");
                    return Ok((ran, Stop::Empty));
                }
            },
            Err(QueueError::Unreadable { path, error }) => {
                let _ = writeln!(
                    out,
                    "work:     {} is not a task file ({error}); moved to failed/",
                    path.display()
                );
                continue;
            }
            Err(e) => return Err(e),
        };
        // A Ctrl-C between the claim and the run's own signal handler would
        // otherwise go unseen until the run ended.
        if runner.stop_requested() {
            claim.release(false)?;
            return Ok((ran, Stop::Interrupted));
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "work:     {} — {}", claim.id, plain(&claim.task.task));
        let end = runner.run(&claim.task);
        ran += 1;
        let id = claim.id.clone();
        let now = iso_now();
        let outcome = |run_id: &Option<String>, exit: &str| Outcome {
            run_id: run_id.clone(),
            exit: exit.to_string(),
            finished_at: now.clone(),
        };
        match end {
            RunEnd::Converged { run_id } => {
                claim.finish(
                    State::Done,
                    outcome(&run_id, "converged; change left uncommitted for review"),
                )?;
                let _ = writeln!(out, "work:     {id} -> done/ (converged)");
                let _ = writeln!(
                    out,
                    "work:     stopping. The converged change awaits review; accept or \
                     revert it, then run `trellis work` again"
                );
                return Ok((ran, Stop::Converged { id }));
            }
            RunEnd::RolledBack { run_id } => {
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "not converged; tree restored"),
                )?;
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/ (not converged; tree restored)"
                );
            }
            RunEnd::NotRestored { run_id } => {
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "not converged; tree NOT restored"),
                )?;
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/. The tree was NOT restored; stopping"
                );
                return Ok((ran, Stop::NotRestored { id }));
            }
            RunEnd::Interrupted {
                restored: true,
                run_id: _,
            } => {
                // The run's rollback put the tree back, and the task never
                // produced an outcome, so it is safe to run again later.
                claim.release(true)?;
                let _ = writeln!(
                    out,
                    "work:     {id} -> pending/ (interrupted; tree restored; attempts counted)"
                );
                return Ok((ran, Stop::Interrupted));
            }
            RunEnd::Interrupted {
                restored: false,
                run_id,
            } => {
                // The tree is not known to be as it was, so re-running the
                // task later is a guess about a state nobody has looked at.
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "interrupted; tree NOT restored"),
                )?;
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/ (interrupted; the tree was NOT restored)"
                );
                return Ok((ran, Stop::Interrupted));
            }
            RunEnd::Error { run_id, error } => {
                claim.finish(State::Failed, outcome(&run_id, &format!("error: {error}")))?;
                let _ = writeln!(out, "work:     {id} -> failed/ ({})", plain(&error));
                // From here a bad task and a dead runtime look alike, and a
                // dead runtime would fail every task after it.
                let _ = writeln!(out, "work:     stopping after an error");
                return Ok((ran, Stop::Error { id }));
            }
        }
    }
}

#[cfg(test)]
mod tests;
