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

// ---------------------------------------------------------------------------
// The task model (ADR 0026 §1–§3)
// ---------------------------------------------------------------------------

fn add(q: &Queue, text: &str, f: impl FnOnce(&mut NewTask)) -> String {
    let mut n = NewTask {
        task: text.into(),
        ..NewTask::default()
    };
    f(&mut n);
    q.add_task(n, Actor::Human).unwrap()
}

fn status(q: &Queue, id: &str) -> Status {
    let (s, t) = q.find(id).unwrap();
    t.status_in(s)
}

fn set_status(q: &Queue, id: &str, st: Status) {
    q.edit(id, |t, _| {
        t.status = Some(st);
        t.blocked = None;
        Ok(Some(st.home()))
    })
    .unwrap();
}

#[test]
fn a_task_file_written_before_adr_0026_still_loads_with_the_directory_status() {
    // Existing queues must survive the upgrade: every new field defaults,
    // and a file in done/ reads as done, not in_review, so nothing that
    // names it as a blocker waits forever.
    let (_d, q) = queue();
    let old = r#"{"task":"old","focus":null,"prompt":null,"added_at":"2026-09-20T00:00:00Z","attempts":1,
                  "outcome":{"run_id":null,"exit":"converged","finished_at":"2026-09-20T00:01:00Z"}}"#;
    std::fs::create_dir_all(q.dir().join("done")).unwrap();
    std::fs::create_dir_all(q.dir().join("pending")).unwrap();
    std::fs::write(q.dir().join("done/20260920-000000-000000000-old.json"), old).unwrap();
    std::fs::write(
        q.dir().join("pending/20260920-000000-000000001-p.json"),
        r#"{"task":"p","focus":null,"prompt":null,"added_at":"x","attempts":0}"#,
    )
    .unwrap();
    assert_eq!(status(&q, "20260920-000000-000000000-old"), Status::Done);
    let (_, t) = q.find("20260920-000000-000000001-p").unwrap();
    assert_eq!(
        (t.status_in(State::Pending), t.priority),
        (Status::Todo, Priority::Medium)
    );
    let b = add(&q, "after", |n| {
        n.blockers = vec!["20260920-000000-000000000-old".into()]
    });
    assert_eq!(
        status(&q, &b),
        Status::Todo,
        "an old done task resolves a blocker"
    );
}

#[test]
fn a_new_file_omits_every_field_left_at_its_default() {
    // A plain task written now reads, by hand, like one written under
    // ADR 0020 plus its status — not a wall of empty arrays.
    let (_d, q) = queue();
    let id = q.add("x", None, None).unwrap();
    let text = std::fs::read_to_string(q.path(State::Pending, &id)).unwrap();
    for absent in [
        "labels",
        "blockers",
        "notes",
        "priority",
        "needs_approval",
        "assignee",
    ] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }
    assert!(text.contains("\"status\": \"todo\""), "{text}");
}

#[test]
fn claims_come_by_priority_then_age() {
    let (_d, q) = queue();
    let low = add(&q, "low", |n| n.priority = Priority::Low);
    let m1 = add(&q, "m1", |_| {});
    let crit = add(&q, "crit", |n| n.priority = Priority::Critical);
    let m2 = add(&q, "m2", |_| {});
    let got: Vec<String> = (0..4)
        .map(|_| q.claim_next().unwrap().unwrap().id)
        .collect();
    assert_eq!(got, vec![crit, m1, m2, low]);
}

#[test]
fn a_blocked_task_waits_for_done_and_a_cancelled_blocker_does_not_release_it() {
    // Paperclip execution-semantics §6: cancelled is terminal for the
    // blocker but does not satisfy the dependency.
    let (_d, q) = queue();
    let a = add(&q, "a", |n| n.backlog = true);
    let c = add(&q, "c", |n| n.backlog = true);
    let b = add(&q, "b", |n| n.blockers = vec![a.clone(), c.clone()]);
    assert_eq!(status(&q, &b), Status::Blocked);
    assert!(
        q.claim_next().unwrap().is_none(),
        "backlog and blocked are not claimable"
    );

    set_status(&q, &a, Status::Done);
    set_status(&q, &c, Status::Cancelled);
    assert!(q.reconcile(Actor::Work).unwrap().is_empty());
    assert!(q.claim_next().unwrap().is_none());

    q.edit(&b, |t, _| {
        t.blockers.retain(|x| x != &c);
        Ok(None)
    })
    .unwrap();
    let woken = q.reconcile(Actor::Work).unwrap();
    assert_eq!(woken.len(), 1);
    assert_eq!(q.claim_next().unwrap().unwrap().id, b);
}

#[test]
fn in_review_does_not_resolve_a_blocker_until_a_person_marks_it_done() {
    // A converged run's change is uncommitted. Letting its dependents run
    // on top of an unreviewed change is the queue making the human's
    // decision for them.
    let (_d, q) = queue();
    let a = q.add("a", None, None).unwrap();
    let b = add(&q, "b", |n| n.blockers = vec![a.clone()]);
    let mut f = Fake {
        ends: [RunEnd::Converged { run_id: None }].into(),
        ..Fake::default()
    };
    drain(&q, &mut f, WorkOptions::default());
    assert_eq!(status(&q, &a), Status::InReview);
    assert!(q.reconcile(Actor::Work).unwrap().is_empty());
    set_status(&q, &a, Status::Done);
    assert_eq!(q.reconcile(Actor::Work).unwrap()[0].0, b);
}

#[test]
fn a_waiting_parent_wakes_when_every_child_is_terminal_failed_included() {
    let (_d, q) = queue();
    let p = add(&q, "parent", |_| {});
    let c1 = add(&q, "c1", |n| n.parent = Some(p.clone()));
    let c2 = add(&q, "c2", |n| n.parent = Some(p.clone()));
    q.edit(&p, |t, _| {
        t.status = Some(Status::Blocked);
        t.blocked = Some(Blocked::Children);
        Ok(None)
    })
    .unwrap();
    // Structure alone is not dependency: children run, the parent waits.
    let mut f = Fake {
        ends: [RunEnd::RolledBack { run_id: None }].into(),
        ..Fake::default()
    };
    drain(
        &q,
        &mut f,
        WorkOptions {
            max_tasks: Some(1),
            every: None,
        },
    );
    assert_eq!(status(&q, &c1), Status::Failed);
    assert!(
        q.reconcile(Actor::Work).unwrap().is_empty(),
        "c2 is still open"
    );
    set_status(&q, &c2, Status::Done);
    let w = q.reconcile(Actor::Work).unwrap();
    assert_eq!(
        w,
        vec![(p.clone(), "every sub-task is finished".to_string())]
    );
    let (_, t) = q.find(&p).unwrap();
    assert!(t.notes.last().unwrap().text.contains("woken"));
}

#[test]
fn a_task_needing_approval_is_skipped_until_approved_and_rejection_cancels() {
    let (_d, q) = queue();
    let a = add(&q, "gated", |n| n.needs_approval = true);
    let b = add(&q, "gated2", |n| n.needs_approval = true);
    let (c, waiting) = q.claim_next_where(&mut |_, _| None).unwrap();
    assert!(c.is_none());
    assert!(
        waiting.iter().all(|(_, why)| why.contains("approve")),
        "{waiting:?}"
    );
    approval::approve_task(&q, &a, Actor::Human).unwrap();
    assert_eq!(q.claim_next().unwrap().unwrap().id, a);
    approval::reject_task(&q, &b, Some("out of scope"), Actor::Human).unwrap();
    assert_eq!(status(&q, &b), Status::Cancelled);
    let log = q.activity().tail(10).unwrap();
    assert!(log.iter().any(|e| e.action == "approval.approve"));
    assert!(
        log.iter()
            .any(|e| e.action == "approval.reject" && e.detail.as_deref() == Some("out of scope"))
    );
}

#[test]
fn a_claimed_task_refuses_edits_rather_than_losing_them() {
    // The running process publishes its own copy on finish; an edit made
    // meanwhile would vanish without a word.
    let (_d, q) = queue();
    let a = q.add("x", None, None).unwrap();
    let c = q.claim_next().unwrap().unwrap();
    let e = q
        .edit(&a, |t, _| {
            t.note(Actor::Human, "hi");
            Ok(None)
        })
        .unwrap_err();
    assert!(matches!(e, QueueError::Claimed { .. }), "{e}");
    c.release(false).unwrap();
    q.edit(&a, |t, _| {
        t.note(Actor::Human, "hi");
        Ok(None)
    })
    .unwrap();
    assert_eq!(q.find(&a).unwrap().1.notes.len(), 1);
}

#[test]
fn a_failed_edit_puts_the_pending_task_back_unchanged() {
    let (_d, q) = queue();
    let a = q.add("x", None, None).unwrap();
    let before = std::fs::read_to_string(q.path(State::Pending, &a)).unwrap();
    let e = q.edit(&a, |t, _| {
        t.task = "changed".into();
        Err(QueueError::Invalid("no".into()))
    });
    assert!(e.is_err());
    assert_eq!(
        std::fs::read_to_string(q.path(State::Pending, &a)).unwrap(),
        before
    );
    assert!(q.ids(State::Claimed).unwrap().is_empty());
}

#[test]
fn a_finished_task_is_edited_under_a_lock_and_a_stale_lock_is_named() {
    let (_d, q) = queue();
    let a = q.add("x", None, None).unwrap();
    q.claim_next()
        .unwrap()
        .unwrap()
        .finish(
            State::Failed,
            Outcome {
                run_id: None,
                exit: "x".into(),
                finished_at: iso_now(),
            },
        )
        .unwrap();
    q.edit(&a, |t, _| {
        t.labels.push("flaky".into());
        Ok(None)
    })
    .unwrap();
    assert_eq!(q.find(&a).unwrap().1.labels, vec!["flaky"]);
    // A lock left by a crashed process: reported with its path, not waited
    // on forever and not broken silently.
    std::fs::create_dir_all(q.trellis_dir().join("locks")).unwrap();
    std::fs::write(q.trellis_dir().join(format!("locks/task-{a}.lock")), "{}").unwrap();
    let e = q.edit(&a, |_, _| Ok(None)).unwrap_err();
    assert!(matches!(e, QueueError::Locked(_)), "{e}");
    assert!(e.to_string().contains("remove the file"), "{e}");
}

#[test]
fn add_refuses_a_parent_or_blocker_that_does_not_exist() {
    let (_d, q) = queue();
    let e = q
        .add_task(
            NewTask {
                task: "x".into(),
                blockers: vec!["nope".into()],
                ..NewTask::default()
            },
            Actor::Human,
        )
        .unwrap_err();
    assert!(matches!(e, QueueError::Unknown(ref id) if id == "nope"));
    assert!(q.ids(State::Pending).unwrap().is_empty());
}

#[test]
fn a_gated_assignee_is_skipped_and_work_says_what_it_waits_for() {
    struct Gated(Fake);
    impl Runner for Gated {
        fn blocker(&mut self) -> Option<String> {
            None
        }
        fn run(&mut self, t: &Task) -> RunEnd {
            self.0.run(t)
        }
        fn stop_requested(&self) -> bool {
            false
        }
        fn idle(&mut self, _: Duration) -> bool {
            false
        }
        fn gate(&mut self, _: &str, t: &Task, _: &mut dyn Write) -> Option<String> {
            (t.assignee.as_deref() == Some("paused")).then(|| "agent `paused` is paused".into())
        }
    }
    let (_d, q) = queue();
    let skipped = add(&q, "theirs", |n| {
        n.assignee = Some("paused".into());
        n.priority = Priority::Critical;
    });
    add(&q, "mine", |_| {});
    let mut g = Gated(Fake::default());
    let (n, stop, out) = drain(&q, &mut g, WorkOptions::default());
    assert_eq!((n, stop), (1, Stop::Waiting(1)));
    assert_eq!(g.0.ran, vec!["mine"]);
    assert!(out.contains(&skipped) && out.contains("is paused"), "{out}");
    assert_eq!(q.ids(State::Pending).unwrap(), vec![skipped]);
}

#[test]
fn every_mutation_the_heartbeat_makes_is_in_the_activity_log() {
    let (_d, q) = queue();
    let a = q.add("x", None, None).unwrap();
    let mut f = Fake {
        ends: [RunEnd::RolledBack {
            run_id: Some("r1".into()),
        }]
        .into(),
        ..Fake::default()
    };
    drain(&q, &mut f, WorkOptions::default());
    let log = q.activity().tail(100).unwrap();
    let acts: Vec<(&str, Actor)> = log.iter().map(|e| (e.action.as_str(), e.actor)).collect();
    assert_eq!(
        acts,
        vec![
            ("task.add", Actor::Human),
            ("task.claim", Actor::Work),
            ("task.finish", Actor::Work)
        ]
    );
    assert_eq!(log[2].run_id.as_deref(), Some("r1"));
    assert_eq!(log[2].entity, format!("task:{a}"));
    assert!(q.activity().path().starts_with(q.trellis_dir()));
}

#[test]
fn a_torn_last_line_does_not_hide_the_log() {
    let (_d, q) = queue();
    q.add("x", None, None).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(q.activity().path())
        .unwrap();
    f.write_all(b"{\"at\":\"2026").unwrap();
    assert_eq!(q.activity().tail(10).unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Budgets (ADR 0026 §5)
// ---------------------------------------------------------------------------

fn budgeted(monthly: u64) -> profile::Profile {
    profile::parse(
        "rev",
        &format!("[budget]\nmonthly_tokens = {monthly}\nwarn_at = 0.5\n"),
    )
    .unwrap()
}

fn spent(month: u64) -> ledger::Spend {
    ledger::Spend {
        month,
        lifetime: month,
        ..ledger::Spend::default()
    }
}

#[test]
fn the_hard_stop_pauses_once_and_opens_one_request() {
    // The risk: a stop that re-fires every tick opening a new request each
    // time, or a pause that a later tick quietly lifts.
    let (_d, q) = queue();
    let p = budgeted(100);
    let w = ledger::enforce(&q, &p, &spent(60), "2026-09", Actor::Work).unwrap();
    assert!(
        !w.paused
            && w.message
                .unwrap()
                .contains("60 of its monthly limit of 100")
    );
    let s1 = ledger::enforce(&q, &p, &spent(100), "2026-09", Actor::Work).unwrap();
    let s2 = ledger::enforce(&q, &p, &spent(120), "2026-09", Actor::Work).unwrap();
    assert!(s1.paused && s2.paused);
    assert_eq!(s1.request, s2.request);
    assert_eq!(approval::list(&q, "pending").unwrap().len(), 1);
    let o = profile::overlay(&q, "rev").unwrap();
    assert!(profile::paused_reason(&p, &o).unwrap().contains("budget"));
}

#[test]
fn approving_with_a_raise_resumes_and_rejecting_keeps_the_pause() {
    let (_d, q) = queue();
    let p = budgeted(100);
    let req = ledger::enforce(&q, &p, &spent(100), "2026-09", Actor::Work)
        .unwrap()
        .request
        .unwrap();
    assert!(approval::approve_budget(&q, &req, 0, Actor::Human).is_err());
    approval::approve_budget(&q, &req, 50, Actor::Human).unwrap();
    let o = profile::overlay(&q, "rev").unwrap();
    assert!(profile::paused_reason(&p, &o).is_none());
    let again = ledger::enforce(&q, &p, &spent(120), "2026-09", Actor::Work).unwrap();
    assert!(!again.paused, "120 < 150 after the raise");

    let req2 = ledger::enforce(&q, &p, &spent(150), "2026-09", Actor::Work)
        .unwrap()
        .request
        .unwrap();
    assert_ne!(req, req2);
    approval::reject_budget(&q, &req2, None, Actor::Human).unwrap();
    let o = profile::overlay(&q, "rev").unwrap();
    assert!(profile::paused_reason(&p, &o).is_some());
}

#[test]
fn a_monthly_budget_pause_lifts_when_the_month_rolls_over_and_a_human_pause_never_does() {
    let (_d, q) = queue();
    let p = budgeted(100);
    ledger::enforce(&q, &p, &spent(100), "2026-09", Actor::Work).unwrap();
    let r = ledger::enforce(&q, &p, &spent(0), "2026-10", Actor::Work).unwrap();
    assert!(r.resumed && !r.paused);
    assert!(profile::overlay(&q, "rev").unwrap().paused.is_none());

    profile::update_overlay(&q, "rev", |o| {
        o.paused = Some(profile::Pause {
            by: profile::PausedBy::Human,
            at: iso_now(),
            reason: None,
            window: None,
            month: None,
        })
    })
    .unwrap();
    let r = ledger::enforce(&q, &p, &spent(0), "2026-11", Actor::Work).unwrap();
    assert!(!r.resumed);
    assert!(profile::overlay(&q, "rev").unwrap().paused.is_some());
}
