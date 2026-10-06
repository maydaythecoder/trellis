//! `trellis run --agents` and `--target`, through the real binary (ADR 0045).
//!
//! Every refusal here has to land before the snapshot: a run refused after
//! it would already have touched the repository's git state, and the user
//! would have to wonder whether a rollback was complete. So each refusal test
//! checks HEAD and the worktree, not only the exit status.
//!
//! Before #465 every `--agents` value but `1` was refused with one message,
//! so each test asserts on what the new message says, not only that the run
//! was refused; an assertion on the exit status alone would pass either way.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-partitions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A committed repository of three files, each holding one function nothing
/// calls, so each is its own partition; a user config directory; and a
/// script outside the repository (inside, it would dirty the worktree).
struct Fixture {
    root: PathBuf,
    user: PathBuf,
    spec: String,
    head: String,
}

fn fixture(name: &str) -> Fixture {
    let root = scratch(name);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    for (file, func) in [("a.py", "fa"), ("b.py", "fb"), ("c.py", "fc")] {
        std::fs::write(root.join(file), format!("def {func}():\n    return 1\n")).unwrap();
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let user = scratch(&format!("{name}-user"));
    let script = scratch(&format!("{name}-script")).join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
    .unwrap();
    Fixture {
        root,
        user,
        spec: format!("scripted:{}", script.display()),
        head,
    }
}

fn run(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .current_dir(&f.root)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env("TRELLIS_CONFIG_DIR", &f.user)
        .args(args)
        .output()
        .unwrap()
}

/// stdout and stderr together: without `--json` a run's human lines go to
/// stdout and a refusal to stderr, and these tests care what was said, not
/// which stream carried it or in what order.
fn said(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Refused, and refused before the snapshot: HEAD where it was, nothing in
/// the worktree changed, and no run started.
fn assert_refused_before_snapshot(f: &Fixture, o: &Output) {
    let err = said(o);
    assert!(!o.status.success(), "the run was not refused:\n{err}");
    assert!(
        !err.contains("snapshot at"),
        "refused after the snapshot:\n{err}"
    );
    assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), f.head, "HEAD moved");
    assert_eq!(
        git(&f.root, &["status", "--porcelain"]),
        "",
        "the worktree changed"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def fa():\n    return 1\n"
    );
}

#[test]
fn agents_above_one_without_target_is_refused_naming_target_and_partitions() {
    // ADR 0045 §2: with no target there are no work units, and guessing
    // them from the task sentence is what no measured model can do.
    let f = fixture("no-target");
    for agents in ["2", "auto"] {
        let o = run(
            &f,
            &["run", "change fa", "--model", &f.spec, "--agents", agents],
        );
        assert_refused_before_snapshot(&f, &o);
        let err = said(&o);
        assert!(err.contains("--target"), "--agents {agents}:\n{err}");
        assert!(
            err.contains("trellis partitions"),
            "--agents {agents}:\n{err}"
        );
    }
}

#[test]
fn agents_zero_is_refused_naming_the_accepted_values() {
    let f = fixture("zero");
    let o = run(
        &f,
        &[
            "run",
            "change fa",
            "--model",
            &f.spec,
            "--agents",
            "0",
            "--target",
            "fa",
        ],
    );
    assert_refused_before_snapshot(&f, &o);
    let err = said(&o);
    assert!(err.contains("at least 1") && err.contains("auto"), "{err}");
}

#[test]
fn agents_that_is_not_a_number_is_refused_naming_the_accepted_values() {
    let f = fixture("nan");
    for agents in ["many", "-1", "1.5"] {
        let o = run(
            &f,
            &[
                "run",
                "change fa",
                "--model",
                &f.spec,
                // `=` so clap takes `-1` as the value rather than a flag.
                &format!("--agents={agents}"),
                "--target",
                "fa",
            ],
        );
        assert_refused_before_snapshot(&f, &o);
        let err = said(&o);
        assert!(
            err.contains("at least 1") && err.contains("auto"),
            "--agents {agents}:\n{err}"
        );
    }
}

#[test]
fn agents_with_an_unknown_target_is_refused_before_the_snapshot() {
    // A target that resolves to nothing would put a partition boundary on a
    // symbol the user did not mean; `radius` refuses it the same way.
    let f = fixture("unknown");
    for agents in ["1", "auto"] {
        let o = run(
            &f,
            &[
                "run",
                "change fa",
                "--model",
                &f.spec,
                "--agents",
                agents,
                "--target",
                "fa",
                "--target",
                "nope",
            ],
        );
        assert_refused_before_snapshot(&f, &o);
        let err = said(&o);
        assert!(
            err.contains("no first-party definition matches `nope`"),
            "--agents {agents}:\n{err}"
        );
    }
}

#[test]
fn agents_auto_on_a_scripted_backend_runs_at_width_one_and_says_why() {
    // S18: a backend with no measured batching capacity yields one agent,
    // and the run says so rather than leaving the user to wonder why four
    // targets did not run at once.
    let f = fixture("auto");
    let o = run(
        &f,
        &[
            "run",
            "change fa",
            "--model",
            &f.spec,
            "--agents",
            "auto",
            "--target",
            "fa",
            "--target",
            "fb",
            "--target",
            "fc",
        ],
    );
    let err = said(&o);
    assert!(err.contains("one at a time"), "{err}");
    assert!(err.contains("unknown"), "the reason is missing:\n{err}");
    // Today's single-agent path ran: it snapshotted, and the script's edit
    // stopped without a check, so it was rolled back.
    assert!(err.contains("snapshot at"), "{err}");
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def fa():\n    return 1\n",
        "the unconverged edit was not rolled back"
    );
}

#[test]
fn agents_above_one_with_targets_is_refused_as_not_built_yet() {
    // Part B (#466) builds running partitions together. Until then a width
    // above 1 is refused rather than quietly run as one agent: a run that
    // appears to do what was asked without doing it is the failure CLAUDE.md
    // names.
    let f = fixture("not-built");
    let o = run(
        &f,
        &[
            "run",
            "change fa",
            "--model",
            &f.spec,
            "--agents",
            "3",
            "--target",
            "fa",
            "--target",
            "fb",
            "--target",
            "fc",
        ],
    );
    assert_refused_before_snapshot(&f, &o);
    let err = said(&o);
    assert!(
        err.contains("not built yet") && err.contains("#466"),
        "{err}"
    );
}
