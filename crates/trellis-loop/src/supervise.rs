//! Running a staged plan: one git worktree per partition, bounded
//! concurrency, merged back in plan order (ADR 0037, amending ADR 0016).
//!
//! The supervisor schedules and moves files. It never judges: a partition's
//! outcome is whatever its own work reported, and nothing here reads one
//! partition's output to decide about another's (rule 5). What it returns
//! carries no verdict, because partition-local passes never substitute for
//! the integrated check over the merged union (rule 6, S19 patch 2). That
//! check runs in [`Supervisor::run_integrated`], the only public way to run a
//! plan, so no caller can merge partitions and skip it (ADR 0042).

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::future::Future;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tracing::Instrument;
use trellis_core::PartitionId;
use trellis_vcs::Worktree;

use crate::budget::{Budget, BudgetTooSmall};
use crate::partition::{FailurePolicy, Partition, Plan};
use crate::path_rules::names_trellis_dir;

/// What one partition's work is handed.
#[derive(Debug, Clone)]
pub struct PartitionTask {
    pub id: PartitionId,
    pub partition: Partition,
    /// The partition's own worktree. Its work reads and writes here, never
    /// in the run's root.
    pub root: PathBuf,
    /// This partition's share of the run budget (ADR 0008).
    pub budget: Budget,
}

/// Why a partition did not land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionFailure {
    /// Its worktree could not be created.
    Setup(String),
    /// Its work finished and reported that it did not converge.
    NotConverged(String),
    /// Its work panicked. The payload's message when it had one.
    Panicked(String),
    /// Its task ended without finishing or panicking.
    Cancelled,
    /// It never ran, for the reason given.
    NotStarted(&'static str),
    /// Its work converged, but what it changed cannot be merged into the
    /// root: a path another partition earlier in plan order also changed, a
    /// path under `.trellis/` or `.git/`, or something that is not a file.
    Unmergeable(String),
}

impl Display for PartitionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(e) => write!(f, "its worktree could not be created: {e}"),
            Self::NotConverged(e) => write!(f, "did not converge: {e}"),
            Self::Panicked(m) => write!(f, "panicked: {m}"),
            Self::Cancelled => f.write_str("its task was cancelled"),
            Self::NotStarted(why) => write!(f, "not started: {why}"),
            Self::Unmergeable(e) => write!(f, "not merged: {e}"),
        }
    }
}

/// One partition's result.
#[derive(Debug)]
pub struct PartitionReport<T> {
    pub id: PartitionId,
    pub outcome: Result<T, PartitionFailure>,
}

/// Every partition's report, in plan order, and the paths merged into the
/// root.
///
/// Not a verdict. `merged` is what now differs in the root's working tree;
/// whether that union is correct is for the integrated check to say.
#[must_use = "the merged union has not been checked; run the integrated check on the root"]
#[derive(Debug)]
pub struct Supervised<T> {
    pub reports: Vec<PartitionReport<T>>,
    /// Root-relative paths written or removed, in the order applied.
    pub merged: Vec<String>,
    /// The partition each path in `merged` came from. What the integrated
    /// check attributes a failure's files to (ADR 0042).
    pub owner: HashMap<String, PartitionId>,
}

#[derive(Debug, thiserror::Error)]
pub enum SuperviseError {
    #[error(transparent)]
    Budget(#[from] BudgetTooSmall),
    /// A write into the root failed part-way through the merge. The root
    /// may hold some of the merge; the run's snapshot is what restores it.
    #[error("merging partition {id} into the root at {path}: {source}")]
    Merge {
        id: u32,
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The root could not be snapshotted before the plan ran (a dirty
    /// worktree, rule 7), or could not be rolled back after it. A failed
    /// rollback leaves the root mid-run, and the message says how to
    /// finish by hand.
    #[error(transparent)]
    Vcs(#[from] trellis_vcs::VcsError),
}

/// Removes its worktree when dropped, however the partition ended.
///
/// Owned by the partition's task, so a panic in the work drops it with the
/// task's future and the directory still goes. On a normal finish the task
/// hands it back, and the worktree lives until the merge has read it.
struct WorktreeGuard(Option<Worktree>);

impl WorktreeGuard {
    fn path(&self) -> Option<&Path> {
        self.0.as_ref().map(|w| w.path.as_path())
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if let Some(wt) = self.0.take() {
            // Never panics: this runs during a partition's unwind, and a
            // second panic there aborts the process, rollback and all.
            if let Err(e) = wt.discard() {
                tracing::warn!(
                    path = %wt.path.display(),
                    error = %e,
                    "a partition worktree was not removed"
                );
            }
        }
    }
}

/// One partition after its task ended: the outcome, and the worktree when
/// the task handed it back.
struct Finished<T> {
    id: PartitionId,
    outcome: Result<T, PartitionFailure>,
    guard: Option<WorktreeGuard>,
}

/// Where and how a plan runs.
#[derive(Debug, Clone)]
pub struct Supervisor<'a> {
    /// The run's root. Worktrees go under its `.trellis/work/<run>/`, and
    /// what lands is merged into its working tree.
    pub root: &'a Path,
    /// Names the worktree directory and branches.
    pub run: &'a str,
    /// The commit every partition's worktree is branched from.
    pub base: &'a str,
    /// The whole run's budget, divided across the plan.
    pub budget: &'a Budget,
    /// How many partitions may run at once.
    pub width: NonZeroUsize,
    pub policy: FailurePolicy,
}

impl Supervisor<'_> {
    /// Runs `plan`: its `shared` partition alone first, then its `parallel`
    /// partitions at most `width` at a time, each in its own worktree branched
    /// from `base`. Merges what landed into `root`'s working tree in plan order.
    ///
    /// `width` bounds concurrency, not the plan: a plan wider than `width`
    /// queues. Budgets come from [`Budget::shares`] over every partition, so
    /// they sum to `budget` (rule 9); a plan wider than the budget is refused
    /// before any worktree exists.
    ///
    /// The parallel stage does not start unless the shared stage landed: ADR
    /// 0016's case for running the rest concurrently is that the contended
    /// work has already landed. Its partitions get what is left of the run's
    /// wall-clock ceiling, because the two stages run one after the other.
    ///
    /// Under [`FailurePolicy::AllOrNothing`] any failure merges nothing. Under
    /// [`FailurePolicy::LandClean`] every partition that converged and can be
    /// merged lands. Either way every worktree is removed before this returns.
    ///
    /// `work` is called on the supervisor's task; the future it returns runs
    /// on its own.
    pub(crate) async fn run_plan<T, E, F, Fut>(
        &self,
        plan: &Plan,
        work: F,
    ) -> Result<Supervised<T>, SuperviseError>
    where
        F: Fn(PartitionTask) -> Fut,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Display + Send + 'static,
    {
        let (root, run, budget) = (self.root, self.run, self.budget);
        let n = u32::try_from(plan.in_order().count()).unwrap_or(u32::MAX);
        let mut shares = budget.shares(n)?.into_iter();
        let started = Instant::now();
        let mut finished: Vec<Finished<T>> = Vec::new();

        let mut shared_landed = true;
        if let Some(shared) = &plan.shared {
            let share = shares
                .next()
                .expect("Budget::shares returns one share per partition in the plan");
            let done = self
                .stage(vec![(shared, share)], NonZeroUsize::MIN, &work)
                .await;
            shared_landed = done.iter().all(|f| f.outcome.is_ok());
            finished.extend(done);
        }

        let remaining = budget.max_wall_clock.saturating_sub(started.elapsed());
        let not_started = if !shared_landed {
            Some("the shared stage did not land")
        } else if remaining.is_zero() {
            Some("the shared stage spent the run's wall-clock ceiling")
        } else {
            None
        };
        match not_started {
            Some(why) => finished.extend(plan.parallel.iter().map(|p| Finished {
                id: p.id,
                outcome: Err(PartitionFailure::NotStarted(why)),
                guard: None,
            })),
            None => {
                let parts = plan
                    .parallel
                    .iter()
                    .zip(shares.by_ref())
                    .map(|(p, mut b)| {
                        b.max_wall_clock = remaining;
                        (p, b)
                    })
                    .collect();
                finished.extend(self.stage(parts, self.width, &work).await);
            }
        }

        // Plan order is integration order (`Plan::in_order`), whatever order
        // the tasks happened to finish in.
        let position: HashMap<PartitionId, usize> = plan
            .in_order()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        finished.sort_by_key(|f| position.get(&f.id).copied().unwrap_or(usize::MAX));

        let (merged, owner) = merge(root, self.base, self.policy, &mut finished)?;

        let mut reports = Vec::with_capacity(finished.len());
        for f in finished {
            // The worktree goes here, once the merge has read it.
            drop(f.guard);
            reports.push(PartitionReport {
                id: f.id,
                outcome: f.outcome,
            });
        }
        // The run's directory under `.trellis/work/` is empty now. Removing it is
        // tidiness, not correctness: `remove_dir` refuses a non-empty directory,
        // so a worktree that failed to go is left where the warning named it.
        let _ = std::fs::remove_dir(root.join(".trellis").join("work").join(run));

        Ok(Supervised {
            reports,
            merged,
            owner,
        })
    }

    /// Runs `parts` in a `JoinSet`, at most `width` at once.
    async fn stage<T, E, F, Fut>(
        &self,
        parts: Vec<(&Partition, Budget)>,
        width: NonZeroUsize,
        work: &F,
    ) -> Vec<Finished<T>>
    where
        F: Fn(PartitionTask) -> Fut,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Display + Send + 'static,
    {
        let permits = Arc::new(Semaphore::new(width.get()));
        let mut set: JoinSet<(Result<T, String>, WorktreeGuard)> = JoinSet::new();
        let mut ids: HashMap<tokio::task::Id, PartitionId> = HashMap::new();
        let mut out = Vec::new();

        for (partition, share) in parts {
            let id = partition.id;
            // The permit is taken before the worktree is made, so at most
            // `width` worktrees exist in this stage before the merge. The
            // semaphore is never closed, so `Err` does not happen; it is
            // reported rather than unwrapped all the same.
            let permit: OwnedSemaphorePermit = match permits.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    out.push(Finished {
                        id,
                        outcome: Err(PartitionFailure::NotStarted("the stage's semaphore closed")),
                        guard: None,
                    });
                    continue;
                }
            };
            // Made here, one at a time, not inside the tasks: concurrent
            // `git worktree add` calls contend for `.git/worktrees`.
            let wt = match Worktree::create(self.root, self.run, id.0, self.base) {
                Ok(wt) => wt,
                Err(e) => {
                    out.push(Finished {
                        id,
                        outcome: Err(PartitionFailure::Setup(e.to_string())),
                        guard: None,
                    });
                    continue;
                }
            };
            let task = PartitionTask {
                id,
                partition: partition.clone(),
                root: wt.path.clone(),
                budget: share,
            };
            let guard = WorktreeGuard(Some(wt));
            let fut = work(task);
            // Per-partition spend nests under the caller's run span (ADR 0008);
            // there is no second telemetry path.
            let span = tracing::info_span!("partition", id = id.0);
            let handle = set.spawn(
                async move {
                    let _permit = permit;
                    let guard = guard;
                    let result = fut.await.map_err(|e| e.to_string());
                    (result, guard)
                }
                .instrument(span),
            );
            ids.insert(handle.id(), id);
        }

        while let Some(joined) = set.join_next_with_id().await {
            match joined {
                Ok((task, (result, guard))) => {
                    if let Some(id) = ids.remove(&task) {
                        out.push(Finished {
                            id,
                            outcome: result.map_err(PartitionFailure::NotConverged),
                            guard: Some(guard),
                        });
                    }
                }
                Err(e) => {
                    let Some(id) = ids.remove(&e.id()) else {
                        continue;
                    };
                    let failure = if e.is_panic() {
                        PartitionFailure::Panicked(panic_message(e.into_panic()))
                    } else {
                        PartitionFailure::Cancelled
                    };
                    tracing::warn!(partition = id.0, %failure, "a partition did not finish");
                    out.push(Finished {
                        id,
                        outcome: Err(failure),
                        guard: None,
                    });
                }
            }
        }
        out
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with no message".to_string()
    }
}

/// One path a partition changed relative to the run's base.
enum Change {
    Write(String),
    Remove(String),
}

impl Change {
    fn path(&self) -> &str {
        match self {
            Change::Write(p) | Change::Remove(p) => p,
        }
    }
}

/// Merges every partition that may land into `root`, in the order given.
///
/// Reads every change set before writing anything, so a conflict found in
/// the last partition still stops an all-or-nothing merge before the first
/// write.
fn merge<T>(
    root: &Path,
    base: &str,
    policy: FailurePolicy,
    finished: &mut [Finished<T>],
) -> Result<(Vec<String>, HashMap<String, PartitionId>), SuperviseError> {
    let mut plans: Vec<(usize, Vec<Change>)> = Vec::new();
    let mut claimed: HashMap<String, PartitionId> = HashMap::new();

    for (i, f) in finished.iter_mut().enumerate() {
        if f.outcome.is_err() {
            continue;
        }
        let Some(wt) = f.guard.as_ref().and_then(WorktreeGuard::path) else {
            f.outcome = Err(PartitionFailure::Unmergeable(
                "its worktree was not available to merge".to_string(),
            ));
            continue;
        };
        let checked = changes(wt, base).and_then(|changes| {
            for c in &changes {
                refuse_path(wt, c.path())?;
                // A plan is a filter, not a guarantee: two partitions can
                // still both touch a file outside the file sets the graph
                // gave them. The earlier one in plan order keeps it and the
                // later one is reported; nothing is overwritten.
                if let Some(other) = claimed.get(c.path()) {
                    return Err(format!(
                        "{} was also changed by partition {}, earlier in plan order",
                        c.path(),
                        other.0
                    ));
                }
            }
            Ok(changes)
        });
        match checked {
            Ok(changes) => {
                for c in &changes {
                    claimed.insert(c.path().to_string(), f.id);
                }
                plans.push((i, changes));
            }
            Err(e) => f.outcome = Err(PartitionFailure::Unmergeable(e)),
        }
    }

    if policy == FailurePolicy::AllOrNothing && finished.iter().any(|f| f.outcome.is_err()) {
        return Ok((Vec::new(), HashMap::new()));
    }

    let mut merged = Vec::new();
    let mut owner = HashMap::new();
    for (i, changes) in plans {
        let f = &finished[i];
        let Some(wt) = f.guard.as_ref().and_then(WorktreeGuard::path) else {
            continue;
        };
        for c in changes {
            let err = |source| SuperviseError::Merge {
                id: f.id.0,
                path: c.path().to_string(),
                source,
            };
            let dest = root.join(c.path());
            match &c {
                Change::Write(p) => {
                    let bytes = std::fs::read(wt.join(p)).map_err(err)?;
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).map_err(err)?;
                    }
                    std::fs::write(&dest, bytes).map_err(err)?;
                }
                Change::Remove(_) => match std::fs::remove_file(&dest) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(err(e)),
                },
            }
            owner.insert(c.path().to_string(), f.id);
            merged.push(c.path().to_string());
        }
    }
    Ok((merged, owner))
}

/// Refuses a path the merge must not write into the root.
///
/// Model writes never land in `.trellis/` or `.git/` (ADR 0023), and a
/// partition's worktree is where a model wrote. A link is refused too:
/// copying what it points at would carry a file from outside the worktree
/// into the root.
fn refuse_path(wt: &Path, rel: &str) -> Result<(), String> {
    if names_trellis_dir(rel) {
        return Err(format!("{rel} is under .trellis/"));
    }
    if rel.split(['/', '\\']).any(|seg| {
        seg.trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(".git")
    }) {
        return Err(format!("{rel} is under .git/"));
    }
    match std::fs::symlink_metadata(wt.join(rel)) {
        Ok(m) if !m.file_type().is_file() => Err(format!("{rel} is not a regular file")),
        _ => Ok(()),
    }
}

/// What the worktree at `wt` changed against `base`: tracked paths from
/// `git diff`, new ones from `git ls-files --others`.
///
/// Read without `git add`, so the worktree's index is left as the partition
/// left it.
fn changes(wt: &Path, base: &str) -> Result<Vec<Change>, String> {
    let diff = git_z(
        wt,
        &["diff", "--name-status", "--no-renames", "-z", base, "--"],
    )?;
    let mut out = Vec::new();
    let mut fields = diff.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else {
            return Err(format!("the worktree diff ended after status {status:?}"));
        };
        match status {
            "A" | "M" | "T" => out.push(Change::Write(path.to_string())),
            "D" => out.push(Change::Remove(path.to_string())),
            other => return Err(format!("{path}: unexpected diff status {other:?}")),
        }
    }
    let untracked = git_z(wt, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut seen: HashSet<String> = out.iter().map(|c| c.path().to_string()).collect();
    for path in untracked.split('\0').filter(|s| !s.is_empty()) {
        if seen.insert(path.to_string()) {
            out.push(Change::Write(path.to_string()));
        }
    }
    Ok(out)
}

fn git_z(wt: &Path, args: &[&str]) -> Result<String, String> {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(wt)
        .output()
        .map_err(|e| format!("running git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "running git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| format!("running git {}: output is not UTF-8", args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::testutil::{Template, git};
    use std::sync::OnceLock;

    /// A committed repository with `a.py` and `b.py`, and its HEAD: a copy
    /// of one built once per process (`testutil`).
    fn repo() -> (tempfile::TempDir, String) {
        static T: OnceLock<Template> = OnceLock::new();
        let t = T.get_or_init(|| {
            Template::build(|root| {
                for args in [
                    ["init", "-q"].as_slice(),
                    &["config", "core.autocrlf", "false"],
                    &["config", "user.email", "t@example.invalid"],
                    &["config", "user.name", "t"],
                ] {
                    git(root, args);
                }
                std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
                std::fs::write(root.join("b.py"), "def b():\n    return 1\n").unwrap();
                git(root, &["add", "-A"]);
                git(root, &["commit", "-qm", "base"]);
            })
        });
        (t.instance(), t.head().to_string())
    }

    fn partition(id: u32, conservative: bool) -> Partition {
        Partition {
            id: PartitionId(id),
            units: Vec::new(),
            radius: HashSet::new(),
            files: HashSet::new(),
            conservative,
        }
    }

    /// `shared` (id 0) when asked for, then `parallel` more, numbered on.
    fn plan(shared: bool, parallel: u32) -> Plan {
        let first = u32::from(shared);
        Plan {
            shared: shared.then(|| partition(0, true)),
            parallel: (first..first + parallel)
                .map(|i| partition(i, false))
                .collect(),
        }
    }

    fn budget(iterations: u32, tokens: u64) -> Budget {
        Budget {
            max_iterations: iterations,
            max_tokens: tokens,
            ..Budget::default()
        }
    }

    fn supervisor<'a>(
        root: &'a Path,
        base: &'a str,
        budget: &'a Budget,
        width: usize,
        policy: FailurePolicy,
    ) -> Supervisor<'a> {
        Supervisor {
            root,
            run: "run",
            base,
            budget,
            width: NonZeroUsize::new(width).unwrap(),
            policy,
        }
    }

    #[tokio::test]
    async fn partition_worktree_concurrency_is_bounded_by_the_width() {
        // The risk: an unbounded fan-out. Five partitions under a width of
        // two must never have three in flight, and must actually overlap,
        // or the bound is only ever tested against serial execution.
        let (dir, base) = repo();
        let b = budget(40, 1_000);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let roots = Arc::new(Mutex::new(Vec::new()));

        let out = supervisor(dir.path(), &base, &b, 2, FailurePolicy::AllOrNothing)
            .run_plan(&plan(false, 5), |task| {
                let (live, peak, roots) = (live.clone(), peak.clone(), roots.clone());
                async move {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    assert!(task.root.join("a.py").exists(), "a checkout of base");
                    roots.lock().unwrap().push(task.root.clone());
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, String>(())
                }
            })
            .await
            .unwrap();

        assert_eq!(peak.load(Ordering::SeqCst), 2, "width 2 means two at once");
        assert!(out.reports.iter().all(|r| r.outcome.is_ok()));
        let roots = roots.lock().unwrap();
        let distinct: HashSet<_> = roots.iter().collect();
        assert_eq!(distinct.len(), 5, "one worktree per partition");
        assert!(roots.iter().all(|r| !r.exists()), "every worktree removed");
    }

    #[tokio::test]
    async fn partition_worktree_shared_runs_alone_first() {
        // ADR 0016: contended work runs alone and before anything else, so
        // no parallel partition may start until the shared one has ended,
        // however wide the stage is allowed to be.
        let (dir, base) = repo();
        let b = budget(40, 1_000);
        let events = Arc::new(Mutex::new(Vec::new()));

        let out = supervisor(dir.path(), &base, &b, 4, FailurePolicy::AllOrNothing)
            .run_plan(&plan(true, 3), |task| {
                let events = events.clone();
                async move {
                    events.lock().unwrap().push((task.id.0, "start"));
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    events.lock().unwrap().push((task.id.0, "end"));
                    Ok::<_, String>(())
                }
            })
            .await
            .unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events[..2], [(0, "start"), (0, "end")], "{events:?}");
        assert_eq!(
            events[2..5].iter().filter(|e| e.1 == "start").count(),
            3,
            "the parallel stage then runs its partitions together: {events:?}"
        );
        let ids: Vec<u32> = out.reports.iter().map(|r| r.id.0).collect();
        assert_eq!(ids, [0, 1, 2, 3], "reports come back in plan order");
    }

    #[tokio::test]
    async fn partition_worktree_a_panicking_partition_is_reported_and_its_worktree_removed() {
        // The risk: a panic swallowed by the JoinSet, so the run reports
        // success with a partition missing, and a worktree left behind
        // holding half its work.
        let (dir, base) = repo();
        let b = budget(40, 1_000);
        let roots = Arc::new(Mutex::new(Vec::new()));

        let out = supervisor(dir.path(), &base, &b, 2, FailurePolicy::AllOrNothing)
            .run_plan(&plan(false, 2), |task| {
                let roots = roots.clone();
                async move {
                    roots.lock().unwrap().push(task.root.clone());
                    std::fs::write(task.root.join("a.py"), "def a():\n    return 2\n").unwrap();
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    if task.id.0 == 1 {
                        panic!("partition one fell over");
                    }
                    Ok::<_, String>(())
                }
            })
            .await
            .unwrap();

        assert!(out.reports[0].outcome.is_ok());
        assert_eq!(
            out.reports[1].outcome.as_ref().err(),
            Some(&PartitionFailure::Panicked(
                "partition one fell over".to_string()
            ))
        );
        let roots = roots.lock().unwrap();
        assert_eq!(roots.len(), 2);
        for r in roots.iter() {
            assert!(!r.exists(), "worktree left behind: {}", r.display());
        }
        let branches = git(dir.path(), &["branch", "--list"]);
        assert!(!branches.contains("trellis/run/p1"), "{branches}");
        assert_eq!(
            git(dir.path(), &["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1,
            "only the root is still a worktree"
        );
        // All or nothing: the partition that converged does not land either.
        assert!(out.merged.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "def a():\n    return 1\n"
        );
    }

    #[tokio::test]
    async fn partition_worktree_budgets_sum_to_the_run_budget() {
        // ADR 0008: budgets divide, they never multiply. 40 iterations over
        // three partitions does not divide evenly, which is the case a
        // floor-and-clamp split gets wrong in one direction or the other.
        let (dir, base) = repo();
        let b = budget(40, 1_000_001);
        let seen = Arc::new(Mutex::new(Vec::new()));

        let out = supervisor(dir.path(), &base, &b, 2, FailurePolicy::AllOrNothing)
            .run_plan(&plan(true, 2), |task| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(task.budget.clone());
                    Ok::<_, String>(())
                }
            })
            .await
            .unwrap();
        assert!(out.reports.iter().all(|r| r.outcome.is_ok()));

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen.iter().map(|s| s.max_iterations).sum::<u32>(), 40);
        assert_eq!(seen.iter().map(|s| s.max_tokens).sum::<u64>(), 1_000_001);
        assert!(
            seen.iter().all(|s| s.max_wall_clock <= b.max_wall_clock),
            "no stage is given more wall-clock than the run"
        );

        // Wider than the budget: refused before any worktree exists, rather
        // than each partition being handed a minimum the run does not have.
        let tiny = budget(2, 1_000);
        let err = supervisor(dir.path(), &base, &tiny, 2, FailurePolicy::AllOrNothing)
            .run_plan(&plan(true, 2), |_| async { Ok::<(), String>(()) })
            .await
            .unwrap_err();
        assert!(matches!(err, SuperviseError::Budget(_)), "{err}");
        assert!(!dir.path().join(".trellis/work/run").exists());
    }

    #[tokio::test]
    async fn a_shared_stage_that_does_not_land_leaves_the_parallel_stage_unstarted() {
        // The parallel stage is safe to run only once the shared stage has
        // landed (ADR 0016). Starting it anyway spends budget on work whose
        // premise failed.
        let (dir, base) = repo();
        let b = budget(40, 1_000);
        let ran = Arc::new(AtomicUsize::new(0));

        let out = supervisor(dir.path(), &base, &b, 2, FailurePolicy::LandClean)
            .run_plan(&plan(true, 2), |task| {
                let ran = ran.clone();
                async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                    if task.id.0 == 0 {
                        Err("ran out of iterations".to_string())
                    } else {
                        Ok(())
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert!(matches!(
            out.reports[0].outcome,
            Err(PartitionFailure::NotConverged(_))
        ));
        assert!(
            out.reports[1..]
                .iter()
                .all(|r| matches!(r.outcome, Err(PartitionFailure::NotStarted(_))))
        );
    }

    #[tokio::test]
    async fn merging_follows_plan_order_and_never_overwrites_an_earlier_partition() {
        // Two partitions writing one file means the plan's file sets were
        // wrong for this run. The earlier one in plan order keeps the path
        // and the later one is reported, rather than whichever finished last
        // silently winning.
        let (dir, base) = repo();
        let b = budget(40, 1_000);

        let out = supervisor(dir.path(), &base, &b, 4, FailurePolicy::LandClean)
            .run_plan(&plan(false, 4), |task| async move {
                let w = |rel: &str, body: &str| {
                    let p = task.root.join(rel);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, body).unwrap();
                };
                match task.id.0 {
                    // Finishes last, lands first.
                    0 => {
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        w("a.py", "def a():\n    return 0\n");
                    }
                    1 => {
                        std::fs::remove_file(task.root.join("b.py")).unwrap();
                        w("pkg/new.py", "def n():\n    return 1\n");
                    }
                    2 => w("a.py", "def a():\n    return 2\n"),
                    _ => w(".trellis/planted", "x"),
                }
                Ok::<_, String>(())
            })
            .await
            .unwrap();

        assert_eq!(out.merged, ["a.py", "b.py", "pkg/new.py"]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "def a():\n    return 0\n"
        );
        assert!(!dir.path().join("b.py").exists());
        assert!(dir.path().join("pkg/new.py").exists());
        assert!(matches!(
            out.reports[2].outcome,
            Err(PartitionFailure::Unmergeable(ref e)) if e.contains("a.py")
        ));
        assert!(matches!(
            out.reports[3].outcome,
            Err(PartitionFailure::Unmergeable(ref e)) if e.contains(".trellis")
        ));
        assert!(!dir.path().join(".trellis/planted").exists());
    }
}
