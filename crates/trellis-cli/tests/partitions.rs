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
    fixture_with(
        name,
        None,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
}

/// The same repository, with `config` committed as `.trellis/config.toml`
/// when given, and `script` as the scripted model.
fn fixture_with(name: &str, config: Option<&str>, script_json: &str) -> Fixture {
    let root = scratch(name);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    for (file, func) in [("a.py", "fa"), ("b.py", "fb"), ("c.py", "fc")] {
        std::fs::write(root.join(file), format!("def {func}():\n    return 1\n")).unwrap();
    }
    if let Some(config) = config {
        std::fs::create_dir_all(root.join(".trellis")).unwrap();
        std::fs::write(root.join(".trellis/config.toml"), config).unwrap();
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let user = scratch(&format!("{name}-user"));
    let script = scratch(&format!("{name}-script")).join("s.json");
    std::fs::write(&script, script_json).unwrap();
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

/// A `[checks]` section whose type check and tests both pass, whatever the
/// tree holds: `git --version` runs everywhere these tests do.
const CHECKS_PASS: &str =
    "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n";

/// One `targets` section: edit `file` to return `value`, run both checks,
/// stop.
fn section(file: &str, value: &str) -> serde_json::Value {
    serde_json::json!({"steps": [
        {"tool": "edit", "path": file, "old": "return 1", "new": format!("return '{value}'")},
        {"tool": "run_checks", "paths": [file], "kinds": ["typecheck", "tests"]},
        {"tool": "done", "summary": format!("{file} returns {value}")},
    ]})
}

/// The one trail the run committed, as JSON.
fn the_trail(root: &Path) -> serde_json::Value {
    let runs: Vec<PathBuf> = std::fs::read_dir(root.join(".trellis/runs"))
        .expect("the run wrote no trail")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(runs.len(), 1, "one run, one trail: {runs:?}");
    serde_json::from_str(&std::fs::read_to_string(&runs[0]).unwrap()).unwrap()
}

#[test]
fn three_partition_run_converges_with_every_partitions_edit() {
    // #466: three targets nothing connects are three parallel partitions,
    // each run by its own agent in its own worktree; the union lands in the
    // root only after the integrated check passed on it (ADR 0042). The
    // risk: a run that converges with one partition's edit, or none.
    let script = serde_json::json!({"targets": {
        "fa": section("a.py", "A_NEW"),
        "fb": section("b.py", "B_NEW"),
        "fc": section("c.py", "C_NEW"),
    }});
    let f = fixture_with("three", Some(CHECKS_PASS), &script.to_string());
    let o = run(
        &f,
        &[
            "run",
            "change fa, fb and fc",
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
    let out = said(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(
        out.contains("CONVERGED") && !out.contains("NOT CONVERGED"),
        "{out}"
    );
    for (file, func, value) in [
        ("a.py", "fa", "A_NEW"),
        ("b.py", "fb", "B_NEW"),
        ("c.py", "fc", "C_NEW"),
    ] {
        assert_eq!(
            std::fs::read_to_string(f.root.join(file)).unwrap(),
            format!("def {func}():\n    return '{value}'\n"),
            "{file} does not hold its partition's edit:\n{out}"
        );
    }
    // Left for review, not committed: the only new commit is the trail's.
    assert_eq!(git(&f.root, &["rev-parse", "HEAD~1"]), f.head, "{out}");
    // One trail, holding every partition's actions, and no gap: the
    // integrated check passed.
    let trail = the_trail(&f.root);
    let edited: std::collections::BTreeSet<&str> = trail["factual"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["tool"] == "edit")
        .map(|a| a["target"].as_str().unwrap())
        .collect();
    assert_eq!(edited, ["a.py", "b.py", "c.py"].into(), "{trail:#}");
    assert!(
        trail["factual"].get("suspected_recall_gaps").is_none(),
        "{trail:#}"
    );
    // The worktrees are gone.
    let work = f.root.join(".trellis/work");
    assert!(
        !work.exists() || std::fs::read_dir(&work).unwrap().next().is_none(),
        "a partition worktree was left behind"
    );
}

/// A type check that fails only when `a.py` holds `A_NEW` and `b.py` holds
/// `B_NEW`, naming both files, as `integrate.rs`'s collision checker does:
/// each partition's own run of it passes, and only the union fails.
fn collision_checks() -> String {
    let typecheck = if cfg!(windows) {
        concat!(
            r#"["cmd", "/C", "findstr", "/C:A_NEW", "a.py", ">NUL", "&&", "#,
            r#""findstr", "/C:B_NEW", "b.py", ">NUL", "&&", "(", "echo", "a.py:1:", "#,
            r#""error", "&", "echo", "b.py:1:", "error", "&", "exit", "1", ")", "&", "#,
            r#""exit", "0"]"#
        )
    } else {
        concat!(
            r#"["sh", "-c", "grep -q A_NEW a.py && grep -q B_NEW b.py && "#,
            r#"{ echo 'a.py:1: error'; echo 'b.py:1: error'; exit 1; }; exit 0"]"#
        )
    };
    format!("[checks]\ntypecheck = {typecheck}\ntests = [\"git\", \"--version\"]\n")
}

#[test]
fn induced_collision_fails_the_integrated_check_rolls_back_and_records_a_gap() {
    // ADR 0042, rule 6: two partitions the graph calls disjoint each pass
    // their own check, and the merged union fails one naming files from
    // both. The risk is threefold: a run that converges on the partitions'
    // passes, a rollback that leaves one partition's edit behind, and a
    // trail that loses the evidence that the graph missed an edge.
    let script = serde_json::json!({"targets": {
        "fa": section("a.py", "A_NEW"),
        "fb": section("b.py", "B_NEW"),
    }});
    let f = fixture_with("collision", Some(&collision_checks()), &script.to_string());
    let o = run(
        &f,
        &[
            "run",
            "change fa and fb",
            "--model",
            &f.spec,
            "--agents",
            "2",
            "--target",
            "fa",
            "--target",
            "fb",
        ],
    );
    let out = said(&o);
    assert_eq!(o.status.code(), Some(1), "not converged, restored:\n{out}");
    assert!(out.contains("NOT CONVERGED"), "{out}");
    // Every file as HEAD has it; the only new commit is the trail's.
    assert_eq!(git(&f.root, &["rev-parse", "HEAD~1"]), f.head, "{out}");
    assert_eq!(git(&f.root, &["status", "--porcelain"]), "", "{out}");
    for (file, func) in [("a.py", "fa"), ("b.py", "fb"), ("c.py", "fc")] {
        assert_eq!(
            std::fs::read_to_string(f.root.join(file)).unwrap(),
            format!("def {func}():\n    return 1\n"),
            "{file} was not rolled back:\n{out}"
        );
    }
    let trail = the_trail(&f.root);
    let gaps = trail["factual"]["suspected_recall_gaps"]
        .as_array()
        .unwrap_or_else(|| panic!("no gap recorded: {trail:#}"));
    assert_eq!(gaps.len(), 1, "{trail:#}");
    let partitions: std::collections::BTreeSet<u64> = gaps[0]["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_u64().unwrap())
        .collect();
    assert_eq!(partitions.len(), 2, "both partitions are named: {trail:#}");
    let files: std::collections::BTreeSet<&str> = gaps[0]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(files, ["a.py", "b.py"].into(), "{trail:#}");
    assert_eq!(gaps[0]["check"], "Typecheck", "{trail:#}");
}

#[test]
fn agents_above_one_in_approve_mode_is_refused_before_the_snapshot() {
    // Agents running at once would put questions about edits in different
    // worktrees to one person at once; refused, not interleaved (#466).
    let f = fixture_with(
        "approve",
        Some(CHECKS_PASS),
        r#"{"steps":[{"tool":"done","summary":"x"}]}"#,
    );
    let o = run(
        &f,
        &[
            "run",
            "change fa and fb",
            "--model",
            &f.spec,
            "--agents",
            "2",
            "--target",
            "fa",
            "--target",
            "fb",
            "--approve",
        ],
    );
    assert_refused_before_snapshot(&f, &o);
    let err = said(&o);
    assert!(
        err.contains("approve mode") && err.contains("--auto"),
        "{err}"
    );
    assert!(
        !err.contains("interactive terminal"),
        "refused for the terminal, not for the width:\n{err}"
    );
}

#[test]
fn agents_above_one_with_trusted_repository_code_is_refused_before_the_snapshot() {
    // The trust guard and the writer guards watch the repository root
    // (tooling.rs), not a partition's worktree, so trusted code running in
    // one would go unwatched. Refused rather than run unguarded (#466).
    let config = format!("{CHECKS_PASS}\n[tools]\nbash = true\nbash_unattended = true\n");
    let f = fixture_with(
        "trusted",
        Some(&config),
        r#"{"steps":[{"tool":"done","summary":"x"}]}"#,
    );
    let trusting = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_trellis"))
            .current_dir(&f.root)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env("TRELLIS_CONFIG_DIR", &f.user)
            .env("APPDATA", &f.user)
            .env("XDG_CONFIG_HOME", &f.user)
            .args(args)
            .output()
            .unwrap()
    };
    let t = trusting(&["trust"]);
    assert!(t.status.success(), "trust: {}", said(&t));
    let o = trusting(&[
        "run",
        "change fa and fb",
        "--model",
        &f.spec,
        "--agents",
        "2",
        "--target",
        "fa",
        "--target",
        "fb",
    ]);
    assert_refused_before_snapshot(&f, &o);
    let err = said(&o);
    assert!(err.contains("bash") && err.contains("--agents 1"), "{err}");
}

#[test]
fn a_partitions_edit_into_the_vault_is_refused_and_named() {
    // ADR 0049 (#357) in a multi-agent run: each partition's orchestrator
    // has the run's governance, the vault's notes root included. The risk:
    // a partition's worktree is a fresh checkout, and governance built for
    // the root alone would let an agent there rewrite a note, which the
    // merge would then carry into the root.
    let config = format!("{CHECKS_PASS}\n[context]\nvault = \"vault\"\n");
    let mut fa = section("a.py", "A_NEW");
    fa["steps"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({"tool": "edit", "path": "vault/retries.md",
            "old": "Backoff doubles.", "new": "Backoff triples."}),
    );
    let script = serde_json::json!({"targets": {"fa": fa, "fb": section("b.py", "B_NEW")}});
    let f = fixture_with("vault", Some(&config), &script.to_string());
    std::fs::create_dir_all(f.root.join("vault")).unwrap();
    let note = "---\ntitle: \"Retry policy\"\nparent: null\n---\n\nBackoff doubles.\n";
    std::fs::write(f.root.join("vault/retries.md"), note).unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "a note"]);
    let o = run(
        &f,
        &[
            "run",
            "change fa and fb",
            "--model",
            &f.spec,
            "--agents",
            "2",
            "--target",
            "fa",
            "--target",
            "fb",
        ],
    );
    let out = said(&o);
    assert_eq!(
        std::fs::read_to_string(f.root.join("vault/retries.md")).unwrap(),
        note,
        "the note changed:\n{out}"
    );
    let trail = the_trail(&f.root).to_string();
    assert!(trail.contains("edit refused"), "{trail}");
    assert!(trail.contains("[context] vault"), "{trail}");
}
