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
    // A model chosen first, as every client must before a prompt (#564).
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "m", "type": "set_model", "model": f.spec})
    )
    .unwrap();
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

/// Stderr of `trellis rpc` after one scripted run, read with `NO_COLOR`
/// removed: the colour decision under test must come from the pipe, not from
/// the variable the other tests set.
fn rpc_stderr_without_no_color(f: &Fixture) -> String {
    let mut child = trellis(f)
        .env_remove("NO_COLOR")
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
        serde_json::json!({"id": "m", "type": "set_model", "model": f.spec})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id": "1", "type": "prompt", "task": "change f", "model": f.spec})
    )
    .unwrap();
    drop(stdin);
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn rpc_stderr_has_no_ansi_on_a_pipe() {
    // The desktop pipes `trellis rpc`'s stderr into a text pane (ADR 0048
    // §2). With colour on regardless of the stream, every tracing line there
    // read `[2m...[0m [32m INFO[0m` with the escapes drawn as glyphs (#642).
    let f = fixture("rpc-no-ansi");
    let err = rpc_stderr_without_no_color(&f);
    // Not vacuous: the run's tracing lines did reach stderr.
    assert!(err.contains("INFO"), "no tracing line on stderr:\n{err}");
    assert!(!err.contains("\x1b["), "colour codes on a pipe:\n{err:?}");
}

#[test]
fn run_human_output_has_no_ansi_on_a_pipe() {
    // Outside JSON mode the human stream, tracing included, is stdout; a
    // `trellis run | tee` or a CI log must not carry escape bytes either.
    let f = fixture("run-no-ansi");
    let o = trellis(&f)
        .env_remove("NO_COLOR")
        .args(["run", "change f", "--model", &f.spec])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(out.contains("INFO"), "no tracing line on stdout:\n{out}");
    assert!(!out.contains("\x1b["), "colour codes on a pipe:\n{out:?}");
    assert!(!err.contains("\x1b["), "colour codes on a pipe:\n{err:?}");
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
        serde_json::json!({"id": "m", "type": "set_model", "model": f.spec})
    )
    .unwrap();
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
    rpc.choose(&f.spec);
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
        serde_json::json!({"id": "m", "type": "set_model", "model": spec})
    )
    .unwrap();
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

/// Reads one HTTP request from `s`, by its `Content-Length`: the whole of
/// it as text, and its body.
fn read_request(s: &mut std::net::TcpStream) -> (String, String) {
    use std::io::Read;
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
    (String::from_utf8_lossy(&raw).to_string(), body)
}

/// Serves `replies` in order as an OpenAI-protocol endpoint on a loopback
/// port, returning each request body it received. A `GET` of `/models` is
/// answered 404 and neither takes a reply nor is returned.
fn provider_stub(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    provider_stub_with_headers(replies, "")
}

/// [`provider_stub`], with `headers` (each line ending `\r\n`) added to
/// every reply's response head.
fn provider_stub_with_headers(
    replies: Vec<String>,
    headers: &'static str,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut replies = replies.into_iter().peekable();
        while let Some(r) = replies.peek() {
            let Ok((mut s, _)) = l.accept() else { break };
            let (raw, body) = read_request(&mut s);
            if is_models_get(&raw) {
                s.write_all(models_404().as_bytes()).ok();
                continue;
            }
            got.push(body);
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{r}",
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
    rpc.choose("provider:stub#m");
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

    /// Chooses `spec` as this session's model, as a client must before its
    /// first `prompt` (#564), and waits for it to be taken. A prompt that
    /// names its own `model` still runs on that one.
    fn choose(&mut self, spec: &str) {
        self.send(serde_json::json!({"id": "choose", "type": "set_model", "model": spec}));
        let r = self.next(|v| v["type"] == "response" && v["id"] == "choose");
        assert_eq!(r["success"], true, "{r}");
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

    /// The first record matching `pred`, whether it was read already,
    /// while waiting for another, or is still to come. For records written
    /// from different threads, whose order on the stream is not fixed.
    fn seen_or_next(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        match self.seen.iter().find(|v| pred(v)) {
            Some(v) => v.clone(),
            None => self.next(pred),
        }
    }

    /// [`Rpc::seen_or_next`] for the response to the command sent with `id`.
    fn response(&mut self, id: &str) -> Value {
        self.seen_or_next(|v| v["type"] == "response" && v["id"] == id)
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
    rpc.choose("provider:stub#m");
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
    rpc.choose(&f.spec);
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
fn rpc_get_state_lists_what_is_not_yet_delivered() {
    // The risks (#801): each client kept its own copy of what it had
    // queued, so a client that restarted, or a second one, could not tell
    // what was still waiting; a delivered steer listed as waiting, and
    // sent again; a follow-up dropped from the list in the step where it
    // moves from the run it was sent during to the chain's queue; and the
    // order lost across steers and follow-ups. The message text is the
    // person's, sent back as a string and never read as a record.
    let f = fixture("rpc-pending");
    let script = scratch("rpc-pending-steps").join("s.json");
    // Two edits, so the first run asks twice and the steer's request is
    // made while the run is still held on a question.
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"edit","path":"a.py","old":"return 2","new":"return 3"},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let c = contract();
    let entry = &c["rpc"]["responses"]["get_state"];
    assert!(
        names(&entry["always"]).contains(&"pending"),
        "the contract does not list get_state's pending"
    );
    let mut rpc = Rpc::start(trellis(&f));
    assert_eq!(state(&mut rpc, "g0")["pending"], serde_json::json!([]));
    rpc.choose(&spec);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": spec}),
    );
    let q1 = rpc.next(|v| v["type"] == "approval_request");
    let steer = r#"STEER-801 {"type":"follow_up","id":"x"} "quoted""#;
    rpc.send(serde_json::json!({"id": "u1", "type": "follow_up", "message": "second task"}));
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": steer}));
    rpc.send(serde_json::json!({"id": "u2", "type": "follow_up", "message": "third task"}));
    let held = state(&mut rpc, "g1");
    assert_eq!(
        held["pending"],
        serde_json::json!([
            {"id": "u1", "type": "follow_up", "message": "second task"},
            {"id": "s", "type": "steer", "message": steer},
            {"id": "u2", "type": "follow_up", "message": "third task"},
        ]),
        "{held}"
    );
    let gaps: Vec<&str> = names(&entry["pending_entries"])
        .into_iter()
        .filter(|p| !carries(&held, p))
        .collect();
    assert!(gaps.is_empty(), "{gaps:?}: {held}");
    let kinds = names(&entry["pending_type_values"]);
    for p in held["pending"].as_array().unwrap() {
        assert!(kinds.contains(&p["type"].as_str().unwrap()), "{p}");
    }

    // The second question is asked after the request that carried the
    // steer, so by then it has been delivered.
    rpc.send(serde_json::json!({"id": q1["id"], "type": "approve", "yes": true}));
    let q2 = rpc.next(|v| v["type"] == "approval_request");
    let after = state(&mut rpc, "g2");
    assert_eq!(
        after["pending"],
        serde_json::json!([
            {"id": "u1", "type": "follow_up", "message": "second task"},
            {"id": "u2", "type": "follow_up", "message": "third task"},
        ]),
        "a delivered steer is still listed: {after}"
    );

    // Once u1's run has started, only u2 waits: it moved from p's record
    // to the chain's queue and is still listed.
    rpc.send(serde_json::json!({"id": q2["id"], "type": "approve", "yes": true}));
    rpc.next(|v| v["type"] == "run_start" && v["id"] == "u1");
    rpc.next(|v| v["type"] == "approval_request");
    let next = state(&mut rpc, "g3");
    assert_eq!(next["run_request_id"], "u1", "{next}");
    assert_eq!(
        next["pending"],
        serde_json::json!([{"id": "u2", "type": "follow_up", "message": "third task"}]),
        "{next}"
    );
    let records = rpc.finish();
    let s = records
        .iter()
        .find(|r| r["type"] == "response" && r["id"] == "s")
        .unwrap_or_else(|| panic!("the steer was answered: {records:?}"));
    assert_eq!(s["success"], true, "{s}");
    assert_eq!(s["data"]["after_step"], 1, "{s}");
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
    rpc.choose(&f.spec);
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
    // an index it built and did not save, so the overview says "not
    // indexed" beside it (#654); a save that dirties the worktree; an index build
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
    assert_eq!(data["index_saved"], true, "{data}");
    let saved = std::fs::metadata(f.root.join(".trellis/index.bin"))
        .expect("get_codebase saved the index cache")
        .len();
    assert_eq!(data["index_bytes"], saved, "{data}");
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
    rpc.choose(&f.spec);
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
    rpc.choose(&f.spec);
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
    // Every field `trellis radius` prints, compared whole. The lists the
    // Atlas reads beside them (owner request of 2026-10-11) are not
    // printed, so they are not in what is read off the text.
    let mut printed_fields = data.clone();
    for listed in ["reached", "reached_not_listed", "sites", "sites_not_listed"] {
        assert!(
            printed_fields
                .as_object_mut()
                .unwrap()
                .remove(listed)
                .is_some(),
            "{listed}: {data}"
        );
    }
    assert_eq!(
        printed_radius(&String::from_utf8_lossy(&o.stdout)),
        printed_fields,
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
            t["parent"] = tag("parent ").map_or(Value::Null, Value::from);
            t["blockers"] = tag("blocked by ")
                .map(|l| l.split(',').map(Value::from).collect())
                .unwrap_or_else(|| Value::Array(Vec::new()));
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
    rpc.choose(&f.spec);
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
    rpc.choose(&f.spec);
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
    // The desktop's vault view reads these by name (ADR 0048 §6).
    let gaps = answer_gaps(&contract(), "vault_search", &["always"], &[&data]);
    assert!(gaps.is_empty(), "{gaps:?}\n{data}");

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

#[test]
fn rpc_vault_search_empty_query_lists_every_note_in_title_order_and_writes_nothing() {
    // The risks (#629): an empty query refused, when the port keeps Apex's
    // empty-query branch (`search.rs`: every note, by title then path,
    // score 0) and a client has no other way to list the vault; a list
    // ranked or scored as if matched; or a read that writes the vault
    // (#70). Held alone, mid-run and before a `quit`, as the other reads.
    let f = vault_fixture("rpc-vault-search-empty");
    let data = answered_three_ways(
        &f,
        &serde_json::json!({"type": "vault_search", "query": ""}),
    );
    let hits = data["hits"].as_array().unwrap_or_else(|| panic!("{data}"));
    assert_eq!(
        hits.iter()
            .map(|h| (h["title"].as_str().unwrap(), h["path"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("Child", "child.md"),
            ("Orphan", "orphan.md"),
            ("Root", "root.md")
        ],
        "every note, by title: {data}"
    );
    assert!(
        hits.iter().all(|h| h["score"] == 0.0),
        "a browse list scores nothing: {data}"
    );
    assert_eq!(
        data["mode"],
        serde_json::json!({"body": true, "trigrams": true})
    );
    // `limit` cuts the list as it cuts a ranking; whitespace is empty too.
    let one = answer(
        &f,
        serde_json::json!({"id": "l", "type": "vault_search", "query": "  ", "limit": 1}),
    );
    assert_eq!(one["success"], true, "{one}");
    assert_eq!(one["data"]["hits"][0]["path"], "child.md", "{one}");
    assert_eq!(one["data"]["hits"].as_array().unwrap().len(), 1, "{one}");
    // The CLI keeps its required query argument: without one it refuses.
    let o = run(&f, &["vault", "search"]);
    assert!(!o.status.success(), "the CLI ran with no query");
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
    // The desktop's vault view reads these by name (ADR 0048 §6). The child
    // has links both ways and no children; the root has the child.
    let root = answer(
        &f,
        serde_json::json!({"id": "r", "type": "vault_graph", "note": "root"}),
    );
    let gaps = answer_gaps(
        &contract(),
        "vault_graph",
        &["always"],
        &[&data, &root["data"]],
    );
    assert!(gaps.is_empty(), "{gaps:?}\n{data}\n{root}");

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

/// What `trellis vault check` printed, in `vault_check`'s shape: the
/// report line, the `issues:` block, the `warning:` lines and the run
/// gate's answer, with the `--fix` hint a refusing gate adds left out.
fn printed_check(stdout: &str) -> Value {
    let mut lines = stdout.lines();
    let line = lines.next().expect("the report line");
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let mut gate = Value::Null;
    let mut in_issues = false;
    for l in lines {
        if l == "issues:" {
            in_issues = true;
        } else if let Some(w) = l.strip_prefix("warning: ") {
            in_issues = false;
            warnings.push(w.to_string());
        } else if let Some(s) = l.strip_prefix("run gate: accepts this vault (") {
            in_issues = false;
            let stats: serde_json::Map<String, Value> = s
                .trim_end_matches(')')
                .split(' ')
                .map(|kv| {
                    let (k, v) = kv.split_once('=').expect("k=v");
                    (k.to_string(), json_u64(v))
                })
                .collect();
            gate = serde_json::json!({"accepts": true, "stats": stats});
        } else if l.starts_with("run gate: refuses this vault, ") {
            in_issues = false;
            gate = serde_json::json!({"accepts": false, "violations": []});
        } else if let Some(item) = l.strip_prefix("  ") {
            if in_issues {
                issues.push(item.to_string());
            } else {
                gate["violations"]
                    .as_array_mut()
                    .expect("after the gate line")
                    .push(item.into());
            }
        }
    }
    serde_json::json!({"line": line, "issues": issues, "warnings": warnings, "gate": gate})
}

fn json_u64(v: &str) -> Value {
    Value::from(v.parse::<u64>().expect("a count"))
}

#[test]
fn rpc_vault_check_returns_what_vault_check_prints() {
    // The risks: a client parsing `vault check`'s text because it has no
    // data form (ADR 0048 §2); a read that writes the vault (every vault
    // write is a `trellis vault` command, #70); a check blocking the
    // reader while a run waits on an approval; a `quit` that exits before
    // the answer; and a gate answer that drifts from what the CLI prints.
    let f = vault_fixture("rpc-vault-check");
    let data = answered_three_ways(&f, &serde_json::json!({"type": "vault_check"}));

    // Last: what `trellis vault check` prints. The fixture's orphan makes
    // the gate refuse, which exits 1 on the CLI and is data here.
    let o = run(&f, &["vault", "check"]);
    assert!(!o.status.success(), "the gate refuses this vault");
    assert_eq!(
        data,
        printed_check(&String::from_utf8_lossy(&o.stdout)),
        "what `trellis vault check` prints"
    );
    assert_eq!(data["gate"]["accepts"], false, "{data}");
    assert_eq!(
        data["gate"]["violations"].as_array().unwrap().len(),
        1,
        "{data}"
    );
    // The desktop's vault view reads these by name (ADR 0048 §6).
    let c = contract();
    let gaps = answer_gaps(
        &c,
        "vault_check",
        &["always", "gate_refuses_only"],
        &[&data],
    );
    assert!(gaps.is_empty(), "{gaps:?}\n{data}");
    assert_eq!(data["issues"].as_array().unwrap().len(), 2, "{data}");

    // A vault the gate accepts: its stats, as counts, and no violation.
    std::fs::write(
        f.root.join("notes/orphan.md"),
        "---\ntitle: \"Orphan\"\nparent: \"[[root]]\"\n---\nA found note. Back to [[child]].\n",
    )
    .unwrap();
    git(&f.root, &["commit", "-qam", "adopted"]);
    let ok = answer(&f, serde_json::json!({"id": "k", "type": "vault_check"}));
    assert_eq!(ok["success"], true, "{ok}");
    let o = run(&f, &["vault", "check"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        ok["data"],
        printed_check(&String::from_utf8_lossy(&o.stdout))
    );
    assert_eq!(ok["data"]["gate"]["accepts"], true, "{ok}");
    assert_eq!(ok["data"]["gate"]["stats"]["notes"], 3, "{ok}");
    let gaps = answer_gaps(
        &c,
        "vault_check",
        &["always", "gate_accepts_only"],
        &[&ok["data"]],
    );
    assert!(gaps.is_empty(), "{gaps:?}\n{ok}");
    assert_eq!(ok["data"]["issues"], serde_json::json!([]), "{ok}");

    // No vault configured: the CLI's message.
    let bare = fixture("rpc-vault-check-bare");
    let got = answer(&bare, serde_json::json!({"id": "e", "type": "vault_check"}));
    assert_eq!(got["success"], false, "{got}");
    assert_eq!(got["error"], cli_error(&bare, &["vault", "check"]), "{got}");
}

#[test]
fn rpc_vault_check_refuses_a_fix_field_by_name_and_writes_nothing() {
    // The risk: a `fix` field taken as `--fix`, so an rpc read repairs the
    // vault (#70); or ignored, so the client reads the unrepaired report as
    // the repaired one. Either value is refused, and the vault is untouched.
    let f = vault_fixture("rpc-vault-check-fix");
    for (id, fix) in [("t", Value::from(true)), ("f", Value::from(false))] {
        let got = answer(
            &f,
            serde_json::json!({"id": id, "type": "vault_check", "fix": fix}),
        );
        assert_eq!(got["command"], "vault_check", "{got}");
        assert_eq!(got["success"], false, "{got}");
        let err = got["error"].as_str().unwrap_or_default();
        assert!(err.contains("`fix`"), "names the field: {got}");
        assert!(
            got.get("data").is_none(),
            "no report beside the refusal: {got}"
        );
    }
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");
}

/// The `paths` of a successful `list_paths` answer, each a JSON string.
fn listed(answer: &Value) -> Vec<String> {
    assert_eq!(answer["command"], "list_paths", "{answer}");
    assert_eq!(answer["success"], true, "{answer}");
    answer["data"]["paths"]
        .as_array()
        .unwrap_or_else(|| panic!("`paths` is an array: {answer}"))
        .iter()
        .map(|p| p.as_str().expect("each path is a JSON string").to_string())
        .collect()
}

#[test]
fn rpc_list_paths_never_lists_an_ignored_file() {
    // The risks: a client offering a `.gitignore`d `.env` or build output
    // for `@` completion, to be typed into a task a model reads; a client
    // listing paths itself, outside `trellis_vcs` (ADR 0023); `.git`
    // internals offered; a read that writes; and a listing that blocks the
    // reader while a run waits on its approval.
    let f = fixture("rpc-list-paths-ignored");
    // Ignored paths that share the tracked `a.py`'s prefix, so a listing
    // that walked the disk would offer them beside it.
    std::fs::write(f.root.join(".gitignore"), "a.env\na_build/\n").unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "ignore rules"]);
    std::fs::write(f.root.join("a.env"), "not for a prompt\n").unwrap();
    std::fs::create_dir_all(f.root.join("a_build")).unwrap();
    std::fs::write(f.root.join("a_build/out.py"), "z = 3\n").unwrap();

    // Not an empty prefix here: the approve-mode run inside commits its
    // trail, a tracked path an empty prefix would rightly list next time.
    let data = answered_three_ways(
        &f,
        &serde_json::json!({"type": "list_paths", "prefix": "a"}),
    );
    assert_eq!(
        data,
        serde_json::json!({"paths": ["a.py"]}),
        "tracked only, ignored left out"
    );

    // An empty prefix is allowed. An untracked file no rule ignores is
    // offered, as the TUI offers it; an ignored one is not, even asked for
    // by name.
    std::fs::write(f.root.join("untracked.py"), "y = 2\n").unwrap();
    let got = answer(
        &f,
        serde_json::json!({"id": "u", "type": "list_paths", "prefix": ""}),
    );
    let paths = listed(&got);
    assert!(paths.iter().any(|p| p == "untracked.py"), "{paths:?}");
    assert!(paths.iter().any(|p| p == ".gitignore"), "{paths:?}");
    assert!(!paths.iter().any(|p| p == "a.env"), "{paths:?}");
    assert!(!paths.iter().any(|p| p.starts_with("a_build")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.starts_with(".git/")), "{paths:?}");
    for prefix in ["a.env", "a_build/", ".git/"] {
        let got = answer(
            &f,
            serde_json::json!({"id": "n", "type": "list_paths", "prefix": prefix}),
        );
        assert!(listed(&got).is_empty(), "{prefix}: {got}");
    }
}

#[test]
fn rpc_list_paths_gives_at_most_ten_shortest_first() {
    // The risks: a client flooded with every path in a large repository;
    // an order that differs from the TUI's (shortest first, then by bytes),
    // so the first completion offered is not the same file; and a prefix
    // read as "contains", offering `lib/m.py` for `m`.
    let f = fixture("rpc-list-paths-ten");
    std::fs::create_dir_all(f.root.join("m")).unwrap();
    std::fs::create_dir_all(f.root.join("lib")).unwrap();
    for p in [
        "m/ccccc.py",
        "m/bbbb.py",
        "m/aaaa.py",
        "m/zzz.py",
        "m/abc.py",
        "m/aaa.py",
        "m/ba.py",
        "m/ab.py",
        "m/aa.py",
        "m/z.py",
        "m/b.py",
        "m/a.py",
        "m/1.py",
        "mm.py",
        "lib/m.py",
    ] {
        std::fs::write(f.root.join(p), "x = 1\n").unwrap();
    }
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "many"]);

    let got = answer(
        &f,
        serde_json::json!({"id": "m", "type": "list_paths", "prefix": "m"}),
    );
    assert_eq!(
        listed(&got),
        [
            "mm.py", "m/1.py", "m/a.py", "m/b.py", "m/z.py", "m/aa.py", "m/ab.py", "m/ba.py",
            "m/aaa.py", "m/abc.py",
        ],
        "at most 10, shortest first, then by bytes"
    );
    let got = answer(
        &f,
        serde_json::json!({"id": "ma", "type": "list_paths", "prefix": "m/a"}),
    );
    assert_eq!(
        listed(&got),
        [
            "m/a.py",
            "m/aa.py",
            "m/ab.py",
            "m/aaa.py",
            "m/abc.py",
            "m/aaaa.py"
        ]
    );
}

#[test]
fn rpc_list_paths_refuses_a_prefix_that_is_not_a_relative_path_start() {
    // The risks: a prefix from the client reaching outside the repository,
    // or carrying a control character into a listing a client renders. The
    // prefix only filters git's listing, so neither could read a file; it
    // is refused anyway, by name, so a client learns its input was wrong
    // rather than seeing an empty list.
    let f = fixture("rpc-list-paths-refused");
    let cases: Vec<(Value, &str)> = vec![
        (Value::Null, "is required"),
        (Value::from(7), "is required"),
        (Value::from(".."), "`..`"),
        (Value::from("../a"), "`..`"),
        (Value::from("a/../b"), "`..`"),
        (Value::from("a\\..\\b"), "`..`"),
        (Value::from("/etc"), "absolute"),
        (Value::from("\\x"), "absolute"),
        (Value::from("C:/x"), "absolute"),
        (Value::from("a\u{7}b"), "control character"),
        (Value::from("a\nb"), "control character"),
        (Value::from("a\u{9b}b"), "control character"),
    ];
    let mut rpc = Rpc::start(trellis(&f));
    for (n, (prefix, _)) in cases.iter().enumerate() {
        let mut cmd = serde_json::json!({"id": n.to_string(), "type": "list_paths"});
        if !prefix.is_null() {
            cmd["prefix"] = prefix.clone();
        }
        rpc.send(cmd);
    }
    for (n, (prefix, rule)) in cases.iter().enumerate() {
        let got = rpc.next(|v| v["id"] == n.to_string().as_str());
        assert_eq!(got["command"], "list_paths", "{got}");
        assert_eq!(got["success"], false, "{prefix}: {got}");
        let e = got["error"].as_str().unwrap();
        assert!(e.contains("prefix") && e.contains(rule), "{prefix}: {e}");
    }
    rpc.finish();
}

#[test]
fn rpc_list_paths_reply_carries_every_field_the_contract_lists() {
    // The risk: a field a client reads from `list_paths` renamed or dropped
    // with nothing failing (ADR 0048 §6), including `paths` left out when
    // nothing matches rather than given as an empty list.
    let f = fixture("rpc-list-paths-contract");
    let always = &contract()["rpc"]["responses"]["list_paths"]["always"];
    for prefix in ["a", "nothing-matches"] {
        let got = answer(
            &f,
            serde_json::json!({"id": "c", "type": "list_paths", "prefix": prefix}),
        );
        assert_eq!(got["success"], true, "{got}");
        let gaps = missing(&got["data"], always, "list_paths");
        assert!(gaps.is_empty(), "{prefix}: {gaps:?} in {got}");
    }
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

    rpc.choose("provider:stub#m");
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

    rpc.choose(&first);
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

/// Each path the contract's `lists` name for `command`'s answer data
/// (`rpc.responses.<command>`) that `answers` do not carry, as
/// `<command>.<path>`. A path through `[]` needs one answer whose array has
/// entries, since an empty list is a correct answer that checks nothing;
/// every other path needs every answer.
fn answer_gaps(c: &Value, command: &str, lists: &[&str], answers: &[&Value]) -> Vec<String> {
    let entry = &c["rpc"]["responses"][command];
    lists
        .iter()
        .flat_map(|l| names(&entry[*l]))
        .filter(|p| {
            if p.contains("[]") {
                !answers.iter().any(|a| carries(a, p))
            } else {
                !answers.iter().all(|a| carries(a, p))
            }
        })
        .map(|p| format!("{command}.{p}"))
        .collect()
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
    // `prompt_bytes`. A byte budget smaller than any request makes every
    // request after the second elide an older turn, so the run sends the
    // `compacted` event too; the replies are canned, so the run is the same.
    let c = contract();
    let rpc_c = &c["rpc"];
    let later = &rpc_c["not_every_client"];
    let f = fixture("rpc-contract");
    commit_passing_checks(&f);
    let config = f.root.join(".trellis/config.toml");
    let mut cfg = std::fs::read_to_string(&config).unwrap();
    cfg.push_str("[model]\ncontext_bytes = 1000\n");
    std::fs::write(&config, cfg).unwrap();
    git(&f.root, &["commit", "-qam", "a byte budget"]);
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
    rpc.choose("provider:stub#m");
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
    let kinds = names(&turn["check_kind_values"]);
    for t in &turns {
        let at = format!("turn {} ({})", t["turn"], t["call"]);
        gaps.extend(missing(t, &turn["always"], &at));
        // No turn of this run is the governance audit: each called the
        // model.
        gaps.extend(missing(t, &turn["model_call_only"], &at));
        gaps.extend(missing(t, &later["events"]["turn"]["model_call_only"], &at));
        if let Some(v) = t.get("verdict")
            && !v.as_str().is_some_and(|v| verdicts.contains(&v))
        {
            gaps.push(format!(
                "turn {} verdict {v}, not one of {verdicts:?}",
                t["turn"]
            ));
        }
        if !t["check_kind"].as_str().is_some_and(|k| kinds.contains(&k)) {
            gaps.push(format!(
                "turn {} check_kind {}, not one of {kinds:?}",
                t["turn"], t["check_kind"]
            ));
        }
    }
    // Every event line's level, which the TUI reads to show a warning.
    let levels = names(&later["event_line"]["level_values"]);
    for r in records.iter().filter(|r| r.get("fields").is_some()) {
        let at = format!("the {} event", r["fields"]["message"]);
        gaps.extend(missing(r, &later["event_line"]["always"], &at));
        if !r["level"].as_str().is_some_and(|l| levels.contains(&l)) {
            gaps.push(format!(
                "{at}'s level {}, not one of {levels:?}",
                r["level"]
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
            gaps.extend(missing(
                t,
                &later["events"]["turn"]["blast_radius_only"],
                "blast_radius turn",
            ));
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
    // The applied edit's lines and diff (#802), in this auto-mode run.
    match turns
        .iter()
        .find(|t| t["tool"] == "edit" && t["wrote"] == true)
    {
        Some(t) => gaps.extend(missing(
            t,
            &later["events"]["turn"]["edit_applied_only"],
            "edit turn",
        )),
        None => gaps.push("no applied edit turn".into()),
    }
    match of("run budget").first() {
        Some(b) => gaps.extend(missing(b, &events["run budget"]["always"], "run budget")),
        None => gaps.push("no run budget event".into()),
    }
    let compacted = of("compacted");
    if compacted.is_empty() {
        gaps.push("no compacted event".into());
    }
    for e in compacted {
        gaps.extend(missing(
            e,
            &later["events"]["compacted"]["always"],
            "compacted",
        ));
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
    let outcomes = names(&rpc_c["framing"]["run_end"]["outcome_values"]);
    for r in records.iter().filter(|r| r["type"] == "run_end") {
        if !r["outcome"].as_str().is_some_and(|o| outcomes.contains(&o)) {
            gaps.push(format!(
                "run_end outcome {}, not one of {outcomes:?}",
                r["outcome"]
            ));
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
fn rpc_turn_carries_queue_wait_from_the_reply_header() {
    // The risk (#312): colibri's queue wait, read from a response header
    // and not the body, is dropped between the adapter and the stream, so
    // a client draws a queued turn as a slow model. The fields come from
    // the contract's header category, so a field added there is held here.
    let c = contract();
    let header_only = &c["rpc"]["events"]["turn"]["response_header_only"];
    let f = fixture("rpc-queue-wait");
    let (base, server) = provider_stub_with_headers(
        vec![
            openai_call("c1", "read_file", serde_json::json!({"path": "a.py"})),
            openai_call("c2", "done", serde_json::json!({"summary": "read it"})),
        ],
        "x-colibri-queue-wait-ms: 1500\r\n",
    );
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_QUEUE_WAIT_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "read a.py",
                           "model": "provider:stub#m"}),
    );
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 2, "{records:?}");

    let turns: Vec<&Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "turn")
        .map(|r| &r["fields"])
        .collect();
    assert!(!turns.is_empty(), "no turn event: {records:?}");
    let mut gaps: Vec<String> = Vec::new();
    for t in &turns {
        gaps.extend(missing(
            t,
            header_only,
            &format!("turn {} ({})", t["turn"], t["call"]),
        ));
    }
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
    for t in &turns {
        assert_eq!(t["queue_wait_ms"], 1500, "{t}");
    }
    // The run took the stub's path, so the turns above are its replies'.
    assert!(
        turns.iter().any(|t| t["call"]
            .as_str()
            .is_some_and(|c| c.starts_with("read_file "))),
        "{turns:?}"
    );
}

#[test]
fn rpc_elapsed_excludes_time_waiting_on_an_approval() {
    // The risk (#799): a client draws a run's elapsed time from its own
    // clock, or trellis reports one that counts a person's time reading a
    // diff, so the figure on screen is not the one the wall-clock budget
    // checks and a run "ends early" against it. The approval is held well
    // past what the rest of the run takes; the edit's `turn` spans that
    // hold, so its elapsed_ms over the `model call` before it is the hold
    // if the wait is charged and milliseconds if it is not. The fields
    // checked are the ones the contract lists, so a rename fails here.
    const HOLD_MS: u64 = 2500;
    let c = contract();
    let later = &c["rpc"]["not_every_client"];
    let f = fixture("rpc-elapsed-approval");
    let (base, server) = provider_stub(vec![
        openai_call("c1", "read_file", serde_json::json!({"path": "a.py"})),
        openai_call(
            "c2",
            "edit",
            serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
        ),
        openai_call("c3", "done", serde_json::json!({"summary": "changed it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_ELAPSED_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                           "model": "provider:stub#m"}),
    );
    let q = rpc.next(|v| v["type"] == "approval_request");
    std::thread::sleep(std::time::Duration::from_millis(HOLD_MS));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 3, "{records:?}");

    let loop_events: Vec<&Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "model call" || r["fields"]["message"] == "turn")
        .map(|r| &r["fields"])
        .collect();
    let end = records
        .iter()
        .find(|r| r["type"] == "run_end")
        .expect("a run_end");
    let mut gaps: Vec<String> = Vec::new();
    for e in &loop_events {
        let what = format!("{} {}", e["message"].as_str().unwrap(), e["turn"]);
        let message = e["message"].as_str().unwrap();
        // elapsed_ms is in rpc.events since both clients read it (#906);
        // what not every client reads yet is under not_every_client, which
        // has no record for an event once all of its fields have moved.
        gaps.extend(missing(e, &c["rpc"]["events"][message]["always"], &what));
        if let Some(spec) = later["events"].get(message) {
            gaps.extend(missing(e, &spec["always"], &what));
            // An approve-mode edit carries its lines and diff as an
            // auto-mode one does (#802): they come from the write, not the
            // question.
            if e["tool"] == "edit" && e["wrote"] == true {
                gaps.extend(missing(e, &spec["edit_applied_only"], &what));
            }
        }
    }
    // Of run_end's optional fields only elapsed_ms is this run's case: a
    // converged run restores nothing and has no error. Named through the
    // contract, so a rename there fails here.
    let run_end_optional = c["rpc"]["framing"]["run_end"]["optional"]
        .as_object()
        .expect("run_end's optional fields");
    match run_end_optional.get_key_value("elapsed_ms") {
        Some((k, _)) if end.get(k.as_str()).is_none() => gaps.push(format!("run_end.{k}")),
        Some(_) => {}
        None => gaps.push("the contract lists no run_end.elapsed_ms".into()),
    }
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );

    let ms = |v: &Value| v["elapsed_ms"].as_u64().expect("elapsed_ms is a count");
    // Counting up, as one clock: never back, and run_end at or past the
    // last turn.
    for w in loop_events.windows(2) {
        assert!(ms(w[0]) <= ms(w[1]), "elapsed went back: {w:?}");
    }
    let edit = loop_events
        .iter()
        .position(|e| {
            e["message"] == "turn" && e["call"].as_str().is_some_and(|c| c.starts_with("edit "))
        })
        .expect("an edit turn");
    let asked = &loop_events[edit - 1];
    assert_eq!(asked["message"], "model call", "{loop_events:?}");
    let across_the_wait = ms(loop_events[edit]) - ms(asked);
    assert!(
        across_the_wait < HOLD_MS,
        "the {HOLD_MS} ms spent waiting on the approval was charged: \
         {across_the_wait} ms between the model call and its turn\n{loop_events:?}"
    );
    let last = loop_events.last().unwrap();
    assert!(ms(end) >= ms(last), "{end} before {last}");
    // Against the edit's turn, which already has the wait taken out, not
    // against the hold alone: what the rest of the run takes on a slow host
    // must not decide this, only whether run_end charged the wait.
    let after_the_wait = ms(end) - ms(loop_events[edit]);
    assert!(
        after_the_wait < HOLD_MS,
        "run_end's elapsed_ms counts the {HOLD_MS} ms approval wait: \
         {after_the_wait} ms past the edit's turn\n{end}"
    );
}

#[test]
fn rpc_run_budget_carries_every_ceiling() {
    // The risk (#799): `run budget` carried only max_iterations, so a client
    // drew the wall-clock and token meters against a constant or its own
    // reading of a config the run may not have used, and the meter and the
    // budget disagreed about when a run ends. Every ceiling is set to a
    // value no default has, so a figure from anywhere but this run's budget
    // fails. The turn's token counter is held to the completion tokens the
    // stub reported, with prompt tokens far larger beside them, so a counter
    // that summed the two kinds (UI rule 5) fails too.
    let c = contract();
    let later = &c["rpc"]["not_every_client"];
    let f = fixture("rpc-run-budget");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[budget]\nmax_iterations = 9\nmax_wall_clock_s = 321\nmax_tokens = 54321\n\n\
         [model]\ncontext_bytes = 123456\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "a budget"]);
    let (base, server) = provider_stub(vec![
        openai_call_with_usage("read_file", serde_json::json!({"path": "a.py"}), 1000, 7),
        openai_call_with_usage("blast_radius", serde_json::json!({"symbol": "f"}), 1000, 11),
        openai_call_with_usage("done", serde_json::json!({"summary": "looked"}), 1000, 13),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_RUN_BUDGET_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "look at f",
                           "model": "provider:stub#m"}),
    );
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 3, "{records:?}");

    let of = |message: &str| -> Vec<&Value> {
        records
            .iter()
            .filter(|r| r["fields"]["message"] == message)
            .map(|r| &r["fields"])
            .collect()
    };
    let budgets = of("run budget");
    assert_eq!(budgets.len(), 1, "one run budget event: {records:?}");
    let b = budgets[0];
    let spec = &later["events"]["run budget"];
    let mut gaps = missing(b, &c["rpc"]["events"]["run budget"]["always"], "run budget");
    gaps.extend(missing(b, &spec["always"], "run budget"));
    // A hosted driver always has a context ceiling.
    gaps.extend(missing(b, &spec["context_driver_only"], "run budget"));
    for t in of("turn") {
        gaps.extend(missing(
            t,
            &later["events"]["turn"]["always"],
            &format!("turn {}", t["turn"]),
        ));
    }
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
    assert_eq!(b["max_iterations"], 9, "{b}");
    assert_eq!(b["max_wall_clock_ms"], 321_000, "{b}");
    assert_eq!(b["max_tokens"], 54321, "{b}");
    assert_eq!(b["context_bytes"], 123_456, "{b}");

    let used: Vec<(String, u64)> = of("turn")
        .iter()
        .map(|t| {
            (
                t["call"].as_str().unwrap().to_string(),
                t["completion_tokens_used"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(used.len(), 2, "{used:?}");
    assert!(used[0].0.starts_with("read_file "), "{used:?}");
    assert!(used[1].0.starts_with("blast_radius "), "{used:?}");
    assert_eq!(
        used.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        [7, 7 + 11],
        "the counter max_tokens is checked against is completion tokens \
         only, summed over the replies so far"
    );
}

#[test]
fn rpc_every_turn_through_the_governance_audit_keeps_the_contract() {
    // The risk: the contract promises a field on every turn that one kind
    // of turn never carries. The governance audit at convergence is a turn
    // event the loop sends without calling the model (trellis-loop
    // agent.rs), so a field from the model call listed as `always` is a
    // promise a client watching a governed run sees broken. Every turn is
    // held to `always`; the turns that called the model to the categories
    // a provider run fills too.
    let c = contract();
    let turn = &c["rpc"]["events"]["turn"];
    let f = fixture("rpc-governance-contract");
    std::fs::create_dir_all(f.root.join(".trellis/governance")).unwrap();
    std::fs::write(
        f.root.join(".trellis/governance/no-vendor.toml"),
        "name = \"Vendored code is not ours to edit\"\nrank = \"non-negotiable\"\n\
         description = \"Changes go upstream.\"\ndeny-write = { paths = [\"vendor/\"] }\n",
    )
    .unwrap();
    commit_passing_checks(&f);
    let (base, server) = provider_stub(vec![
        openai_call(
            "c1",
            "run_checks",
            serde_json::json!({"paths": ["a.py"], "kinds": ["typecheck", "tests"]}),
        ),
        openai_call("c2", "done", serde_json::json!({"summary": "checked it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_GOVERNANCE_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "check f",
                           "model": "provider:stub#m"}),
    );
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 2, "{records:?}");

    let turns: Vec<&Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "turn")
        .map(|r| &r["fields"])
        .collect();
    let is_audit = |t: &Value| t["check_kind"] == "Governance";
    assert!(
        turns.iter().any(|t| is_audit(t)),
        "the run reached no governance audit: {records:?}"
    );
    assert!(
        turns.iter().any(|t| !is_audit(t)),
        "no turn called the model: {records:?}"
    );
    let label = |t: &Value| format!("turn {} ({}, {})", t["turn"], t["call"], t["check_kind"]);
    let gaps: Vec<String> = turns
        .iter()
        .flat_map(|t| {
            let mut g = missing(t, &turn["always"], &label(t));
            g.extend(missing(
                t,
                &c["rpc"]["not_every_client"]["events"]["turn"]["always"],
                &label(t),
            ));
            g
        })
        .collect();
    assert!(
        gaps.is_empty(),
        "a turn lacks what tests/client_contract.json lists as always present: {gaps:?}\n{records:?}"
    );
    let gaps: Vec<String> = turns
        .iter()
        .filter(|t| !is_audit(t))
        .flat_map(|t| {
            let mut g = missing(t, &turn["model_call_only"], &label(t));
            g.extend(missing(t, &turn["prompt_driver_only"], &label(t)));
            g.extend(missing(
                t,
                &c["rpc"]["not_every_client"]["events"]["turn"]["model_call_only"],
                &label(t),
            ));
            g
        })
        .collect();
    assert!(
        gaps.is_empty(),
        "a turn that called the model lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
    let end = records.iter().find(|r| r["type"] == "run_end").unwrap();
    assert_eq!(end["outcome"], "converged", "{end}");
}

#[test]
fn rpc_plan_event_keeps_the_contract_and_the_trail_records_the_call_as_an_action() {
    // The risks (ADR 0058 §5, #810): the `plan` event's fields are renamed
    // and a client's Plan card goes blank with nothing failing; or the
    // trail records the plan in a new field (a format change ADR 0058 does
    // not make) or not at all. The fields are the contract's list; the
    // trail is held to one `plan` action and nothing new.
    let c = contract();
    let listed = &c["rpc"]["not_every_client"]["events"]["plan"];
    let f = fixture("rpc-plan");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[tools]\nenable = [\"plan\"]\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "the plan tool"]);
    let steps: Vec<Value> = (1..=9)
        .map(|i| serde_json::json!({"text": format!("step {i}"), "state": "todo"}))
        .collect();
    let script = scratch("rpc-plan-steps").join("s.json");
    std::fs::write(
        &script,
        serde_json::json!({"steps": [
            {"tool": "plan", "steps": steps},
            {"tool": "done", "summary": "planned"}
        ]})
        .to_string(),
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let mut rpc = Rpc::start(trellis(&f));
    rpc.choose(&spec);
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "plan it", "model": spec}));
    let records = rpc.finish();
    let plans: Vec<&Value> = records
        .iter()
        .map(|r| &r["fields"])
        .filter(|f| f["message"] == "plan")
        .collect();
    assert_eq!(plans.len(), 1, "{records:?}");
    let gaps = missing(plans[0], &listed["always"], "plan");
    assert!(gaps.is_empty(), "{gaps:?}\n{records:?}");
    assert_eq!(plans[0]["turn"], 1);
    assert_eq!(plans[0]["steps_cut"], 1);
    let kept: Value = serde_json::from_str(plans[0]["steps"].as_str().expect("a string"))
        .expect("steps is a JSON array");
    assert_eq!(kept.as_array().map(Vec::len), Some(8), "{kept}");

    let trails = trails(&f);
    assert_eq!(trails.len(), 1, "{trails:?}");
    let actions = trails[0]["factual"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["tool"], "plan");
    assert_eq!(actions[0]["wrote"], false);
    let turn = records
        .iter()
        .map(|r| &r["fields"])
        .find(|f| f["message"] == "turn")
        .expect("the plan's turn");
    assert_eq!(actions[0]["target"], turn["target"], "{records:?}");
}

#[test]
fn rpc_turn_tool_target_wrote_match_the_trail_actions() {
    // The risk (#800): a client works out what a turn touched by splitting
    // `call` at a space and searching the observation's tail for
    // `applied: true`, and the trail works it out from the whole turn, so
    // the live view and the record of the same run disagree. The script has
    // an edit whose anchor is missing (an edit that wrote nothing), one that
    // applied, a created file and a check, and the governance rule adds the
    // audit's turn, the one event the loop sends without calling the model.
    // Compared in order, not by `turn`: the audit's event carries the last
    // call's number and its trail action the next one. With no approvals,
    // gates, partitions, services or compaction (the scripted driver has no
    // byte budget), every trail action is a turn's.
    let f = fixture("rpc-turn-action");
    std::fs::create_dir_all(f.root.join(".trellis/governance")).unwrap();
    std::fs::write(
        f.root.join(".trellis/governance/no-vendor.toml"),
        "name = \"Vendored code is not ours to edit\"\nrank = \"non-negotiable\"\n\
         description = \"Changes go upstream.\"\ndeny-write = { paths = [\"vendor/\"] }\n",
    )
    .unwrap();
    commit_passing_checks(&f);
    let config = f.root.join(".trellis/config.toml");
    let mut cfg = std::fs::read_to_string(&config).unwrap();
    cfg.push_str("[tools]\nenable = [\"write_file\"]\n");
    std::fs::write(&config, cfg).unwrap();
    git(&f.root, &["commit", "-qam", "write_file"]);
    let script = scratch("rpc-turn-action-steps").join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"read_file","path":"a.py"},
            {"tool":"edit","path":"a.py","old":"return 9","new":"return 2"},
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"write_file","path":"b.py","content":"X = 1\n"},
            {"tool":"run_checks","paths":["a.py"],"kinds":["typecheck","tests"]},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let mut rpc = Rpc::start(trellis(&f));
    rpc.choose(&spec);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": spec}),
    );
    let records = rpc.finish();
    let end = records.iter().find(|r| r["type"] == "run_end").unwrap();
    assert_eq!(end["outcome"], "converged", "{end}\n{records:?}");

    let from_events: Vec<(Value, Value, Value)> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "turn")
        .map(|r| {
            let t = &r["fields"];
            (t["tool"].clone(), t["target"].clone(), t["wrote"].clone())
        })
        .collect();
    let trails = trails(&f);
    assert_eq!(trails.len(), 1, "{trails:?}");
    let actions = trails[0]["factual"]["actions"].as_array().unwrap();
    let from_trail: Vec<(Value, Value, Value)> = actions
        .iter()
        .map(|a| (a["tool"].clone(), a["target"].clone(), a["wrote"].clone()))
        .collect();
    assert_eq!(
        from_events, from_trail,
        "the turn events and the trail disagree about what the run did\n{records:?}"
    );
    // Each case the comparison is there for, so a run that never reached
    // one cannot pass by agreeing on less.
    let wrote = |tool: &str| -> Vec<bool> {
        from_trail
            .iter()
            .filter(|(t, _, _)| t == tool)
            .map(|(_, _, w)| w.as_bool().expect("wrote is a bool"))
            .collect()
    };
    assert_eq!(wrote("edit"), [false, true], "{from_trail:?}");
    assert_eq!(wrote("write_file"), [true], "{from_trail:?}");
    assert_eq!(wrote("run_checks"), [false, false], "{from_trail:?}");

    let audit = records
        .iter()
        .map(|r| &r["fields"])
        .find(|t| t["message"] == "turn" && t["check_kind"] == "Governance")
        .unwrap_or_else(|| panic!("no governance audit turn: {records:?}"));
    assert!(
        audit["duration_ms"].is_u64(),
        "the governance audit's turn carries no duration_ms: {audit}"
    );
}

/// Holds the `model call` contract over one stream: each carries what the
/// contract lists and no `prompt_bytes`; each `turn` that called the model
/// has one before it under its own number. Returns how many `model call`s
/// there were, for the caller to hold against the calls actually made.
fn model_calls_precede_turns(records: &[Value], c: &Value) -> usize {
    let spec = &c["rpc"]["events"]["model call"];
    let loop_events: Vec<&Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "model call" || r["fields"]["message"] == "turn")
        .map(|r| &r["fields"])
        .collect();
    let mut gaps: Vec<String> = Vec::new();
    let mut announced: Vec<&Value> = Vec::new();
    let mut calls = 0;
    for e in &loop_events {
        if e["message"] == "model call" {
            calls += 1;
            gaps.extend(missing(
                e,
                &spec["always"],
                &format!("model call {}", e["turn"]),
            ));
            if e.get("prompt_bytes").is_some() {
                gaps.push(format!(
                    "model call {} carries prompt_bytes, a size not measured before the call",
                    e["turn"]
                ));
            }
            announced.push(&e["turn"]);
            continue;
        }
        // The audit's turn is numbered after the last call (trellis-loop
        // agent.rs), so it is held by the count the caller checks against
        // the provider, not by pairing.
        let audit = e["check_kind"] == "Governance";
        let preceded = announced.last() == Some(&&e["turn"]);
        if !audit && !preceded {
            gaps.push(format!(
                "turn {} ({}) had no model call announced under its number before it",
                e["turn"], e["call"]
            ));
        }
    }
    assert!(gaps.is_empty(), "{gaps:?}\n{records:?}");
    calls
}

#[test]
fn rpc_and_run_json_announce_each_model_call_before_its_turn() {
    // The risk: replies are not streamed, so between two turns a client
    // hears nothing from a slow local model and reads the run as hung
    // (qa/REPORT-2026-10-02.md). The `model call` event is the one
    // difference a client can draw, so it has to be on both streams before
    // the wait, under the turn number it produces, and never for the
    // governance audit, which calls no model (ADR 0025 §4, #614).
    let c = contract();
    let f = fixture("rpc-model-call");
    std::fs::create_dir_all(f.root.join(".trellis/governance")).unwrap();
    std::fs::write(
        f.root.join(".trellis/governance/no-vendor.toml"),
        "name = \"Vendored code is not ours to edit\"\nrank = \"non-negotiable\"\n\
         description = \"Changes go upstream.\"\ndeny-write = { paths = [\"vendor/\"] }\n",
    )
    .unwrap();
    commit_passing_checks(&f);
    let (base, server) = provider_stub(vec![
        openai_call(
            "c1",
            "run_checks",
            serde_json::json!({"paths": ["a.py"], "kinds": ["typecheck", "tests"]}),
        ),
        openai_call("c2", "done", serde_json::json!({"summary": "checked it"})),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_MODEL_CALL_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "check f",
                           "model": "provider:stub#m"}),
    );
    let records = rpc.finish();
    let requests = server.join().unwrap().len();
    assert!(
        records
            .iter()
            .any(|r| r["fields"]["message"] == "turn" && r["fields"]["check_kind"] == "Governance"),
        "the run reached no governance audit: {records:?}"
    );
    // One announcement per request the provider saw, no more: a count that
    // disagreed with the provider's would be a call that never happened.
    assert_eq!(
        model_calls_precede_turns(&records, &c),
        requests,
        "{records:?}"
    );

    // `run --json`, through the scripted driver, which sends no prompt and
    // still makes a call per step.
    let o = run(&f, &["run", "change f", "--model", &f.spec, "--json"]);
    let lines = jsonl(&o);
    assert_eq!(model_calls_precede_turns(&lines, &c), 2, "{lines:?}");
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
    let later = &c["log_json"]["not_every_client"];
    let gaps: Vec<&str> = names(&c["log_json"]["always"])
        .into_iter()
        .chain(names(&later["always"]))
        .filter(|p| !carries(&lines[0], p))
        .collect();
    assert!(
        gaps.is_empty(),
        "the log --json line lacks what tests/client_contract.json lists: {gaps:?}\n{}",
        lines[0]
    );
    // Optional only for trails written before #622: this one has it.
    let optional = later["optional"].as_object().unwrap();
    assert!(optional.contains_key("trail.ended_at"));
    assert!(carries(&lines[0], "trail.ended_at"), "{}", lines[0]);
    // A scripted run carries no prompt size, so its context is null, never a
    // measured zero; the contract says when.
    assert!(later["null_when"]["trail.factual.context"].is_string());
    assert!(
        lines[0]["trail"]["factual"]["context"].is_null(),
        "{}",
        lines[0]
    );
}

#[test]
fn rpc_steer_abort_compact_answers_carry_what_the_contract_lists() {
    // The risk (ADR 0048 §6): the clients read these answers by name (the
    // TUI a steer's after_step and abort's undelivered ids; the desktop
    // abort's undelivered text, its starters from get_commands, its model
    // and session-token panes), and a field renamed in rpc.rs passes every
    // other test while a pane goes blank. One session, as a client drives
    // it: a run held on its question while a steer, a follow_up and a
    // compact arrive; the follow_up's own run held the same way and aborted
    // with another follow_up waiting; then the read-only answers and quit.
    let c = contract();
    let later = &c["rpc"]["not_every_client"];
    let f = fixture("rpc-answers-contract");
    // Large enough that folding its read shrinks the request after it.
    std::fs::write(
        f.root.join("big.py"),
        format!("X = 1\n{}", "# pad\n".repeat(800)),
    )
    .unwrap();
    // A template, so get_commands has a starter to list.
    std::fs::create_dir_all(f.root.join(".trellis/prompts")).unwrap();
    std::fs::write(
        f.root.join(".trellis/prompts/review.md"),
        "---\ndescription: Review one file\n---\nReview it.\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "a big file and a template"]);
    let edit = || {
        openai_call(
            "e",
            "edit",
            serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
        )
    };
    let (base, server) = provider_stub(vec![
        openai_call("c1", "read_file", serde_json::json!({"path": "big.py"})),
        edit(),
        openai_call("c3", "done", serde_json::json!({"summary": "changed it"})),
        // The follow_up's run, held on its edit until the abort.
        edit(),
    ]);
    // A variable nobody sets: a loopback endpoint is then used with no key.
    let key_env = "TRELLIS_TEST_RPC_ANSWERS_KEY";
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                           "model": "provider:stub#m"}),
    );
    // Held on the edit's question, one turn behind it. Read in order, so
    // each is in before the yes that lets the run make its next request.
    let q = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": "keep it small"}));
    rpc.send(serde_json::json!({"id": "u", "type": "follow_up", "message": "now say why"}));
    rpc.send(serde_json::json!({"id": "c", "type": "compact"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    // The follow_up's run, held on its own question, with another
    // follow_up waiting behind it for abort to hand back.
    let q2 = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "u2", "type": "follow_up", "message": "and then stop"}));
    rpc.send(serde_json::json!({"id": "x", "type": "abort"}));
    rpc.next(|v| v["type"] == "response" && v["id"] == "x");
    for (id, ty) in [
        ("t", "get_last_assistant_text"),
        ("st", "get_session_stats"),
        ("m", "get_available_models"),
        ("g", "get_commands"),
        ("qq", "quit"),
    ] {
        rpc.send(serde_json::json!({"id": id, "type": ty}));
    }
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 4, "{records:?}");
    let by = |id: &str| -> &Value {
        records
            .iter()
            .find(|r| r["type"] == "response" && r["id"] == id)
            .unwrap_or_else(|| panic!("no answer to {id}: {records:?}"))
    };

    let mut gaps: Vec<String> = Vec::new();
    let no_data = c["rpc"]["framing"]["response"]["no_data"]
        .as_object()
        .expect("framing.response.no_data");
    let approve_id = q["id"].as_str().expect("an approval id");
    for (id, command) in [
        ("s", "steer"),
        ("c", "compact"),
        (approve_id, "approve"),
        ("x", "abort"),
        ("t", "get_last_assistant_text"),
        ("st", "get_session_stats"),
        ("m", "get_available_models"),
        ("g", "get_commands"),
    ] {
        let r = by(id);
        assert_eq!(r["success"], true, "{r}");
        assert!(!no_data.contains_key(command), "no_data names {command}");
        gaps.extend(answer_gaps(&c, command, &["always"], &[&r["data"]]));
    }
    for (id, command) in [
        ("p", "prompt"),
        ("u", "follow_up"),
        ("u2", "follow_up"),
        ("qq", "quit"),
    ] {
        let r = by(id);
        assert_eq!(r["success"], true, "{r}");
        assert!(
            no_data.contains_key(command),
            "no_data does not name {command}"
        );
        if r.get("data").is_some() {
            gaps.push(format!("{command} ({id}) answered with data: {r}"));
        }
    }
    let kinds = names(&c["rpc"]["responses"]["abort"]["type_values"]);
    for u in by("x")["data"]["undelivered"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !u["type"].as_str().is_some_and(|t| kinds.contains(&t)) {
            gaps.push(format!(
                "abort's undelivered type {}, not one of {kinds:?}",
                u["type"]
            ));
        }
    }
    // Both questions, with the id of the command whose run asked.
    let asked: Vec<&Value> = records
        .iter()
        .filter(|r| r["type"] == "approval_request")
        .collect();
    for a in &asked {
        gaps.extend(missing(
            a,
            &c["rpc"]["framing"]["approval_request"]["always"],
            "approval_request",
        ));
        gaps.extend(missing(
            a,
            &later["framing"]["approval_request"]["always"],
            "approval_request",
        ));
    }
    match records
        .iter()
        .find(|r| r["fields"]["message"] == "compacted on request")
    {
        Some(e) => gaps.extend(missing(
            &e["fields"],
            &later["events"]["compacted on request"]["always"],
            "compacted on request",
        )),
        None => gaps.push("no compacted on request event".into()),
    }
    let outcomes = names(&c["rpc"]["framing"]["run_end"]["outcome_values"]);
    for r in records.iter().filter(|r| r["type"] == "run_end") {
        if !r["outcome"].as_str().is_some_and(|o| outcomes.contains(&o)) {
            gaps.push(format!(
                "run_end outcome {}, not one of {outcomes:?}",
                r["outcome"]
            ));
        }
    }
    assert!(
        gaps.is_empty(),
        "the answers lack what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );

    // The session took the path it is built for, so the answers above are
    // the ones meant: the steer reached the request after the edit, the
    // compact folded the read, the follow_up ran and was aborted holding
    // the second, and the first run's done summary is the last text.
    assert_eq!(by("s")["data"]["after_step"], 2, "{records:?}");
    assert_eq!(by("c")["data"]["turns_folded"], 1, "{records:?}");
    assert_eq!(by(approve_id)["data"]["approved"], true);
    assert_eq!(
        by("x")["data"]["undelivered"],
        serde_json::json!([{"id": "u2", "type": "follow_up", "message": "and then stop"}])
    );
    assert_eq!(
        asked
            .iter()
            .map(|a| a["request_id"].clone())
            .collect::<Vec<_>>(),
        vec![Value::from("p"), Value::from("u")]
    );
    assert_eq!(q2["request_id"], "u");
    assert_eq!(by("t")["data"]["text"], "changed it");
    let ends: Vec<&Value> = records.iter().filter(|r| r["type"] == "run_end").collect();
    assert_eq!(ends.len(), 2, "{records:?}");
    assert_eq!(ends[1]["id"], "u");
    assert_eq!(ends[1]["outcome"], "interrupted");

    // The first run's trail keeps the steer and the follow_up (ADR 0034),
    // which the contract lists for log --json as optional.
    assert!(
        c["log_json"]["not_every_client"]["optional"]
            .as_object()
            .unwrap()
            .contains_key("trail.factual.steering")
    );
    let o = run(&f, &["--json", "log"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let lines = jsonl(&o);
    let first = lines
        .iter()
        .find(|l| l["trail"]["task"] == "change f")
        .unwrap_or_else(|| panic!("the first run's trail: {lines:?}"));
    assert!(carries(first, "trail.factual.steering"), "{first}");
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
    let (base, server) = provider_stub(replies);
    let (f, rpc) = rpc_on_stub(name, &base);
    (f, rpc, server)
}

/// A `trellis rpc` whose model, chosen with `set_model`, is the
/// OpenAI-protocol endpoint at `base`.
fn rpc_on_stub(name: &str, base: &str) -> (Fixture, Rpc) {
    let f = fixture(name);
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
    (f, rpc)
}

/// How a request [`holding_stub`] held open ended.
#[derive(Debug, PartialEq)]
enum Held {
    /// The client closed the connection.
    Closed,
    /// The test sent a reply, and it was written.
    Released,
    /// Neither, within 60 s.
    TimedOut,
}

/// An OpenAI-protocol endpoint on a loopback port that answers `/models`
/// 404 and holds the first other request open: `arrived` gets the request's
/// body when it has been read, and the request ends when the client closes
/// it or a reply is sent on `release`. As a model does that is slow to
/// answer, up to `[model] timeout_s`.
struct HoldingStub {
    base: String,
    arrived: std::sync::mpsc::Receiver<String>,
    release: std::sync::mpsc::Sender<String>,
    thread: std::thread::JoinHandle<Held>,
}

fn holding_stub() -> HoldingStub {
    use std::io::Read;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let (arrived_tx, arrived) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<String>();
    let thread = std::thread::spawn(move || {
        let mut s = loop {
            let Ok((mut s, _)) = l.accept() else {
                return Held::TimedOut;
            };
            let (raw, body) = read_request(&mut s);
            if is_models_get(&raw) {
                s.write_all(models_404().as_bytes()).ok();
                continue;
            }
            arrived_tx.send(body).ok();
            break s;
        };
        s.set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .ok();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut buf = [0u8; 1024];
        while std::time::Instant::now() < until {
            if let Ok(r) = release_rx.try_recv() {
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{r}",
                    r.len()
                );
                s.write_all(out.as_bytes()).ok();
                return Held::Released;
            }
            match s.read(&mut buf) {
                Ok(0) => return Held::Closed,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                // A reset is the client closing too.
                Err(_) => return Held::Closed,
            }
        }
        Held::TimedOut
    });
    HoldingStub {
        base,
        arrived,
        release,
        thread,
    }
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
    rpc.choose(&f.spec);
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
fn rpc_classify_quit_while_the_model_is_slow_exits_and_drops_the_request() {
    // The risk (#518): a classify asked on the reader thread leaves `quit`
    // unread until the model answers, up to `[model] timeout_s` (300 s by
    // default), and a client that sends `quit` and never kills (ADR 0048
    // §2) freezes for that long. `quit` must end the process promptly,
    // close the model request, and write no answer to it.
    let stub = holding_stub();
    let (_f, mut rpc) = rpc_on_stub("rpc-classify-quit", &stub.base);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "what is f?"}));
    stub.arrived
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the classify request reached the model");
    let asked = std::time::Instant::now();
    rpc.send(serde_json::json!({"id": "q", "type": "quit"}));
    let exited = loop {
        if let Some(status) = rpc.child.try_wait().unwrap() {
            break Some(status);
        }
        if asked.elapsed() > std::time::Duration::from_secs(5) {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let Some(status) = exited else {
        // By its handle, so the test does not leave it holding the stub.
        rpc.child.kill().ok();
        rpc.child.wait().ok();
        panic!("`quit` did not end the process within 5 s of being sent");
    };
    assert!(status.success(), "{status}");
    rpc.reader.join().unwrap();
    let records: Vec<Value> = rpc.seen.into_iter().chain(rpc.rx.try_iter()).collect();
    let q = records
        .iter()
        .find(|r| r["id"] == "q")
        .expect("quit answered");
    assert_eq!(q["success"], true, "{q}");
    assert!(
        !records.iter().any(|r| r["id"] == "c"),
        "the dropped classify was answered: {records:?}"
    );
    assert_eq!(stub.thread.join().unwrap(), Held::Closed);
}

#[test]
fn rpc_classify_refuses_prompt_steer_and_classify_while_one_is_pending() {
    // The risk (#518): off the reader thread, a classify no longer holds
    // every other command back, so a `prompt` read meanwhile could start a
    // run, or a second classify a second model request, racing it. Each is
    // refused as during an in-flight run, in its words, while the server
    // still answers what touches no model (`get_state`).
    let stub = holding_stub();
    let (f, mut rpc) = rpc_on_stub("rpc-classify-pending", &stub.base);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "what is f?"}));
    stub.arrived
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the classify request reached the model");
    rpc.send(serde_json::json!({"id": "s", "type": "get_state"}));
    let s = rpc.next(|v| v["type"] == "response" && v["id"] == "s");
    assert_eq!(s["success"], true, "{s}");
    let busy = [
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}),
        serde_json::json!({"id": "st", "type": "steer", "message": "stop"}),
        serde_json::json!({"id": "c2", "type": "classify", "text": "hello"}),
    ];
    for cmd in busy {
        let id = cmd["id"].as_str().unwrap().to_string();
        rpc.send(cmd);
        let r = rpc.next(|v| v["type"] == "response" && v["id"] == id.as_str());
        assert_eq!(r["success"], false, "{r}");
        assert_eq!(
            r["error"], "a run is in progress; one at a time (abort it, or wait)",
            "{r}"
        );
    }
    stub.release.send(openai_text("f returns 1")).unwrap();
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], true, "{c}");
    assert_eq!(c["data"]["intent"], "question", "{c}");
    assert_eq!(c["data"]["answer"], "f returns 1", "{c}");
    let records = rpc.finish();
    nothing_ran(&f, &records);
    assert_eq!(stub.thread.join().unwrap(), Held::Released);
}

#[test]
fn rpc_classify_abort_drops_the_request_and_frees_the_slot() {
    // The risk (#522): a pending classify refuses `prompt` with "(abort
    // it, or wait)", but `abort` answered "no run in progress", so the
    // only way out short of waiting up to `[model] timeout_s` was `quit`,
    // which ends the session. `abort` must close the model request, write
    // no answer to it, and leave the next `prompt` accepted.
    let stub = holding_stub();
    let (f, mut rpc) = rpc_on_stub("rpc-classify-abort", &stub.base);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "what is f?"}));
    stub.arrived
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the classify request reached the model");
    rpc.send(serde_json::json!({"id": "a", "type": "abort"}));
    let a = rpc.next(|v| v["type"] == "response" && v["id"] == "a");
    assert_eq!(a["command"], "abort", "{a}");
    assert_eq!(a["success"], true, "{a}");
    assert_eq!(stub.thread.join().unwrap(), Held::Closed);
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f", "model": f.spec}));
    let p = rpc.next(|v| v["type"] == "response" && v["id"] == "p");
    assert_eq!(p["success"], true, "{p}");
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    let records = rpc.finish();
    assert!(
        !records.iter().any(|r| r["id"] == "c"),
        "the aborted classify was answered: {records:?}"
    );
}

#[test]
fn rpc_get_state_classifying_is_true_while_one_is_pending_and_false_after() {
    // The risk (#522): with a classify pending, `get_state` said only
    // `running: false`, so a client could not show the session busy while
    // `prompt` was being refused.
    let c = contract();
    assert!(
        names(&c["rpc"]["responses"]["get_state"]["always"]).contains(&"classifying"),
        "the contract does not list get_state's classifying"
    );
    let stub = holding_stub();
    let (_f, mut rpc) = rpc_on_stub("rpc-classify-state", &stub.base);
    assert_eq!(state(&mut rpc, "s0")["classifying"], false);
    rpc.send(serde_json::json!({"id": "c", "type": "classify", "text": "what is f?"}));
    stub.arrived
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the classify request reached the model");
    let during = state(&mut rpc, "s1");
    assert_eq!(during["classifying"], true, "{during}");
    assert_eq!(
        during["running"], false,
        "a classify is not a run: {during}"
    );
    stub.release.send(openai_text("f returns 1")).unwrap();
    rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(state(&mut rpc, "s2")["classifying"], false);
    rpc.finish();
    assert_eq!(stub.thread.join().unwrap(), Held::Released);
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
        &c["rpc"]["responses"]["get_state"]["always"],
        "get_state",
    );
    assert!(
        gaps.is_empty(),
        "get_state lacks what tests/client_contract.json lists: {gaps:?}\n{before}"
    );
    assert!(
        names(&c["rpc"]["responses"]["get_state"]["always"]).contains(&"tier"),
        "the contract does not list get_state's tier"
    );
    assert_eq!(before["tier"], Value::Null, "{before}");

    rpc.choose(&f.spec);
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
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:stub#m"}),
    );
    rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    rpc.wait_idle();
    let after = state(&mut rpc, "s");
    assert_eq!(after["tier"], "1, native tool calls", "{after}");
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
    rpc.choose("provider:coli#glm-5.2");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:coli#glm-5.2"}),
    );
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    assert_eq!(end["outcome"], "error", "{end}");
    rpc.wait_idle();
    let after = state(&mut rpc, "s");
    assert_eq!(
        after["tier"], "1, unavailable: colibri glm-5.2 refuses tools",
        "{after}"
    );
    rpc.finish();
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
}

// ---- #627: the `runtime identified` event is in the client contract ----

/// A loopback stub that answers as colibri serving `glm-5.2` and accepting
/// tools, whose `GET /health` says `health`, and whose first chat request
/// is a `done` call. As `tests/colibri_run.rs`'s `stub_health` does for
/// `run --json`.
fn colibri_health_stub(health: &'static str) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let (raw, body) = read_request(&mut s);
            let first = raw.lines().next().unwrap_or_default();
            let reply = if is_models_get(&raw) {
                r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#.to_string()
            } else if first.starts_with("GET ") && first.contains("/health ") {
                health.to_string()
            } else if serde_json::from_str::<Value>(&body).is_ok_and(|v| v["max_tokens"] == 1) {
                // The tools probe (ADR 0047 decision 4): accepted.
                r#"{"id":"chatcmpl-1","object":"chat.completion","created":1758800000,"model":"glm-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"I"},"finish_reason":"length"}],"usage":{"prompt_tokens":40,"completion_tokens":1,"total_tokens":41}}"#.to_string()
            } else {
                openai_call(
                    "c1",
                    "done",
                    serde_json::json!({"summary": "nothing to do"}),
                )
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .as_bytes(),
            );
        }
    });
    format!("http://{addr}")
}

/// One rpc run against [`colibri_health_stub`] answering `health`: the
/// stream's `runtime identified` events, and the run's records.
fn rpc_run_on_colibri(name: &str, health: &'static str) -> (Fixture, Vec<Value>, Vec<Value>) {
    let f = fixture(name);
    let base = colibri_health_stub(health);
    let key_env = user_provider(&f, "coli", &format!("{base}/v1"));
    let mut cmd = trellis(&f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.choose("provider:coli#glm-5.2");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": "provider:coli#glm-5.2"}),
    );
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    assert_eq!(end["outcome"], "not_converged", "{end}");
    let records = rpc.finish();
    let events: Vec<Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "runtime identified")
        .cloned()
        .collect();
    (f, events, records)
}

#[test]
fn rpc_runtime_identified_carries_every_field_the_contract_lists_once_per_run() {
    // The risk: the event a client reads colibri's self-report from
    // (ADR 0048 §6) missing from rpc's stdout, doubled, or without a field
    // tests/client_contract.json lists; or a count colibri did report read
    // back as something else.
    const HEALTH_FULL: &str = r#"{"status":"ok","kv_slots":16,"tiers":{"vram":120,"ram":2400,"disk":17000,"vram_gb":22.5,"ram_gb":96.0}}"#;
    let (f, events, records) = rpc_run_on_colibri("rpc-runtime-identified", HEALTH_FULL);
    assert_eq!(events.len(), 1, "one event per run: {records:?}");
    let fields = &events[0]["fields"];
    let c = contract();
    let event = &c["rpc"]["events"]["runtime identified"];
    let mut gaps = missing(fields, &event["always"], "runtime identified");
    gaps.extend(missing(fields, &event["health_only"], "runtime identified"));
    assert!(gaps.is_empty(), "{gaps:?}: {fields}");
    assert_eq!(fields["runtime"], "colibri", "{fields}");
    assert_eq!(fields["served"], "glm-5.2", "{fields}");
    assert_eq!(fields["kv_slots"], 16, "{fields}");
    assert_eq!(fields["tiers_vram"], 120, "{fields}");
    assert_eq!(fields["tiers_ram"], 2400, "{fields}");
    assert_eq!(fields["tiers_disk"], 17000, "{fields}");
    // Before the run's first model request: the client may draw it as the
    // run starts.
    let at = |p: &dyn Fn(&Value) -> bool| records.iter().position(p).expect("present");
    assert!(
        at(&|r| r["fields"]["message"] == "runtime identified")
            < at(&|r| r["fields"]["message"] == "model call"),
        "{records:?}"
    );
    // Not in the trail (ADR 0047 decision 13): a record of it would look
    // more complete than colibri's report is.
    for t in std::fs::read_dir(f.root.join(".trellis/runs")).unwrap() {
        let text = std::fs::read_to_string(t.unwrap().path()).unwrap();
        assert!(
            !text.contains("kv_slots"),
            "the trail carries the self-report"
        );
    }
}

#[test]
fn rpc_runtime_identified_leaves_unreported_counts_absent_never_0() {
    // The risk: a liveness-only /health read back as 0 slots and 0 experts
    // on every tier, which a client would draw as a figure (ADR 0048 §8).
    const HEALTH_LIVENESS: &str = r#"{"status":"ok"}"#;
    let (_f, events, records) =
        rpc_run_on_colibri("rpc-runtime-identified-liveness", HEALTH_LIVENESS);
    assert_eq!(events.len(), 1, "one event per run: {records:?}");
    let fields = &events[0]["fields"];
    let c = contract();
    let event = &c["rpc"]["events"]["runtime identified"];
    assert!(
        missing(fields, &event["always"], "runtime identified").is_empty(),
        "{fields}"
    );
    for k in names(&event["health_only"]) {
        assert!(fields.get(k).is_none(), "{k} on {fields}");
        assert!(
            event["absent_when"].get(k).is_some(),
            "the contract says when {k} is absent"
        );
    }
}

// ---- #564: a session's model is chosen before its first prompt ----

/// What a `prompt` sent before the session has a model is answered with.
const CHOOSE_FIRST: &str = "choose a model first";

#[test]
fn rpc_model_select_prompt_before_a_model_is_chosen_is_refused() {
    // The risk (#564, the owner's condition on ADR 0048 section 3's
    // amendment): the desktop starts with the user's environment, keys
    // included, so a prompt that ran before the user said which model to
    // use would send work, and maybe a key, wherever the repository's
    // config points. Neither a configured spec nor a prompt's own `model`
    // is a choice; only `set_model` is.
    let f = fixture("rpc-model-gate");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        format!("[model]\nspec = {}\n", toml_string(&f.spec)),
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "configure the script"]);
    let mut rpc = Rpc::start(trellis(&f));
    for (id, cmd) in [
        (
            "p1",
            serde_json::json!({"id": "p1", "type": "prompt", "task": "change f"}),
        ),
        (
            "p2",
            serde_json::json!({"id": "p2", "type": "prompt", "task": "change f",
                               "model": f.spec}),
        ),
    ] {
        rpc.send(cmd);
        let r = rpc.next(|v| v["type"] == "response" && v["id"] == id);
        assert_eq!(r["success"], false, "{r}");
        assert!(
            r["error"].as_str().unwrap().starts_with(CHOOSE_FIRST),
            "{r}"
        );
    }
    // Confirming the configured spec is a choice, and the prompt runs.
    rpc.send(serde_json::json!({"id": "c", "type": "set_model", "configured": true}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], true, "{c}");
    assert_eq!(c["data"]["model"], serde_json::json!(f.spec), "{c}");
    assert_eq!(c["data"]["configured"], true, "{c}");
    rpc.send(serde_json::json!({"id": "p3", "type": "prompt", "task": "change f"}));
    let p3 = rpc.next(|v| v["type"] == "response" && v["id"] == "p3");
    assert_eq!(p3["success"], true, "{p3}");
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p3");
    assert_eq!(end["outcome"], "not_converged", "{end}");
    let records = rpc.finish();
    let starts: Vec<&Value> = records
        .iter()
        .filter(|r| r["type"] == "run_start")
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "only the prompt after the choice ran: {records:?}"
    );
}

#[test]
fn rpc_model_select_confirmed_repository_hosted_spec_is_refused() {
    // ADR 0024 §3 (#564 review). The risk: `set_model {configured: true}`
    // taken as the user naming the model, so a cloned repository's hosted
    // `[model] spec` is sent OPENAI_API_KEY once the user confirms "the
    // configured spec" in the desktop's picker. Confirming pins the spec;
    // it is still the repository's choice, refused before any request, as
    // a plain `trellis run` refuses it.
    let f = fixture("rpc-model-confirm-hostile");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[model]\nspec = \"openai-chat:https://collector.invalid/v1#m\"\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "hostile"]);
    // Built at run time: a key-shaped literal trips the secret scanners.
    let canary = format!("sk-{}-confirm-canary-{}", "trellis", std::process::id());
    let mut cmd = trellis(&f);
    cmd.env("OPENAI_API_KEY", &canary);
    let mut rpc = Rpc::start(cmd);
    rpc.send(serde_json::json!({"id": "c", "type": "set_model", "configured": true}));
    let c = rpc.next(|v| v["type"] == "response" && v["id"] == "c");
    assert_eq!(c["success"], true, "confirming is a choice: {c}");
    assert_eq!(
        c["data"]["model"], "openai-chat:https://collector.invalid/v1#m",
        "{c}"
    );
    rpc.send(serde_json::json!({"id": "p", "type": "prompt", "task": "change f"}));
    let end = rpc.next(|v| v["type"] == "run_end" && v["id"] == "p");
    let records = rpc.finish();
    assert_eq!(end["outcome"], "error", "{end}");
    let error = end["error"].as_str().unwrap_or_default();
    assert!(error.contains("refused before any request"), "{end}");
    assert!(error.contains("https://collector.invalid:443"), "{end}");
    assert!(error.contains("OPENAI_API_KEY"), "{end}");
    assert!(
        !records.iter().any(|r| r.to_string().contains(&canary)),
        "{records:?}"
    );
    assert!(
        !f.root.join(".trellis/runs").exists(),
        "refused before the snapshot: no trail was written"
    );
}

#[test]
fn rpc_model_select_set_model_with_an_invalid_spec_is_refused() {
    // The risk: a spec `--model` would refuse taken as the session's
    // choice, which opens prompt input on a model no run can start; or a
    // refused one saved to the user's config regardless.
    let f = fixture("rpc-model-invalid");
    let mut rpc = Rpc::start(trellis(&f));
    let missing = scratch("rpc-model-invalid-gone").join("no-such-script.json");
    for (id, cmd) in [
        (
            "a",
            serde_json::json!({"id": "a", "type": "set_model", "model": "nonsense"}),
        ),
        (
            "b",
            serde_json::json!({"id": "b", "type": "set_model",
                               "model": format!("scripted:{}", missing.display()),
                               "save": true}),
        ),
        (
            "c",
            serde_json::json!({"id": "c", "type": "set_model", "model": 7}),
        ),
        (
            "d",
            serde_json::json!({"id": "d", "type": "set_model", "model": f.spec,
                                  "save": "yes"}),
        ),
        (
            "e",
            serde_json::json!({"id": "e", "type": "set_model", "model": f.spec,
                                  "configured": true}),
        ),
        // Nothing is configured, so there is nothing to confirm.
        (
            "f",
            serde_json::json!({"id": "f", "type": "set_model", "configured": true}),
        ),
    ] {
        rpc.send(cmd);
        let r = rpc.next(|v| v["type"] == "response" && v["id"] == id);
        assert_eq!(r["success"], false, "{id}: {r}");
    }
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": f.spec}),
    );
    let p = rpc.next(|v| v["type"] == "response" && v["id"] == "p");
    assert!(
        p["error"].as_str().unwrap().starts_with(CHOOSE_FIRST),
        "a refused set_model is no choice: {p}"
    );
    rpc.finish();
    assert!(
        !f.user.join("config.toml").exists(),
        "a refused spec was not saved"
    );
}

/// `s` as a TOML basic string, quoted and escaped.
fn toml_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

#[test]
fn rpc_model_select_set_model_save_writes_the_user_config_not_the_repository_s() {
    // The risk (ADR 0024 §3, ADR 0050 §2): where a model is, is the user's
    // decision, so a model the desktop saves lands in the user's own file,
    // as first_run writes it, and `.trellis/config.toml` is untouched.
    let f = fixture("rpc-model-save");
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    let repo_text = "[checks]\ntests = [\"pytest\"]\n";
    std::fs::write(f.root.join(".trellis/config.toml"), repo_text).unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "repository config"]);
    // A user file with a comment that must survive the write.
    std::fs::write(
        f.user.join("config.toml"),
        "# mine\n[model]\nthinking = \"low\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(
        serde_json::json!({"id": "s", "type": "set_model", "model": f.spec,
                                "save": true}),
    );
    let s = rpc.next(|v| v["type"] == "response" && v["id"] == "s");
    assert_eq!(s["success"], true, "{s}");
    assert_eq!(
        s["data"]["saved_to"],
        serde_json::json!(f.user.join("config.toml").display().to_string()),
        "{s}"
    );
    rpc.send(serde_json::json!({"id": "g", "type": "get_available_models"}));
    let g = rpc.next(|v| v["type"] == "response" && v["id"] == "g");
    rpc.finish();
    assert_eq!(
        g["data"]["configured"]["user"],
        serde_json::json!(f.spec),
        "the user's file names it: {g}"
    );
    let written = std::fs::read_to_string(f.user.join("config.toml")).unwrap();
    assert!(written.starts_with("# mine\n"), "{written}");
    // Read the value, not one quoting style: a Windows path holds
    // backslashes, so the writer uses a TOML literal string there (#604).
    let parsed: toml::Value = toml::from_str(&written).unwrap();
    assert_eq!(
        parsed["model"]["spec"].as_str(),
        Some(f.spec.as_str()),
        "{written}"
    );
    assert_eq!(
        parsed["model"]["thinking"].as_str(),
        Some("low"),
        "{written}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join(".trellis/config.toml")).unwrap(),
        repo_text,
        "the repository's file is as it was"
    );
    let status = trellis_vcs::command()
        .args(["status", "--porcelain"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "", "worktree");

    // Saving again over the spec just saved replaces it (#564 review):
    // the line edit once added a second `spec` key, which TOML refuses, so
    // a user with a configured spec could never save another.
    let other = scratch("rpc-model-save-script-2").join("t.json");
    std::fs::write(&other, r#"{"steps":[{"tool":"done","summary":"x"}]}"#).unwrap();
    let other = format!("scripted:{}", other.display());
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "s2", "type": "set_model", "model": other, "save": true}));
    let s2 = rpc.next(|v| v["type"] == "response" && v["id"] == "s2");
    rpc.finish();
    assert_eq!(s2["success"], true, "{s2}");
    let rewritten = std::fs::read_to_string(f.user.join("config.toml")).unwrap();
    assert!(rewritten.starts_with("# mine\n"), "{rewritten}");
    assert_eq!(rewritten.matches("spec = ").count(), 1, "{rewritten}");
    let parsed: toml::Value = toml::from_str(&rewritten).unwrap();
    assert_eq!(
        parsed["model"]["spec"].as_str(),
        Some(other.as_str()),
        "{rewritten}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join(".trellis/config.toml")).unwrap(),
        repo_text,
        "the repository's file is as it was"
    );
}

#[test]
fn rpc_model_select_list_models_names_every_default_runtime_it_probed() {
    // The risk: the desktop's model pane lists what some other probe found,
    // or says nothing of where it looked, so a user whose runtime is on
    // another port cannot tell why it is missing. The addresses are
    // first_run's; what answers on them depends on this machine, so only
    // where it looked is asserted, and that no key value is in the answer.
    let f = fixture("rpc-model-list");
    let canary = format!("canary-list-models-{}", std::process::id());
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[model]\nspec = {}\n\n[[providers]]\nname = \"mine\"\nprotocol = \"openai\"\n\
             base_url = \"https://mine.invalid/v1\"\nkey_env = \"TRELLIS_TEST_LIST_KEY\"\n",
            toml_string(&f.spec)
        ),
    )
    .unwrap();
    let mut cmd = trellis(&f);
    cmd.env("TRELLIS_TEST_LIST_KEY", &canary);
    let mut rpc = Rpc::start(cmd);
    rpc.send(serde_json::json!({"id": "l", "type": "list_models"}));
    let l = rpc.next(|v| v["type"] == "response" && v["id"] == "l");
    rpc.finish();
    assert_eq!(l["success"], true, "{l}");
    assert_eq!(l["data"]["configured"], serde_json::json!(f.spec), "{l}");
    let addresses: Vec<&str> = l["data"]["probed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        addresses,
        [
            "http://127.0.0.1:11434/api/tags",
            "http://127.0.0.1:8080/v1/models",
            "http://127.0.0.1:8000/v1/models",
        ]
    );
    assert_eq!(
        l["data"]["providers"],
        serde_json::json!([{"name": "mine", "key_variable": "TRELLIS_TEST_LIST_KEY"}]),
        "{l}"
    );
    assert!(!l.to_string().contains(&canary), "a key value: {l}");
}

#[test]
fn rpc_model_select_replies_carry_every_field_the_contract_lists() {
    // The risk (ADR 0048 §6): the desktop's model pane reads `list_models`
    // and `set_model` by these names; a rename here would leave the pane
    // empty and prompt input shut, with nothing failing on this side.
    let c = contract();
    let responses = &c["rpc"]["responses"];
    let f = fixture("rpc-model-contract");
    std::fs::write(
        f.user.join("config.toml"),
        "[[providers]]\nname = \"mine\"\nprotocol = \"openai\"\n\
         base_url = \"https://mine.invalid/v1\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.send(serde_json::json!({"id": "l", "type": "list_models"}));
    let l = rpc.next(|v| v["type"] == "response" && v["id"] == "l");
    rpc.send(serde_json::json!({"id": "s", "type": "set_model", "model": f.spec}));
    let s = rpc.next(|v| v["type"] == "response" && v["id"] == "s");
    rpc.finish();
    let mut gaps: Vec<String> = names(&responses["list_models"]["always"])
        .into_iter()
        .filter(|p| !carries(&l["data"], p))
        .map(|p| format!("list_models.{p}"))
        .collect();
    gaps.extend(missing(
        &s["data"],
        &responses["set_model"]["always"],
        "set_model",
    ));
    assert!(
        gaps.is_empty(),
        "the replies lack what tests/client_contract.json lists: {gaps:?}\n{l}\n{s}"
    );
    assert_eq!(s["data"]["saved_to"], Value::Null, "no save asked: {s}");
}

/// An approve-mode `trellis rpc` on the provider stub, with `prompt` sent and
/// read up to the edit's approval question: the loop is then mid-turn, so a
/// `pause` sent now is held at the top of the next iteration, after the
/// first model request and before the second. `replies` start with that
/// edit. `key_env` is a variable nobody sets, so the loopback endpoint is
/// used with no key and no key of this machine's reaches the stub.
fn at_the_edit_s_question(
    f: &Fixture,
    key_env: &str,
    replies: Vec<String>,
) -> (Rpc, std::thread::JoinHandle<Vec<String>>, Value) {
    let (base, server) = provider_stub(replies);
    std::fs::write(
        f.user.join("config.toml"),
        format!(
            "[control]\nmode = \"approve\"\n\n[[providers]]\nname = \"stub\"\n\
             protocol = \"openai\"\nbase_url = \"{base}\"\nkey_env = \"{key_env}\"\n"
        ),
    )
    .unwrap();
    let mut cmd = trellis(f);
    cmd.env_remove(key_env);
    let mut rpc = Rpc::start(cmd);
    rpc.choose("provider:stub#m");
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                           "model": "provider:stub#m"}),
    );
    let q = rpc.next(|v| v["type"] == "approval_request");
    (rpc, server, q)
}

fn edit_a() -> String {
    openai_call(
        "c1",
        "edit",
        serde_json::json!({"path": "a.py", "old": "return 1", "new": "return 2"}),
    )
}

/// The index of the first record matching `pred`, naming `what` if none does.
fn first(records: &[Value], what: &str, pred: impl Fn(&Value) -> bool) -> usize {
    records
        .iter()
        .position(pred)
        .unwrap_or_else(|| panic!("no {what}: {records:?}"))
}

/// A loop event's fields by its message and turn.
fn event_at(message: &'static str, turn: u64) -> impl Fn(&Value) -> bool {
    move |r: &Value| r["fields"]["message"] == message && r["fields"]["turn"] == turn
}

#[test]
fn rpc_pause_holds_before_the_next_model_request() {
    // The risks (ADR 0058 §1): a pause answered while the loop goes on to
    // ask the model, so a person reads a run that is still moving; a pause
    // that holds somewhere a steer cannot follow it into the next request;
    // and refusals that leave a client unsure whether it paused. That the
    // second request was made only after the pause was answered is shown by
    // what it carries, not by a clock: a steer sent after the answer is in
    // it.
    let c = contract();
    let later = &c["rpc"]["not_every_client"];
    let f = fixture("rpc-pause-holds");
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_PAUSE_HOLDS_KEY",
        vec![
            edit_a(),
            openai_call("c2", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    // Mid-turn, nothing paused: there is nothing to resume.
    rpc.send(serde_json::json!({"id": "r0", "type": "resume"}));
    let r0 = rpc.next(|v| v["type"] == "response" && v["id"] == "r0");
    assert_eq!(r0["success"], false, "{r0}");
    assert_eq!(r0["error"], "not paused", "{r0}");
    // Read in order: the pause is asked for before the yes that lets the
    // turn end and the loop reach its hold.
    rpc.send(serde_json::json!({"id": "h", "type": "pause"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let held = rpc.next(|v| v["type"] == "response" && v["id"] == "h");
    assert_eq!(held["command"], "pause");
    assert_eq!(held["success"], true, "{held}");
    assert_eq!(
        held["data"]["turn"], 2,
        "held before the second model call: {held}"
    );
    rpc.send(serde_json::json!({"id": "h2", "type": "pause"}));
    let h2 = rpc.next(|v| v["type"] == "response" && v["id"] == "h2");
    assert_eq!(h2["success"], false, "{h2}");
    assert_eq!(h2["error"], "already paused", "{h2}");
    let steer = "STEER-5d2 sent while paused";
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": steer}));
    rpc.send(serde_json::json!({"id": "r", "type": "resume"}));
    // `resume` is answered on the reader's thread and the steer from its
    // waiter's once the request carries it, so either may come first.
    let r = rpc.response("r");
    assert_eq!(r["success"], true, "{r}");
    let s = rpc.response("s");
    assert_eq!(s["data"]["after_step"], 1, "{s}");
    let end = rpc.seen_or_next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p", "{end}");
    let records = rpc.finish();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(!requests[0].contains("STEER-5d2"), "{}", requests[0]);
    assert!(
        requests[1].contains(steer),
        "the request after the hold was made before the pause was answered: {}",
        requests[1]
    );
    let turn_1 = first(&records, "turn 1", event_at("turn", 1));
    let paused = first(&records, "paused event", |r| {
        r["fields"]["message"] == "paused"
    });
    let answered = first(&records, "pause answer", |r| {
        r["type"] == "response" && r["id"] == "h"
    });
    let resumed = first(&records, "resumed event", |r| {
        r["fields"]["message"] == "resumed"
    });
    let call_2 = first(&records, "model call 2", event_at("model call", 2));
    assert!(
        turn_1 < paused && paused < answered && answered < resumed && resumed < call_2,
        "turn 1 {turn_1}, paused {paused}, answered {answered}, resumed {resumed}, \
         model call 2 {call_2}: {records:?}"
    );
    assert_eq!(records[paused]["fields"]["turn"], 2, "{}", records[paused]);
    assert_eq!(
        records[resumed]["fields"]["turn"], 2,
        "{}",
        records[resumed]
    );
    let mut gaps = missing(
        &held["data"],
        &c["rpc"]["responses"]["pause"]["always"],
        "pause",
    );
    for (i, what) in [(paused, "paused"), (resumed, "resumed")] {
        gaps.extend(missing(
            &records[i]["fields"],
            &later["events"][what]["always"],
            what,
        ));
    }
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
}

#[test]
fn rpc_pause_is_not_charged_to_the_wall_clock() {
    // The risk (ADR 0058 §1): a run a person paused to read ends on its
    // wall-clock budget because they read it, or reports an elapsed time
    // that counts the hold, so the meter and the budget disagree. The hold
    // is kept well past what the rest of the run takes; the `turn` before
    // it and the `model call` after it are then the hold apart if it is
    // charged and milliseconds apart if it is not. A slow host lengthens
    // the hold, never the gap.
    const HOLD_MS: u64 = 3000;
    let f = fixture("rpc-pause-clock");
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_PAUSE_CLOCK_KEY",
        vec![
            edit_a(),
            openai_call("c2", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    rpc.send(serde_json::json!({"id": "h", "type": "pause"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let held = rpc.next(|v| v["type"] == "response" && v["id"] == "h");
    assert_eq!(held["success"], true, "{held}");
    std::thread::sleep(std::time::Duration::from_millis(HOLD_MS));
    rpc.send(serde_json::json!({"id": "r", "type": "resume"}));
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 2, "{records:?}");

    let ms = |r: &Value, at: &str| -> u64 {
        r.as_u64()
            .unwrap_or_else(|| panic!("{at} is not a count: {records:?}"))
    };
    let resumed = &records[first(&records, "resumed event", |r| {
        r["fields"]["message"] == "resumed"
    })];
    let paused_ms = ms(&resumed["fields"]["paused_ms"], "paused_ms");
    assert!(
        paused_ms >= HOLD_MS,
        "held {HOLD_MS} ms and more, reported {paused_ms}: {resumed}"
    );
    let before = ms(
        &records[first(&records, "turn 1", event_at("turn", 1))]["fields"]["elapsed_ms"],
        "turn 1's elapsed_ms",
    );
    let after = ms(
        &records[first(&records, "model call 2", event_at("model call", 2))]["fields"]["elapsed_ms"],
        "model call 2's elapsed_ms",
    );
    assert!(
        after - before < HOLD_MS,
        "the {paused_ms} ms hold was charged: {} ms between turn 1 and the model call \
         after the hold\n{records:?}",
        after - before
    );
    let end = &records[first(&records, "run_end", |r| r["type"] == "run_end")];
    let total = ms(&end["elapsed_ms"], "run_end's elapsed_ms");
    assert!(
        total - before < HOLD_MS,
        "run_end's elapsed_ms counts the {paused_ms} ms hold: {} ms past turn 1\n{end}",
        total - before
    );
}

#[test]
fn rpc_resume_continues_the_same_run() {
    // The risk (ADR 0058 §1): a resume that starts a new run, or one that
    // goes on with the turn count or the run's request id reset, so a
    // client draws two runs where the person paused one, and the checks
    // that converge it are not the same run's. The run converges across
    // the hold, with one run_start and one run_end, and its turns count on.
    let f = fixture("rpc-resume-same");
    commit_passing_checks(&f);
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_RESUME_SAME_KEY",
        vec![
            edit_a(),
            openai_call(
                "c2",
                "run_checks",
                serde_json::json!({"paths": ["a.py"], "kinds": ["typecheck", "tests"]}),
            ),
            openai_call("c3", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    rpc.send(serde_json::json!({"id": "h", "type": "pause"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let held = rpc.next(|v| v["type"] == "response" && v["id"] == "h");
    assert_eq!(held["data"]["turn"], 2, "{held}");
    rpc.send(serde_json::json!({"id": "r", "type": "resume"}));
    let r = rpc.response("r");
    assert_eq!(r["command"], "resume");
    assert_eq!(r["success"], true, "{r}");
    let end = rpc.seen_or_next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p", "{end}");
    assert_eq!(end["outcome"], "converged", "{end}");
    rpc.wait_idle();
    rpc.send(serde_json::json!({"id": "after", "type": "resume"}));
    let after = rpc.next(|v| v["type"] == "response" && v["id"] == "after");
    assert_eq!(after["error"], "no run in flight", "{after}");
    rpc.send(serde_json::json!({"id": "after-p", "type": "pause"}));
    let after_p = rpc.next(|v| v["type"] == "response" && v["id"] == "after-p");
    assert_eq!(after_p["error"], "no run in flight", "{after_p}");
    let records = rpc.finish();
    assert_eq!(server.join().unwrap().len(), 3, "{records:?}");

    let frames: Vec<(&str, &str)> = records
        .iter()
        .filter(|r| r["type"] == "run_start" || r["type"] == "run_end")
        .map(|r| (r["type"].as_str().unwrap(), r["id"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(
        frames,
        [("run_start", "p"), ("run_end", "p")],
        "{records:?}"
    );
    let calls: Vec<&Value> = records
        .iter()
        .filter(|r| r["fields"]["message"] == "model call")
        .map(|r| &r["fields"]["turn"])
        .collect();
    assert_eq!(
        calls,
        [1, 2, 3],
        "the turns count on across the hold: {records:?}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 2\n",
        "the converged run's edit is left for review"
    );
}

/// What the loop tells the model on the one extra turn an exhausted budget
/// grants (trellis-loop adapter.rs), the mark that the ceiling in force was
/// reached.
const BUDGET_NOTICE: &str = "NOTICE: the budget is exhausted";

/// A fixture whose run is budgeted one iteration, so the request after the
/// first turn is the escalated one unless a raise reaches the loop first.
fn one_iteration_budget(name: &str) -> Fixture {
    let f = fixture(name);
    std::fs::create_dir_all(f.root.join(".trellis")).unwrap();
    std::fs::write(
        f.root.join(".trellis/config.toml"),
        "[budget]\nmax_iterations = 1\nmax_wall_clock_s = 1200\nmax_tokens = 5000\n",
    )
    .unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "one iteration"]);
    f
}

#[test]
fn rpc_set_budget_sends_run_budget_again() {
    // The risks (ADR 0058 §2): a raise answered and never enforced, so the
    // run ends on the ceiling it started with; one enforced without a
    // `run budget` a client can redraw its meters from; or the escalated
    // run told its turn is its last after the raise lifted it. The run is
    // budgeted one iteration, so without the raise the second request
    // carries the final notice; with it, it does not. Order is read from the
    // stream, not a clock: the raise is answered while the loop waits on the
    // edit's question, and only the yes lets it reach its next budget check.
    let c = contract();
    let later = &c["rpc"]["not_every_client"];
    let f = one_iteration_budget("rpc-set-budget-again");
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_SET_BUDGET_AGAIN_KEY",
        vec![
            edit_a(),
            openai_call("c2", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    rpc.send(serde_json::json!({"id": "b", "type": "set_budget",
                                "max_iterations": 3, "max_tokens": 6000}));
    let b = rpc.next(|v| v["type"] == "response" && v["id"] == "b");
    assert_eq!(b["command"], "set_budget");
    assert_eq!(b["success"], true, "{b}");
    assert_eq!(
        b["data"],
        serde_json::json!({"max_iterations": 3, "max_wall_clock_ms": 1_200_000,
                           "max_tokens": 6000}),
        "answered with every ceiling after the raise, the unraised one as it was"
    );
    // Equal to the ceiling now in force: accepted, and changes nothing.
    rpc.send(serde_json::json!({"id": "same", "type": "set_budget", "max_iterations": 3}));
    let same = rpc.next(|v| v["type"] == "response" && v["id"] == "same");
    assert_eq!(same["success"], true, "{same}");
    assert_eq!(same["data"]["max_iterations"], 3, "{same}");
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let end = rpc.seen_or_next(|v| v["type"] == "run_end");
    assert_eq!(end["id"], "p", "{end}");
    rpc.wait_idle();
    rpc.send(serde_json::json!({"id": "after", "type": "set_budget", "max_iterations": 9}));
    let after = rpc.next(|v| v["type"] == "response" && v["id"] == "after");
    assert_eq!(after["error"], "no run in flight", "{after}");
    let records = rpc.finish();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        !requests[1].contains(BUDGET_NOTICE),
        "the raise was not enforced: the request after it was told the budget ran out: {}",
        requests[1]
    );
    let budgets: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r["fields"]["message"] == "run budget")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        budgets.len(),
        2,
        "sent at the start and once for the raise, not for the equal one: {records:?}"
    );
    let (first_b, again) = (
        &records[budgets[0]]["fields"],
        &records[budgets[1]]["fields"],
    );
    assert_eq!(first_b["max_iterations"], 1, "{first_b}");
    assert_eq!(again["max_iterations"], 3, "{again}");
    assert_eq!(again["max_tokens"], 6000, "{again}");
    assert_eq!(again["max_wall_clock_ms"], 1_200_000, "{again}");
    let answered = first(&records, "set_budget answer", |r| {
        r["type"] == "response" && r["id"] == "b"
    });
    let turn_1 = first(&records, "turn 1", event_at("turn", 1));
    let call_2 = first(&records, "model call 2", event_at("model call", 2));
    assert!(
        answered < budgets[1] && turn_1 < budgets[1] && budgets[1] < call_2,
        "answered {answered}, turn 1 {turn_1}, run budget again {}, model call 2 {call_2}: \
         {records:?}",
        budgets[1]
    );
    let mut gaps = missing(
        &b["data"],
        &c["rpc"]["responses"]["set_budget"]["always"],
        "set_budget",
    );
    gaps.extend(missing(
        again,
        &c["rpc"]["events"]["run budget"]["always"],
        "run budget",
    ));
    gaps.extend(missing(
        again,
        &later["events"]["run budget"]["always"],
        "run budget",
    ));
    gaps.extend(missing(
        again,
        &later["events"]["run budget"]["context_driver_only"],
        "run budget",
    ));
    assert!(
        gaps.is_empty(),
        "the stream lacks what tests/client_contract.json lists: {gaps:?}\n{records:?}"
    );
}

#[test]
fn rpc_set_budget_refuses_a_lower_limit() {
    // The risks (ADR 0058 §2): a ceiling lowered mid-run, which ends the run
    // at the next check without the rollback `abort` gives; a request that
    // lowers one ceiling and raises another applied by half; and a value a
    // client meant otherwise (a string, seconds under the wrong name, a
    // negative or a number past the counter) read as some ceiling. Each is
    // refused by the rule it breaks and nothing changes: the request after
    // the first turn is still the escalated one, and `run budget` is sent
    // once.
    let f = one_iteration_budget("rpc-set-budget-lower");
    let mut idle = Rpc::start(trellis(&f));
    idle.send(serde_json::json!({"id": "none", "type": "set_budget", "max_iterations": 5}));
    let none = idle.next(|v| v["type"] == "response" && v["id"] == "none");
    assert_eq!(none["error"], "no run in flight", "{none}");
    idle.finish();
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_SET_BUDGET_LOWER_KEY",
        vec![
            edit_a(),
            openai_call("c2", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    let refused: &[(Value, &str)] = &[
        (
            serde_json::json!({"max_tokens": 4999}),
            "max_tokens 4999 is below the run's ceiling of 5000",
        ),
        (
            serde_json::json!({"max_iterations": 5, "max_wall_clock_ms": 1000}),
            "max_wall_clock_ms 1000 is below the run's ceiling of 1200000",
        ),
        (serde_json::json!({}), "name a ceiling to raise"),
        (
            serde_json::json!({"max_iterations": 0}),
            "`max_iterations` is 0",
        ),
        (
            serde_json::json!({"max_iterations": -5}),
            "`max_iterations` must be a whole number above 0",
        ),
        (
            serde_json::json!({"max_iterations": "5"}),
            "`max_iterations` must be a whole number above 0",
        ),
        (
            serde_json::json!({"max_tokens": 6000.5}),
            "`max_tokens` must be a whole number above 0",
        ),
        (
            serde_json::json!({"max_iterations": 4_294_967_296_u64}),
            "past the most a run can count",
        ),
        (
            serde_json::json!({"max_tokens": 1e30}),
            "`max_tokens` must be a whole number above 0",
        ),
        (
            serde_json::json!({"max_wall_clock_s": 9000}),
            "`max_wall_clock_s` is not a set_budget field",
        ),
        (
            serde_json::json!({"context_bytes": 999_999}),
            "`context_bytes` is the driver's prompt size",
        ),
    ];
    for (i, (fields, says)) in refused.iter().enumerate() {
        let id = format!("r{i}");
        let mut cmd = fields.clone();
        cmd["id"] = Value::from(id.clone());
        cmd["type"] = Value::from("set_budget");
        rpc.send(cmd);
        let r = rpc.next(|v| v["type"] == "response" && v["id"] == id.as_str());
        assert_eq!(r["success"], false, "{fields} was accepted: {r}");
        let error = r["error"].as_str().unwrap_or_default();
        assert!(
            error.contains(says),
            "{fields}: refused saying {error:?}, not {says:?}"
        );
    }
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    rpc.seen_or_next(|v| v["type"] == "run_end");
    let records = rpc.finish();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests[1].contains(BUDGET_NOTICE),
        "a refused request changed the ceiling: the run is in budget past it: {}",
        requests[1]
    );
    let budgets = records
        .iter()
        .filter(|r| r["fields"]["message"] == "run budget")
        .count();
    assert_eq!(
        budgets, 1,
        "nothing changed, so nothing was sent again: {records:?}"
    );
}

/// The `withdraw` response sent for `id`: a withdrawn steer's own `steer`
/// response carries the same id, so the command tells the two apart.
fn withdrawn(rpc: &mut Rpc, id: &str) -> Value {
    rpc.seen_or_next(|v| v["type"] == "response" && v["command"] == "withdraw" && v["id"] == id)
}

#[test]
fn rpc_withdraw_refuses_a_delivered_steer() {
    // The risks (ADR 0058 §3): a steer the person took back still put in a
    // model request, or one the model was sent reported as taken back; a
    // `get_state` read after `steer delivered` still listing the steer as
    // waiting (#801's window); and a withdraw with a bad id or no run
    // answered as if it took something back. "Never reached the model" is
    // read from what the provider was sent, and order from the stream.
    let c = contract();
    let entry = &c["rpc"]["responses"]["withdraw"];
    let f = fixture("rpc-withdraw-steer");
    let mut idle = Rpc::start(trellis(&f));
    idle.send(serde_json::json!({"id": "s", "type": "withdraw"}));
    let none = withdrawn(&mut idle, "s");
    assert_eq!(none["error"], "no run in flight", "{none}");
    idle.finish();
    let (mut rpc, server, q1) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_WITHDRAW_STEER_KEY",
        vec![
            edit_a(),
            openai_call(
                "c2",
                "edit",
                serde_json::json!({"path": "a.py", "old": "return 2", "new": "return 3"}),
            ),
            openai_call("c3", "done", serde_json::json!({"summary": "changed it"})),
        ],
    );
    // Both wait while the loop is held on the first edit's question.
    let gone = "STEER-808-WITHDRAWN look at g instead";
    rpc.send(serde_json::json!({"id": "s1", "type": "steer", "message": "STEER-808-KEPT"}));
    rpc.send(serde_json::json!({"id": "s2", "type": "steer", "message": gone}));
    rpc.send(serde_json::json!({"id": "s2", "type": "withdraw"}));
    let w = withdrawn(&mut rpc, "s2");
    assert_eq!(w["success"], true, "{w}");
    assert_eq!(w["data"], serde_json::json!({"id": "s2", "type": "steer"}));
    let gaps = missing(&w["data"], &entry["always"], "withdraw");
    assert!(gaps.is_empty(), "{gaps:?}: {w}");
    assert!(names(&entry["type_values"]).contains(&"steer"), "{entry}");
    let s2 =
        rpc.seen_or_next(|v| v["type"] == "response" && v["command"] == "steer" && v["id"] == "s2");
    assert_eq!(
        s2["success"], false,
        "a withdrawn steer was answered delivered: {s2}"
    );
    let held = state(&mut rpc, "g1");
    assert_eq!(
        held["pending"],
        serde_json::json!([{"id": "s1", "type": "steer", "message": "STEER-808-KEPT"}]),
        "{held}"
    );

    // The yes lets the loop drain s1 into the second request. Once its
    // event is read, the steer is delivered wherever a client asks.
    rpc.send(serde_json::json!({"id": q1["id"], "type": "approve", "yes": true}));
    rpc.next(|v| v["fields"]["message"] == "steer delivered");
    let after = state(&mut rpc, "g2");
    assert_eq!(
        after["pending"],
        serde_json::json!([]),
        "listed after its event: {after}"
    );
    rpc.send(serde_json::json!({"id": "s1", "type": "withdraw"}));
    let late = withdrawn(&mut rpc, "s1");
    assert_eq!(
        late["success"], false,
        "a delivered steer was withdrawn: {late}"
    );
    assert_eq!(late["error"], "already delivered", "{late}");
    rpc.send(serde_json::json!({"id": "nobody", "type": "withdraw"}));
    let unknown = withdrawn(&mut rpc, "nobody");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("unknown id"),
        "{unknown}"
    );
    rpc.send(serde_json::json!({"type": "withdraw"}));
    let bare = rpc.seen_or_next(|v| {
        v["type"] == "response" && v["command"] == "withdraw" && v["id"].is_null()
    });
    assert_eq!(bare["success"], false, "{bare}");
    assert!(
        bare["error"]
            .as_str()
            .unwrap_or_default()
            .contains("needs the"),
        "{bare}"
    );
    rpc.send(serde_json::json!({"id": 7, "type": "withdraw"}));
    let number = rpc.seen_or_next(|v| {
        v["type"] == "response"
            && v["command"] == "withdraw"
            && v["id"].is_null()
            && v["error"].as_str().is_some_and(|e| e.contains("got 7"))
    });
    assert_eq!(number["success"], false, "{number}");

    // The second edit's question is written by the run's thread once the
    // steer is delivered, so it can arrive while the waits above are still
    // reading for their answers; q1 is in `seen` too, hence the id.
    let q2 = rpc.seen_or_next(|v| v["type"] == "approval_request" && v["id"] != q1["id"]);
    rpc.send(serde_json::json!({"id": q2["id"], "type": "approve", "yes": true}));
    rpc.seen_or_next(|v| v["type"] == "run_end");
    let s1 =
        rpc.seen_or_next(|v| v["type"] == "response" && v["command"] == "steer" && v["id"] == "s1");
    assert_eq!(s1["success"], true, "{s1}");
    assert_eq!(s1["data"]["after_step"], 1, "{s1}");
    rpc.finish();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(requests[1].contains("STEER-808-KEPT"), "{}", requests[1]);
    for (i, r) in requests.iter().enumerate() {
        assert!(
            !r.contains("STEER-808-WITHDRAWN"),
            "a withdrawn steer reached request {}: {r}",
            i + 1
        );
    }
}

#[test]
fn rpc_withdraw_removes_an_undelivered_follow_up() {
    // The risks (ADR 0058 §3): a follow-up the person took back still
    // started as a run, from the run it was sent during or from the chain's
    // queue once that run ended; still listed as waiting; or handed back by
    // `abort` as if it were still queued. One whose run has started is
    // refused, since that run cannot be taken back by withdrawing it.
    let f = fixture("rpc-withdraw-follow-up");
    let script = scratch("rpc-withdraw-follow-up-steps").join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[
            {"tool":"edit","path":"a.py","old":"return 1","new":"return 2"},
            {"tool":"done","summary":"changed it"}
        ]}"#,
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    std::fs::write(
        f.user.join("config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    let mut rpc = Rpc::start(trellis(&f));
    rpc.choose(&spec);
    rpc.send(
        serde_json::json!({"id": "p", "type": "prompt", "task": "change f",
                                "model": spec}),
    );
    let q1 = rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "u1", "type": "follow_up", "message": "second task"}));
    rpc.send(serde_json::json!({"id": "u2", "type": "follow_up", "message": "FOLLOW-808-A"}));
    rpc.send(serde_json::json!({"id": "u3", "type": "follow_up", "message": "FOLLOW-808-B"}));
    // u2 is still in the record of the run it was sent during.
    rpc.send(serde_json::json!({"id": "u2", "type": "withdraw"}));
    let w2 = withdrawn(&mut rpc, "u2");
    assert_eq!(w2["success"], true, "{w2}");
    assert_eq!(
        w2["data"],
        serde_json::json!({"id": "u2", "type": "follow_up"})
    );
    let held = state(&mut rpc, "g1");
    assert_eq!(
        held["pending"],
        serde_json::json!([
            {"id": "u1", "type": "follow_up", "message": "second task"},
            {"id": "u3", "type": "follow_up", "message": "FOLLOW-808-B"},
        ]),
        "{held}"
    );

    // p ends and u1's run starts: u3 has moved to the chain's queue.
    rpc.send(serde_json::json!({"id": q1["id"], "type": "approve", "yes": true}));
    rpc.next(|v| v["type"] == "run_start" && v["id"] == "u1");
    rpc.next(|v| v["type"] == "approval_request");
    rpc.send(serde_json::json!({"id": "u3", "type": "withdraw"}));
    let w3 = withdrawn(&mut rpc, "u3");
    assert_eq!(
        w3["data"],
        serde_json::json!({"id": "u3", "type": "follow_up"}),
        "{w3}"
    );
    rpc.send(serde_json::json!({"id": "u1", "type": "withdraw"}));
    let started = withdrawn(&mut rpc, "u1");
    assert_eq!(started["error"], "already delivered", "{started}");
    rpc.send(serde_json::json!({"id": "p", "type": "withdraw"}));
    let prompt = withdrawn(&mut rpc, "p");
    assert!(
        prompt["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("unknown id"),
        "a prompt is no queued message: {prompt}"
    );
    rpc.send(serde_json::json!({"id": "u4", "type": "follow_up", "message": "fifth task"}));
    let now = state(&mut rpc, "g2");
    assert_eq!(
        now["pending"],
        serde_json::json!([{"id": "u4", "type": "follow_up", "message": "fifth task"}]),
        "{now}"
    );
    rpc.send(serde_json::json!({"id": "x", "type": "abort"}));
    let x = rpc.seen_or_next(|v| v["type"] == "response" && v["id"] == "x");
    assert_eq!(
        x["data"]["undelivered"],
        serde_json::json!([{"id": "u4", "type": "follow_up", "message": "fifth task"}]),
        "abort handed back a withdrawn follow-up: {x}"
    );
    let records = rpc.finish();
    for id in ["u2", "u3"] {
        assert!(
            !records
                .iter()
                .any(|r| r["type"] == "run_start" && r["id"] == id),
            "withdrawn follow-up {id} started a run: {records:?}"
        );
    }
}

#[test]
fn rpc_abort_while_paused_rolls_back_and_hands_back_the_held_steer() {
    // The risk (ADR 0058 §1): a held run that `abort` cannot reach, so the
    // only way out of a pause is to resume work the person wanted stopped,
    // or an abort that leaves the run's edit in the tree. A steer sent
    // while held never reached a request, so abort hands it back.
    let f = fixture("rpc-pause-abort");
    let (mut rpc, server, q) = at_the_edit_s_question(
        &f,
        "TRELLIS_TEST_RPC_PAUSE_ABORT_KEY",
        // One reply, so the stub stops after the one request the run makes
        // before it holds; a second would find no listener and fail the run
        // rather than leave it interrupted.
        vec![edit_a()],
    );
    rpc.send(serde_json::json!({"id": "h", "type": "pause"}));
    rpc.send(serde_json::json!({"id": q["id"], "type": "approve", "yes": true}));
    let held = rpc.next(|v| v["type"] == "response" && v["id"] == "h");
    assert_eq!(held["success"], true, "{held}");
    rpc.send(serde_json::json!({"id": "s", "type": "steer", "message": "STEER-held"}));
    rpc.send(serde_json::json!({"id": "x", "type": "abort"}));
    let end = rpc.next(|v| v["type"] == "run_end");
    assert_eq!(end["outcome"], "interrupted", "{end}");
    assert_eq!(end["restored"], true, "{end}");
    let x = rpc.next(|v| v["type"] == "response" && v["id"] == "x");
    assert_eq!(
        x["data"]["undelivered"],
        serde_json::json!([{"id": "s", "type": "steer", "message": "STEER-held"}]),
        "{x}"
    );
    let records = rpc.finish();
    // Only the first request: the held run never asked again.
    assert_eq!(server.join().unwrap().len(), 1, "{records:?}");
    assert!(
        !records.iter().any(|r| r["fields"]["message"] == "resumed"),
        "an aborted hold is not a resume: {records:?}"
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("a.py")).unwrap(),
        "def f():\n    return 1\n"
    );
}
