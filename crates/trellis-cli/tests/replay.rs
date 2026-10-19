//! The replay recording every run writes (#620, ADR 0056), through the real
//! binary: written on every path, ignored, refused while tracked, loadable
//! as a script, and played back to the same result.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-replay-{name}-{}", std::process::id()));
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Checks that pass whatever the tree holds: `git --version` runs wherever
/// these tests do.
const CHECKS_PASS: &str =
    "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n";

/// Edit `file` to return `value`, run both checks, stop: a run that
/// converges.
fn converging(file: &str, value: &str) -> serde_json::Value {
    serde_json::json!({"steps": [
        {"tool": "edit", "path": file, "old": "return 1", "new": format!("return '{value}'")},
        {"tool": "run_checks", "paths": [file], "kinds": ["typecheck", "tests"]},
        {"tool": "done", "summary": format!("{file} returns {value}")},
    ]})
}

struct Fixture {
    root: PathBuf,
    user: PathBuf,
    spec: String,
    head: String,
}

/// A committed repository of two files, each one function nothing calls, with
/// checks that pass; a user config directory; and `script` outside the
/// repository (inside, it would dirty the worktree).
fn fixture(name: &str, script: &serde_json::Value) -> Fixture {
    let root = scratch(name);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    for (file, func) in [("a.py", "fa"), ("b.py", "fb")] {
        std::fs::write(root.join(file), format!("def {func}():\n    return 1\n")).unwrap();
    }
    std::fs::create_dir_all(root.join(".trellis")).unwrap();
    std::fs::write(root.join(".trellis/config.toml"), CHECKS_PASS).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let user = scratch(&format!("{name}-user"));
    let path = scratch(&format!("{name}-script")).join("s.json");
    std::fs::write(&path, script.to_string()).unwrap();
    Fixture {
        root,
        user,
        spec: format!("scripted:{}", path.display()),
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

fn said(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The recordings under `.trellis/replays/`, by file name.
fn recordings(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root.join(".trellis/replays")) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    v.sort();
    v
}

/// The one trail under `.trellis/runs/`, as JSON.
fn the_trail(root: &Path) -> serde_json::Value {
    let runs: Vec<PathBuf> = std::fs::read_dir(root.join(".trellis/runs"))
        .expect("the run wrote no trail")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(runs.len(), 1, "one run, one trail: {runs:?}");
    serde_json::from_str(&std::fs::read_to_string(&runs[0]).unwrap()).unwrap()
}

fn read_json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// `trail.factual[key]` with each entry's `duration_ms` removed. A replay
/// must run the same checks to the same results, but how long each took is
/// the loop's clock reading on this run (ADR 0007, amended for #622), so
/// two runs of one recording differ there and nowhere else.
fn untimed(trail: &serde_json::Value, key: &str) -> serde_json::Value {
    let mut v = trail["factual"][key].clone();
    for entry in v.as_array_mut().into_iter().flatten() {
        if let Some(fields) = entry.as_object_mut() {
            fields.remove("duration_ms");
        }
    }
    v
}

#[test]
fn replay_recording_is_written_by_every_run_loadable_ignored_and_refused_while_tracked() {
    // The risks (#620): a run path that writes no recording, or one a
    // rollback removes; a recording the scripted loader cannot read; one that
    // dirties the worktree, so the next run refuses (ADR 0009); and one that
    // came with the repository being played as if it recorded a run here.
    //
    // Not converged: an edit and `done` with no check, so the run rolls back,
    // and the recording must be written after that.
    let script = serde_json::json!({"steps": [
        {"tool": "edit", "path": "a.py", "old": "return 1", "new": "return 2"},
        {"tool": "done", "summary": "changed it"},
    ]});
    let f = fixture("written", &script);
    let o = run(&f, &["run", "change fa", "--model", &f.spec]);
    let out = said(&o);
    assert!(out.contains("NOT CONVERGED"), "{out}");
    let recs = recordings(&f.root);
    assert_eq!(recs.len(), 1, "one run, one recording:\n{out}");
    assert!(out.contains("replay:   .trellis"), "{out}");

    // Named for the run's trail, holding the run's base and every step.
    let trail = the_trail(&f.root);
    let rec = read_json(&recs[0]);
    assert_eq!(rec["run_id"], trail["run_id"], "{rec:#}\n{trail:#}");
    assert_eq!(rec["base_commit"], f.head.as_str());
    assert_eq!(rec["steps"], script["steps"], "{rec:#}");

    // Loadable by the scripted loader against the run's tool set.
    let text = std::fs::read_to_string(&recs[0]).unwrap();
    let m = trellis_loop::ScriptedModel::from_json_with(&text, &trellis_loop::ToolSet::default())
        .unwrap();
    assert_eq!(m.len(), 2);

    // Ignored, the directory's own .gitignore with it, so the tree the
    // rollback restored is clean and a second run starts.
    let rel = recs[0]
        .strip_prefix(&f.root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    git(&f.root, &["check-ignore", "-q", &rel]);
    git(
        &f.root,
        &["check-ignore", "-q", ".trellis/replays/.gitignore"],
    );
    assert_eq!(git(&f.root, &["status", "--porcelain"]), "", "{out}");
    let o = run(&f, &["run", "change fa again", "--model", &f.spec]);
    assert!(said(&o).contains("snapshot at"), "{}", said(&o));
    assert_eq!(recordings(&f.root).len(), 2);

    // Tracked, the run is refused before its snapshot, naming the file.
    git(&f.root, &["add", "-f", &rel]);
    git(&f.root, &["commit", "-qm", "track a recording"]);
    let head = git(&f.root, &["rev-parse", "HEAD"]);
    let o = run(&f, &["run", "change fa", "--model", &f.spec]);
    let out = said(&o);
    assert!(!o.status.success(), "{out}");
    assert!(
        !out.contains("snapshot at"),
        "refused after the snapshot:\n{out}"
    );
    assert!(
        out.contains(&rel) && out.contains("git rm -r --cached"),
        "{out}"
    );
    assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(recordings(&f.root).len(), 2, "a refused run recorded");
}

#[test]
fn replay_recording_of_a_converged_run_reproduces_its_proposed_list_and_checks() {
    // The risk: a replay that does something other than the run did. Played
    // back on the same commit, the recording must land the same proposal
    // and run the same checks to the same results.
    let f = fixture("converged", &converging("a.py", "A_NEW"));
    let o = run(&f, &["run", "change fa", "--model", &f.spec]);
    let out = said(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    let first = the_trail(&f.root);
    let recs = recordings(&f.root);
    assert_eq!(recs.len(), 1, "{out}");
    let changed = std::fs::read_to_string(f.root.join("a.py")).unwrap();

    // Back to the base: the edit and the trail's commit go, the ignored
    // recording stays.
    git(&f.root, &["reset", "-q", "--hard", &f.head]);
    assert_eq!(recordings(&f.root), recs);
    let rel = recs[0]
        .strip_prefix(&f.root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let replay = format!("scripted:{rel}");
    let o = run(&f, &["run", "change fa", "--model", &replay]);
    let out = said(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    let second = the_trail(&f.root);
    for key in ["proposed", "check_outcomes"] {
        assert_eq!(
            untimed(&second, key),
            untimed(&first, key),
            "{key} differs:\n{first:#}\n{second:#}"
        );
    }
    assert!(
        !first["factual"]["check_outcomes"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{first:#}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        changed
    );
}

#[test]
fn replay_recording_of_a_multi_agent_run_writes_one_targets_section_per_agent() {
    // The risk: a multi-agent run recorded as one `steps` list, which no
    // agent of a replay could play, or with one agent's steps dropped.
    let script = serde_json::json!({"targets": {
        "fa": converging("a.py", "A_NEW"),
        "fb": converging("b.py", "B_NEW"),
    }});
    let f = fixture("multi", &script);
    let targets = ["--agents", "1", "--target", "fa", "--target", "fb"];
    let mut args = vec!["run", "change fa and fb", "--model", &f.spec];
    args.extend(targets);
    let o = run(&f, &args);
    let out = said(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    let recs = recordings(&f.root);
    assert_eq!(recs.len(), 1, "{out}");
    let rec = read_json(&recs[0]);
    assert!(rec.get("steps").is_none(), "{rec:#}");
    for t in ["fa", "fb"] {
        assert_eq!(
            rec["targets"][t]["steps"], script["targets"][t]["steps"],
            "{t}: {rec:#}"
        );
    }
    let first = the_trail(&f.root);

    // And it plays back, each agent its own section.
    git(&f.root, &["reset", "-q", "--hard", &f.head]);
    let rel = recs[0]
        .strip_prefix(&f.root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let replay = format!("scripted:{rel}");
    let mut args = vec!["run", "change fa and fb", "--model", &replay];
    args.extend(targets);
    let o = run(&f, &args);
    let out = said(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    let second = the_trail(&f.root);
    for key in ["proposed", "check_outcomes"] {
        assert_eq!(
            untimed(&second, key),
            untimed(&first, key),
            "{key} differs:\n{first:#}\n{second:#}"
        );
    }
}
