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
fn work_json_puts_only_jsonl_on_stdout_and_brackets_the_invocation() {
    // `work` prints through `work.rs` and `trellis-queue`, which other work
    // owns; this is what catches one of their lines reaching stdout.
    let f = fixture("work-json");
    let o = run(&f, &["queue", "add", "change f"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(&f, &["work", "--once", "--model", &f.spec, "--json"]);
    let lines = jsonl(&o);
    let types: Vec<&str> = lines.iter().filter_map(|l| l["type"].as_str()).collect();
    assert_eq!(
        types,
        ["work_start", "run_start", "run_end", "work_end"],
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap()["exit"], o.status.code().unwrap());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("work:     queue at"), "{err}");
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
    // The process-level version of the unit test. The unit tests drive
    // `serve` in-process; here a run thread writes to a real stdout while the
    // main thread blocks reading a real stdin, which only a process shows.
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

#[test]
fn rpc_compact_with_no_run_in_flight_answers_no_run_in_flight() {
    // The risk: a `compact` with nothing to compact reported as done, or as
    // "not built" after it was built. There is no run, so there is no model
    // request to shrink, and the client is told exactly that.
    let f = fixture("rpc-compact-idle");
    let mut child = trellis(&f)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id":"c","type":"compact"}}"#).unwrap();
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    let lines = jsonl(&o);
    let resp = lines.iter().find(|l| l["id"] == "c").expect("answered");
    assert_eq!(resp["type"], "response");
    assert_eq!(resp["command"], "compact");
    assert_eq!(resp["success"], false, "{resp}");
    assert_eq!(resp["error"], "no run in flight", "{resp}");
}

/// Serves `replies` in order as an OpenAI-protocol endpoint on a loopback
/// port, returning each request body it received.
fn provider_stub(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let mut got = Vec::new();
        for r in replies {
            let Ok((mut s, _)) = l.accept() else { break };
            s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
                .ok();
            let mut raw = Vec::new();
            let mut buf = [0u8; 16384];
            let body = loop {
                let n = s.read(&mut buf).unwrap_or(0);
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length").then_some(v)
                        })
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if body.len() >= len || n == 0 {
                        break body.to_string();
                    }
                } else if n == 0 {
                    break String::new();
                }
            };
            got.push(body);
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{r}",
                r.len()
            );
            s.write_all(out.as_bytes()).ok();
        }
        got
    });
    (format!("http://{addr}"), h)
}

fn openai_call(id: &str, name: &str, args: Value) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,
        "arguments":args.to_string()}}]}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

#[test]
fn rpc_compact_mid_run_folds_the_older_turns_before_the_next_request() {
    // The risk: `compact` answered with figures that describe no request
    // the model was actually sent. The answer must come from the request
    // after it, and that request, as the provider received it, must carry
    // the elision note in place of the first turn's observation.
    let f = fixture("rpc-compact-run");
    // Large enough that its shown tail outweighs the note replacing it.
    std::fs::write(
        f.root.join("big.py"),
        format!("X = 1\n{}", "# pad\n".repeat(800)),
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "big"]);
    let (base, server) = provider_stub(vec![
        openai_call("c1", "read_file", serde_json::json!({"path": "big.py"})),
        openai_call(
            "c2",
            "edit",
            serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
        ),
        openai_call("c3", "done", serde_json::json!({"summary": "changed it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key,
    // so no key of this machine's reaches the stub.
    let key_env = "TRELLIS_TEST_RPC_COMPACT_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[control]\nmode = \"approve\"\n\n[[providers]]\nname = \"stub\"\n\
             protocol = \"openai\"\nbase_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut child = trellis(&f)
        .arg("rpc")
        .env_remove(key_env)
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
    let mut seen: Vec<Value> = Vec::new();
    let mut next = |pred: &dyn Fn(&Value) -> bool| -> Value {
        loop {
            let v = rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("a record within 60 s; so far: {seen:?}"));
            seen.push(v.clone());
            if pred(&v) {
                return v;
            }
        }
    };
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                           "model": "provider:stub#m"})
    )
    .unwrap();
    stdin.flush().unwrap();
    // The run is paused on the edit's question, one turn behind it.
    let q = next(&|v| v["type"] == "approval_request");
    // Read in order, so the request is registered before the yes that lets
    // the run reach its next model request.
    writeln!(stdin, r#"{{"id":"c","type":"compact"}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"id":"{}","type":"approve","yes":true}}"#,
        q["id"].as_str().unwrap()
    )
    .unwrap();
    stdin.flush().unwrap();
    let c = next(&|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["command"], "compact");
    assert_eq!(c["success"], true, "{c}");
    let d = &c["data"];
    // Two turns (the read, the edit) before the third request: the read is
    // folded, and the edit is the newest, which never is.
    assert_eq!(d["turn"], 3, "{c}");
    assert_eq!(d["turns_folded"], 1, "{c}");
    assert_eq!(d["elided_turns"], 1, "{c}");
    let (before, after) = (
        d["bytes_before"].as_u64().unwrap(),
        d["bytes_after"].as_u64().unwrap(),
    );
    assert!(after < before, "{c}");
    let end = next(&|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(
        !requests[1].contains("elided: "),
        "nothing was elided before the request: {}",
        requests[1]
    );
    assert!(
        requests[2].contains("elided: read_file big.py"),
        "the request after `compact` carries the note: {}",
        requests[2]
    );
    assert!(!requests[2].contains("# pad"), "{}", requests[2]);
    assert_eq!(
        requests[2].len() as u64,
        after,
        "the size reported is the size sent"
    );
}
