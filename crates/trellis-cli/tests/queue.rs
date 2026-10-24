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
    let out = trellis_vcs::command()
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
    trellis_env(root, args, &[])
}

/// `trellis` with `env` added. The run markers `queue note` refuses on are
/// removed first, so a suite started from inside a trellis run behaves as
/// one started from a shell, and only a test that sets one sees one.
fn trellis_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_trellis"));
    cmd.env_remove("TRELLIS_RUN_ID")
        .env_remove("TRELLIS_TASK_ID");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        // Hermetic: the user-level config and context files (ADR 0025 §7)
        // of whoever runs the suite must not change what it sees.
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
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
fn task_worktree_two_converging_tasks_each_leave_their_change_in_a_kept_worktree() {
    // ADR 0053: one `work` runs both tasks, each converged change waits
    // uncommitted in its own worktree on `trellis/task/<id>`, and the
    // worktree `work` started in keeps its tree and HEAD. Before, `work`
    // stopped at the first converged change (ADR 0020 §5).
    let d = fixture("converged");
    add(&d, &["change the greeting"]);
    add(&d, &["a second task"]);
    let head = git(&d, &["rev-parse", "HEAD"]);
    let o = trellis(&d, &["work", "--model", &model(&d, "converge.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert_eq!(s.matches("CONVERGED").count(), 2, "{s}");
    assert!(s.contains("queue empty"), "went on past the first:\n{s}");

    let done = ids(&d, "done");
    assert_eq!(done.len(), 2, "{s}");
    assert!(ids(&d, "pending").is_empty() && ids(&d, "claimed").is_empty());
    for id in &done {
        let t = task_json(&d, "done", id);
        let branch = format!("trellis/task/{id}");
        assert_eq!(t["outcome"]["branch"], branch.as_str(), "{t}");
        let wt = PathBuf::from(t["outcome"]["worktree"].as_str().expect("recorded"));
        assert_eq!(
            std::fs::canonicalize(&wt).unwrap(),
            std::fs::canonicalize(d.join(".trellis/work/task").join(id)).unwrap()
        );
        assert_eq!(
            git(&wt, &["branch", "--show-current"]).trim(),
            branch.as_str()
        );
        // The trail is committed on the task branch, as a run commits it
        // wherever it runs; the change is not.
        assert_eq!(
            git(&wt, &["rev-parse", "HEAD^"]),
            head,
            "one commit on the base"
        );
        let run_id = t["outcome"]["run_id"].as_str().expect("a trail");
        assert_eq!(
            git(&wt, &["show", "--name-only", "--format=", "HEAD"]).trim(),
            format!(".trellis/runs/{run_id}.json"),
            "only the trail is committed"
        );
        assert_eq!(
            porcelain(&wt).trim(),
            "M app/core.py",
            "the change waits there, uncommitted"
        );
        assert_eq!(t["attempts"], 1);
    }

    // The starting worktree: tree and HEAD as they were.
    assert_eq!(porcelain(&d), "");
    assert_eq!(git(&d, &["rev-parse", "HEAD"]), head);
    assert!(
        std::fs::read_to_string(d.join("app/core.py"))
            .unwrap()
            .contains("hello")
    );

    // `queue list` says where each change is.
    let l = combined(&trellis(&d, &["queue", "list"]));
    for id in &done {
        assert!(l.contains(&format!("on trellis/task/{id}")), "{l}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn task_worktree_of_a_rolled_back_task_is_removed_and_its_branch_keeps_the_trail() {
    // ADR 0053 decision 4: the trail `record_trail` commits on the task
    // branch is the record of what the run did (ADR 0007); deleting the
    // branch with the worktree would delete it.
    let d = fixture("rolled-back-branch");
    add(&d, &["one"]);
    let head = git(&d, &["rev-parse", "HEAD"]);
    let o = trellis(&d, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    let id = &ids(&d, "failed")[0];
    let t = task_json(&d, "failed", id);
    assert!(t["outcome"]["worktree"].is_null(), "removed: {t}");
    let branch = format!("trellis/task/{id}");
    assert_eq!(t["outcome"]["branch"], branch.as_str(), "{t}");
    assert!(!d.join(".trellis/work/task").join(id).exists());
    let run_id = t["outcome"]["run_id"].as_str().unwrap();
    git(
        &d,
        &[
            "cat-file",
            "-e",
            &format!("{branch}:.trellis/runs/{run_id}.json"),
        ],
    );
    assert_eq!(
        git(&d, &["rev-parse", &format!("{branch}^")]),
        head,
        "one trail commit on the base"
    );
    assert_eq!(porcelain(&d), "");
    assert_eq!(git(&d, &["rev-parse", "HEAD"]), head);

    // Requeued, it is refused until a person clears the branch (decision
    // 2), and put back untouched.
    assert!(trellis(&d, &["queue", "requeue", id]).status.success());
    let before = std::fs::read(queue_dir(&d, "pending").join(format!("{id}.json"))).unwrap();
    let o = trellis(&d, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains(&format!("git branch -D {branch}")), "{s}");
    assert_eq!(
        std::fs::read(queue_dir(&d, "pending").join(format!("{id}.json"))).unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Runs `work` once against a task that `setup` arranges to be refused, and
/// checks the refusal of ADR 0053 decision 2: exit 1, the reason printed,
/// the task back in `pending/` byte for byte, nothing run, and nothing
/// left in `claimed/`.
fn refused_untouched(name: &str, setup: impl FnOnce(&Path, &str), why: &str) -> (PathBuf, String) {
    let d = fixture(name);
    add(&d, &["one"]);
    let id = ids(&d, "pending")[0].clone();
    setup(&d, &id);
    let id = ids(&d, "pending")[0].clone();
    let before = std::fs::read(queue_dir(&d, "pending").join(format!("{id}.json"))).unwrap();
    let o = trellis(&d, &["work", "--model", &model(&d, "converge.json")]);
    let s = combined(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains(why), "{s}");
    assert!(s.contains("attempts unchanged"), "{s}");
    assert!(!s.contains("snapshot at"), "no run started:\n{s}");
    assert_eq!(
        std::fs::read(queue_dir(&d, "pending").join(format!("{id}.json"))).unwrap(),
        before,
        "put back byte for byte"
    );
    assert!(ids(&d, "claimed").is_empty());
    assert!(ids(&d, "failed").is_empty() && ids(&d, "done").is_empty());
    assert_eq!(porcelain(&d), "");
    (d, id)
}

#[test]
fn task_worktree_over_an_existing_branch_is_refused_and_the_task_put_back() {
    // The branch may hold an earlier attempt's trail or a converged change.
    let (d, _) = refused_untouched(
        "refuse-branch",
        |d, id| {
            git(d, &["branch", &format!("trellis/task/{id}")]);
        },
        "already exists",
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn task_worktree_over_an_existing_directory_is_refused_and_the_task_put_back() {
    // A directory a person left there, or one a removal did not finish.
    let (d, _) = refused_untouched(
        "refuse-dir",
        |d, id| {
            let p = d.join(".trellis/work/task").join(id);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("notes.txt"), "mine\n").unwrap();
            std::fs::write(d.join(".trellis/.gitignore"), ".gitignore\nwork/\n").unwrap();
        },
        "already exists",
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn task_worktree_for_an_id_that_is_not_a_branch_name_is_refused_and_the_task_put_back() {
    // `queue add` never makes such an id; a file placed by hand might, and
    // `..` in it would climb out of `.trellis/work/task/`.
    let (d, _) = refused_untouched(
        "refuse-id",
        |d, id| {
            let dir = queue_dir(d, "pending");
            std::fs::rename(dir.join(format!("{id}.json")), dir.join("by..hand.json")).unwrap();
        },
        "does not make a valid branch name",
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn task_worktree_that_is_not_clean_after_checkout_is_refused_and_removed() {
    // A checkout that leaves the new worktree dirty (here a smudge filter
    // that writes a file beside what it checks out) would be snapshotted
    // dirty; it is refused, and nothing created for it is left behind.
    let (d, id) = refused_untouched(
        "refuse-dirty",
        |d, _| {
            std::fs::write(d.join(".gitattributes"), "*.py filter=mark\n").unwrap();
            git(d, &["add", ".gitattributes"]);
            git(d, &["commit", "-qm", "a filter"]);
            git(
                d,
                &[
                    "config",
                    "filter.mark.smudge",
                    "sh -c 'cat; : > smudged.txt'",
                ],
            );
            git(d, &["config", "filter.mark.clean", "cat"]);
        },
        "is not clean after checkout",
    );
    assert!(!d.join(".trellis/work/task").join(&id).exists(), "removed");
    assert!(
        git(&d, &["branch", "--list", "trellis/task/*"])
            .trim()
            .is_empty(),
        "and its branch"
    );
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
fn a_backward_clock_step_still_records_the_run_id_of_a_rolled_back_run() {
    // A trail's name is `<wall-clock stamp>-<slug>`. After the clock steps
    // back, the next run's trail sorts below the one written before the
    // step. Run one task, then rename its trail to a stamp in the future:
    // the same files on disk as a clock that was ahead and was pulled back
    // (#472), with no sleeping and no change to the real clock.
    let d = fixture("clock-step");
    add(&d, &["one"]);
    let o = trellis(&d, &["work", "--once", "--model", &model(&d, "stop.json")]);
    assert!(o.status.success(), "{}", combined(&o));
    // The run was in its own worktree, so its trail is on its task branch
    // (ADR 0053); it is brought into this worktree's HEAD under the later
    // stamp, which every later task's worktree is based on.
    let runs = d.join(".trellis/runs");
    let first = task_json(&d, "failed", &ids(&d, "failed")[0]);
    let branch = first["outcome"]["branch"].as_str().unwrap().to_string();
    let first = first["outcome"]["run_id"].as_str().unwrap().to_string();
    let ahead = format!("9{}", &first[1..]);
    let trail = git(
        &d,
        &["show", &format!("{branch}:.trellis/runs/{first}.json")],
    );
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(runs.join(format!("{ahead}.json")), trail).unwrap();
    git(&d, &["add", "-A"]);
    git(
        &d,
        &[
            "commit",
            "-qm",
            "a trail stamped before the clock stepped back",
        ],
    );

    add(&d, &["two"]);
    let o = trellis(&d, &["work", "--once", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    let two = ids(&d, "failed")
        .into_iter()
        .find(|i| i.ends_with("-two"))
        .unwrap();
    let t = task_json(&d, "failed", &two);
    assert!(t["outcome"]["exit"].as_str().unwrap().contains("restored"));
    let id = t["outcome"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{t}"));
    assert_ne!(id, ahead, "the id of the run before the step:\n{t}");
    assert!(id.ends_with("-two"), "{t}");
    let branch = t["outcome"]["branch"].as_str().unwrap();
    git(
        &d,
        &[
            "cat-file",
            "-e",
            &format!("{branch}:.trellis/runs/{id}.json"),
        ],
    );
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

fn activity(d: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(d.join(".git/trellis/activity.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn queue_note_in_run_is_refused_writes_nothing_and_is_logged() {
    // The risk: a `bash` call, hook or custom tool spawns `trellis queue
    // note`, the note is recorded as a person's, and its `@reviewer` wakes
    // another agent's task: an agent waking an agent (ADR 0005, #189).
    let d = fixture("note-in-run");
    add(&d, &["rename greet"]);
    let id = ids(&d, "pending").remove(0);
    let before = task_json(&d, "pending", &id);
    let logged = activity(&d).len();

    let o = trellis_env(
        &d,
        &["queue", "note", &id, "@reviewer please look"],
        &[("TRELLIS_RUN_ID", "run-189")],
    );
    let s = combined(&o);
    assert!(!o.status.success(), "{s}");
    assert!(s.contains("a run cannot write a person's note"), "{s}");
    assert!(s.contains("TRELLIS_RUN_ID"), "{s}");

    assert_eq!(ids(&d, "pending"), vec![id.clone()]);
    assert_eq!(
        task_json(&d, "pending", &id),
        before,
        "a refused note must not touch the task"
    );
    let events = activity(&d);
    assert_eq!(events.len(), logged + 1, "{events:?}");
    let e = events.last().unwrap();
    assert_eq!(e["action"], "task.note_refused", "{e}");
    assert_eq!(e["entity"], format!("task:{id}"), "{e}");
    assert_eq!(e["run_id"], "run-189", "{e}");
    assert_eq!(porcelain(&d), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_note_in_run_of_a_process_agent_is_refused_too() {
    // The risk: a process agent is given `TRELLIS_TASK_ID`, not a run id,
    // so a check on `TRELLIS_RUN_ID` alone leaves its path open.
    let d = fixture("note-in-agent");
    add(&d, &["rename greet"]);
    let id = ids(&d, "pending").remove(0);
    let before = task_json(&d, "pending", &id);

    let o = trellis_env(
        &d,
        &["queue", "note", &id, "@reviewer please look"],
        &[("TRELLIS_TASK_ID", &id)],
    );
    let s = combined(&o);
    assert!(!o.status.success(), "{s}");
    assert!(s.contains("a run cannot write a person's note"), "{s}");
    assert_eq!(task_json(&d, "pending", &id), before);
    let e = activity(&d).pop().unwrap();
    assert_eq!(e["action"], "task.note_refused", "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_note_in_run_absent_writes_a_person_note_as_before() {
    // The risk: the check fires outside a run too, and a person can no
    // longer write the note that wakes a profile.
    let d = fixture("note-no-run");
    add(&d, &["rename greet"]);
    let id = ids(&d, "pending").remove(0);

    let o = trellis(&d, &["queue", "note", &id, "@reviewer please look"]);
    assert!(o.status.success(), "{}", combined(&o));
    let t = task_json(&d, "pending", &id);
    let notes = t["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{t}");
    assert_eq!(notes[0]["actor"], "human", "{t}");
    assert_eq!(notes[0]["id"], "n1", "{t}");
    assert_eq!(notes[0]["text"], "@reviewer please look", "{t}");
    let e = activity(&d).pop().unwrap();
    assert_eq!(e["action"], "task.note", "{e}");
    assert_eq!(porcelain(&d), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_model_message_is_why_work_says_a_queued_task_waits() {
    // #440, ADR 0050 section 2: `trellis run` and bare `trellis` share one
    // missing-model message, and `trellis work` gave a task's wait its own
    // older wording, which named neither the picker nor the user config
    // file. The risk is that drift: the queue telling the user something
    // different from every other command.
    let d = fixture("no-model-message");
    add(&d, &["rename greet"]);
    let id = ids(&d, "pending").remove(0);
    let config = std::env::temp_dir()
        .join("trellis-tests-no-user-config")
        .join("config.toml");

    let o = trellis(&d, &["work", "--once"]);
    let s = combined(&o);
    let block = waiting_block(&s, &id);
    // The shared message's parts: its opening, the picker, one complete
    // `--model` form, and the user's own config file.
    assert!(
        block.contains("no model configured. Any one of these sets one:"),
        "{s}"
    );
    assert!(block.contains("  trellis "), "names bare trellis:\n{s}");
    assert!(
        block.contains("--model llamacpp:http://127.0.0.1:8080"),
        "{s}"
    );
    assert!(block.contains(&config.display().to_string()), "{s}");
    assert_eq!(ids(&d, "pending"), vec![id], "the task still waits");
    let _ = std::fs::remove_dir_all(&d);
}

/// The waiting line `trellis work` prints for `id`, and the indented lines
/// under it up to the next line that is not part of it.
fn waiting_block(s: &str, id: &str) -> String {
    let mut lines = s.lines();
    let first = lines
        .by_ref()
        .find(|l| l.trim_start().starts_with(&format!("{id}: ")))
        .unwrap_or_else(|| panic!("a waiting line for {id}:\n{s}"));
    let mut block = vec![first];
    block.extend(lines.take_while(|l| l.starts_with("          ") && !l.trim().is_empty()));
    block.join("\n")
}

#[test]
fn no_model_message_prints_one_line_per_line_of_the_reason() {
    // #444: the waiting reason went through `plain()`, which escapes
    // control characters, so the multi-line no-model message printed as one
    // line with a literal `\n` between its parts. The risk is a reason that
    // reads differently, and worse, from `trellis work` than from `trellis run`.
    let d = fixture("no-model-lines");
    add(&d, &["rename greet"]);
    let id = ids(&d, "pending").remove(0);

    let o = trellis(&d, &["work", "--once"]);
    let s = combined(&o);
    let block = waiting_block(&s, &id);
    // An escaped newline before the message's two-space indent; a bare
    // `\n` could be part of a Windows path such as the config file's.
    assert!(
        !block.contains("\\n  "),
        "a literal \\n in the reason:\n{s}"
    );
    let lines: Vec<&str> = block.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "one printed line per line of the reason:\n{s}"
    );
    assert!(
        lines[0].ends_with(&format!(
            "{id}: no model configured. Any one of these sets one:"
        )),
        "{s}"
    );
    assert!(lines[1].contains("  trellis      in a terminal"), "{s}");
    assert!(
        lines[2].contains("  trellis run --model llamacpp:http://127.0.0.1:8080"),
        "{s}"
    );
    assert!(
        lines[3].contains("  [model] spec in your user config file, "),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// `queue add`, returning the id it made: the one id in pending/ that was
/// not there before.
fn add_id(d: &Path, args: &[&str]) -> String {
    let before = ids(d, "pending");
    add(d, args);
    let mut new: Vec<String> = ids(d, "pending")
        .into_iter()
        .filter(|i| !before.contains(i))
        .collect();
    assert_eq!(new.len(), 1, "{new:?}");
    new.remove(0)
}

fn cancel_events(d: &Path) -> Vec<serde_json::Value> {
    activity(d)
        .into_iter()
        .filter(|e| e["action"] == "task.cancel")
        .collect()
}

#[test]
fn queue_cancel_children_cancels_the_task_and_its_pending_descendants_at_any_depth() {
    // The risk: a person drops a breakdown filed with `queue add --parent`
    // and a grandchild, or a sibling outside the tree, is missed or caught.
    let d = fixture("cancel-children");
    let p = add_id(&d, &["the breakdown"]);
    let c = add_id(&d, &["a part", "--parent", &p]);
    let g = add_id(&d, &["a part of the part", "--parent", &c]);
    let b = add_id(&d, &["blocked part", "--parent", &p, "--backlog"]);
    let other = add_id(&d, &["not in the tree"]);

    let o = trellis(
        &d,
        &["queue", "cancel", &p, "--children", "--reason", "dropped"],
    );
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    let mut want = vec![p.clone(), c.clone(), g.clone(), b.clone()];
    want.sort();
    assert_eq!(ids(&d, "cancelled"), want, "{s}");
    assert_eq!(ids(&d, "pending"), vec![other], "{s}");
    for id in &want {
        assert!(s.contains(&format!("cancelled: {id}")), "{s}");
        let t = task_json(&d, "cancelled", id);
        assert_eq!(t["status"], "cancelled", "{t}");
    }

    let events = cancel_events(&d);
    assert_eq!(events.len(), 4, "one task.cancel per task: {events:?}");
    for e in &events {
        let detail = e["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("dropped"), "{e}");
        if e["entity"] != format!("task:{p}") {
            assert!(detail.contains(&p), "a descendant's line names {p}: {e}");
        }
    }
    assert_eq!(porcelain(&d), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_cancel_children_leaves_finished_descendants_and_names_each() {
    // The risk: a descendant already done, in review, failed or cancelled
    // is rewritten as cancelled, losing its outcome, or skipped unsaid.
    let d = fixture("cancel-children-finished");
    let p = add_id(&d, &["the breakdown"]);
    let done = add_id(&d, &["done part", "--parent", &p]);
    let review = add_id(&d, &["review part", "--parent", &p]);
    let failed = add_id(&d, &["failed part", "--parent", &p]);
    let gone = add_id(&d, &["cancelled part", "--parent", &p]);
    let open = add_id(&d, &["open part", "--parent", &p]);
    for (id, st) in [(&done, "done"), (&review, "in_review"), (&failed, "failed")] {
        let o = trellis(&d, &["queue", "set", id, "--status", st]);
        assert!(o.status.success(), "{}", combined(&o));
    }
    let o = trellis(&d, &["queue", "cancel", &gone]);
    assert!(o.status.success(), "{}", combined(&o));
    let before = (
        task_json(&d, "done", &done),
        task_json(&d, "done", &review),
        task_json(&d, "failed", &failed),
        task_json(&d, "cancelled", &gone),
    );

    let o = trellis(&d, &["queue", "cancel", &p, "--children"]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    let mut done_ids = vec![done.clone(), review.clone()];
    done_ids.sort();
    assert_eq!(ids(&d, "done"), done_ids, "{s}");
    assert_eq!(ids(&d, "failed"), vec![failed.clone()], "{s}");
    let mut cancelled = vec![p.clone(), gone.clone(), open.clone()];
    cancelled.sort();
    assert_eq!(ids(&d, "cancelled"), cancelled, "{s}");
    assert!(ids(&d, "pending").is_empty(), "{s}");
    assert_eq!(
        (
            task_json(&d, "done", &done),
            task_json(&d, "done", &review),
            task_json(&d, "failed", &failed),
            task_json(&d, "cancelled", &gone),
        ),
        before,
        "a finished descendant is left as it is"
    );
    for id in [&done, &review, &failed, &gone] {
        assert!(
            s.lines()
                .any(|l| l.contains("left") && l.contains(id.as_str())),
            "{id} is named as left:\n{s}"
        );
    }
    // The one cancelled earlier, then the parent and the open part.
    assert_eq!(cancel_events(&d).len(), 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_cancel_children_refuses_when_any_is_claimed_and_writes_nothing() {
    // The risk: a running descendant is cancelled under its run, which
    // then publishes its own copy and drops the cancel silently.
    let d = fixture("cancel-children-claimed");
    let p = add_id(&d, &["the breakdown"]);
    let c = add_id(&d, &["a part", "--parent", &p]);
    let g = add_id(&d, &["a part of the part", "--parent", &c]);
    let check = |claimed: &[&String]| {
        std::fs::create_dir_all(queue_dir(&d, "claimed")).unwrap();
        for id in claimed {
            std::fs::rename(
                queue_dir(&d, "pending").join(format!("{id}.json")),
                queue_dir(&d, "claimed").join(format!("{id}.json")),
            )
            .unwrap();
        }
        let pending = ids(&d, "pending");
        let logged = activity(&d).len();
        let o = trellis(&d, &["queue", "cancel", &p, "--children"]);
        let s = combined(&o);
        assert!(!o.status.success(), "{s}");
        for id in ids(&d, "claimed") {
            assert!(s.contains(&id), "the refusal names {id}:\n{s}");
        }
        assert_eq!(ids(&d, "pending"), pending, "nothing written:\n{s}");
        assert!(ids(&d, "cancelled").is_empty(), "{s}");
        assert_eq!(activity(&d).len(), logged, "nothing logged:\n{s}");
    };
    // A grandchild claimed, then the named task as well: both are named.
    check(&[&g]);
    check(&[&p]);
    assert_eq!(ids(&d, "claimed").len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

/// Of `among`, the ids `queue list <args>` prints, in `among`'s order.
fn listed(d: &Path, args: &[&str], among: &[&String]) -> Vec<String> {
    let mut a = vec!["queue", "list"];
    a.extend_from_slice(args);
    let o = trellis(d, &a);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    among
        .iter()
        .filter(|id| s.contains(id.as_str()))
        .map(|id| id.to_string())
        .collect()
}

#[test]
fn queue_list_text_matches_id_text_labels_and_notes_ignoring_case() {
    // The risk: a backlog routines keep adding cannot be searched by what a
    // task says, or a match is missed for its case or for where it is.
    let d = fixture("list-text");
    let text = add_id(&d, &["Rename Greet"]);
    let label = add_id(&d, &["tidy", "--label", "FrontEnd"]);
    let note = add_id(&d, &["later"]);
    let o = trellis(&d, &["queue", "note", &note, "Needs a DB Migration"]);
    assert!(o.status.success(), "{}", combined(&o));
    let none = add_id(&d, &["unrelated"]);
    let all = [&text, &label, &note, &none];

    assert_eq!(
        listed(&d, &["--text", "rename GREET"], &all),
        vec![text.clone()]
    );
    assert_eq!(
        listed(&d, &["--text", "frontend"], &all),
        vec![label.clone()]
    );
    assert_eq!(
        listed(&d, &["--text", "db migration"], &all),
        vec![note.clone()]
    );
    assert_eq!(
        listed(&d, &["--text", &none.to_uppercase()], &all),
        vec![none.clone()]
    );
    let o = trellis(&d, &["queue", "list", "--text", "no such words"]);
    let s = combined(&o);
    assert!(o.status.success() && s.contains("no task matches"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_list_text_narrows_each_other_filter() {
    // The risk: `--text` replaces a filter given with it instead of
    // narrowing it, and the list shows tasks the person excluded.
    let d = fixture("list-text-combined");
    std::fs::create_dir_all(d.join(".trellis/agents")).unwrap();
    std::fs::write(d.join(".trellis/agents/m.toml"), "model = \"scripted:x\"\n").unwrap();
    let login = add_id(
        &d,
        &[
            "fix login",
            "--priority",
            "high",
            "--label",
            "ui",
            "--assignee",
            "m",
        ],
    );
    let logout = add_id(
        &d,
        &[
            "fix logout",
            "--priority",
            "low",
            "--label",
            "ui",
            "--assignee",
            "m",
        ],
    );
    let signup = add_id(&d, &["fix signup", "--priority", "high", "--label", "api"]);
    let search = add_id(
        &d,
        &[
            "fix search",
            "--priority",
            "high",
            "--label",
            "ui",
            "--backlog",
        ],
    );
    let other = add_id(
        &d,
        &[
            "rename greet",
            "--priority",
            "high",
            "--label",
            "ui",
            "--assignee",
            "m",
        ],
    );
    let all = [&login, &logout, &signup, &search, &other];
    let fix = |extra: &[&str]| {
        let mut a = vec!["--text", "FIX"];
        a.extend_from_slice(extra);
        listed(&d, &a, &all)
    };

    assert_eq!(
        fix(&[]),
        vec![
            login.clone(),
            logout.clone(),
            signup.clone(),
            search.clone()
        ]
    );
    assert_eq!(
        fix(&["--priority", "high"]),
        vec![login.clone(), signup.clone(), search.clone()]
    );
    assert_eq!(
        fix(&["--label", "ui"]),
        vec![login.clone(), logout.clone(), search.clone()]
    );
    assert_eq!(
        fix(&["--assignee", "m"]),
        vec![login.clone(), logout.clone()]
    );
    assert_eq!(fix(&["--status", "backlog"]), vec![search.clone()]);
    assert_eq!(
        fix(&[
            "--priority",
            "high",
            "--label",
            "ui",
            "--assignee",
            "m",
            "--status",
            "todo"
        ]),
        vec![login.clone()]
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_list_text_still_lists_an_unreadable_task_file() {
    // The risk: a corrupt task file has no text to match, so a search hides
    // the one thing in the queue a person must repair.
    let d = fixture("list-text-unreadable");
    let fine = add_id(&d, &["rename greet"]);
    std::fs::write(queue_dir(&d, "pending").join("broken.json"), "not json").unwrap();

    let o = trellis(&d, &["queue", "list", "--text", "no such words"]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(
        s.lines()
            .any(|l| l.contains("broken") && l.contains("unreadable")),
        "{s}"
    );
    assert!(!s.contains(&fine), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_list_priority_hides_an_unreadable_task_file_as_label_does() {
    // The risk: a corrupt task file has no priority, so listing it under
    // `--priority high` reports a match that is not one, unlike `--label`.
    let d = fixture("list-priority-unreadable");
    let high = add_id(&d, &["rename greet", "--priority", "high"]);
    std::fs::write(queue_dir(&d, "pending").join("broken.json"), "not json").unwrap();

    for filter in [&["--priority", "high"], &["--label", "x"]] {
        let mut args = vec!["queue", "list"];
        args.extend_from_slice(filter);
        let o = trellis(&d, &args);
        let s = combined(&o);
        assert!(o.status.success(), "{filter:?}: {s}");
        assert!(!s.contains("broken"), "{filter:?}: {s}");
    }
    let s = combined(&trellis(&d, &["queue", "list", "--priority", "high"]));
    assert!(s.contains(&high), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_list_text_empty_or_blank_is_refused_and_lists_nothing() {
    // The risk: an empty search matches every task and reads as a result.
    let d = fixture("list-text-empty");
    let id = add_id(&d, &["rename greet"]);
    for text in ["", "   ", "\t"] {
        let o = trellis(&d, &["queue", "list", "--text", text]);
        let s = combined(&o);
        assert!(!o.status.success(), "{text:?}: {s}");
        assert!(s.contains("--text is empty"), "{text:?}: {s}");
        assert!(!s.contains(&id), "{text:?}: nothing listed:\n{s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_set_reason_until_refuses_a_missing_or_malformed_at() {
    // The risk: a wait with no instant, or one read as some other instant
    // than the person meant, would park a task that nothing wakes or wake
    // it at the wrong time. Each is refused, naming `--at`, before the task
    // file is touched.
    let d = fixture("set-until-refused");
    let id = add_id(&d, &["rename greet"]);
    let before = task_json(&d, "pending", &id);
    let set = |extra: &[&str]| {
        let mut a = vec!["queue", "set", id.as_str()];
        a.extend_from_slice(extra);
        trellis(&d, &a)
    };
    let mut cases: Vec<Vec<&str>> = vec![
        vec!["--status", "blocked", "--reason", "until"],
        vec!["--at", "2999-01-01T00:00:00Z"],
        vec![
            "--status",
            "blocked",
            "--reason",
            "manual",
            "--detail",
            "x",
            "--at",
            "2999-01-01T00:00:00Z",
        ],
    ];
    for bad in [
        "tomorrow",
        "2999-01-01",
        "2999-01-01T00:00:00",
        "2999-01-01T00:00Z",
        "29990101T000000Z",
        "2999-01-01T00:00:00+01",
        "2999-01-01T00:00:00Z[UTC]",
        "",
    ] {
        cases.push(vec![
            "--status", "blocked", "--reason", "until", "--at", bad,
        ]);
    }
    for c in &cases {
        let o = set(c);
        let s = combined(&o);
        assert!(!o.status.success(), "{c:?}: {s}");
        assert!(s.contains("--at"), "{c:?}: the refusal names --at:\n{s}");
        assert_eq!(
            task_json(&d, "pending", &id),
            before,
            "{c:?}: task file unchanged"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn queue_set_reason_until_is_listed_with_its_instant() {
    // The risk: a waiting task whose list line does not say until when,
    // so a person cannot tell a timed wait from a stuck one. The instant is
    // stored and shown in UTC whatever offset it was given in.
    let d = fixture("set-until-listed");
    let id = add_id(&d, &["rename greet"]);
    let o = trellis(
        &d,
        &[
            "queue",
            "set",
            &id,
            "--status",
            "blocked",
            "--reason",
            "until",
            "--at",
            "2999-01-01T01:30:00+01:00",
        ],
    );
    assert!(o.status.success(), "{}", combined(&o));
    let t = task_json(&d, "pending", &id);
    assert_eq!(t["status"], "blocked", "{t}");
    assert_eq!(t["blocked"]["reason"], "until", "{t}");
    assert_eq!(t["blocked"]["at"], "2999-01-01T00:30:00Z", "{t}");

    let o = trellis(&d, &["queue", "list"]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(
        s.contains("waiting: blocked until 2999-01-01T00:30:00Z"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn work_sends_no_key_to_an_endpoint_the_repository_chose() {
    // #248, ADR 0024 §3. The risk: `work` passing the repository's own
    // `[model] spec` to the run as the person's choice, so a cloned
    // repository's port received OPENAI_API_KEY from a heartbeat where a
    // plain `trellis run` refuses it.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let d = fixture("work-hostile-spec");
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!(
            "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n\
             [model]\nspec = \"openai-chat:http://127.0.0.1:{port}/v1#m\"\n"
        ),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "the repository's spec"]);
    add(&d, &["rename greet"]);
    // Built at run time: a key-shaped literal trips the secret scanners.
    let canary = format!("sk-{}-leak-canary-{}", "trellis", std::process::id());
    let o = trellis_env(&d, &["work", "--once"], &[("OPENAI_API_KEY", &canary)]);
    let s = combined(&o);
    assert!(
        s.contains("OPENAI_API_KEY"),
        "the refusal names the key: {s}"
    );
    assert!(!s.contains(&canary), "{s}");
    assert!(
        matches!(l.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "something connected to the repository's port: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// `fixture` with an `after_run` hook that records each run in `marker`, a
/// file outside the repository (hooks run in a throwaway clone, ADR 0032),
/// and a trust store of its own in `home`.
fn hooked(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let d = fixture(name);
    let home = scratch(&format!("{name}-home"));
    let marker = home.join("marker.cfg");
    let config = std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!(
            "{config}\n[hooks]\nafter_run = [\"git\", \"config\", \"--file\", {:?}, \"--add\", \"hook.ran\", \"yes\"]\n",
            marker.display().to_string()
        ),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "a hook"]);
    (d, home, marker)
}

fn trellis_home(d: &Path, home: &Path, args: &[&str]) -> Output {
    let h = home.display().to_string();
    trellis_env(d, args, &[("APPDATA", &h), ("XDG_CONFIG_HOME", &h)])
}

#[test]
fn task_worktree_runs_trusted_hooks_without_a_second_trust() {
    // ADR 0053 decision 3: trust is looked up under the worktree `work`
    // started in, and the task worktree's code must match it. The risk on
    // one side is a task worktree treated as a new repository, so every
    // task runs without the hooks the person trusted.
    let (d, home, marker) = hooked("hooks-trusted");
    let t = trellis_home(&d, &home, &["trust"]);
    assert!(t.status.success(), "{}", combined(&t));
    add(&d, &["one"]);
    let o = trellis_home(&d, &home, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(!s.contains("not trusted"), "{s}");
    assert!(
        std::fs::read_to_string(&marker).is_ok_and(|m| m.contains("ran = yes")),
        "the trusted hook ran in the task worktree:\n{s}"
    );
    for p in [&d, &home] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn task_worktree_whose_trusted_code_differs_from_the_trusted_entry_is_untrusted() {
    // The other side: a task worktree whose hook is not what was trusted
    // (here a commit changed it after `trellis trust`) runs without it and
    // says so, as the same change would in place (ADR 0023).
    let (d, home, marker) = hooked("hooks-changed");
    let t = trellis_home(&d, &home, &["trust"]);
    assert!(t.status.success(), "{}", combined(&t));
    let config = std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        config.replace("\"yes\"]", "\"changed\"]"),
    )
    .unwrap();
    git(
        &d,
        &["commit", "-qam", "the hook changed after it was trusted"],
    );
    add(&d, &["one"]);
    let o = trellis_home(&d, &home, &["work", "--model", &model(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.contains("not trusted (run trellis trust)"), "{s}");
    assert!(!marker.exists(), "the changed hook did not run:\n{s}");
    for p in [&d, &home] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn trust_linked_worktree_kept_by_work_leaves_the_starting_worktree_trusted() {
    // #718: a converged task's kept worktree sits under `.trellis/work/`,
    // and the `.trellis/` trust digest walked it, so after one `work` the
    // starting worktree read "changed since it was trusted" and its hooks
    // stopped running in place.
    let (d, home, _marker) = hooked("trust-kept-worktree");
    let t = trellis_home(&d, &home, &["trust"]);
    assert!(t.status.success(), "{}", combined(&t));
    add(&d, &["change the greeting"]);
    let o = trellis_home(&d, &home, &["work", "--model", &model(&d, "converge.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.contains("CONVERGED"), "{s}");
    let id = ids(&d, "done").pop().expect("one done task");
    assert!(
        d.join(".trellis/work/task")
            .join(&id)
            .join(".git")
            .is_file()
    );

    let shown = combined(&trellis_home(&d, &home, &["trust", "--show"]));
    assert!(shown.contains("state:   trusted"), "{shown}");
    for p in [&d, &home] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn task_worktree_runs_hold_no_queue_of_their_own() {
    // ADR 0053 decision 5: the queue is the starting worktree's; nothing
    // run in a task worktree makes one in that worktree's git directory.
    let d = fixture("no-second-queue");
    add(&d, &["one"]);
    let o = trellis(&d, &["work", "--model", &model(&d, "converge.json")]);
    assert!(o.status.success(), "{}", combined(&o));
    let id = &ids(&d, "done")[0];
    let wt = d.join(".trellis/work/task").join(id);
    // Canonical on both sides: git prints the long form of the path and the
    // fixture may sit under a short (8.3) temp directory name on Windows.
    let gd = PathBuf::from(git(&wt, &["rev-parse", "--absolute-git-dir"]).trim())
        .canonicalize()
        .unwrap();
    let worktrees = d.join(".git/worktrees").canonicalize().unwrap();
    assert!(gd.starts_with(&worktrees), "{}", gd.display());
    assert!(!gd.join("trellis").exists(), "{}", gd.display());
    let _ = std::fs::remove_dir_all(&d);
}
