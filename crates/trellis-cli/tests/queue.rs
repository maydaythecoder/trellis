//! `trellis queue` and `trellis work`, driven through the real binary with a
//! scripted model (ADR 0020).
//!
//! The heartbeat's decisions are unit-tested in `trellis-queue` against a
//! fake runner. These tests check the half that only exists in the binary:
//! that a queued task really goes through `trellis run`'s path, that the
//! queue never dirties the worktree a run needs clean, and that what the
//! task file records matches the trail the run wrote.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-queue-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn trellis(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        // The JSONL test depends on the default filter; an inherited
        // RUST_LOG would change what is logged.
        .env_remove("RUST_LOG")
        .output()
        .expect("the binary must be built before its integration tests run")
}

fn combined(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A repository whose checks always pass (`git --version`), and two
/// scripts: `converge.json` edits and converges, `stop.json` stops at once
/// and so does not converge and is rolled back.
fn fixture(name: &str) -> PathBuf {
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet():\n    return \"hello\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
    )
    .unwrap();
    std::fs::write(
        d.join("converge.json"),
        concat!(
            r#"{"steps":[{"tool":"edit","path":"app/core.py","#,
            r#""old":"return \"hello\"","new":"return \"hi\""},"#,
            r#"{"tool":"run_checks","paths":["app/core.py"],"kinds":["typecheck"]},"#,
            r#"{"tool":"done","summary":"changed"}]}"#,
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("stop.json"),
        r#"{"steps":[{"tool":"done","summary":"nothing to do"}]}"#,
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

fn model(d: &Path, script: &str) -> String {
    format!("scripted:{}", d.join(script).display())
}

fn queue_dir(d: &Path, state: &str) -> PathBuf {
    d.join(".git/trellis/queue").join(state)
}

fn ids(d: &Path, state: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(queue_dir(d, state))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name()
                        .to_str()?
                        .strip_suffix(".json")
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn task_json(d: &Path, state: &str, id: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(queue_dir(d, state).join(format!("{id}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn add(d: &Path, args: &[&str]) {
    let mut a = vec!["queue", "add"];
    a.extend_from_slice(args);
    let o = trellis(d, &a);
    assert!(o.status.success(), "{}", combined(&o));
}

fn porcelain(d: &Path) -> String {
    git(d, &["status", "--porcelain"])
}

#[test]
fn queue_add_and_list_leave_the_worktree_clean() {
    let d = fixture("add-list");
    add(&d, &["rename greet"]);
    add(&d, &["second task"]);
    assert_eq!(porcelain(&d), "", "the queue must never dirty the worktree");
    assert_eq!(ids(&d, "pending").len(), 2);

    let o = trellis(&d, &["queue", "list"]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    let first = s.find("rename greet").unwrap();
    let second = s.find("second task").unwrap();
    assert!(first < second, "oldest first:\n{s}");
    assert!(s.contains("pending"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_prompt_that_does_not_exist_is_refused_when_queued_not_hours_later() {
    let d = fixture("add-bad-prompt");
    let o = trellis(&d, &["queue", "add", "--prompt", "nope", "x"]);
    assert!(!o.status.success());
    assert!(
        combined(&o).contains("no prompt `nope`"),
        "{}",
        combined(&o)
    );
    assert!(ids(&d, "pending").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_converged_run_stops_work_and_the_task_records_the_trail_it_wrote() {
    let d = fixture("converged");
    add(&d, &["change the greeting"]);
    add(&d, &["a second task"]);
    let o = trellis(&d, &["work", "--model", &model(&d, "converge.json")]);
    let s = combined(&o);
    assert!(
        o.status.success(),
        "a stop that is the design is exit 0:\n{s}"
    );
    assert!(s.contains("CONVERGED"), "{s}");
    assert!(
        s.contains(
            "converged change awaits review; accept or revert it, then run `trellis work` again"
        ),
        "{s}"
    );

    let done = ids(&d, "done");
    assert_eq!(done.len(), 1, "{s}");
    assert_eq!(ids(&d, "pending").len(), 1, "the second task waits");
    assert!(ids(&d, "claimed").is_empty());

    let t = task_json(&d, "done", &done[0]);
    let run_id = t["outcome"]["run_id"]
        .as_str()
        .expect("a converged run wrote a trail");
    assert!(
        d.join(".trellis/runs")
            .join(format!("{run_id}.json"))
            .exists(),
        "the recorded run id names the trail on disk"
    );
    assert_eq!(t["attempts"], 1);
    assert!(t["outcome"]["exit"].as_str().unwrap().contains("converged"));

    // The change is left for the human, uncommitted.
    assert!(porcelain(&d).contains("app/core.py"));
    // And the next `work` refuses rather than claiming the second task.
    let o = trellis(&d, &["work", "--model", &model(&d, "converge.json")]);
    let s = combined(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains("not claiming a task"), "{s}");
    assert_eq!(
        ids(&d, "pending").len(),
        1,
        "never claim a task that cannot start"
    );
    assert!(ids(&d, "claimed").is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rolled_back_runs_continue_to_the_next_task_and_the_queue_survives_them() {
    // Rollback is `reset --hard` plus `clean -fd`. A queue in the worktree
    // would be deleted by the first rollback; this one is in the git dir.
    let d = fixture("rolled-back");
    add(&d, &["one"]);
    add(&d, &["two"]);
    add(&d, &["three"]);
    let o = trellis(&d, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert_eq!(s.matches("NOT CONVERGED").count(), 3, "{s}");
    assert!(s.contains("queue empty"), "{s}");
    let failed = ids(&d, "failed");
    assert_eq!(failed.len(), 3);
    assert!(failed[0].ends_with("-one") && failed[2].ends_with("-three"));
    for id in &failed {
        let t = task_json(&d, "failed", id);
        assert!(t["outcome"]["run_id"].is_string(), "{t}");
        assert!(t["outcome"]["exit"].as_str().unwrap().contains("restored"));
    }
    assert_eq!(porcelain(&d), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_dirty_tree_refuses_to_claim() {
    let d = fixture("dirty");
    add(&d, &["one"]);
    std::fs::write(d.join("stray.txt"), "x").unwrap();
    let o = trellis(&d, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains("stray.txt"), "names what is dirty:\n{s}");
    assert_eq!(ids(&d, "pending").len(), 1);
    assert!(ids(&d, "claimed").is_empty());
    assert!(!s.contains("snapshot at"), "no run started:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn max_tasks_and_once_bound_one_invocation() {
    let d = fixture("max-tasks");
    for t in ["one", "two", "three", "four"] {
        add(&d, &[t]);
    }
    let o = trellis(
        &d,
        &[
            "work",
            "--max-tasks",
            "2",
            "--model",
            &model(&d, "stop.json"),
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    assert_eq!((ids(&d, "failed").len(), ids(&d, "pending").len()), (2, 2));

    let o = trellis(&d, &["work", "--once", "--model", &model(&d, "stop.json")]);
    assert!(o.status.success(), "{}", combined(&o));
    assert_eq!((ids(&d, "failed").len(), ids(&d, "pending").len()), (3, 1));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_stale_claim_is_listed_with_its_age_not_re_run_and_can_be_requeued() {
    let d = fixture("stale");
    add(&d, &["left behind"]);
    let id = ids(&d, "pending").remove(0);
    // What a crashed `work` leaves: the file in claimed/.
    std::fs::create_dir_all(queue_dir(&d, "claimed")).unwrap();
    std::fs::rename(
        queue_dir(&d, "pending").join(format!("{id}.json")),
        queue_dir(&d, "claimed").join(format!("{id}.json")),
    )
    .unwrap();

    let s = combined(&trellis(&d, &["queue", "list"]));
    assert!(s.contains("claimed") && s.contains(" ago"), "{s}");

    let o = trellis(&d, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(
        s.contains(&id) && s.contains("trellis queue requeue"),
        "{s}"
    );
    assert!(
        !s.contains("snapshot at"),
        "the stale task was not run:\n{s}"
    );
    assert_eq!(ids(&d, "claimed"), vec![id.clone()]);

    let o = trellis(&d, &["queue", "requeue", &id]);
    assert!(o.status.success(), "{}", combined(&o));
    assert_eq!(ids(&d, "pending"), vec![id]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_queued_focus_is_the_goal_ancestry_the_run_is_shown() {
    // ADR 0020 §6: nothing extra is built. The task's focus goes to the
    // same vault compile `run --focus` uses.
    let d = fixture("focus");
    let v = d.join("notes");
    std::fs::create_dir_all(&v).unwrap();
    std::fs::write(
        v.join("root.md"),
        "---\ntitle: \"Root\"\nparent: null\n---\nROOT-BODY\n",
    )
    .unwrap();
    std::fs::write(
        v.join("child.md"),
        "---\ntitle: \"Child\"\nparent: \"[[root]]\"\n---\nCHILD-BODY\n",
    )
    .unwrap();
    std::fs::write(v.join("manifest.json"), r#"["child.md","root.md"]"#).unwrap();
    let cfg = std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("{cfg}[context]\nvault = \"notes\"\n"),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "vault"]);

    let bad = trellis(&d, &["queue", "add", "--focus", "nope", "x"]);
    assert!(
        !bad.status.success(),
        "a focus naming no note is refused at add"
    );

    add(&d, &["--focus", "child", "work under the child goal"]);
    let o = trellis(&d, &["work", "--once", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(
        s.contains("vault note `child` from notes loaded ("),
        "the queued focus reached the run:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_json_writes_the_per_turn_events_as_jsonl() {
    let d = fixture("log-json");
    // Outside the repository: a log inside it would dirty the worktree.
    let log = scratch("log-json-out").join("events.jsonl");
    add(&d, &["change the greeting"]);
    let o = trellis(
        &d,
        &[
            "work",
            "--once",
            "--log-json",
            log.to_str().unwrap(),
            "--model",
            &model(&d, "converge.json"),
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    let text = std::fs::read_to_string(&log).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("every line is one JSON object"))
        .collect();
    let turns: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["fields"]["message"] == "turn")
        .collect();
    assert!(turns.len() >= 2, "edit and run_checks are turns:\n{text}");
    assert!(
        turns.iter().any(|e| e["fields"]["call"]
            .as_str()
            .is_some_and(|c| c.contains("edit"))),
        "{text}"
    );

    // The same flag on `run`, appending to the same file.
    git(&d, &["checkout", "--", "app/core.py"]);
    let before = text.lines().count();
    let o = trellis(
        &d,
        &[
            "run",
            "--log-json",
            log.to_str().unwrap(),
            "--model",
            &model(&d, "converge.json"),
            "again",
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    let after = std::fs::read_to_string(&log).unwrap().lines().count();
    assert!(after > before, "appended, not truncated");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}
