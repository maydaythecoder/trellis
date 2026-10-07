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
fn rpc_a_backward_clock_step_still_reports_the_run_id_of_the_run() {
    // A trail's name is `<wall-clock stamp>-<slug>`, so after the clock
    // steps back the next run's trail sorts below an older one. Staged as
    // #472's queue test stages it: run once, rename that trail to a stamp in
    // the future, commit, then run over rpc (#475).
    let f = fixture("rpc-clock-step");
    run(&f, &["run", "change f", "--model", &f.spec]);
    let runs = f.root.join(".trellis/runs");
    let first = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(|e| {
            e.unwrap()
                .file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .next()
        .unwrap();
    let ahead = format!("9{}", &first[1..]);
    std::fs::rename(
        runs.join(format!("{first}.json")),
        runs.join(format!("{ahead}.json")),
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(
        &f.root,
        &[
            "commit",
            "-qm",
            "a trail stamped before the clock stepped back",
        ],
    );

    let mut child = trellis(&f)
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
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
    let end = lines.iter().find(|l| l["type"] == "run_end").unwrap();
    let id = end["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no run_id: {end}"));
    assert_ne!(id, ahead, "the id of the run before the step: {end}");
    assert!(runs.join(format!("{id}.json")).is_file(), "{end}");
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
    // What the desktop shows a question by (ADR 0048 §6).
    let gaps = missing(
        &q,
        &contract()["rpc"]["framing"]["approval_request"]["always"],
        "approval_request",
    );
    assert!(gaps.is_empty(), "{gaps:?}: {q}");
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

    /// Waits, bounded, until `get_state` says no run is in flight. `run_end`
    /// is framed from inside the run's thread, which still has its tail to
    /// do. A `prompt` sent on reading it waits that out (#495); this is for
    /// a test that wants the server idle before it goes on.
    fn wait_idle(&mut self) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        for i in 0.. {
            let id = format!("idle-{i}");
            self.send(serde_json::json!({"id": id, "type": "get_state"}));
            let s = self.next(|v| v["id"] == id.as_str());
            if s["data"]["running"] == false {
                return;
            }
            assert!(std::time::Instant::now() < until, "still running: {s}");
            std::thread::sleep(std::time::Duration::from_millis(20));
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
    let records = rpc.finish();
    // The event the desktop marks a delivered steer by carries what
    // tests/client_contract.json lists (ADR 0048 §6).
    let delivered = records
        .iter()
        .find(|r| r["fields"]["message"] == "steer delivered")
        .unwrap_or_else(|| panic!("no steer delivered event: {records:?}"));
    let gaps = missing(
        &delivered["fields"],
        &contract()["rpc"]["events"]["steer delivered"]["always"],
        "steer delivered",
    );
    assert!(gaps.is_empty(), "{gaps:?}: {delivered}");
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
fn rpc_a_prompt_right_after_run_end_is_accepted_and_runs() {
    // The risk: `run_end` is framed from inside the run's thread, which
    // still has its tail to do, and a client that sends the next `prompt`
    // on reading it (as ADR 0025 §5's "run_end says how it finished"
    // invites, and the desktop does) was refused as a run in progress.
    // The seam holds that tail open, as a loaded machine does.
    let f = fixture("rpc-prompt-after-end");
    let mut cmd = trellis(&f);
    cmd.env("TRELLIS_TEST_RPC_CLEANUP_DELAY_MS", "1500");
    let mut rpc = Rpc::start(cmd);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": f.spec}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    rpc.send(
        serde_json::json!({"id": "q", "type": "prompt", "task": "change f again",
                                "model": f.spec}),
    );
    let q = rpc.next(|v| v["type"] == "response" && v["id"] == "q");
    assert_eq!(q["success"], true, "{q}");
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "q");
    assert_eq!(end["outcome"], "not_converged", "{end}");
    let rs = rpc.finish();
    let frames: Vec<(&str, &str)> = rs
        .iter()
        .filter(|r| r["type"] == "run_start" || r["type"] == "run_end")
        .map(|r| (r["type"].as_str().unwrap(), r["id"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(
        frames,
        [
            ("run_start", "p"),
            ("run_end", "p"),
            ("run_start", "q"),
            ("run_end", "q"),
        ],
        "{rs:?}"
    );
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

/// One module as `trellis index --graph` draws it: name, definitions,
/// ambiguous, unresolved, and its edges as (callee, call sites).
type DrawnModule = (String, u64, u64, u64, Vec<(String, u64)>);

/// The header counts and modules `trellis index --graph` printed.
fn drawn_graph(stdout: &str) -> ((u64, u64, u64, u64), Vec<DrawnModule>) {
    let mut lines = stdout
        .lines()
        .skip_while(|l| !l.starts_with("codebase graph — "));
    let header = lines.next().expect("a graph header");
    let n = |s: &str| s.trim().parse::<u64>().unwrap();
    let indexed = n(header
        .trim_start_matches("codebase graph — ")
        .trim_end_matches(" module(s)"));
    let counts: Vec<u64> = lines
        .next()
        .unwrap()
        .split(',')
        .map(|part| n(part.split_whitespace().next().unwrap()))
        .collect();
    let mut modules: Vec<DrawnModule> = Vec::new();
    for l in lines {
        if let Some(node) = l.strip_prefix("  ● ").or_else(|| l.strip_prefix("  ○ ")) {
            // The legend under the graph starts with the same glyphs and
            // has no ` def`.
            let Some((left, doubt)) = node.split_once(" def") else {
                continue;
            };
            let (name, defines) = left.trim().rsplit_once(' ').unwrap();
            let mark = |m: char| {
                doubt
                    .split_whitespace()
                    .find_map(|t| t.strip_prefix(m))
                    .map_or(0, n)
            };
            modules.push((
                name.trim().to_string(),
                n(defines),
                mark('~'),
                mark('?'),
                Vec::new(),
            ));
        } else if let Some(edge) = l
            .strip_prefix("    ├─▶ ")
            .or_else(|| l.strip_prefix("    └─▶ "))
        {
            let (callee, count) = edge.rsplit_once("  ×").unwrap();
            modules
                .last_mut()
                .unwrap()
                .4
                .push((callee.to_string(), n(count)));
        }
    }
    ((indexed, counts[0], counts[1], counts[2]), modules)
}

/// Every key in `v`, at any depth, for the no-ratio checks (CLAUDE.md rule 3).
fn keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                out.push(k.clone());
                keys(v, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| keys(v, out)),
        _ => {}
    }
}

#[test]
fn rpc_get_codebase_returns_the_graph_codebase_lines_prints() {
    // The risks: a client parsing human text because the graph has no data
    // form (ADR 0048 §2); coverage arriving as a ratio (CLAUDE.md rule 3);
    // a read that writes the index cache or the worktree; an index build
    // blocking the reader while a run waits on an approval; and data that
    // drifts from what `trellis index --graph` draws.
    let f = fixture("rpc-codebase");
    std::fs::write(
        f.root.join("b.py"),
        "from a import f\n\ndef g():\n    return f() + f()\n",
    )
    .unwrap();
    std::fs::write(
        f.root.join("c.py"),
        "from b import g\n\ndef h():\n    return g() + missing()\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "modules that call each other"]);

    // Alone first: a run would write its trail under `.trellis/runs/`.
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "g", "type": "get_codebase"}));
    let got = rpc.next(|v| v["id"] == "g");
    rpc.finish();
    assert_eq!(got["command"], "get_codebase", "{got}");
    assert_eq!(got["success"], true, "{got}");
    let data = got["data"].clone();
    let mut all = Vec::new();
    keys(&data, &mut all);
    for k in &all {
        for banned in ["ratio", "percent", "fraction", "total"] {
            assert!(!k.contains(banned), "key `{k}` in {data}");
        }
    }
    assert!(
        !f.root.join(".trellis/index.bin").exists(),
        "get_codebase wrote the index cache"
    );
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");

    // Answered while an approve-mode run waits on its question.
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let q = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "g2", "type": "get_codebase", "limit": 40}));
    let mid = rpc.next(|v| v["id"] == "g2" || v["type"] == "run_end");
    assert_eq!(mid["id"], "g2", "answered before the run ended: {mid}");
    assert_eq!(mid["success"], true, "{mid}");
    assert_eq!(mid["data"], data, "the same graph mid-run");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": false}));
    rpc.next(|v| v["type"] == "run_end");
    rpc.finish();

    // Last: `index` writes the cache, which is its job.
    let o = run(&f, &["index", "--graph"]);
    assert!(o.status.success());
    let (header, drawn) = drawn_graph(&String::from_utf8_lossy(&o.stdout));
    let u = |v: &Value| v.as_u64().unwrap();
    assert_eq!(
        header,
        (
            u(&data["modules_indexed"]),
            u(&data["bound_call_sites"]),
            u(&data["ambiguous"]),
            u(&data["unresolved"])
        ),
        "{data}"
    );
    let answered: Vec<DrawnModule> = data["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            assert_eq!(m["calls_not_drawn"], 0, "{m}");
            (
                m["module"].as_str().unwrap().to_string(),
                u(&m["defines"]),
                u(&m["ambiguous"]),
                u(&m["unresolved"]),
                m["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        (
                            c["module"].as_str().unwrap().to_string(),
                            u(&c["call_sites"]),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(answered, drawn, "{data}");
    assert_eq!(drawn.len(), 3, "a, b and c: {drawn:?}");
    assert!(
        drawn.iter().map(|m| m.4.len()).sum::<usize>() >= 2,
        "the fixture's calls bound across modules: {drawn:?}"
    );
    assert_eq!(data["modules_not_drawn"], 0, "{data}");
}

/// What `trellis radius <symbol>` printed, in `get_radius`'s `data` shape,
/// read off the text: the reach block, then the doubt block with its reason
/// rows and the files it is concentrated in.
fn printed_radius(stdout: &str) -> Value {
    let mut lines = stdout.lines();
    let (symbol, file) = lines.next().unwrap().split_once("  (").unwrap();
    let n = |s: &str| s.trim().parse::<u64>().unwrap();
    let first = |l: &str| n(l.split_whitespace().next().unwrap());
    let mut lines = lines.skip_while(|l| !l.starts_with("reach — "));
    lines.next();
    let reach: Vec<u64> = lines.by_ref().take(3).map(first).collect();
    let mut lines = lines.skip_while(|l| !l.starts_with("doubt — "));
    lines.next();
    let head = lines.next().unwrap();
    let touched = n(head
        .split_once(" the ")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap());
    let (mut reasons, mut files) = (Vec::new(), Vec::new());
    let mut in_files = false;
    for l in lines.take_while(|l| !l.starts_with("  partitionable:")) {
        let t = l.trim();
        if t.is_empty() || t.starts_with("Any of them") {
            continue;
        }
        assert!(!t.contains("further"), "the fixture fits uncollapsed: {l}");
        if t == "concentrated in:" {
            in_files = true;
        } else if in_files {
            let (count, path) = t.split_once("  ").unwrap();
            files.push(serde_json::json!({"path": path.trim(), "call_sites": n(count)}));
        } else {
            let (reason, rest) = t.split_once(' ').unwrap();
            let (count, explain) = rest.trim_start().split_once(' ').unwrap();
            reasons.push(serde_json::json!({
                "reason": reason,
                "call_sites": n(count),
                "explain": explain.trim(),
            }));
        }
    }
    serde_json::json!({
        "symbol": symbol,
        "file": file.trim_end_matches(')'),
        "reach": {
            "resolved": reach[0],
            "ambiguous": reach[1],
            "symbols_reached": reach[2],
        },
        "doubt": {
            "unbound_in_touched_files": first(head),
            "touched_files": touched,
            "reasons": reasons,
            "files": files,
        },
    })
}

#[test]
fn rpc_get_radius_returns_reach_and_doubt_as_separate_counts() {
    // The risks: a client parsing human text because a radius has no data
    // form (ADR 0048 §2); reach and doubt arriving as one ratio, which once
    // read as a resolver twenty times worse than audited (ADR 0014,
    // CLAUDE.md rule 3); a read that writes the index cache or the worktree;
    // an index build blocking the reader while a run waits on an approval;
    // and data that drifts from what `trellis radius` prints.
    let f = fixture("rpc-radius");
    std::fs::write(
        f.root.join("b.py"),
        "from a import f\n\ndef g(x):\n    return f() + f() + x.go()\n",
    )
    .unwrap();
    std::fs::write(
        f.root.join("c.py"),
        "from b import g\n\ndef h(y):\n    return g(y) + missing() + y.run()\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "callers, some unbindable"]);

    // Alone first: a run would write its trail under `.trellis/runs/`.
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "r", "type": "get_radius", "symbol": "f"}));
    let got = rpc.next(|v| v["id"] == "r");
    rpc.finish();
    assert_eq!(got["command"], "get_radius", "{got}");
    assert_eq!(got["success"], true, "{got}");
    let data = got["data"].clone();
    let mut all = Vec::new();
    keys(&data, &mut all);
    for k in &all {
        for banned in ["ratio", "percent", "fraction", "total"] {
            assert!(!k.contains(banned), "key `{k}` in {data}");
        }
    }
    assert!(
        !f.root.join(".trellis/index.bin").exists(),
        "get_radius wrote the index cache"
    );
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");

    // Answered while an approve-mode run waits on its question.
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let q = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "r2", "type": "get_radius", "symbol": "a.f"}));
    let mid = rpc.next(|v| v["id"] == "r2" || v["type"] == "run_end");
    assert_eq!(mid["id"], "r2", "answered before the run ended: {mid}");
    assert_eq!(mid["success"], true, "{mid}");
    assert_eq!(mid["data"], data, "the same radius mid-run");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": false}));
    rpc.next(|v| v["type"] == "run_end");
    rpc.finish();

    // Last: `radius` builds through the cache, which writes it.
    let o = run(&f, &["radius", "f"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        printed_radius(&String::from_utf8_lossy(&o.stdout)),
        data,
        "the counts `trellis radius f` prints"
    );
    // Both blocks carry something, so the comparison is not of zeros.
    assert_eq!(data["reach"]["resolved"], 3, "{data}");
    assert_eq!(
        data["doubt"]["files"].as_array().unwrap().len(),
        2,
        "{data}"
    );
}

/// Every file under `dir`, by path relative to it, with its bytes.
fn files_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(p.strip_prefix(dir).unwrap().to_path_buf(), bytes);
            }
        }
    }
    out
}

/// The id `trellis queue add` printed on its `queued:` line.
fn queued(o: &Output) -> String {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    let line = out.lines().find(|l| l.starts_with("queued:")).unwrap();
    line.split_whitespace().nth(1).unwrap().to_string()
}

/// What `trellis queue list` printed, in `get_queue`'s task shape, read off
/// the text: each entry's head line, its task line and any `waiting:` line,
/// up to the approvals block, which is not a task.
fn printed_queue(stdout: &str) -> Vec<Value> {
    let mut tasks: Vec<Value> = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() || line == "approvals waiting:" {
            break;
        }
        if let Some(rest) = line.strip_prefix("          ") {
            let t = tasks.last_mut().unwrap();
            if let Some(why) = rest.strip_prefix("waiting: ") {
                t["waiting"] = Value::from(why);
                continue;
            }
            let (text, tags) = rest.split_once(" [").map_or((rest, ""), |(a, b)| (a, b));
            t["task"] = Value::from(text);
            let tag = |name: &str| {
                tags.split(" [")
                    .find_map(|s| s.strip_prefix(name))
                    .map(|s| s.trim_end_matches(']').to_string())
            };
            t["labels"] = tag("labels ")
                .map(|l| l.split(',').map(Value::from).collect())
                .unwrap_or_else(|| Value::Array(Vec::new()));
            t["assignee"] = tag("agent ").map_or(Value::Null, Value::from);
            continue;
        }
        let mut head = line.split_whitespace();
        let (state, id) = (head.next().unwrap(), head.next().unwrap());
        let rest = line.split_once(id).unwrap().1.trim_start();
        if let Some(why) = rest.strip_prefix("unreadable: ") {
            tasks.push(serde_json::json!({"id": id, "state": state, "error": why}));
            continue;
        }
        let mut words = rest.split_whitespace();
        tasks.push(serde_json::json!({
            "id": id,
            "state": state,
            "status": words.next().unwrap(),
            "priority": words.next().unwrap(),
            "waiting": null,
        }));
    }
    tasks
}

#[test]
fn rpc_get_queue_lists_what_queue_list_prints() {
    // The risks: a client parsing `queue list`'s text because the queue has
    // no data form (ADR 0048 §2); a read that writes, claims or moves a task
    // (the queue's writes are `trellis queue`'s and `trellis work`'s, ADR
    // 0020 §5), which `git status` cannot see, since the queue lives in the
    // git directory; a read blocking the reader while a run waits on an
    // approval; a `quit` that exits before the answer is written; and data
    // that drifts from what `trellis queue list` prints.
    let f = fixture("rpc-queue");
    let blocker = queued(&run(
        &f,
        &["queue", "add", "fix the parser", "--priority", "high"],
    ));
    queued(&run(
        &f,
        &[
            "queue",
            "add",
            "then the docs",
            "--label",
            "docs",
            "--label",
            "later",
            "--blocked-by",
            &blocker,
        ],
    ));
    queued(&run(
        &f,
        &["queue", "add", "some day", "--backlog", "--priority", "low"],
    ));
    let queue_dir = trellis_vcs::git_dir(&f.root).unwrap().join("trellis");
    std::fs::write(
        queue_dir.join("queue/pending/20260101-000000-000000000-broken.json"),
        "{ not a task",
    )
    .unwrap();
    let before = files_under(&queue_dir);

    // Alone first: a run would write its trail under `.trellis/runs/`.
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "q", "type": "get_queue"}));
    let got = rpc.next(|v| v["id"] == "q");
    rpc.finish();
    assert_eq!(got["command"], "get_queue", "{got}");
    assert_eq!(got["success"], true, "{got}");
    let data = got["data"].clone();
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");
    let after = files_under(&queue_dir);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "the files under the git directory's trellis/"
    );
    for (path, bytes) in &before {
        assert!(after[path] == *bytes, "{} changed", path.display());
    }

    // Answered while an approve-mode run waits on its question.
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let q = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "q2", "type": "get_queue"}));
    let mid = rpc.next(|v| v["id"] == "q2" || v["type"] == "run_end");
    assert_eq!(mid["id"], "q2", "answered before the run ended: {mid}");
    assert_eq!(mid["success"], true, "{mid}");
    assert_eq!(mid["data"], data, "the same queue mid-run");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": false}));
    rpc.next(|v| v["type"] == "run_end");
    rpc.finish();

    // A `quit` on the next line waits for the answer before its own.
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "q3", "type": "get_queue"}));
    rpc.send(serde_json::json!({"id": "x", "type": "quit"}));
    let first = rpc.next(|v| v["id"] == "q3" || v["id"] == "x");
    assert_eq!(first["id"], "q3", "answered before `quit`: {first}");
    assert_eq!(first["success"], true, "{first}");
    rpc.next(|v| v["id"] == "x");
    rpc.finish();

    // Last: what `trellis queue list` prints, task by task in its order.
    let o = run(&f, &["queue", "list"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let printed = printed_queue(&String::from_utf8_lossy(&o.stdout));
    let answered = data["tasks"].as_array().unwrap();
    assert_eq!(answered.len(), printed.len(), "{data}");
    for (a, p) in answered.iter().zip(&printed) {
        assert_eq!(a, p, "a task as `queue list` prints it");
    }
    // Each kind the fixture set up is in the comparison, so it is not of
    // empty fields.
    assert_eq!(printed.len(), 4, "{printed:?}");
    assert!(
        printed.iter().any(|t| t["error"].is_string()),
        "{printed:?}"
    );
    assert!(
        printed
            .iter()
            .any(|t| t["labels"].as_array().is_some_and(|l| l.len() == 2))
    );
    assert!(
        printed.iter().any(|t| t["waiting"] == "in backlog"),
        "{printed:?}"
    );
    assert!(
        printed
            .iter()
            .any(|t| t["status"] == "blocked" && t["waiting"].as_str().is_some()),
        "{printed:?}"
    );
}

/// A repository whose committed `[context] vault` holds a parent, its
/// child, and a note under a missing parent that links both ways with the
/// child. A link along a parent edge is not a reference (`Graph::build`),
/// so the links are between the child and the other note.
fn vault_fixture(name: &str) -> Fixture {
    let f = fixture(name);
    let notes = f.root.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    for (file, body) in [
        (
            "root.md",
            "---\ntitle: \"Root\"\nparent: null\n---\nOverview.\n",
        ),
        (
            "child.md",
            "---\ntitle: \"Child\"\nparent: \"[[root]]\"\n---\nThe retry policy doubles. See [[orphan]], and [[orphan]] again.\n",
        ),
        (
            "orphan.md",
            "---\ntitle: \"Orphan\"\nparent: \"[[gone]]\"\n---\nA lost retry note. Back to [[child]].\n",
        ),
        (
            "manifest.json",
            "[\"child.md\",\"orphan.md\",\"root.md\"]\n",
        ),
    ] {
        std::fs::write(notes.join(file), body).unwrap();
    }
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[context]\nvault = \"notes\"\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "a vault"]);
    f
}

/// Sends the read-only `cmd` three times, as `get_queue` is held: alone,
/// then the worktree must be clean; while an approve-mode run waits on its
/// question, answered before that run's `run_end`, and the run declined;
/// and followed by `quit` on the next line, answered first. Returns the
/// first answer's `data`, which the other two must equal.
fn answered_three_ways(f: &Fixture, cmd: &Value) -> Value {
    let with_id = |id: &str| {
        let mut c = cmd.clone();
        c["id"] = Value::from(id);
        c
    };
    // Alone first: a run would write its trail under `.trellis/runs/`.
    let mut rpc = Rpc::start(trellis(f));
    rpc.send(with_id("a"));
    let got = rpc.next(|v| v["id"] == "a");
    rpc.finish();
    assert_eq!(got["command"], cmd["type"], "{got}");
    assert_eq!(got["success"], true, "{got}");
    let data = got["data"].clone();
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");

    // Answered while an approve-mode run waits on its question.
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let q = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(with_id("b"));
    let mid = rpc.next(|v| v["id"] == "b" || v["type"] == "run_end");
    assert_eq!(mid["id"], "b", "answered before the run ended: {mid}");
    assert_eq!(mid["success"], true, "{mid}");
    assert_eq!(mid["data"], data, "the same answer mid-run");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": false}));
    rpc.next(|v| v["type"] == "run_end");
    rpc.finish();

    // A `quit` on the next line waits for the answer before its own.
    let mut rpc = Rpc::start(trellis(f));
    rpc.send(with_id("c"));
    rpc.send(serde_json::json!({"id": "x", "type": "quit"}));
    let first = rpc.next(|v| v["id"] == "c" || v["id"] == "x");
    assert_eq!(first["id"], "c", "answered before `quit`: {first}");
    assert_eq!(first["success"], true, "{first}");
    assert_eq!(first["data"], data, "the same answer before `quit`");
    rpc.next(|v| v["id"] == "x");
    rpc.finish();
    data
}

/// One rpc answer, alone, for the error cases.
fn answer(f: &Fixture, cmd: Value) -> Value {
    let mut rpc = Rpc::start(trellis(f));
    rpc.send(cmd);
    let got = rpc.next(|v| v["type"] == "response");
    rpc.finish();
    got
}

/// The first line `trellis <args>` printed on stderr, with the `Error: `
/// the binary puts before a failure's message taken off.
fn cli_error(f: &Fixture, args: &[&str]) -> String {
    let o = run(f, args);
    assert!(!o.status.success(), "{args:?} succeeded");
    let err = String::from_utf8_lossy(&o.stderr);
    err.lines()
        .find_map(|l| l.strip_prefix("Error: "))
        .unwrap_or_else(|| panic!("no `Error:` line in {err}"))
        .to_string()
}

/// What `trellis vault search` printed, in `vault_search`'s shape, scores
/// as the four decimals it prints.
fn printed_search(stdout: &str) -> Value {
    let titles_only = stdout.lines().any(|l| l.starts_with("search reduced"));
    let fuzzy_off = stdout
        .lines()
        .any(|l| l.starts_with("fuzzy search reduced"));
    let hits: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.starts_with("search reduced") && !l.starts_with("fuzzy search reduced"))
        .filter(|l| !l.starts_with("no matches for"))
        .map(|l| {
            let (score, rest) = l.trim_start().split_once("  ").unwrap();
            let (path, title) = rest.split_once("  ").unwrap();
            serde_json::json!({
                "path": path,
                "title": title.trim_matches('"'),
                "score": score,
            })
        })
        .collect();
    // Titles-only drops trigrams too (`Mode::for_vault`), though `search`
    // then prints only the first of its two lines.
    serde_json::json!({
        "hits": hits,
        "mode": {"body": !titles_only, "trigrams": !(titles_only || fuzzy_off)},
    })
}

/// `vault_search`'s answer with each score cut to the four decimals
/// `trellis vault search` prints.
fn scores_as_printed(data: &Value) -> Value {
    let mut d = data.clone();
    for h in d["hits"].as_array_mut().unwrap() {
        h["score"] = Value::from(format!("{:.4}", h["score"].as_f64().unwrap()));
    }
    d
}

#[test]
fn rpc_vault_search_returns_what_vault_search_prints() {
    // The risks: a client parsing `vault search`'s text because it has no
    // data form (ADR 0048 §2); a read that writes the vault (every vault
    // write is a `trellis vault` command, #70); a search blocking the reader
    // while a run waits on an approval; a `quit` that exits before the
    // answer; and an rpc ranking that drifts from the CLI's.
    let f = vault_fixture("rpc-vault-search");
    let data = answered_three_ways(
        &f,
        &serde_json::json!({"type": "vault_search", "query": "retry"}),
    );

    // Last: what `trellis vault search retry` prints, hit by hit.
    let o = run(&f, &["vault", "search", "retry"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        scores_as_printed(&data),
        printed_search(&String::from_utf8_lossy(&o.stdout)),
        "the hits `trellis vault search retry` prints"
    );
    // Two hits, so the comparison is of an order and not of nothing.
    assert_eq!(data["hits"].as_array().unwrap().len(), 2, "{data}");

    // `limit` is the CLI's `--limit`; no match is an empty list.
    let one = answer(
        &f,
        serde_json::json!({"id": "l", "type": "vault_search", "query": "retry", "limit": 1}),
    );
    let o = run(&f, &["vault", "search", "retry", "--limit", "1"]);
    assert_eq!(
        scores_as_printed(&one["data"]),
        printed_search(&String::from_utf8_lossy(&o.stdout))
    );
    let none = answer(
        &f,
        serde_json::json!({"id": "n", "type": "vault_search", "query": "zzqqxx"}),
    );
    assert_eq!(none["success"], true, "{none}");
    assert_eq!(none["data"]["hits"], serde_json::json!([]), "{none}");

    // No vault configured: the CLI's message.
    let bare = fixture("rpc-vault-search-bare");
    let got = answer(
        &bare,
        serde_json::json!({"id": "e", "type": "vault_search", "query": "retry"}),
    );
    assert_eq!(got["success"], false, "{got}");
    assert_eq!(
        got["error"],
        cli_error(&bare, &["vault", "search", "retry"]),
        "{got}"
    );
}

/// `path "title"`, as `vault graph` labels a note.
fn printed_label(s: &str) -> Value {
    let (path, title) = s.split_once(" \"").unwrap();
    serde_json::json!({"path": path, "title": title.trim_end_matches('"')})
}

/// What `trellis vault graph` printed, in `vault_graph`'s shape.
fn printed_graph(stdout: &str) -> Value {
    let mut d = serde_json::json!({
        "unresolved_parent": null,
        "children": [], "refs_out": [], "refs_in": [], "issues": [],
    });
    let mut section = "";
    for line in stdout.lines() {
        if let Some(item) = line.strip_prefix("  ") {
            let entry = match section {
                "children" => printed_label(item),
                "issues" => Value::from(item),
                _ => {
                    let (label, w) = item.rsplit_once(" x").unwrap();
                    let mut e = printed_label(label);
                    e["weight"] = Value::from(w.parse::<u64>().unwrap());
                    e
                }
            };
            d[section].as_array_mut().unwrap().push(entry);
            continue;
        }
        let (head, rest) = line.split_once(':').unwrap();
        let rest = rest.trim_start();
        section = "";
        match head {
            "note" => d["note"] = printed_label(rest),
            "depth" => {
                d["depth"] = rest.parse::<u64>().map_or(Value::Null, Value::from);
            }
            "lineage" => d["lineage"] = rest.split(" > ").map(Value::from).collect(),
            "parent" => {
                let t = rest.strip_suffix(" does not resolve").unwrap();
                d["unresolved_parent"] = Value::from(t.trim_matches(['[', ']']));
            }
            "children" => section = "children",
            "refs out" => section = "refs_out",
            "refs in" => section = "refs_in",
            "issues" => section = "issues",
            other => panic!("a line `vault graph` does not print: {other}"),
        }
    }
    d
}

#[test]
fn rpc_vault_graph_returns_what_vault_graph_prints() {
    // The risks: a client parsing `vault graph`'s text because it has no
    // data form (ADR 0048 §2); a read that writes the vault (every vault
    // write is a `trellis vault` command, #70); a graph build blocking the
    // reader while a run waits on an approval; a `quit` that exits before
    // the answer; and data that drifts from what `trellis vault graph`
    // prints.
    let f = vault_fixture("rpc-vault-graph");
    let data = answered_three_ways(
        &f,
        &serde_json::json!({"type": "vault_graph", "note": "child"}),
    );

    // Last: what `trellis vault graph child` prints.
    let o = run(&f, &["vault", "graph", "child"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        data,
        printed_graph(&String::from_utf8_lossy(&o.stdout)),
        "what `trellis vault graph child` prints"
    );
    // The parent, the child and the links both ways are in the comparison.
    assert_eq!(data["depth"], 1, "{data}");
    assert_eq!(data["lineage"], serde_json::json!(["root.md", "child.md"]));
    assert_eq!(data["refs_out"][0]["weight"], 2, "{data}");
    assert_eq!(data["refs_in"][0]["path"], "orphan.md", "{data}");

    // A note under a missing parent: no depth, the target that does not
    // resolve, and its issue.
    let orphan = answer(
        &f,
        serde_json::json!({"id": "o", "type": "vault_graph", "note": "orphan"}),
    );
    assert_eq!(orphan["success"], true, "{orphan}");
    let o = run(&f, &["vault", "graph", "orphan"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        orphan["data"],
        printed_graph(&String::from_utf8_lossy(&o.stdout))
    );
    assert_eq!(orphan["data"]["depth"], Value::Null, "{orphan}");
    assert_eq!(orphan["data"]["unresolved_parent"], "gone", "{orphan}");
    assert!(
        !orphan["data"]["issues"].as_array().unwrap().is_empty(),
        "{orphan}"
    );

    // A missing note, and no vault configured: the CLI's message.
    let got = answer(
        &f,
        serde_json::json!({"id": "m", "type": "vault_graph", "note": "nowhere"}),
    );
    assert_eq!(got["success"], false, "{got}");
    assert_eq!(
        got["error"],
        cli_error(&f, &["vault", "graph", "nowhere"]),
        "{got}"
    );
    let bare = fixture("rpc-vault-graph-bare");
    let got = answer(
        &bare,
        serde_json::json!({"id": "e", "type": "vault_graph", "note": "child"}),
    );
    assert_eq!(got["success"], false, "{got}");
    assert_eq!(
        got["error"],
        cli_error(&bare, &["vault", "graph", "child"]),
        "{got}"
    );
}

/// An OpenAI-protocol tool call that reports both token figures.
fn openai_call_with_usage(name: &str, args: Value, prompt: u64, completion: u64) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"c","type":"function","function":{"name":name,
        "arguments":args.to_string()}}]}}],
        "usage":{"prompt_tokens":prompt,"completion_tokens":completion}})
    .to_string()
}

/// Each trail under `.trellis/runs`, as JSON.
fn trails(f: &Fixture) -> Vec<Value> {
    std::fs::read_dir(f.root.join(".trellis/runs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect()
}

#[test]
fn rpc_get_session_stats_counts_each_model_s_tokens_from_the_trails_apart() {
    // The risks: prompt and completion added into one figure, which bills
    // context as output; a run that reported no figure counted as 0, which
    // reads as an exact total that is not one; and a second count beside
    // `/status`'s, which could disagree with what the trails record.
    let f = fixture("rpc-stats");
    let edit = || serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"});
    let done = || serde_json::json!({"summary": "changed it"});
    let (base, server) = provider_stub(vec![
        openai_call_with_usage("edit", edit(), 100, 3),
        openai_call_with_usage("done", done(), 150, 4),
        openai_call_with_usage("edit", edit(), 1000, 30),
        openai_call_with_usage("done", done(), 1500, 40),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_STATS_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"openai\"\n\
             base_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);

    rpc.send(serde_json::json!({"id": "s0", "type": "get_session_stats"}));
    let before = rpc.next(|v| v["id"] == "s0");
    assert_eq!(before["command"], "get_session_stats", "{before}");
    assert_eq!(before["success"], true, "{before}");
    assert_eq!(
        before["data"],
        serde_json::json!({"models": [], "runs_not_counted": 0}),
        "no run yet: {before}"
    );

    // Two runs of the stub's model with a scripted run between them: the
    // stub's figures are summed across its runs, and the script, which
    // reports neither figure, is its own model with both unknown.
    for (id, model) in [
        ("p1", "provider:stub#m"),
        ("p2", f.spec.as_str()),
        ("p3", "provider:stub#m"),
    ] {
        rpc.send(
            serde_json::json!({"id": id, "type": "prompt", "task": "change f",
                                    "model": model}),
        );
        let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == id);
        assert!(end["run_id"].is_string(), "a trail was written: {end}");
        rpc.wait_idle();
    }
    rpc.send(serde_json::json!({"id": "s1", "type": "get_session_stats"}));
    let got = rpc.next(|v| v["id"] == "s1");
    rpc.finish();
    assert_eq!(server.join().unwrap().len(), 4);
    assert_eq!(got["success"], true, "{got}");

    // What the trails record, per model. The three runs land in the same
    // second, so `started_at` cannot order them (and a wall clock can step
    // back); the stub's model is the one two trails name, the script's the
    // one a single trail names.
    let runs = trails(&f);
    assert_eq!(runs.len(), 3);
    let model = |r: &serde_json::Value| r["provenance"]["model"].clone();
    assert!(runs.iter().all(|r| model(r).is_string()), "{runs:?}");
    let named = |m: &serde_json::Value| runs.iter().filter(|r| &model(r) == m).count();
    let stub_model = runs
        .iter()
        .map(model)
        .find(|m| named(m) == 2)
        .expect("two stub runs");
    let script_model = runs
        .iter()
        .map(model)
        .find(|m| named(m) == 1)
        .expect("one scripted run");
    assert_ne!(stub_model, script_model);
    let stub_runs: Vec<_> = runs.iter().filter(|r| model(r) == stub_model).collect();
    let sum = |field: &str| -> u64 {
        stub_runs
            .iter()
            .map(|r| r["factual"][field].as_u64().unwrap())
            .sum()
    };
    assert_eq!(
        (sum("prompt_tokens"), sum("tokens")),
        (2750, 77),
        "{runs:?}"
    );
    assert_eq!(
        got["data"],
        serde_json::json!({
            "models": [
                {"model": stub_model, "prompt_tokens": 2750, "completion_tokens": 77},
                {"model": script_model, "prompt_tokens": null, "completion_tokens": null},
            ],
            "runs_not_counted": 0,
        }),
        "{got}"
    );
    let mut names = Vec::new();
    keys(&got["data"], &mut names);
    for k in &names {
        let k = k.to_ascii_lowercase();
        assert!(
            !["total", "sum", "ratio", "percent"]
                .iter()
                .any(|w| k.contains(w)),
            "`{k}` names a figure that is not one the trails record"
        );
    }
}

#[test]
fn rpc_get_last_assistant_text_is_the_last_done_summary_as_data() {
    // The risks: a run that ended some other way (here an abort) replacing
    // the summary with nothing, which loses what `/copy` would still copy;
    // and model output reaching the client as anything but a JSON string,
    // where a terminal escape could act on whoever prints it.
    let f = fixture("rpc-last-text");
    let scripts = scratch("rpc-last-text-scripts");
    let script = |name: &str, summary: &str| -> String {
        let p = scripts.join(format!("{name}.json"));
        let steps = serde_json::json!({"steps": [
            {"tool": "edit", "path": "a.py", "old": "return 1", "new": "return 2"},
            {"tool": "done", "summary": summary},
        ]});
        std::fs::write(&p, steps.to_string()).unwrap();
        format!("scripted:{}", p.display())
    };
    // ESC, a C1 CSI and a line break: printed raw, the first two drive a
    // terminal.
    let hostile = "made it red \u{1b}[31m\u{9b}2J\nand cleared";
    let first = script("first", hostile);
    let later = script("later", "the later one");

    let mut rpc = Rpc::start(trellis(&f));
    let ask = |rpc: &mut Rpc, id: &str| -> Value {
        rpc.send(serde_json::json!({"id": id, "type": "get_last_assistant_text"}));
        let got = rpc.next(|v| v["id"] == id);
        assert_eq!(got["command"], "get_last_assistant_text", "{got}");
        assert_eq!(got["success"], true, "{got}");
        got["data"].clone()
    };
    assert_eq!(
        ask(&mut rpc, "t0"),
        serde_json::json!({"text": null}),
        "no run yet"
    );

    rpc.send(
        serde_json::json!({"id": "p1", "type": "prompt", "task": "change f",
                                "model": first}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p1");
    assert_eq!(
        ask(&mut rpc, "t1"),
        serde_json::json!({"text": hostile}),
        "the summary, byte for byte, as a string"
    );
    rpc.wait_idle();

    // Stopped at its question: it never reaches `done`.
    rpc.send(
        serde_json::json!({"id": "p2", "type": "prompt", "task": "change f",
                                "model": later, "control": "approve"}),
    );
    rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "a", "type": "abort"}));
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p2");
    assert_eq!(end["outcome"], "interrupted", "{end}");
    assert_eq!(
        ask(&mut rpc, "t2"),
        serde_json::json!({"text": hostile}),
        "an aborted run leaves the last summary as it was"
    );
    rpc.wait_idle();

    rpc.send(
        serde_json::json!({"id": "p3", "type": "prompt", "task": "change f",
                                "model": later}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p3");
    assert_eq!(
        ask(&mut rpc, "t3"),
        serde_json::json!({"text": "the later one"}),
        "the most recent run that ended on `done`"
    );
    rpc.finish();
}

/// The rpc and `log --json` fields a client may rely on (ADR 0048 §6), as
/// data the tests below hold the binary to.
fn contract() -> Value {
    serde_json::from_str(include_str!("client_contract.json"))
        .expect("client_contract.json is JSON")
}

/// The field names in one of the contract's lists.
fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("a contract list")
        .iter()
        .map(|n| n.as_str().expect("a field name"))
        .collect()
}

/// Each field of `fields` that `record` does not carry, as `<what>.<field>`.
/// A field present with `null` is carried.
fn missing(record: &Value, fields: &Value, what: &str) -> Vec<String> {
    names(fields)
        .into_iter()
        .filter(|f| record.get(f).is_none())
        .map(|f| format!("{what}.{f}"))
        .collect()
}

/// Whether `record` carries `path`: dotted keys, with `[]` meaning every
/// element of a non-empty array. A key present with `null` is carried.
fn carries(record: &Value, path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    let (key, each) = match head.strip_suffix("[]") {
        Some(k) => (k, true),
        None => (head, false),
    };
    let Some(v) = record.get(key) else {
        return false;
    };
    match (each, rest) {
        (false, None) => true,
        (false, Some(r)) => carries(v, r),
        (true, r) => v
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(|e| r.is_none_or(|r| carries(e, r)))),
    }
}

/// Commits a `.trellis/config.toml` whose checks pass on Windows and Linux,
/// as `cli.rs`'s fixtures do, so a run that calls `run_checks` converges
/// without depending on the machine's toolchain.
fn commit_passing_checks(f: &Fixture) {
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "checks"]);
}

#[test]
fn rpc_stream_carries_every_field_the_desktop_reads() {
    // The risk: a field the desktop's run panes read is renamed or dropped
    // in trellis-loop or the framing. The TUI reads the same events in
    // process, so its tests would pass and the desktop would go blank with
    // nothing failing (ADR 0048 §6). The model is the provider stub, not a
    // script: the scripted driver sends no prompt, so its turns never carry
    // `prompt_bytes`.
    let c = contract();
    let rpc_c = &c["rpc"];
    let f = fixture("rpc-contract");
    commit_passing_checks(&f);
    let (base, server) = provider_stub(vec![
        openai_call("c1", "blast_radius", serde_json::json!({"symbol": "f"})),
        openai_call(
            "c2",
            "edit",
            serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
        ),
        openai_call(
            "c3",
            "run_checks",
            serde_json::json!({"paths": ["a.py"], "kinds": ["typecheck", "tests"]}),
        ),
        openai_call("c4", "done", serde_json::json!({"summary": "changed it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_CONTRACT_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"openai\"\n\
             base_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
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
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 4, "{records:?}");

    let events = &rpc_c["events"];
    let of = |message: &str| -> Vec<&Value> {
        records
            .iter()
            .filter(|r| r["fields"]["message"] == message)
            .map(|r| &r["fields"])
            .collect()
    };
    let mut gaps: Vec<String> = Vec::new();
    let turns = of("turn");
    let turn = &events["turn"];
    if turns.is_empty() {
        gaps.push("no turn event".into());
    }
    let verdicts = names(&turn["verdict_values"]);
    for t in &turns {
        gaps.extend(missing(
            t,
            &turn["always"],
            &format!("turn {} ({})", t["turn"], t["call"]),
        ));
        if let Some(v) = t.get("verdict")
            && !v.as_str().is_some_and(|v| verdicts.contains(&v))
        {
            gaps.push(format!(
                "turn {} verdict {v}, not one of {verdicts:?}",
                t["turn"]
            ));
        }
    }
    match turns.iter().find(|t| {
        t["call"]
            .as_str()
            .is_some_and(|c| c.starts_with("blast_radius "))
    }) {
        Some(t) => {
            gaps.extend(missing(t, &turn["blast_radius_only"], "blast_radius turn"));
            // A turn the provider's reply made: the driver sent a prompt.
            gaps.extend(missing(t, &turn["prompt_driver_only"], "blast_radius turn"));
            if t["check_kind"] != "" {
                gaps.push(format!(
                    "blast_radius turn check_kind {}, not \"\" on a turn that ran no check",
                    t["check_kind"]
                ));
            }
        }
        None => gaps.push("no blast_radius turn".into()),
    }
    match of("run budget").first() {
        Some(b) => gaps.extend(missing(b, &events["run budget"]["always"], "run budget")),
        None => gaps.push("no run budget event".into()),
    }
    for ty in ["run_start", "run_end", "response"] {
        let found: Vec<&Value> = records.iter().filter(|r| r["type"] == ty).collect();
        if found.is_empty() {
            gaps.push(format!("no {ty} record"));
        }
        for r in found {
            gaps.extend(missing(r, &rpc_c["framing"][ty]["always"], ty));
        }
    }
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
    // The run took the path the fixture is built for, so the turns above
    // are the ones meant.
    let end = records.iter().find(|r| r["type"] == "run_end").unwrap();
    assert_eq!(end["outcome"], "converged", "{end}");
}

#[test]
fn log_json_carries_every_trail_field_the_desktop_reads() {
    // The risk: a path the desktop's trails view reads is renamed or moved
    // in the record or the trail, and the view goes blank with nothing
    // failing (ADR 0048 §6). The run calls `run_checks`, so the trail has a
    // check outcome to read through.
    let c = contract();
    let f = fixture("log-json-contract");
    commit_passing_checks(&f);
    let script = scratch("log-json-contract-checks").join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"run_checks","paths":["a.py"],"kinds":["typecheck","tests"]},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let o = run(&f, &["run", "change f", "--model", &spec]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(&f, &["--json", "log"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let lines = jsonl(&o);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let gaps: Vec<&str> = names(&c["log_json"]["always"])
        .into_iter()
        .filter(|p| !carries(&lines[0], p))
        .collect();
    assert!(
        gaps.is_empty(),
        "the log --json line lacks what tests/client_contract.json lists: {gaps:?}\n{}",
        lines[0]
    );
}

/// An OpenAI-protocol reply carrying `text` as the assistant's message,
/// the shape a classification gets (it offers no tools).
fn openai_text(text: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":text}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

/// A `trellis rpc` whose model is the provider stub serving `replies`,
/// chosen with `set_model` as a client chooses it (so it is the user's
/// choice, ADR 0024 §3), and the stub's thread.
fn classify_against_stub(
    name: &str,
    replies: Vec<String>,
) -> (Fixture, Rpc, std::thread::JoinHandle<Vec<String>>) {
    let f = fixture(name);
    let (base, server) = provider_stub(replies);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_CLASSIFY_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"openai\"\n\
             base_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(serde_json::json!({"id": "m", "type": "set_model", "model": "provider:stub#m"}));
    let m = rpc.next(|v| v["id"] == "m");
    assert_eq!(m["success"], true, "{m}");
    (f, rpc, server)
}

/// No run started: no framing of one, no trail directory, a clean worktree.
fn nothing_ran(f: &Fixture, records: &[Value]) {
    assert!(
        !records
            .iter()
            .any(|r| r["type"] == "run_start" || r["type"] == "run_end"),
        "classify started no run: {records:?}"
    );
    assert!(
        !f.root.join(".trellis/runs").exists(),
        "classify wrote no trail"
    );
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");
}

#[test]
fn rpc_classify_answers_a_question_as_data_and_starts_no_run() {
    // The risks: a question sent to `prompt` gets a snapshot, a run and a
    // rollback (ADR 0052 §2.3); and a model's answer that looks like a
    // frame is taken for one. The answer here holds a newline and a
    // `run_end` record; it must arrive as one string inside one response.
    let answer = "a.py defines f.\n{\"type\":\"run_end\",\"outcome\":\"converged\"}";
    let (f, mut rpc, server) =
        classify_against_stub("rpc-classify-question", vec![openai_text(answer)]);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "what is in Q-5c1?"}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["command"], "classify", "{c}");
    assert_eq!(c["success"], true, "{c}");
    assert_eq!(c["data"]["intent"], "question", "{c}");
    assert_eq!(c["data"]["answer"], answer, "{c}");
    let records = rpc.finish();
    nothing_ran(&f, &records);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(requests[0].contains("Q-5c1"), "{}", requests[0]);
}

#[test]
fn rpc_classify_says_task_with_no_answer_and_starts_no_run() {
    // The risk: a task classified over rpc starts the run itself, or
    // carries an answer a client would show as if it were the reply.
    // Starting the run stays `prompt`'s.
    let (f, mut rpc, server) =
        classify_against_stub("rpc-classify-task", vec![openai_text("<<TRELLIS_TASK>>")]);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "change f to return 2"}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], true, "{c}");
    assert_eq!(c["data"]["intent"], "task", "{c}");
    assert!(c["data"].get("answer").is_none(), "{c}");
    let records = rpc.finish();
    nothing_ran(&f, &records);
    assert_eq!(server.join().unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n"
    );
}

#[test]
fn rpc_classify_is_refused_while_a_run_is_in_flight() {
    // The risk: routing asked mid-run, where a client would take the
    // answer for one about the run, and a model request raced the run's
    // own. It is refused as a second `prompt` is, in the same words.
    let f = fixture("rpc-classify-busy");
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "hello"}));
    rpc.send(serde_json::json!({"id": "p2", "type": "prompt", "task": "again", "model": f.spec}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    let p2 = rpc.next(|v| v["type"] == "response" && v["id"] == "p2");
    assert_eq!(c["command"], "classify", "{c}");
    assert_eq!(c["success"], false, "{c}");
    assert_eq!(p2["success"], false, "{p2}");
    assert_eq!(c["error"], p2["error"], "prompt's wording");
    let records = rpc.finish();
    let starts = records.iter().filter(|r| r["type"] == "run_start").count();
    assert_eq!(starts, 1, "only the first prompt ran: {records:?}");
}

#[test]
fn rpc_classify_refuses_text_that_is_missing_empty_or_not_a_string() {
    // The risk: a client bug that sends no text gets a model's guess about
    // nothing, read as routing.
    let f = fixture("rpc-classify-bad-text");
    let mut rpc = Rpc::start(trellis(&f));
    let bad = [
        serde_json::json!({"id": "a", "type": "classify"}),
        serde_json::json!({"id": "b", "type": "classify", "text": "  "}),
        serde_json::json!({"id": "c", "type": "classify", "text": 5}),
    ];
    for cmd in bad {
        rpc.send(cmd);
    }
    let records = rpc.finish();
    for id in ["a", "b", "c"] {
        let r = records.iter().find(|r| r["id"] == id).expect("answered");
        assert_eq!(r["command"], "classify", "{r}");
        assert_eq!(r["success"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains("\"text\""), "{r}");
    }
    nothing_ran(&f, &records);
}

#[test]
fn rpc_classify_reports_a_model_it_cannot_reach_and_starts_no_run() {
    // The risk: an unreachable model read as "a task" and handed to a run
    // that fails the same way later, after a snapshot; or no answer at all.
    let f = fixture("rpc-classify-unreachable");
    // Bound and closed: nothing listens on it now.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let key_env = "TRELLIS_TEST_RPC_CLASSIFY_GONE_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"gone\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:{port}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(serde_json::json!({"id": "m", "type": "set_model", "model": "provider:gone#m"}));
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "hello"}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], false, "{c}");
    assert!(
        c["error"]
            .as_str()
            .unwrap()
            .contains("asking the model what this input is"),
        "{c}"
    );
    let records = rpc.finish();
    nothing_ran(&f, &records);
}

#[test]
fn rpc_classify_sends_no_key_to_a_loopback_port_the_repository_chose() {
    // ADR 0024 §3. The risk: classify is a model request too, and one that
    // skipped a run's key rule would send the user's OPENAI_API_KEY to a
    // port named by a cloned repository's `[model] spec`.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let f = fixture("rpc-classify-hostile");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        format!("[model]\nspec = \"openai-chat:http://127.0.0.1:{port}/v1#m\"\n"),
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "hostile"]);
    // Built at run time: a key-shaped literal trips the secret scanners.
    let canary = format!("sk-{}-classify-canary-{}", "trellis", std::process::id());
    let mut cmd = trellis(&f);
    cmd.env("OPENAI_API_KEY", &canary);
    let mut rpc = Rpc::start(cmd);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "hello"}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], false, "{c}");
    assert!(
        c["error"].as_str().unwrap().contains("OPENAI_API_KEY"),
        "{c}"
    );
    let records = rpc.finish();
    assert!(
        !records.iter().any(|r| r.to_string().contains(&canary)),
        "{records:?}"
    );
    assert!(
        matches!(l.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connected to the repository's port"
    );
}

#[test]
fn rpc_classify_reply_carries_every_field_the_contract_lists() {
    // The risk: the Go TUI and the desktop route on `intent` and show
    // `answer`; a rename here would send every input down one path with
    // nothing failing on this side (ADR 0048 §6).
    let c = contract();
    let listed = c["rpc"]["responses"]
        .get("classify")
        .expect("client_contract.json lists rpc.responses.classify");
    let (_f, mut rpc, server) = classify_against_stub(
        "rpc-classify-contract",
        vec![openai_text("an answer"), openai_text("<<TRELLIS_TASK>>")],
    );
    rpc.send(serde_json::json!({"id": "q", "type": "classify", "text": "what is f?"}));
    let q = rpc.next(|v| v["type"] == "response" && v["id"] == "q");
    rpc.send(serde_json::json!({"id": "t", "type": "classify", "text": "change f"}));
    let t = rpc.next(|v| v["type"] == "response" && v["id"] == "t");
    rpc.finish();
    server.join().unwrap();
    let mut gaps = missing(&q["data"], &listed["always"], "question");
    gaps.extend(missing(&q["data"], &listed["question_only"], "question"));
    gaps.extend(missing(&t["data"], &listed["always"], "task"));
    assert!(
        gaps.is_empty(),
        "the classify reply lacks what tests/client_contract.json lists: {gaps:?}\n{q}\n{t}"
    );
    assert_eq!(q["data"]["intent"], "question", "{q}");
    assert_eq!(t["data"]["intent"], "task", "{t}");
    let values = names(&listed["intent_values"]);
    for r in [&q, &t] {
        assert!(
            r["data"]["intent"]
                .as_str()
                .is_some_and(|i| values.contains(&i)),
            "intent not one of {values:?}: {r}"
        );
    }
    for field in names(&listed["question_only"]) {
        assert!(t["data"].get(field).is_none(), "task carries {field}: {t}");
    }
}

// ---- #269: `get_state` carries the tier the run decided ----

/// `get_state`'s data, asked as `id`.
fn state(rpc: &mut Rpc, id: &str) -> Value {
    rpc.send(serde_json::json!({"id": id, "type": "get_state"}));
    let got = rpc.next(|v| v["id"] == id);
    assert_eq!(got["success"], true, "{got}");
    got["data"].clone()
}

/// Writes a user `[[providers]]` entry named `name` at `base`, whose key
/// variable nobody sets: a loopback endpoint is then used with no key.
fn user_provider(f: &Fixture, name: &str, base: &str) -> &'static str {
    let key_env = "TRELLIS_TEST_RPC_TIER_KEY";
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"{name}\"\nprotocol = \"openai\"\n\
             base_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    key_env
}

#[test]
fn rpc_get_state_tier_is_null_before_any_run_and_then_the_run_s_own() {
    // The risks: the Go TUI's status bar (#511) reading a tier that is not
    // there, or one guessed from the spec before any run decided it.
    let c = contract();
    let f = fixture("rpc-tier-before");
    let mut rpc = Rpc::start(trellis(&f));
    let before = state(&mut rpc, "s0");
    let gaps = missing(
        &before,
        &c["rpc"]["commands"]["get_state"]["data"]["always"],
        "get_state",
    );
    assert!(
        gaps.is_empty(),
        "get_state lacks what tests/client_contract.json lists: {gaps:?}\n{before}"
    );
    assert!(
        names(&c["rpc"]["commands"]["get_state"]["data"]["always"]).contains(&"tier"),
        "the contract does not list get_state's tier"
    );
    assert_eq!(before["tier"], Value::Null, "{before}");

    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": f.spec}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    rpc.wait_idle();
    let after = state(&mut rpc, "s1");
    assert_eq!(after["tier"], "scripted", "{after}");
    rpc.finish();
}

#[test]
fn rpc_get_state_tier_after_a_hosted_run_is_tier_1_from_the_protocol() {
    // A loopback server that is not colibri (its /v1/models is a 404) is
    // sent no probe, and its tier is the protocol's, as the status bar says.
    let f = fixture("rpc-tier-native");
    let (base, server) = provider_stub(vec![openai_call(
        "c1",
        "done",
        serde_json::json!({"summary": "nothing to do"}),
    )]);
    let key_env = user_provider(&f, "stub", &base);
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:stub#m"}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    rpc.wait_idle();
    let after = state(&mut rpc, "s");
    assert_eq!(after["tier"], "tier 1, native tool calls", "{after}");
    rpc.finish();
    assert_eq!(server.join().unwrap().len(), 1);
}

/// A loopback stub that answers as colibri serving `glm-5.2` whose template
/// refuses tools (ce370e8, c/openai_server.py:1364-1367 and :3885-3886, as
/// `tests/colibri_run.rs` has them), until the test process ends; and how
/// many tools probes it got.
fn colibri_refusing_stub() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let probes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = probes.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .ok();
            let mut raw = Vec::new();
            let mut buf = [0u8; 16384];
            loop {
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
                    if body.len() >= len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let (status, body) = if is_models_get(&String::from_utf8_lossy(&raw)) {
                (
                    "200 OK",
                    r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#,
                )
            } else {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    "400 Bad Request",
                    r#"{"error":{"message":"OLMoE has no tool-calling chat template; set COLI_TOOL_FALLBACK=1 to render tools as text.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#,
                )
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (format!("http://{addr}"), probes)
}

#[test]
fn rpc_get_state_tier_after_a_colibri_refusal_is_the_refusal_the_run_decided() {
    // The risk: get_state's tier re-guessed from the spec (tier 1, from the
    // protocol) while the run it reports on found colibri refusing tools and
    // stopped before its snapshot (ADR 0047 decision 4).
    let f = fixture("rpc-tier-colibri");
    let (base, probes) = colibri_refusing_stub();
    let key_env = user_provider(&f, "coli", &format!("{base}/v1"));
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:coli#glm-5.2"}),
    );
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    assert_eq!(end["outcome"], "error", "{end}");
    rpc.wait_idle();
    let after = state(&mut rpc, "s");
    assert_eq!(
        after["tier"], "tier 1 unavailable: colibri glm-5.2 refuses tools",
        "{after}"
    );
    rpc.finish();
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
}
