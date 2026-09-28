//! `--json`, `log --json`, `rpc` and the user config directory, through the
//! real binary (ADR 0025).
//!
//! The property most worth a test is the negative one: under `--json` and
//! `rpc`, nothing but JSON reaches stdout. One stray `println!` breaks every
//! consumer, and no unit test sees stdout.

use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-modes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH");
    assert!(out.status.success(), "git {args:?}");
}

/// A committed repository, an empty user config directory, and a script
/// outside the repository (inside, it would dirty the worktree).
struct Fixture {
    root: PathBuf,
    user: PathBuf,
    spec: String,
}

fn fixture(name: &str) -> Fixture {
    let root = scratch(name);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    std::fs::write(root.join("a.py"), "def f():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
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
    }
}

fn trellis(f: &Fixture) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.current_dir(&f.root)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env("TRELLIS_CONFIG_DIR", &f.user);
    c
}

fn run(f: &Fixture, args: &[&str]) -> Output {
    trellis(f).args(args).output().unwrap()
}

/// Every stdout line parsed as JSON; panics naming the first that is not.
fn jsonl(o: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| {
                panic!(
                    "stdout line is not JSON ({e}): {l:?}\nstderr:\n{}",
                    String::from_utf8_lossy(&o.stderr)
                )
            })
        })
        .collect()
}

#[test]
fn run_json_puts_only_jsonl_on_stdout_bracketed_by_start_and_end() {
    let f = fixture("run-json");
    let o = run(&f, &["run", "change f", "--model", &f.spec, "--json"]);
    let lines = jsonl(&o);
    assert_eq!(lines.first().unwrap()["type"], "run_start", "{lines:?}");
    let end = lines.last().unwrap();
    assert_eq!(end["type"], "run_end");
    // Edited and stopped without a check: not converged, rolled back.
    assert_eq!(end["outcome"], "not_converged");
    assert_eq!(end["exit"], 1);
    assert_eq!(o.status.code(), Some(1), "exit codes are unchanged");
    // The loop's tracing events, in `--log-json`'s shape, in between.
    assert!(
        lines
            .iter()
            .any(|l| l["fields"]["message"] == "turn" && l["target"].is_string()),
        "{lines:?}"
    );
    // The human text went to stderr, not nowhere.
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("snapshot at"), "{err}");
}

#[test]
fn json_is_refused_where_it_would_do_nothing() {
    let f = fixture("json-refused");
    let o = run(&f, &["index", "--json"]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("--json applies to"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn log_json_exports_each_trail_as_one_line_of_data() {
    let f = fixture("log-json");
    run(&f, &["run", "change f", "--model", &f.spec]);
    let o = run(&f, &["log", "--json"]);
    assert!(o.status.success());
    let lines = jsonl(&o);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["integrity"], "intact");
    assert_eq!(lines[0]["trail"]["task"], "change f");
    assert!(lines[0]["run_id"].is_string());
}

#[test]
fn rpc_runs_a_prompt_to_completion_when_input_ends() {
    let f = fixture("rpc");
    let mut child = trellis(&f)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id":"s","type":"get_state"}}"#).unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "1", "type": "prompt", "task": "change f", "model": f.spec})
    )
    .unwrap();
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    let lines = jsonl(&o);
    let state = lines.iter().find(|l| l["id"] == "s").unwrap();
    assert_eq!(state["data"]["running"], false);
    let resp = lines
        .iter()
        .find(|l| l["type"] == "response" && l["id"] == "1")
        .unwrap();
    assert_eq!(resp["success"], true);
    let end = lines.iter().find(|l| l["type"] == "run_end").unwrap();
    assert_eq!(end["id"], "1");
    assert_eq!(end["outcome"], "not_converged");
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n",
        "rolled back"
    );
}

#[test]
fn rpc_approve_mode_asks_on_the_stream_while_input_stays_open() {
    // The process-level version of the unit test, because the defect it
    // defends against lived only in a real process: a run thread's writes to
    // stdout must not wait on the main thread's read of stdin.
    let f = fixture("rpc-approve");
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut child = trellis(&f)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let line = line.unwrap();
            let _ = tx.send(serde_json::from_str(&line).expect("stdout is JSONL"));
        }
    });
    let next = |pred: &dyn Fn(&Value) -> bool| -> Value {
        loop {
            let v = rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("a record within 60 s");
            if pred(&v) {
                return v;
            }
        }
    };
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec})
    )
    .unwrap();
    stdin.flush().unwrap();
    let q = next(&|v| v["type"] == "approval_request");
    assert_eq!(q["question"], "edit a.py (+1 -1)");
    writeln!(
        stdin,
        r#"{{"id":"{}","type":"approve","yes":true}}"#,
        q["id"].as_str().unwrap()
    )
    .unwrap();
    stdin.flush().unwrap();
    let end = next(&|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p");
    writeln!(stdin, r#"{{"id":"m","type":"get_messages"}}"#).unwrap();
    stdin.flush().unwrap();
    let m = next(&|v| v["id"] == "m");
    assert_eq!(m["data"]["messages"][0]["call"], "edit a.py", "{m}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
}

#[test]
fn a_user_context_file_and_config_reach_a_run_and_are_named() {
    let f = fixture("user-dir");
    std::fs::write(f.user.join("AGENTS.md"), "Prefer small diffs.\n").unwrap();
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let o = run(&f, &["run", "change f", "--model", &f.spec]);
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    // The user file asked for approval and there is no terminal here, so the
    // run refuses rather than running hands-off (ADR 0018, ADR 0025 §7).
    assert!(
        err.contains("approve mode needs an interactive terminal"),
        "{out}\n{err}"
    );
    let o = run(&f, &["run", "change f", "--model", &f.spec, "--auto"]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("context files: <user>/AGENTS.md loaded"),
        "{out}"
    );
}

#[test]
fn a_key_in_the_user_config_stops_every_command_that_reads_it() {
    let f = fixture("user-key");
    std::fs::write(
        f.user.join("config.toml"),
        "[model]\napi_key = \"sk-do-not-print-me\"\n",
    )
    .unwrap();
    let o = run(&f, &["run", "change f", "--model", &f.spec]);
    assert!(!o.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(all.contains("ADR 0010"), "{all}");
    assert!(!all.contains("sk-do-not-print-me"), "{all}");
}
