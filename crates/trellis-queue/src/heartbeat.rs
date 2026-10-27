//! The heartbeat: what drains the queue one run at a time (ADR 0020). It is
//! generic over a [`Runner`] so that claim, continue and stop are decided
//! without a model, a runtime or a repository being involved. The task model
//! and the file-backed queue it drives are in the crate root.

use crate::{
    Actor, Event, Liveness, MAX_STALE_RELEASES, Outcome, Queue, QueueError, StaleClaims, State,
    Swept, Task, iso_now, mention, now_secs, plain, profile, question_note,
};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

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
    /// Did not converge because the model asked a person a question
    /// (`ask_human`), and the tree is back as it was. The task waits,
    /// `blocked` with reason `question`, for an answer.
    Asked {
        run_id: Option<String>,
        question: String,
        options: Vec<String>,
    },
    /// The run returned an error rather than an outcome: a vault that would
    /// not compile, a missing prompt template, an unreachable runtime.
    Error {
        run_id: Option<String>,
        error: String,
    },
}

/// The worktree a claimed task runs in, made by [`Runner::prepare`]
/// (ADR 0053 decision 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub path: PathBuf,
    pub branch: String,
}

/// What is left of a [`Workspace`] after [`Runner::remove_workspace`]: what
/// the task's outcome then records (ADR 0053 decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Left {
    /// The directory is still there: its removal failed.
    pub worktree: bool,
    /// The branch is still there: it holds a commit past its base (a trail),
    /// or its removal failed.
    pub branch: bool,
}

impl Left {
    /// Nothing was removed.
    pub const ALL: Left = Left {
        worktree: true,
        branch: true,
    };
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
    /// Makes the place the claimed task runs in, after the claim and before
    /// [`Runner::starting`]: a worktree of its own (ADR 0053). `Ok(None)`
    /// runs it where `work` started. `Err` refuses it, having removed
    /// whatever it created: the task goes back to `pending/` untouched and
    /// `work` stops, exit 1 (decision 2).
    fn prepare(&mut self, _id: &str, _task: &Task) -> Result<Option<Workspace>, String> {
        Ok(None)
    }
    /// Removes the workspace `prepare` made, after an outcome whose tree
    /// was restored (ADR 0053 decision 4). Says what it kept and why, and
    /// returns what is left for the task's outcome to record.
    fn remove_workspace(&mut self, _out: &mut dyn Write) -> Left {
        Left::ALL
    }
    /// The platform probe `[work] stale_claims = "release"` needs. Without
    /// one, stale claims are listed and nothing is released.
    fn liveness(&self) -> Option<&dyn Liveness> {
        None
    }
    /// Unix seconds, for the stale-claim sweep's age and throttle.
    fn now(&self) -> i64 {
        now_secs()
    }
    /// The agent profiles an `@name` in a person's note may wake, as of
    /// this tick (`mention.rs`). `None` scans nothing: a runner that knows
    /// no profiles cannot tell a mention from an unknown name.
    fn profiles(&self) -> Option<&BTreeMap<String, profile::Profile>> {
        None
    }
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
    /// The worktree is not clean; nothing was claimed.
    Blocked(String),
    /// The claimed task's worktree was refused before its run; the task is
    /// back in `pending/` untouched (ADR 0053 decision 2).
    Refused { id: String, why: String },
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
            Stop::Empty | Stop::Waiting(_) | Stop::MaxTasks => 0,
            Stop::Blocked(_) | Stop::Refused { .. } | Stop::Error { .. } => 1,
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
            Stop::Blocked(_) => write!(f, "worktree not clean; nothing claimed"),
            Stop::Refused { id, .. } => write!(f, "{id} not run; put back in pending/"),
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
    /// What to do with a task another process left in `claimed/`.
    pub stale_claims: StaleClaims,
}

/// Wakes, logged and printed. Failure to reconcile is reported and not
/// fatal: a wake missed now is found on the next tick.
fn reconcile_and_say(q: &Queue, now: i64, out: &mut dyn Write) {
    match q.reconcile_at(Actor::Work, now) {
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

/// The stale-claim sweep, printed. Each release is said and logged (by
/// `release_stale_claims`); why a claim was kept is said once per claim per
/// invocation, since under `--every` it would otherwise repeat every tick.
/// A sweep that fails is reported and not fatal, as a reconcile is.
fn release_stale_and_say(
    q: &Queue,
    runner: &dyn Runner,
    said: &mut HashSet<String>,
    out: &mut dyn Write,
) {
    let Some(live) = runner.liveness() else {
        if said.insert(String::new()) {
            let _ = writeln!(
                out,
                "queue:    stale_claims = \"release\", but no process probe is available; \
                 stale claims are listed, not released"
            );
        }
        return;
    };
    match q.release_stale_claims(live, runner.now()) {
        Ok(swept) => {
            for (id, s) in swept {
                match s {
                    Swept::Released(detail) => {
                        let _ = writeln!(
                            out,
                            "queue:    {id} -> pending/ (stale claim released: {detail}; attempts kept)"
                        );
                    }
                    Swept::Blocked(detail) => {
                        let _ = writeln!(
                            out,
                            "queue:    {id} -> pending/ blocked (stale claim not released again: \
                             {detail}; released {MAX_STALE_RELEASES} times already; \
                             `trellis queue set {id} --status todo` gives it another run)"
                        );
                    }
                    Swept::Kept(why) => {
                        if said.insert(format!("{id}\n{why}")) {
                            let _ = writeln!(
                                out,
                                "queue:    {id} stays in claimed/ and will not be re-run: {why}"
                            );
                        }
                    }
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "queue:    could not release stale claims: {e}");
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
    let mut said_kept = HashSet::new();
    let stale = if opts.stale_claims == StaleClaims::Release {
        release_stale_and_say(q, runner, &mut said_kept, out);
        let left = q.ids(State::Claimed)?;
        if !left.is_empty() {
            let _ = writeln!(
                out,
                "          `trellis queue requeue <id>` puts one back in pending/ by hand"
            );
        }
        Vec::new()
    } else {
        q.ids(State::Claimed)?
    };
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
        // Each tick, not only at start: under `--every` a `queue set` in
        // another terminal can crash holding a claim hours after this began.
        if opts.stale_claims == StaleClaims::Release {
            release_stale_and_say(q, runner, &mut said_kept, out);
        }
        // Before the claim, so a task a mention just woke can be claimed on
        // this same tick.
        if let Some(profiles) = runner.profiles() {
            mention::wake_and_say(q, profiles, out);
        }
        reconcile_and_say(q, runner.now(), out);
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
                    // One printed line per line of the reason, so a
                    // multi-line one (the shared no-model message, #444)
                    // reads as it does from `trellis run`; `plain()` still
                    // escapes every other control character in each line.
                    for (id, why) in &waiting {
                        let mut lines = why.lines();
                        let first = lines.next().unwrap_or("");
                        let _ = writeln!(out, "          {id}: {}", plain(first));
                        for l in lines {
                            let _ = writeln!(out, "          {}", plain(l));
                        }
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
        // After the claim, so the id is this process's to use, and before
        // the run: a refusal puts the task back as it was, not counted as
        // an attempt (ADR 0053 decision 2).
        let workspace = match runner.prepare(&claim.id, &claim.task) {
            Ok(w) => w,
            Err(why) => {
                let id = claim.id.clone();
                claim.release(false)?;
                log_or_say(
                    q,
                    Event::new(Actor::Work, "task.release", format!("task:{id}"))
                        .detail(format!("not run: {why}")),
                    out,
                );
                let _ = writeln!(
                    out,
                    "work:     {id} -> pending/ (not run; attempts unchanged): {}",
                    plain(&why)
                );
                return Ok((ran, Stop::Refused { id, why }));
            }
        };
        if let Some(w) = &workspace {
            let _ = writeln!(
                out,
                "work:     in {} on {}",
                plain(&w.path.display().to_string()),
                plain(&w.branch)
            );
        }
        runner.starting(&claim.id);
        let end = runner.run(&claim.task);
        ran += 1;
        let id = claim.id.clone();
        let entity = format!("task:{id}");
        let now = iso_now();
        // ADR 0053 decision 4: a restored tree's worktree is removed (its
        // branch kept when it holds the trail), every other is kept. What
        // is left is what the outcome records.
        let mut settle = |keep: bool, out: &mut dyn Write| match &workspace {
            None => None,
            Some(w) => {
                let left = if keep {
                    Left::ALL
                } else {
                    runner.remove_workspace(out)
                };
                Some((w.clone(), left))
            }
        };
        let outcome = |run_id: &Option<String>, exit: &str, left: &Option<(Workspace, Left)>| {
            let (worktree, branch) = match left {
                Some((w, l)) => (
                    l.worktree.then(|| w.path.display().to_string()),
                    l.branch.then(|| w.branch.clone()),
                ),
                None => (None, None),
            };
            Outcome {
                run_id: run_id.clone(),
                exit: exit.to_string(),
                finished_at: now.clone(),
                worktree,
                branch,
            }
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
                let left = settle(true, out);
                claim.finish(
                    State::Done,
                    outcome(
                        &run_id,
                        "converged; change left uncommitted for review",
                        &left,
                    ),
                )?;
                finished(&run_id, "in_review", out);
                let _ = writeln!(out, "work:     {id} -> done/ (converged; in review)");
                if let Some((w, _)) = &left {
                    let _ = writeln!(
                        out,
                        "work:     the change is uncommitted in {} on {}; trellis does not \
                         commit, merge or remove it",
                        plain(&w.path.display().to_string()),
                        plain(&w.branch)
                    );
                }
                let _ = writeln!(
                    out,
                    "          once accepted, `trellis queue set {id} --status done` \
                     releases whatever it blocks"
                );
                // The change is in its own worktree, so the tree the next
                // task is based on is untouched: `work` goes on (ADR 0053
                // decision 4). A runner that ran it in place left the change
                // here, and the next claim's clean check stops on it.
            }
            RunEnd::RolledBack { run_id } => {
                let left = settle(false, out);
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "not converged; tree restored", &left),
                )?;
                finished(&run_id, "failed", out);
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/ (not converged; tree restored)"
                );
            }
            RunEnd::NotRestored { run_id } => {
                // Kept: nobody has looked at it, and removing it would
                // destroy the evidence.
                let left = settle(true, out);
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "not converged; tree NOT restored", &left),
                )?;
                finished(&run_id, "failed; tree NOT restored", out);
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/. The tree was NOT restored; stopping"
                );
                say_kept(&left, out);
                return Ok((ran, Stop::NotRestored { id }));
            }
            RunEnd::Interrupted {
                restored: true,
                run_id,
            } => {
                // The run's rollback put the tree back, and the task never
                // produced an outcome, so it is safe to run again later.
                settle(false, out);
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
                let left = settle(true, out);
                claim.finish(
                    State::Failed,
                    outcome(&run_id, "interrupted; tree NOT restored", &left),
                )?;
                finished(&run_id, "failed; interrupted, tree NOT restored", out);
                let _ = writeln!(
                    out,
                    "work:     {id} -> failed/ (interrupted; the tree was NOT restored)"
                );
                say_kept(&left, out);
                return Ok((ran, Stop::Interrupted));
            }
            RunEnd::Asked {
                run_id,
                question,
                options,
            } => {
                // The tree is back as it was, so its worktree goes as a
                // rolled-back run's does, and the heartbeat goes on to the
                // next task; this one waits for a person.
                settle(false, out);
                claim.ask(&question, &options)?;
                let note = question_note(&question, &options);
                log_or_say(
                    q,
                    Event::new(Actor::Work, "task.question", entity.clone())
                        .run(run_id)
                        .detail(note.clone()),
                    out,
                );
                let _ = writeln!(
                    out,
                    "work:     {id} -> pending/ (blocked: asked a question; tree restored)"
                );
                let _ = writeln!(out, "          {}", plain(&note));
                let _ = writeln!(
                    out,
                    "          answer with `trellis queue note {id} <text>`, then \
                     `trellis queue set {id} --status todo`"
                );
            }
            RunEnd::Error { run_id, error } => {
                let left = settle(false, out);
                claim.finish(
                    State::Failed,
                    outcome(&run_id, &format!("error: {error}"), &left),
                )?;
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

/// Where a kept worktree is, for the outcomes that keep one for a person.
fn say_kept(left: &Option<(Workspace, Left)>, out: &mut dyn Write) {
    if let Some((w, _)) = left {
        let _ = writeln!(
            out,
            "          its worktree is kept for a person to look at: {} on {}",
            plain(&w.path.display().to_string()),
            plain(&w.branch)
        );
    }
}
