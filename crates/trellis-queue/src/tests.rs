use super::*;
use std::collections::VecDeque;
use std::process::Command;
use std::sync::{Arc, Barrier};

fn queue() -> (tempfile::TempDir, Queue) {
    let d = tempfile::tempdir().unwrap();
    let q = Queue::at(d.path().join("queue"));
    (d, q)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A repository with one commit, which `git worktree add` needs.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "t@example.invalid"]);
    git(d.path(), &["config", "user.name", "t"]);
    git(d.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(d.path().join("a.py"), "x = 1\n").unwrap();
    git(d.path(), &["add", "-A"]);
    git(d.path(), &["commit", "-qm", "fixture"]);
    d
}

fn same_dir(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
}

// ---------------------------------------------------------------------------
// Where the queue lives
// ---------------------------------------------------------------------------

#[test]
fn the_queue_lives_in_the_git_directory_and_survives_a_rollback() {
    // The reason for the location (ADR 0020 §1): rollback is `reset --hard`
    // plus `clean -fd`, and a queue in the worktree would be deleted by the
    // very rollback of the run it was feeding.
    let r = repo();
    let q = Queue::for_repo(r.path()).unwrap();
    let id = q.add("rename greet", None, None).unwrap();
    assert!(same_dir(
        &r.path().join(".git/trellis/queue/pending"),
        &q.dir().join("pending")
    ));

    git(r.path(), &["reset", "--hard", "HEAD"]);
    git(r.path(), &["clean", "-fdx"]);
    assert_eq!(q.ids(State::Pending).unwrap(), vec![id]);
}

#[test]
fn queue_operations_leave_the_worktree_clean() {
    // A dirty worktree blocks every run (ADR 0009). If adding a task dirtied
    // it, the queue would stop the heartbeat it exists to feed.
    let r = repo();
    let q = Queue::for_repo(r.path()).unwrap();
    q.add("one", Some("child"), Some("fix")).unwrap();
    let c = q.claim_next().unwrap().unwrap();
    c.finish(
        State::Done,
        Outcome {
            run_id: None,
            exit: "x".into(),
            finished_at: iso_now(),
        },
    )
    .unwrap();
    q.add("two", None, None).unwrap();
    assert_eq!(git(r.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_linked_worktree_resolves_its_own_git_directory() {
    // In a linked worktree `.git` is a file. Hard-coding `root/.git` would
    // either fail or, worse, put the queue in a file's path.
    let r = repo();
    let wt = r.path().join("wt");
    git(r.path(), &["worktree", "add", "-q", "wt"]);
    assert!(wt.join(".git").is_file(), "the case under test");

    let q = Queue::for_repo(&wt).unwrap();
    assert!(same_dir(
        q.dir().parent().unwrap().parent().unwrap(),
        &r.path().join(".git/worktrees/wt")
    ));
    q.add("in the worktree", None, None).unwrap();
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
    // Per-worktree: the main checkout's queue does not see it.
    let main = Queue::for_repo(r.path()).unwrap();
    assert!(main.ids(State::Pending).unwrap().is_empty());
}

#[test]
fn a_directory_that_is_not_a_repository_is_refused() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(
        Queue::for_repo(d.path()),
        Err(QueueError::NotARepo(..))
    ));
}

// ---------------------------------------------------------------------------
// Files, order, claim
// ---------------------------------------------------------------------------

#[test]
fn a_task_file_has_the_documented_fields() {
    let (_d, q) = queue();
    let id = q.add("Rename greet()", Some("child"), None).unwrap();
    assert!(id.ends_with("-rename-greet"), "{id}");
    let text = std::fs::read_to_string(q.path(State::Pending, &id)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["task"], "Rename greet()");
    assert_eq!(v["focus"], "child");
    assert!(v["prompt"].is_null(), "null, not omitted: {text}");
    assert_eq!(v["attempts"], 0);
    assert!(v["added_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn claims_come_oldest_first() {
    let (_d, q) = queue();
    let a = q.add("first", None, None).unwrap();
    let b = q.add("second", None, None).unwrap();
    let c = q.add("third", None, None).unwrap();
    let got: Vec<String> = (0..3)
        .map(|_| q.claim_next().unwrap().unwrap().id)
        .collect();
    assert_eq!(got, vec![a, b, c]);
    assert!(q.claim_next().unwrap().is_none());
}

#[test]
fn two_claimers_one_file_exactly_one_wins() {
    // The atomic checkout (ADR 0020 §3). Repeated because a race that is
    // lost once in a hundred tries is still a race.
    for _ in 0..50 {
        let (_d, q) = queue();
        let id = q.add("contended", None, None).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (q, id, b) = (q.clone(), id.clone(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    q.claim(&id).map(|c| c.is_some())
                })
            })
            .collect();
        let results: Vec<bool> = handles
            .into_iter()
            .map(|h| h.join().unwrap().expect("losing is not an error"))
            .collect();
        assert_eq!(results.iter().filter(|w| **w).count(), 1, "{results:?}");
        assert_eq!(q.ids(State::Claimed).unwrap(), vec![id]);
        assert!(q.ids(State::Pending).unwrap().is_empty());
    }
}

#[test]
fn a_released_claim_returns_as_it_was_unless_a_run_was_attempted() {
    let (_d, q) = queue();
    let id = q.add("x", None, None).unwrap();
    q.claim_next().unwrap().unwrap().release(false).unwrap();
    let c = q.claim(&id).unwrap().unwrap();
    assert_eq!(c.task.attempts, 0);
    assert!(c.task.claimed_by_pid.is_some());
    c.release(true).unwrap();
    let t = read_task(&q.path(State::Pending, &id)).unwrap();
    assert_eq!(t.attempts, 1);
    assert_eq!(t.claimed_at, None, "a pending task is claimed by nobody");
}

#[test]
fn requeue_moves_a_stale_claim_back_and_refuses_what_is_not_there() {
    let (_d, q) = queue();
    let id = q.add("x", None, None).unwrap();
    std::mem::forget(q.claim_next().unwrap().unwrap()); // a crashed process
    assert_eq!(q.requeue(&format!("{id}.json")).unwrap(), State::Claimed);
    assert_eq!(q.ids(State::Pending).unwrap(), vec![id.clone()]);
    assert!(matches!(q.requeue("nope"), Err(QueueError::NoSuchTask(_))));

    // From failed/: the old outcome goes, the attempt count stays.
    let c = q.claim(&id).unwrap().unwrap();
    c.finish(
        State::Failed,
        Outcome {
            run_id: Some("r".into()),
            exit: "not converged".into(),
            finished_at: iso_now(),
        },
    )
    .unwrap();
    assert_eq!(q.requeue(&id).unwrap(), State::Failed);
    let t = read_task(&q.path(State::Pending, &id)).unwrap();
    assert_eq!((t.outcome, t.attempts), (None, 1));
}

#[test]
fn list_shows_every_state_and_an_unreadable_file_rather_than_hiding_it() {
    let (_d, q) = queue();
    q.add("a", None, None).unwrap();
    let b = q.add("b", None, None).unwrap();
    q.claim(&b).unwrap().unwrap();
    std::fs::write(q.dir().join("failed/junk.json"), "not json").unwrap();
    let l = q.list().unwrap();
    let states: Vec<State> = l.iter().map(|e| e.state).collect();
    assert_eq!(states, vec![State::Pending, State::Claimed, State::Failed]);
    assert!(l[2].task.is_err());
    assert!(l[1].age.is_some());
}

#[test]
fn slugs_are_bounded_and_never_empty() {
    assert_eq!(slugify("  Fix: the BUG!! "), "fix-the-bug");
    assert_eq!(slugify("¿?"), "task");
    assert!(slugify(&"a ".repeat(100)).len() <= 40);
}

// ---------------------------------------------------------------------------
// The heartbeat, against a runner that returns whatever it is told to
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Fake {
    ends: VecDeque<RunEnd>,
    blocked: Option<String>,
    ran: Vec<String>,
    stop_after_runs: Option<usize>,
    /// Set by `run`, seen by the check after the next claim.
    stop_after_claim: bool,
    idles: u32,
    idle_budget: u32,
}

impl Runner for Fake {
    fn blocker(&mut self) -> Option<String> {
        self.blocked.clone()
    }
    fn run(&mut self, task: &Task) -> RunEnd {
        self.ran.push(task.task.clone());
        self.ends
            .pop_front()
            .unwrap_or(RunEnd::RolledBack { run_id: None })
    }
    fn stop_requested(&self) -> bool {
        self.stop_after_claim || self.stop_after_runs.is_some_and(|n| self.ran.len() >= n)
    }
    fn idle(&mut self, _d: Duration) -> bool {
        self.idles += 1;
        self.idles <= self.idle_budget
    }
}

fn drain(q: &Queue, f: &mut dyn Runner, opts: WorkOptions) -> (u32, Stop, String) {
    let mut out = Vec::new();
    let (n, s) = work(q, f, opts, &mut out).unwrap();
    (n, s, String::from_utf8(out).unwrap())
}

fn three(q: &Queue) -> [String; 3] {
    [
        q.add("one", None, None).unwrap(),
        q.add("two", None, None).unwrap(),
        q.add("three", None, None).unwrap(),
    ]
}

#[test]
fn a_converged_run_stops_work_and_records_the_outcome_in_done() {
    // The change is uncommitted and the next run needs a clean tree, so
    // going on would mean either refusing every later task or committing
    // the human's decision for them.
    let (_d, q) = queue();
    let [a, _, _] = three(&q);
    let mut f = Fake {
        ends: [RunEnd::Converged {
            run_id: Some("20260927-x".into()),
        }]
        .into(),
        ..Fake::default()
    };
    let (n, stop, out) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, &stop), (1, &Stop::Converged { id: a.clone() }));
    assert_eq!(stop.code(), 0);
    assert!(out.contains("converged change awaits review"), "{out}");
    assert!(out.contains("run `trellis work` again"), "{out}");
    let t = read_task(&q.path(State::Done, &a)).unwrap();
    let o = t.outcome.unwrap();
    assert_eq!(o.run_id.as_deref(), Some("20260927-x"));
    assert!(o.exit.contains("converged"));
    assert_eq!(t.attempts, 1);
    assert_eq!(q.ids(State::Pending).unwrap().len(), 2);
}

#[test]
fn a_rolled_back_run_continues_to_the_next_task() {
    let (_d, q) = queue();
    let [a, b, c] = three(&q);
    let mut f = Fake {
        ends: [
            RunEnd::RolledBack {
                run_id: Some("r1".into()),
            },
            RunEnd::RolledBack { run_id: None },
            RunEnd::Converged { run_id: None },
        ]
        .into(),
        ..Fake::default()
    };
    let (n, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!(f.ran, vec!["one", "two", "three"], "oldest first");
    assert_eq!((n, stop), (3, Stop::Converged { id: c.clone() }));
    assert_eq!(q.ids(State::Failed).unwrap(), vec![a.clone(), b]);
    let t = read_task(&q.path(State::Failed, &a)).unwrap();
    assert_eq!(t.outcome.unwrap().run_id.as_deref(), Some("r1"));
}

#[test]
fn a_dirty_tree_is_refused_before_anything_is_claimed() {
    // Never claim a task that cannot start: a claimed task nobody runs is
    // stranded until a human notices it.
    let (_d, q) = queue();
    let ids = three(&q);
    let mut f = Fake {
        blocked: Some("M app/core.py".into()),
        ..Fake::default()
    };
    let (n, stop, out) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, stop.code()), (0, 1));
    assert!(matches!(stop, Stop::Blocked(_)));
    assert!(out.contains("M app/core.py"), "{out}");
    assert!(f.ran.is_empty());
    assert_eq!(q.ids(State::Pending).unwrap(), ids.to_vec());
    assert!(q.ids(State::Claimed).unwrap().is_empty());
}

#[test]
fn max_tasks_bounds_one_invocation() {
    let (_d, q) = queue();
    three(&q);
    let mut f = Fake::default();
    let opts = WorkOptions {
        max_tasks: Some(2),
        every: None,
    };
    let (n, stop, _) = drain(&q, &mut f, opts);
    assert_eq!((n, stop), (2, Stop::MaxTasks));
    assert_eq!(q.ids(State::Pending).unwrap().len(), 1);
}

#[test]
fn an_empty_queue_returns_and_every_waits_instead() {
    let (_d, q) = queue();
    let (n, stop, out) = drain(&q, &mut Fake::default(), WorkOptions::default());
    assert_eq!((n, stop), (0, Stop::Empty));
    assert!(out.contains("queue empty"));

    let mut f = Fake {
        idle_budget: 3,
        ..Fake::default()
    };
    let opts = WorkOptions {
        max_tasks: None,
        every: Some(Duration::from_secs(60)),
    };
    let (n, stop, _) = drain(&q, &mut f, opts);
    assert_eq!((n, stop), (0, Stop::Interrupted));
    assert_eq!(f.idles, 4, "three waits, and the fourth is interrupted");
}

#[test]
fn an_interrupted_run_that_was_restored_goes_back_to_pending_with_the_attempt_counted() {
    let (_d, q) = queue();
    let [a, _, _] = three(&q);
    let mut f = Fake {
        ends: [RunEnd::Interrupted {
            run_id: Some("r".into()),
            restored: true,
        }]
        .into(),
        ..Fake::default()
    };
    let (n, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, stop.code()), (1, 130));
    assert!(
        q.ids(State::Claimed).unwrap().is_empty(),
        "nothing stranded"
    );
    let t = read_task(&q.path(State::Pending, &a)).unwrap();
    assert_eq!(t.attempts, 1);
    assert_eq!(
        q.claim_next().unwrap().unwrap().id,
        a,
        "still first in line"
    );
}

#[test]
fn an_interrupted_run_that_was_not_restored_is_failed_not_retried() {
    let (_d, q) = queue();
    let [a, _, _] = three(&q);
    let mut f = Fake {
        ends: [RunEnd::Interrupted {
            run_id: None,
            restored: false,
        }]
        .into(),
        ..Fake::default()
    };
    let (_, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!(stop, Stop::Interrupted);
    let t = read_task(&q.path(State::Failed, &a)).unwrap();
    assert!(t.outcome.unwrap().exit.contains("NOT restored"));
}

#[test]
fn a_tree_not_restored_stops_work_with_code_two() {
    let (_d, q) = queue();
    three(&q);
    let mut f = Fake {
        ends: [RunEnd::NotRestored { run_id: None }].into(),
        ..Fake::default()
    };
    let (n, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, stop.code()), (1, 2));
}

#[test]
fn a_run_error_fails_the_task_and_stops() {
    let (_d, q) = queue();
    let [a, _, _] = three(&q);
    let mut f = Fake {
        ends: [RunEnd::Error {
            run_id: None,
            error: "no prompt `nope`".into(),
        }]
        .into(),
        ..Fake::default()
    };
    let (n, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, stop), (1, Stop::Error { id: a.clone() }));
    let t = read_task(&q.path(State::Failed, &a)).unwrap();
    let o = t.outcome.unwrap();
    assert_eq!(o.run_id, None);
    assert!(o.exit.contains("no prompt `nope`"));
}

#[test]
fn a_stop_between_runs_leaves_nothing_claimed() {
    let (_d, q) = queue();
    three(&q);
    let mut f = Fake {
        stop_after_runs: Some(1),
        ..Fake::default()
    };
    let (n, stop, _) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!((n, stop), (1, Stop::Interrupted));
    assert!(q.ids(State::Claimed).unwrap().is_empty());
    assert_eq!(q.ids(State::Pending).unwrap().len(), 2);
}

#[test]
fn a_stop_seen_right_after_a_claim_puts_the_task_back_untouched() {
    // Ctrl-C landing between the claim and the run's own signal handler.
    // Counted calls: the check before the claim says no, the one after yes.
    struct StopOnClaim(std::cell::Cell<u32>);
    impl Runner for StopOnClaim {
        fn blocker(&mut self) -> Option<String> {
            None
        }
        fn run(&mut self, _: &Task) -> RunEnd {
            panic!("must not start a run after a stop was requested")
        }
        fn stop_requested(&self) -> bool {
            self.0.set(self.0.get() + 1);
            self.0.get() > 1
        }
        fn idle(&mut self, _: Duration) -> bool {
            false
        }
    }
    let (_d, q) = queue();
    let a = q.add("x", None, None).unwrap();
    let mut r = StopOnClaim(std::cell::Cell::new(0));
    let (n, stop, _) = drain(&q, &mut r, WorkOptions::default());
    assert_eq!((n, stop), (0, Stop::Interrupted));
    assert_eq!(r.0.get(), 2, "the post-claim check is the one that fired");
    let t = read_task(&q.path(State::Pending, &a)).unwrap();
    assert_eq!(t.attempts, 0);
    assert_eq!(t.claimed_by_pid, None);
}

#[test]
fn a_stale_claim_is_named_and_not_re_run() {
    // A task in claimed/ may belong to a live process in another terminal;
    // the heartbeat cannot tell, so it neither runs it nor stays silent.
    let (_d, q) = queue();
    let stale = q.add("stale", None, None).unwrap();
    std::mem::forget(q.claim_next().unwrap().unwrap());
    q.add("fresh", None, None).unwrap();
    let mut f = Fake::default();
    let (_, _, out) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!(f.ran, vec!["fresh"]);
    assert!(
        out.contains(&stale) && out.contains("trellis queue requeue"),
        "{out}"
    );
    assert_eq!(q.ids(State::Claimed).unwrap(), vec![stale]);
}

#[test]
fn an_unreadable_pending_file_is_moved_aside_and_the_queue_goes_on() {
    let (_d, q) = queue();
    q.add("real", None, None).unwrap();
    std::fs::write(q.dir().join("pending/00000000-bad.json"), "{").unwrap();
    let mut f = Fake::default();
    let (n, _, out) = drain(&q, &mut f, WorkOptions::default());
    assert_eq!(n, 1);
    assert_eq!(f.ran, vec!["real"]);
    assert!(out.contains("is not a task file"), "{out}");
    assert!(
        q.ids(State::Failed)
            .unwrap()
            .contains(&"00000000-bad".to_string())
    );
}
