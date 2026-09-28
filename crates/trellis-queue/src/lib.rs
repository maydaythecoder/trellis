//! A local, file-backed work queue and the heartbeat that drains it
//! (ADR 0020), with the task model, profiles, budgets, approvals, routines
//! and activity log of ADR 0026.
//!
//! One task is one JSON file. Its state is the directory it sits in:
//! `pending/`, `claimed/`, `done/`, `failed/` or `cancelled/`, all under
//! `<git-dir>/trellis/queue/`; its `status` refines that (ADR 0026 §1). Every
//! file is written whole under a temporary name and hard-linked into place,
//! which fails if the name exists; a claim is exactly that link into
//! `claimed/`, so one claimer wins. A move is a publish at the new name and
//! then a removal of the old. There is no database to keep consistent with
//! the files.
//!
//! The queue lives in the git directory because anything in the worktree
//! makes it dirty, which blocks every run (ADR 0009), and rollback's
//! `git clean -fd` would delete it.
//!
//! [`work`] is the heartbeat. It is generic over a [`Runner`] so that what it
//! decides — claim, continue, stop — is tested against every way a run can
//! end without a model, a runtime or a repository being involved.

pub mod activity;
pub mod approval;
pub mod cron;
mod files;
pub mod ledger;
pub mod portable;
pub mod profile;
pub mod routine;
mod stamp;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub use activity::{ActivityLog, Actor, Event};
use files::{io, move_to, present, publish, read_json, remove};
pub use stamp::iso_now;
pub use stamp::{civil_from_secs, month_of, now_secs, parse_iso, secs_to_iso};

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
    #[error("no task `{0}` in the queue")]
    Unknown(String),
    #[error(
        "task `{id}` is claimed{} and cannot be changed until its run ends: the \
         running process publishes its own copy when it finishes, and would \
         silently drop this change",
        pid.map(|p| format!(" by pid {p}")).unwrap_or_default()
    )]
    Claimed { id: String, pid: Option<u32> },
    #[error(
        "{0} is held. If no other trellis command is running, a crashed one \
         left it behind; remove the file and retry"
    )]
    Locked(PathBuf),
    #[error("{0}")]
    Invalid(String),
}

/// Which directory a task file is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    Pending,
    Claimed,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub const ALL: [State; 5] = [
        State::Pending,
        State::Claimed,
        State::Done,
        State::Failed,
        State::Cancelled,
    ];

    pub fn dir_name(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Claimed => "claimed",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    /// The status a task in this directory has when its file names none —
    /// every file written before ADR 0026.
    ///
    /// `done/` reads as `done`, not `in_review`: those tasks converged under
    /// ADR 0020 and have almost certainly been reviewed since, and reading
    /// them as in review would leave every task naming one as a blocker
    /// waiting forever.
    pub fn default_status(self) -> Status {
        match self {
            State::Pending => Status::Todo,
            State::Claimed => Status::InProgress,
            State::Done => Status::Done,
            State::Failed => Status::Failed,
            State::Cancelled => Status::Cancelled,
        }
    }
}

/// Paperclip's status set, plus `failed` (ADR 0026 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Blocked,
    Done,
    Cancelled,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Todo => "todo",
            Status::InProgress => "in_progress",
            Status::InReview => "in_review",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
            Status::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "backlog" => Status::Backlog,
            "todo" => Status::Todo,
            "in_progress" => Status::InProgress,
            "in_review" => Status::InReview,
            "blocked" => Status::Blocked,
            "done" => Status::Done,
            "cancelled" => Status::Cancelled,
            "failed" => Status::Failed,
            _ => return None,
        })
    }

    /// The directory a task with this status lives in.
    pub fn home(self) -> State {
        match self {
            Status::Backlog | Status::Todo | Status::Blocked => State::Pending,
            Status::InProgress => State::Claimed,
            Status::InReview | Status::Done => State::Done,
            Status::Failed => State::Failed,
            Status::Cancelled => State::Cancelled,
        }
    }

    /// Terminal for a parent's wake: nothing moves it again without a
    /// person. `failed` counts, which Paperclip (having no `failed`) does
    /// not say; `in_review` does not, because the change is still undecided.
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Cancelled | Status::Failed)
    }
}

/// Why a task is `blocked`. Paperclip requires a routable reason; prose
/// alone "routes to nobody", so `manual` must say what unblocks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Blocked {
    /// Waiting for every task in `blockers` to be `done`.
    Blockers,
    /// Waiting for every direct child to be terminal.
    Children,
    /// Waiting for something outside the queue, named in `detail`.
    Manual { detail: String },
}

impl Blocked {
    pub fn describe(&self) -> String {
        match self {
            Blocked::Blockers => "blocked until its blockers are done".into(),
            Blocked::Children => "blocked until its sub-tasks finish".into(),
            Blocked::Manual { detail } => format!("blocked: {}", plain(detail)),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Critical,
    High,
    #[default]
    Medium,
    Low,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Critical => "critical",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Priority> {
        Some(match s {
            "critical" => Priority::Critical,
            "high" => Priority::High,
            "medium" => Priority::Medium,
            "low" => Priority::Low,
            _ => return None,
        })
    }

    fn is_default(&self) -> bool {
        *self == Priority::Medium
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    Rejected,
}

/// A person's decision on a task that needed one (ADR 0026 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub decision: Verdict,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub at: String,
    pub actor: Actor,
    pub text: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A task file's contents.
///
/// `focus` and `prompt` serialise as `null` rather than being omitted, so a
/// file read by hand says what was *not* asked for. Everything after
/// `attempts` is optional on disk, so a file written under ADR 0020 loads
/// unchanged; the ADR 0026 fields are omitted when they hold their default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Blocked>,
    #[serde(default, skip_serializing_if = "Priority::is_default")]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Structure, not dependency (Paperclip `execution-semantics.md` §6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// An agent profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Task ids that must be `done` before this one may run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_approval: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The routine that created this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
}

impl Task {
    /// The status this task has in directory `s`: its own if it names one
    /// that belongs there, otherwise the directory's default.
    pub fn status_in(&self, s: State) -> Status {
        match self.status {
            Some(st) if st.home() == s => st,
            _ => s.default_status(),
        }
    }

    pub fn awaiting_approval(&self) -> bool {
        self.needs_approval
            && !matches!(
                self.approval,
                Some(Decision {
                    decision: Verdict::Approved,
                    ..
                })
            )
    }

    pub fn note(&mut self, actor: Actor, text: impl Into<String>) {
        self.notes.push(Note {
            at: iso_now(),
            actor,
            text: text.into(),
        });
    }
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

/// What `queue add` asks for. `Default` is a plain `todo` task.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub task: String,
    pub focus: Option<String>,
    pub prompt: Option<String>,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub parent: Option<String>,
    pub assignee: Option<String>,
    pub blockers: Vec<String>,
    pub needs_approval: bool,
    /// Start in `backlog` rather than `todo`.
    pub backlog: bool,
    pub routine: Option<String>,
}

/// Every task's state and status by id: what blockers and wakes are decided
/// against.
#[derive(Debug, Default)]
pub struct Index {
    pub by_id: HashMap<String, (State, Status)>,
    pub children: HashMap<String, Vec<String>>,
}

impl Index {
    pub fn status(&self, id: &str) -> Option<Status> {
        self.by_id.get(id).map(|(_, s)| *s)
    }

    /// Blockers not yet `done`. A cancelled blocker is unresolved: it is
    /// terminal for itself but does not satisfy the dependency, and an id
    /// that names no task is unresolved too.
    pub fn unresolved<'a>(&self, t: &'a Task) -> Vec<&'a str> {
        t.blockers
            .iter()
            .filter(|b| self.status(b) != Some(Status::Done))
            .map(String::as_str)
            .collect()
    }
}

/// The queue for one git directory.
#[derive(Debug, Clone)]
pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    /// The queue for the repository containing `root`.
    ///
    /// `git rev-parse --absolute-git-dir` rather than `root/.git`: in a
    /// linked worktree `.git` is a file naming `<main>/.git/worktrees/<name>/`,
    /// and that per-worktree directory is what this returns. Each worktree
    /// therefore has its own queue, which is what ADR 0009's
    /// one-run-per-worktree requires of a serial heartbeat.
    pub fn for_repo(root: &Path) -> Result<Self, QueueError> {
        let dir = trellis_vcs::git_dir(root)
            .map_err(|e| QueueError::NotARepo(root.to_path_buf(), e.to_string()))?;
        Ok(Queue::at(dir.join("trellis").join("queue")))
    }

    /// A queue at an explicit directory. Nothing is created until written.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Queue { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<git-dir>/trellis`: the queue's siblings — locks, the activity log,
    /// profile overlays, routine state, approval requests — live here.
    pub fn trellis_dir(&self) -> PathBuf {
        self.dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.dir.clone())
    }

    pub(crate) fn locks(&self) -> PathBuf {
        self.trellis_dir().join("locks")
    }

    pub fn activity(&self) -> ActivityLog {
        ActivityLog::at(self.trellis_dir().join("activity.jsonl"))
    }

    /// Records an event. A log that cannot be written is reported to the
    /// caller, never swallowed: an audit trail with silent gaps reads as a
    /// complete one.
    pub fn log(&self, e: Event) -> Result<(), QueueError> {
        self.activity().append(&e)
    }

    fn state_dir(&self, s: State) -> PathBuf {
        self.dir.join(s.dir_name())
    }

    pub(crate) fn path(&self, s: State, id: &str) -> PathBuf {
        self.state_dir(s).join(format!("{id}.json"))
    }

    fn ensure(&self) -> Result<(), QueueError> {
        for s in State::ALL {
            let d = self.state_dir(s);
            std::fs::create_dir_all(&d).map_err(io(&d))?;
        }
        Ok(())
    }

    /// Adds a plain task to `pending/` and returns its id (ADR 0020 §5).
    pub fn add(
        &self,
        task: &str,
        focus: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<String, QueueError> {
        self.add_task(
            NewTask {
                task: task.to_string(),
                focus: focus.map(str::to_string),
                prompt: prompt.map(str::to_string),
                ..NewTask::default()
            },
            Actor::Human,
        )
    }

    /// Adds a task to `pending/` and returns its id.
    ///
    /// The parent and every blocker must already exist: a dangling id is a
    /// task that would wait forever, and it is refused when a person is
    /// there to fix it rather than discovered by the heartbeat later.
    /// Written under a dot-name and linked into place, so a heartbeat
    /// listing `pending/` at the same moment never reads half a file.
    pub fn add_task(&self, new: NewTask, actor: Actor) -> Result<String, QueueError> {
        self.ensure()?;
        let index = self.index()?;
        for id in new.parent.iter().chain(new.blockers.iter()) {
            if !index.by_id.contains_key(id) {
                return Err(QueueError::Unknown(id.clone()));
            }
        }
        let mut t = Task {
            task: new.task,
            focus: new.focus,
            prompt: new.prompt,
            added_at: iso_now(),
            priority: new.priority,
            labels: new.labels,
            parent: new.parent,
            assignee: new.assignee,
            blockers: new.blockers,
            needs_approval: new.needs_approval,
            routine: new.routine,
            ..Task::default()
        };
        t.status = Some(if new.backlog {
            Status::Backlog
        } else if !index.unresolved(&t).is_empty() {
            t.blocked = Some(Blocked::Blockers);
            Status::Blocked
        } else {
            Status::Todo
        });
        let slug = slugify(&t.task);
        // The stamp has nanosecond digits, but a coarse clock can repeat
        // one; `publish` refuses an existing name, so a collision is a retry
        // rather than an overwrite.
        let id = loop {
            let id = format!("{}-{slug}", stamp::sortable_now());
            if publish(&self.path(State::Pending, &id), &t)? {
                break id;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        self.log(
            Event::new(actor, "task.add", format!("task:{id}")).detail(format!(
                "{} [{}]",
                t.status_in(State::Pending).as_str(),
                t.priority.as_str()
            )),
        )?;
        Ok(id)
    }

    /// Every task in every state: pending oldest first, then claimed, done,
    /// failed and cancelled, each in id order.
    pub fn list(&self) -> Result<Vec<Entry>, QueueError> {
        let mut all = Vec::new();
        for s in State::ALL {
            for id in self.ids(s)? {
                let path = self.path(s, &id);
                let task = read_json::<Task>(&path).map_err(|e| e.to_string());
                let age = files::read_settled(&path, |p| std::fs::metadata(p)?.modified())
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

    pub fn index(&self) -> Result<Index, QueueError> {
        Ok(Self::index_of(&self.list()?))
    }

    pub fn index_of(entries: &[Entry]) -> Index {
        let mut ix = Index::default();
        for e in entries {
            let Ok(t) = &e.task else { continue };
            ix.by_id
                .insert(e.id.clone(), (e.state, t.status_in(e.state)));
            if let Some(p) = &t.parent {
                ix.children.entry(p.clone()).or_default().push(e.id.clone());
            }
        }
        ix
    }

    /// Where a task is, and what it says.
    pub fn find(&self, id: &str) -> Result<(State, Task), QueueError> {
        let id = id.strip_suffix(".json").unwrap_or(id);
        for s in State::ALL {
            let p = self.path(s, id);
            if present(&p) {
                return Ok((s, read_json(&p)?));
            }
        }
        Err(QueueError::Unknown(id.to_string()))
    }

    /// The per-task lock every mutation of a task outside `pending/` and
    /// `claimed/` takes, with any replacement of it that was cut short
    /// finished first.
    ///
    /// That settle is for the mutations that move or remove the name
    /// (`requeue`, `remove_task`, an `edit` that changes state): each works
    /// on the canonical file, and a side copy left beside it would bring
    /// the task back in its old state. An edit in place goes through
    /// `files::replace`, which settles for itself.
    ///
    /// `edit`, `requeue` and `remove_task` all take it: one that did not
    /// could move the task while an edit held it between its read and its
    /// write, and the edit would then write it back where it was — one
    /// task in two states.
    fn lock_task(&self, id: &str) -> Result<files::Lock, QueueError> {
        let lock = files::Lock::take(&self.locks(), &format!("task-{id}"))?;
        for s in State::ALL {
            files::settle(&self.path(s, id))?;
        }
        Ok(lock)
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
                let mut name = e.file_name().to_str()?.to_string();
                if name.starts_with('.') {
                    // A replacement cut short between removing the old copy
                    // and linking the new: the new copy is the task, and
                    // hiding it would hide the task.
                    let orig = files::unside(&name)?;
                    if d.join(orig).exists() {
                        return None;
                    }
                    name = orig.to_string();
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
        let mut task = match read_json::<Task>(&from) {
            Ok(t) => t,
            // Someone else claimed it between the listing and now.
            Err(QueueError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e @ QueueError::Unreadable { .. }) => {
                // Not runnable and not retryable. Moved to failed/ so it
                // stops blocking the head of the queue, and reported.
                let _ = std::fs::create_dir_all(self.state_dir(State::Failed));
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

    /// Claims the next claimable pending task, with no assignee gate.
    pub fn claim_next(&self) -> Result<Option<Claim>, QueueError> {
        Ok(self.claim_next_where(&mut |_, _| None)?.0)
    }

    /// Why a pending task may not be claimed, ignoring its assignee, or
    /// `None` if it may.
    pub fn waiting_reason(t: &Task, ix: &Index) -> Option<String> {
        match t.status_in(State::Pending) {
            Status::Backlog => return Some("in backlog".into()),
            Status::Blocked => {
                return Some(
                    t.blocked
                        .as_ref()
                        .map(Blocked::describe)
                        .unwrap_or_else(|| "blocked".into()),
                );
            }
            _ => {}
        }
        let open = ix.unresolved(t);
        if !open.is_empty() {
            return Some(format!("blockers not done: {}", open.join(", ")));
        }
        if t.awaiting_approval() {
            return Some("needs approval: `trellis queue approve <id>`".into());
        }
        None
    }

    /// Claims the highest-priority, then oldest, pending task that is
    /// claimable and that `gate` does not refuse. Also returns every pending
    /// task that was not claimable, with why, so the heartbeat can say what
    /// it is waiting for rather than "queue empty".
    ///
    /// Every pending file is read to sort by priority: one read per pending
    /// task per tick, for a queue that is a person's backlog.
    #[allow(clippy::type_complexity)]
    pub fn claim_next_where(
        &self,
        gate: &mut dyn FnMut(&str, &Task) -> Option<String>,
    ) -> Result<(Option<Claim>, Vec<(String, String)>), QueueError> {
        self.ensure()?;
        let entries = self.list()?;
        let ix = Self::index_of(&entries);
        let mut waiting = Vec::new();
        let mut ready: Vec<(Priority, String)> = Vec::new();
        for e in entries.iter().filter(|e| e.state == State::Pending) {
            let t = match &e.task {
                Ok(t) => t,
                // `claim` is what moves an unreadable file to failed/ and
                // reports it; keep that behaviour at the head of the queue.
                Err(_) => {
                    self.claim(&e.id)?;
                    continue;
                }
            };
            if let Some(why) = Self::waiting_reason(t, &ix).or_else(|| gate(&e.id, t)) {
                waiting.push((e.id.clone(), why));
            } else {
                ready.push((t.priority, e.id.clone()));
            }
        }
        ready.sort();
        for (_, id) in ready {
            let Some(c) = self.claim(&id)? else { continue };
            // Re-checked against what was actually claimed: a person may
            // have changed the file between the listing and the claim.
            if let Some(why) = Self::waiting_reason(&c.task, &ix) {
                c.release(false)?;
                waiting.push((id, why));
                continue;
            }
            return Ok((Some(c), waiting));
        }
        Ok((None, waiting))
    }

    /// Changes a task where it stands (ADR 0026 §2).
    ///
    /// `f` edits the task and may name a new directory for it. A pending
    /// task is edited by claiming it, so a heartbeat never runs a task
    /// half-edited and never claims one being edited; a claimed task is
    /// refused; a finished one is replaced under a lock. Returns where the
    /// task was and where it is now.
    pub fn edit(
        &self,
        id: &str,
        f: impl FnOnce(&mut Task, State) -> Result<Option<State>, QueueError>,
    ) -> Result<(State, State), QueueError> {
        let id = id.strip_suffix(".json").unwrap_or(id).to_string();
        let (state, current) = self.find(&id)?;
        match state {
            State::Claimed => Err(QueueError::Claimed {
                id,
                pid: current.claimed_by_pid,
            }),
            State::Pending => self.edit_pending(id, current, f),
            _ => {
                let lock = self.lock_task(&id)?;
                // Found again under the lock: a requeue that ran between the
                // first look and the lock has moved it out of this state.
                let (s, t) = self.find(&id)?;
                if matches!(s, State::Pending | State::Claimed) {
                    drop(lock);
                    return self.edit(&id, f);
                }
                self.edit_finished(&id, s, t, f)
            }
        }
    }

    fn edit_pending(
        &self,
        id: String,
        current: Task,
        f: impl FnOnce(&mut Task, State) -> Result<Option<State>, QueueError>,
    ) -> Result<(State, State), QueueError> {
        let Some(mut c) = self.claim(&id)? else {
            let pid = self.find(&id).ok().and_then(|(_, t)| t.claimed_by_pid);
            return Err(QueueError::Claimed { id, pid });
        };
        c.task.claimed_at = None;
        c.task.claimed_by_pid = None;
        let to = match f(&mut c.task, State::Pending) {
            Ok(Some(State::Claimed)) => Err(QueueError::Invalid(
                "a task becomes in_progress only by being claimed".into(),
            )),
            Ok(to) => Ok(to.unwrap_or(State::Pending)),
            Err(e) => Err(e),
        };
        let to = match to {
            Ok(to) => to,
            Err(e) => {
                // Put back exactly as it was: the edit did not apply.
                let original = current;
                let from = self.path(State::Claimed, &id);
                move_to(&from, &self.path(State::Pending, &id), &original)?;
                return Err(e);
            }
        };
        let from = self.path(State::Claimed, &id);
        move_to(&from, &self.path(to, &id), &c.task)?;
        Ok((State::Pending, to))
    }

    /// Edits a task in `done/`, `failed/` or `cancelled/`. The caller holds
    /// its lock.
    fn edit_finished(
        &self,
        id: &str,
        s: State,
        mut t: Task,
        f: impl FnOnce(&mut Task, State) -> Result<Option<State>, QueueError>,
    ) -> Result<(State, State), QueueError> {
        let to = f(&mut t, s)?.unwrap_or(s);
        if to == State::Claimed {
            return Err(QueueError::Invalid(
                "a task becomes in_progress only by being claimed".into(),
            ));
        }
        let from = self.path(s, id);
        if to == s {
            // Not remove-then-publish: a failure between those two lost the
            // task. `replace` keeps a complete copy at every step.
            files::replace(&from, &t)?;
        } else {
            move_to(&from, &self.path(to, id), &t)?;
        }
        Ok((s, to))
    }

    /// Wakes tasks whose wait is over (ADR 0026 §1): a task `blocked` on
    /// blockers once every blocker is `done`, and one `blocked` on its
    /// children once every direct child is terminal. Returns `(id, why)` for
    /// each wake, each already logged.
    ///
    /// A task someone else holds right now is left for the next reconcile.
    pub fn reconcile(&self, actor: Actor) -> Result<Vec<(String, String)>, QueueError> {
        let entries = self.list()?;
        let ix = Self::index_of(&entries);
        let mut woken = Vec::new();
        for e in entries.iter().filter(|e| e.state == State::Pending) {
            let Ok(t) = &e.task else { continue };
            if t.status_in(State::Pending) != Status::Blocked {
                continue;
            }
            let why = match &t.blocked {
                Some(Blocked::Blockers)
                    if !t.blockers.is_empty() && ix.unresolved(t).is_empty() =>
                {
                    "every blocker is done"
                }
                Some(Blocked::Children) => {
                    let kids = ix.children.get(&e.id).map(Vec::as_slice).unwrap_or(&[]);
                    if kids.is_empty()
                        || !kids
                            .iter()
                            .all(|k| ix.status(k).is_some_and(Status::is_terminal))
                    {
                        continue;
                    }
                    "every sub-task is finished"
                }
                _ => continue,
            };
            let r = self.edit(&e.id, |t, _| {
                t.status = Some(Status::Todo);
                t.blocked = None;
                t.note(actor, format!("woken: {why}"));
                Ok(None)
            });
            match r {
                Ok(_) => {
                    self.log(Event::new(actor, "task.wake", format!("task:{}", e.id)).detail(why))?;
                    woken.push((e.id.clone(), why.to_string()));
                }
                Err(QueueError::Claimed { .. } | QueueError::Locked(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(woken)
    }

    /// Puts a task from `claimed/` or `failed/` back in `pending/`.
    ///
    /// The way out for a claim a crashed process left behind: the heartbeat
    /// never re-runs one on its own, because it cannot tell a dead claimant
    /// from a live one in another terminal. `attempts` is kept.
    pub fn requeue(&self, id: &str) -> Result<State, QueueError> {
        let id = id.strip_suffix(".json").unwrap_or(id);
        let _lock = self.lock_task(id)?;
        for s in [State::Claimed, State::Failed] {
            let from = self.path(s, id);
            if !from.exists() {
                continue;
            }
            let mut task: Task = read_json(&from)?;
            task.claimed_at = None;
            task.claimed_by_pid = None;
            // A pending task has not ended; an old outcome on it would be
            // read as this attempt's.
            task.outcome = None;
            task.status = Some(Status::Todo);
            move_to(&from, &self.path(State::Pending, id), &task)?;
            self.log(
                Event::new(Actor::Human, "task.requeue", format!("task:{id}"))
                    .detail(format!("from {}/", s.dir_name())),
            )?;
            return Ok(s);
        }
        Err(QueueError::NoSuchTask(id.to_string()))
    }

    /// Imports a task file under a given id and state, refusing an existing
    /// id. Used by `queue import`.
    pub(crate) fn publish_as(&self, s: State, id: &str, t: &Task) -> Result<bool, QueueError> {
        self.ensure()?;
        publish(&self.path(s, id), t)
    }

    /// Removes a task wherever it is, except from `claimed/`.
    pub(crate) fn remove_task(&self, id: &str) -> Result<(), QueueError> {
        let _lock = self.lock_task(id)?;
        let (s, t) = self.find(id)?;
        if s == State::Claimed {
            return Err(QueueError::Claimed {
                id: id.to_string(),
                pid: t.claimed_by_pid,
            });
        }
        remove(&self.path(s, id))
    }

    /// A fresh id with this slug, for an import that renames on conflict.
    pub(crate) fn fresh_id(slug: &str) -> String {
        format!("{}-{slug}", stamp::sortable_now())
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
    /// Records the outcome and moves the task to `done/` (as `in_review`:
    /// its change awaits a person) or `failed/`.
    pub fn finish(mut self, to: State, outcome: Outcome) -> Result<PathBuf, QueueError> {
        debug_assert!(matches!(to, State::Done | State::Failed));
        self.task.attempts += 1;
        self.task.outcome = Some(outcome);
        self.task.status = Some(match to {
            State::Done => Status::InReview,
            _ => Status::Failed,
        });
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

#[cfg(test)]
pub(crate) fn read_task(path: &Path) -> Result<Task, QueueError> {
    read_json(path)
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
    /// Runs one task to its end. Its id arrives first through
    /// [`Runner::starting`].
    fn run(&mut self, task: &Task) -> RunEnd;
    /// Whether the person has asked the heartbeat to stop.
    fn stop_requested(&self) -> bool;
    /// Waits `d` or until a stop is requested. Returns false on a stop.
    fn idle(&mut self, d: Duration) -> bool;
    /// Called at the top of every tick, before anything is claimed: where
    /// routines fire and budget windows roll over (ADR 0026 §5, §7).
    fn tick(&mut self, _q: &Queue, _out: &mut dyn Write) {}
    /// Why this task's assignee cannot take it now — paused, over budget,
    /// needs a terminal it does not have — or `None`.
    fn gate(&mut self, _id: &str, _task: &Task, _out: &mut dyn Write) -> Option<String> {
        None
    }
    /// How long until something scheduled is due; bounds `--every`'s idle.
    fn next_due(&self) -> Option<Duration> {
        None
    }
    /// The id of the task about to run, before `run`: what a process agent
    /// receives as `TRELLIS_TASK_ID`.
    fn starting(&mut self, _id: &str) {}
}

/// Why [`work`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Nothing pending, and no `every` to wait for more.
    Empty,
    /// Tasks are pending but none is claimable — blocked, awaiting
    /// approval, or held by its agent's gate — and no `every`.
    Waiting(usize),
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
            Stop::Empty | Stop::Waiting(_) | Stop::MaxTasks | Stop::Converged { .. } => 0,
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
            Stop::Waiting(n) => write!(f, "nothing claimable; {n} pending task(s) waiting"),
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
    /// When nothing is claimable, wait this long (or until the next routine
    /// is due, if sooner) and look again, rather than returning. Tasks run
    /// back to back while there are any.
    pub every: Option<Duration>,
}

/// Wakes, logged and printed. Failure to reconcile is reported and not
/// fatal: a wake missed now is found on the next tick.
fn reconcile_and_say(q: &Queue, out: &mut dyn Write) {
    match q.reconcile(Actor::Work) {
        Ok(w) => {
            for (id, why) in w {
                let _ = writeln!(out, "work:     {id} -> todo ({why})");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "work:     could not reconcile blockers: {e}");
        }
    }
}

fn log_or_say(q: &Queue, e: Event, out: &mut dyn Write) {
    if let Err(err) = q.log(e) {
        let _ = writeln!(out, "work:     activity log not written: {err}");
    }
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
    let mut said_waiting = false;
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
        runner.tick(q, out);
        reconcile_and_say(q, out);
        let claimed = {
            let mut gate = |id: &str, t: &Task| runner.gate(id, t, out);
            q.claim_next_where(&mut gate)
        };
        let claim = match claimed {
            Ok((Some(c), _)) => {
                said_waiting = false;
                c
            }
            Ok((None, waiting)) => {
                if !waiting.is_empty() && !said_waiting {
                    let _ = writeln!(
                        out,
                        "work:     nothing claimable; {} pending task(s) waiting:",
                        waiting.len()
                    );
                    for (id, why) in &waiting {
                        let _ = writeln!(out, "          {id}: {}", plain(why));
                    }
                    said_waiting = true;
                }
                match opts.every {
                    Some(d) => {
                        let d = runner.next_due().map_or(d, |n| n.min(d));
                        if !runner.idle(d) {
                            return Ok((ran, Stop::Interrupted));
                        }
                        continue;
                    }
                    None if waiting.is_empty() => {
                        let _ = writeln!(out, "work:     queue empty");
                        return Ok((ran, Stop::Empty));
                    }
                    None => return Ok((ran, Stop::Waiting(waiting.len()))),
                }
            }
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
        log_or_say(
            q,
            Event::new(Actor::Work, "task.claim", format!("task:{}", claim.id)),
            out,
        );
        runner.starting(&claim.id);
        let end = runner.run(&claim.task);
        ran += 1;
        let id = claim.id.clone();
        let entity = format!("task:{id}");
        let now = iso_now();
        let outcome = |run_id: &Option<String>, exit: &str| Outcome {
            run_id: run_id.clone(),
            exit: exit.to_string(),
            finished_at: now.clone(),
        };
        let finished = |run_id: &Option<String>, status: &str, out: &mut dyn Write| {
            log_or_say(
                q,
                Event::new(Actor::Work, "task.finish", entity.clone())
                    .run(run_id.clone())
                    .detail(status),
                out,
            );
        };
        match end {
            RunEnd::Converged { run_id } => {
                claim.finish(
                    State::Done,
                    outcome(&run_id, "converged; change left uncommitted for review"),
                )?;
                finished(&run_id, "in_review", out);
                let _ = writeln!(out, "work:     {id} -> done/ (converged; in review)");
                let _ = writeln!(
                    out,
                    "work:     stopping. The converged change awaits review; accept or \
                     revert it, then run `trellis work` again"
                );
                let _ = writeln!(
                    out,
                    "          once accepted, `trellis queue set {id} --status done` \
                     releases whatever it blocks"
                );
                return Ok((ran, Stop::Converged { id }));
            }
            RunEnd::RolledBack { run_id } => {
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "not converged; tree restored"),
                )?;
                finished(&run_id, "failed", out);
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
                finished(&run_id, "failed; tree NOT restored", out);
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/. The tree was NOT restored; stopping"
                );
                return Ok((ran, Stop::NotRestored { id }));
            }
            RunEnd::Interrupted {
                restored: true,
                run_id,
            } => {
                // The run's rollback put the tree back, and the task never
                // produced an outcome, so it is safe to run again later.
                claim.release(true)?;
                log_or_say(
                    q,
                    Event::new(Actor::Work, "task.release", entity.clone())
                        .run(run_id)
                        .detail("interrupted; tree restored"),
                    out,
                );
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
                finished(&run_id, "failed; interrupted, tree NOT restored", out);
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/ (interrupted; the tree was NOT restored)"
                );
                return Ok((ran, Stop::Interrupted));
            }
            RunEnd::Error { run_id, error } => {
                claim.finish(State::Failed, outcome(&run_id, &format!("error: {error}")))?;
                finished(&run_id, "failed; error", out);
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
