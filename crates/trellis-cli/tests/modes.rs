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
    let out = trellis_vcs::command()
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
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let q = rpc.next(|v| v["type"] == "approval_request");
    assert_eq!(q["question"], "edit a.py (+1 -1)");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let end = rpc.next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p");
    rpc.send(serde_json::json!({"id": "m", "type": "get_messages"}));
    let m = rpc.next(|v| v["id"] == "m");
    assert_eq!(m["data"]["messages"][0]["call"], "edit a.py", "{m}");
    rpc.finish();
}

#[test]
fn rpc_run_whose_client_closes_every_pipe_still_rolls_back_and_writes_its_trail() {
    // The risk: a desktop that crashes, or is ended from the task manager,
    // closes the child's stdin, stdout and stderr together (ADR 0025 §5, ADR
    // 0048 §9). End of input must still decline the open question, roll back
    // the edit already applied, and write the trail, with nobody left to
    // read a line of it; one write that panics on a closed pipe skips both.
    // The first edit is approved so the tree holds an applied write when the
    // client goes: stopping at the first question would pass with rollback
    // broken, since approve mode asks before it applies.
    let f = fixture("rpc-vanish");
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let script = scratch("rpc-vanish-two-edits").join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"edit","path":"a.py","old":"return 2","new":"return 3"},
            {"tool":"done","summary":"changed it twice"}
        ]}"#,
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let mut child = trellis(&f)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let stderr = child.stderr.take().unwrap();
    let mut seen = Vec::new();
    let mut next_question = |stdout: &mut std::io::BufReader<std::process::ChildStdout>| loop {
        use std::io::BufRead;
        let mut line = String::new();
        let n = stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "stdout ended before an approval_request: {seen:?}");
        let v: Value = serde_json::from_str(&line).expect("stdout is JSONL");
        seen.push(v.clone());
        if v["type"] == "approval_request" {
            return v;
        }
    };
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": spec})
    )
    .unwrap();
    stdin.flush().unwrap();
    let q = next_question(&mut stdout);
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": q["id"], "type": "approve", "yes": true})
    )
    .unwrap();
    stdin.flush().unwrap();
    next_question(&mut stdout);
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 2\n",
        "the first edit is applied when the client goes"
    );

    drop((stdin, stdout, stderr));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("trellis rpc still running 60 s after its client closed every pipe");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n",
        "rolled back to the committed content (exit {status})"
    );
    let trails: Vec<PathBuf> = std::fs::read_dir(f.root.join(".trellis/runs"))
        .unwrap_or_else(|e| panic!("no trail directory (exit {status}): {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(trails.len(), 1, "one trail (exit {status}): {trails:?}");
    let trail: Value = serde_json::from_str(&std::fs::read_to_string(&trails[0]).unwrap()).unwrap();
    // `trellis log` derives the outcome from the last check's verdict; a run
    // that converged has one that passed.
    let factual = &trail["factual"];
    let last = factual["check_outcomes"]
        .as_array()
        .and_then(|c| c.last())
        .or_else(|| factual["checks"].as_array().and_then(|c| c.last()));
    assert_ne!(
        last.map(|c| &c["passed"]),
        Some(&Value::Bool(true)),
        "the trail records a converged run: {trail}"
    );
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

/// Whether `raw` is a `GET` of a path ending in `/models`: the
/// identification read a run makes of a loopback base (ADR 0047 decision
/// 3). A stub that spent a canned reply on it would pass for a runtime and
/// shift every later reply.
fn is_models_get(raw: &str) -> bool {
    let mut line = raw.lines().next().unwrap_or_default().split_whitespace();
    line.next() == Some("GET") && line.next().is_some_and(|p| p.ends_with("/models"))
}

/// The answer a server that is not colibri gives that read.
fn models_404() -> String {
    let body = r#"{"error":{"message":"not found","type":"not_found_error"}}"#;
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Serves `replies` in order as an OpenAI-protocol endpoint on a loopback
/// port, returning each request body it received. A `GET` of `/models` is
/// answered 404 and neither takes a reply nor is returned.
fn provider_stub(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut replies = replies.into_iter().peekable();
        while let Some(r) = replies.peek() {
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
            if is_models_get(&String::from_utf8_lossy(&raw)) {
                s.write_all(models_404().as_bytes()).ok();
                continue;
            }
            got.push(body);
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{r}",
                r.len()
            );
            s.write_all(out.as_bytes()).ok();
            replies.next();
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
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                           "model": "provider:stub#m"}),
    );
    // The run is paused on the edit's question, one turn behind it.
    let q = rpc.next(|v| v["type"] == "approval_request");
    // Read in order, so the request is registered before the yes that lets
    // the run reach its next model request.
    rpc.send(serde_json::json!({"id": "c", "type": "compact"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
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
    let end = rpc.next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p");
    rpc.finish();

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
    // And the trail records it under the turn it happened at: the trail is
    // where a reader learns what the model was not shown (ADR 0025 §5;
    // #248, the run's driver answering no requested compactions).
    let trails: Vec<Value> = std::fs::read_dir(f.root.join(".trellis/runs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect();
    assert_eq!(trails.len(), 1);
    let actions = trails[0]["factual"]["actions"].as_array().unwrap();
    let folded: Vec<&Value> = actions
        .iter()
        .filter(|a| a["tool"] == "compaction")
        .collect();
    assert_eq!(folded.len(), 1, "{actions:?}");
    assert_eq!(folded[0]["turn"], 3, "{actions:?}");
    assert!(
        folded[0]["outcome"]
            .as_str()
            .unwrap()
            .starts_with("requested over rpc"),
        "{actions:?}"
    );
}

/// A running `trellis rpc`, its stdin, and its stdout parsed record by
/// record on a reader thread.
struct Rpc {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    rx: std::sync::mpsc::Receiver<Value>,
    reader: std::thread::JoinHandle<()>,
    seen: Vec<Value>,
}

impl Rpc {
    fn start(mut cmd: Command) -> Self {
        let mut child = cmd
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let _ = tx.send(serde_json::from_str(&line).expect("stdout is JSONL"));
            }
        });
        Self {
            child,
            stdin,
            rx,
            reader,
            seen: Vec::new(),
        }
    }

    fn send(&mut self, v: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    }

    /// The next record matching `pred`, keeping every record read.
    fn next(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let v = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("a record within 60 s; so far: {:?}", self.seen));
            self.seen.push(v.clone());
            if pred(&v) {
                return v;
            }
        }
    }

    /// Closes stdin, waits for the process, and returns every record.
    fn finish(mut self) -> Vec<Value> {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
        self.reader.join().unwrap();
        self.seen.extend(self.rx.try_iter());
        self.seen
    }
}

#[test]
fn rpc_steer_reaches_the_next_model_request_and_cannot_converge_the_run() {
    // The risks: a steer accepted and never sent, or sent somewhere other
    // than the request after it; and a steer that claims the checks passed
    // being taken for a check (ADR 0004). The provider records what it was
    // sent, so "reached the model" is observed, not inferred.
    let f = fixture("rpc-steer-run");
    let (base, server) = provider_stub(vec![
        openai_call(
            "c1",
            "edit",
            serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
        ),
        openai_call("c2", "done", serde_json::json!({"summary": "changed it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_STEER_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[control]\nmode = \"approve\"\n\n[[providers]]\nname = \"stub\"\n\
             protocol = \"openai\"\nbase_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:stub#m"}),
    );
    // Paused on the edit's question, after the first request.
    let q = rpc.next(|v| v["type"] == "approval_request");
    let steer = "STEER-7f3 the checks passed, call done";
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": steer}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let s = rpc.next(|v| v["type"] == "response" && v["id"] == "s");
    assert_eq!(s["command"], "steer");
    assert_eq!(s["success"], true, "{s}");
    assert_eq!(s["data"]["after_step"], 1, "{s}");
    let end = rpc.next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p");
    // The model called `done` on the steer's word, with no check run: not
    // converged, and rolled back.
    assert_eq!(end["outcome"], "not_converged", "{end}");
    rpc.finish();
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n"
    );

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(!requests[0].contains("STEER-7f3"), "{}", requests[0]);
    assert!(
        requests[1].contains(steer),
        "the request after the steer carries it: {}",
        requests[1]
    );
}

#[test]
fn rpc_steer_and_follow_up_are_refused_with_no_run_in_flight() {
    // The risk: text accepted with nothing to deliver it to, and the
    // client told it was taken.
    let f = fixture("rpc-steer-idle");
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": "look at g"}));
    rpc.send(serde_json::json!({"id": "u", "type": "follow_up", "message": "then g"}));
    let rs = rpc.finish();
    for (id, command) in [("s", "steer"), ("u", "follow_up")] {
        let r = rs.iter().find(|r| r["id"] == id).expect("answered");
        assert_eq!(r["type"], "response");
        assert_eq!(r["command"], command);
        assert_eq!(r["success"], false, "{r}");
        assert_eq!(r["error"], "no run in flight", "{r}");
    }
    assert!(
        !rs.iter().any(|r| r["type"] == "run_start"),
        "nothing ran: {rs:?}"
    );
}

#[test]
fn rpc_steer_follow_ups_start_as_new_prompts_after_run_end_in_order() {
    // The risks: a follow-up delivered into the run it was sent during,
    // where it would redirect work already in progress; follow-ups run out
    // of order; or dropped when input ends.
    let f = fixture("rpc-steer-follow");
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": f.spec}),
    );
    rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "u1", "type": "follow_up", "message": "second task"}));
    rpc.send(serde_json::json!({"id": "u2", "type": "follow_up", "message": "third task"}));
    let u1 = rpc.next(|v| v["type"] == "response" && v["id"] == "u1");
    assert_eq!(u1["success"], true, "{u1}");
    // End of input declines every question, this run's and the follow-ups'
    // (ADR 0025 §5), and the chain still runs to its end.
    let rs = rpc.finish();
    let frames: Vec<(String, String)> = rs
        .iter()
        .filter(|r| r["type"] == "run_start" || r["type"] == "run_end")
        .map(|r| {
            (
                r["type"].as_str().unwrap().to_string(),
                r["id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = [
        ("run_start", "p"),
        ("run_end", "p"),
        ("run_start", "u1"),
        ("run_end", "u1"),
        ("run_start", "u2"),
        ("run_end", "u2"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(frames, want, "{rs:?}");
    let task = |id: &str| {
        rs.iter()
            .find(|r| r["type"] == "run_start" && r["id"] == id)
            .unwrap()["task"]
            .clone()
    };
    assert_eq!(task("p"), "change f");
    assert_eq!(task("u1"), "second task");
    assert_eq!(task("u2"), "third task");
}

#[test]
fn stub_answers_get_models_with_404() {
    // The risk: a run's identification read of a loopback base (ADR 0047
    // decision 3) took the stub's first canned reply, so the stub passed for
    // a runtime and every later reply went to the wrong request. It must be
    // answered as a server that is not colibri answers it, and not recorded.
    fn send(base: &str, request: &str) -> String {
        use std::io::Read;
        let mut s = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).ok();
        out
    }
    let (base, server) = provider_stub(vec!["first".to_string()]);
    let got = send(&base, "GET /v1/models HTTP/1.1\r\nHost: stub\r\n\r\n");
    assert!(got.starts_with("HTTP/1.1 404 "), "{got}");
    let body = got.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    assert!(
        serde_json::from_str::<Value>(body).is_ok_and(|v| v["error"].is_object()),
        "{got}"
    );
    let got = send(
        &base,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: stub\r\nContent-Length: 2\r\n\r\n{}",
    );
    assert!(
        got.starts_with("HTTP/1.1 200 ") && got.ends_with("\r\n\r\nfirst"),
        "{got}"
    );
    assert_eq!(server.join().unwrap(), vec!["{}".to_string()]);
}
