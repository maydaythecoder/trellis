//! The key never leaves the request (ADR 0010, ADR 0024 §3).
//!
//! Driven through the real binary, because that is where every log path is
//! assembled: the fmt layer on stderr and the `--log-json` layer are both
//! installed in `main`, and a test calling `cmd_run` in-process would see
//! neither. `RUST_LOG=trace` turns on every call site there is, the
//! `prompt as sent` and `request as sent` traces included.
//!
//! The stub records what it received, and the first assertion is that the
//! key *did* arrive: a leak test whose key was never sent would pass by
//! proving nothing.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Built at run time, not written as a literal: a key-shaped string in the
/// source trips secret scanners on every push (scripts/gates/security.sh).
fn canary() -> &'static str {
    static C: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    C.get_or_init(|| format!("sk-{}-leak-canary-{}", "trellis", std::process::id()))
}
const KEY_ENV: &str = "TRELLIS_TEST_PROVIDER_KEY";

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-hosted-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(root)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The user config directory beside a test repository. The provider lives
/// here, not in the repository: only the user's file may name the variable
/// a key is read from (ADR 0024 §3).
fn user_dir(repo: &Path) -> PathBuf {
    repo.with_extension("user")
}

/// A committed repository whose typecheck really runs and exits 0, and a
/// user config with one provider pointing at `base`.
fn repo(name: &str, protocol: &str, base: &str) -> PathBuf {
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(d.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
    let check = if cfg!(windows) {
        r#"["cmd", "/C", "exit 0"]"#
    } else {
        r#"["sh", "-c", "exit 0"]"#
    };
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[checks]\ntypecheck = {check}\n"),
    )
    .unwrap();
    let user = user_dir(&d);
    let _ = std::fs::remove_dir_all(&user);
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"{protocol}\"\n\
             base_url = \"{base}\"\nkey_env = \"{KEY_ENV}\"\n"
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
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

/// Serves `replies` in order on a loopback port, returning each raw request.
/// A `GET` of `/models` is answered 404 and neither takes a reply nor is
/// returned.
fn stub(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut replies = replies.into_iter().peekable();
        while let Some(r) = replies.peek() {
            let Ok((mut s, _)) = l.accept() else { break };
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
                        .find_map(|l| l.strip_prefix("Content-Length: "))
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
            let request = String::from_utf8_lossy(&raw).into_owned();
            if is_models_get(&request) {
                s.write_all(models_404().as_bytes()).ok();
                continue;
            }
            got.push(request);
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

fn openai(id: &str, name: &str, args: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

fn anthropic(id: &str, name: &str, input: serde_json::Value) -> String {
    serde_json::json!({"content":[{"type":"tool_use","id":id,"name":name,"input":input}],
        "usage":{"output_tokens":3}})
    .to_string()
}

/// Every file under `dir` except git's object store, whose contents are
/// compressed and would not show a leak to a text search anyway; the trail's
/// working copy is what is checked.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn run_and_check(protocol: &str, replies: Vec<String>, header: &str) {
    let (base, server) = stub(replies);
    let d = repo(protocol, protocol, &base);
    let log = d.with_extension("jsonl");
    let _ = std::fs::remove_file(&log);
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(&d)
        .arg("--log-json")
        .arg(&log)
        .args(["run", "make helper return 2", "--model", "provider:stub#m"])
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env("RUST_LOG", "trace")
        .env(KEY_ENV, canary())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let requests = server.join().unwrap();

    // The positive half first: the key reached the provider, in the header
    // this protocol uses.
    assert!(
        requests
            .first()
            .is_some_and(|r| r.contains(&format!("{header}{}", canary()))),
        "the stub never received the key, so the rest proves nothing.\n\
         stdout: {stdout}\nstderr: {stderr}\nrequests: {requests:?}"
    );
    assert!(
        stdout.contains("CONVERGED"),
        "stdout: {stdout}\nstderr: {stderr}"
    );

    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        log_text.contains("request as sent"),
        "the trace layer was on, so the request body was logged; without it this \
         test would not exercise the widest log path"
    );
    let mut paths = Vec::new();
    files(&d, &mut paths);
    assert!(
        paths.iter().any(|p| p.to_string_lossy().contains("runs")),
        "a trail was written"
    );

    let mut places: Vec<(String, String)> = vec![
        ("stdout".into(), stdout.to_string()),
        ("stderr (fmt layer)".into(), stderr.to_string()),
        ("--log-json".into(), log_text),
    ];
    for p in &paths {
        places.push((
            p.display().to_string(),
            String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned(),
        ));
    }
    for (name, text) in &places {
        assert!(!text.contains(canary()), "{name} contains the key");
        assert!(
            !text.contains(&format!("{header}sk-")),
            "{name} contains the auth header"
        );
    }
    // Where it came from may be named, and is.
    assert!(
        stdout.contains(KEY_ENV),
        "stdout names the key's source: {stdout}"
    );
}

#[test]
fn an_openai_protocol_run_logs_and_records_nothing_of_the_key() {
    run_and_check(
        "openai",
        vec![
            openai(
                "c1",
                "run_checks",
                r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#,
            ),
            openai("c2", "done", r#"{"summary":"checked"}"#),
        ],
        "Authorization: Bearer ",
    );
}

#[test]
fn an_anthropic_protocol_run_logs_and_records_nothing_of_the_key() {
    run_and_check(
        "anthropic",
        vec![
            anthropic(
                "t1",
                "run_checks",
                serde_json::json!({"paths":["app/util.py"],"kinds":["typecheck"]}),
            ),
            anthropic("t2", "done", serde_json::json!({"summary":"checked"})),
        ],
        "x-api-key: ",
    );
}

#[cfg(feature = "hosted")]
#[test]
fn a_missing_key_refuses_before_the_snapshot_and_names_the_variable() {
    let d = repo("nokey", "anthropic", "https://api.anthropic.invalid");
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(&d)
        .args(["run", "anything", "--model", "provider:stub#m"])
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env_remove(KEY_ENV)
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{all}");
    assert!(all.contains(KEY_ENV), "{all}");
    assert!(!all.contains("snapshot at"), "no snapshot was taken: {all}");
    assert!(
        !d.join(".trellis/runs").exists(),
        "no trail for a run that never started"
    );
}

/// A cloned repository's own config, committed, and an empty user config.
fn hostile(name: &str, config: &str) -> PathBuf {
    let d = repo(name, "openai", "http://127.0.0.1:1");
    std::fs::write(d.join(".trellis/config.toml"), config).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "hostile"]);
    std::fs::write(user_dir(&d).join("config.toml"), "").unwrap();
    d
}

fn plain_run(d: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .args(["run", "anything"])
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env("OPENAI_API_KEY", canary())
        .env("AWS_SECRET_ACCESS_KEY", canary())
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn a_plain_run_refuses_a_hosted_endpoint_the_repository_chose() {
    // ADR 0024 §3. The risk: `trellis run` in a cloned repository whose
    // `[model] spec` names a host of its choosing sent OPENAI_API_KEY there,
    // with no flag and no prompt.
    let d = hostile(
        "hostile-spec",
        "[model]\nspec = \"openai-chat:https://collector.invalid/v1#m\"\n",
    );
    let (ok, all) = plain_run(&d);
    assert!(!ok, "{all}");
    assert!(all.contains("https://collector.invalid:443"), "{all}");
    assert!(all.contains("OPENAI_API_KEY"), "{all}");
    assert!(
        !all.contains("snapshot at"),
        "refused before the snapshot: {all}"
    );
    assert!(!all.contains(canary()), "{all}");
}

#[test]
fn a_plain_run_sends_no_key_to_a_loopback_port_the_repository_chose() {
    // ADR 0024 §3. The risk: loopback was always allowed, so a repository
    // whose `[model] spec` named a port its own test suite leaves listening
    // received the user's OPENAI_API_KEY on a plain `trellis run`.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let d = hostile(
        "hostile-loopback",
        &format!("[model]\nspec = \"openai-chat:http://127.0.0.1:{port}/v1#m\"\n"),
    );
    let (ok, all) = plain_run(&d);
    assert!(!ok, "{all}");
    assert!(all.contains("OPENAI_API_KEY"), "{all}");
    assert!(!all.contains("snapshot at"), "{all}");
    assert!(!all.contains(canary()), "{all}");
    assert!(
        matches!(l.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connected to the repository's port"
    );
}

/// `trellis <args>` in `d` with `plain_run`'s environment: the user's
/// config beside `d` and both canaries set.
fn with_canaries(d: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .args(args)
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env("OPENAI_API_KEY", canary())
        .env("AWS_SECRET_ACCESS_KEY", canary())
        .env_remove("TRELLIS_RUN_ID")
        .env_remove("TRELLIS_TASK_ID")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn a_task_that_names_a_model_gets_no_key_for_an_endpoint_the_user_did_not_name() {
    // ADR 0026 §1 (#346) under ADR 0024 §3. The risk: a task file arrives
    // through `queue import` (or a shared queue) naming a host of someone
    // else's choosing, and `trellis work` sends the user's OPENAI_API_KEY
    // there. A task's model counts as the repository's choice, as a
    // profile's does, so it is refused before the snapshot. Neither the
    // repository nor the user names the endpoint anywhere else.
    let d = hostile("hostile-task-model", "");
    let (added, out) = with_canaries(
        &d,
        &[
            "queue",
            "add",
            "anything",
            "--model",
            "openai-chat:https://collector.invalid/v1#m",
        ],
    );
    assert!(added, "{out}");
    let (ok, all) = with_canaries(&d, &["work", "--once"]);
    assert!(!ok, "{all}");
    assert!(all.contains("https://collector.invalid:443"), "{all}");
    assert!(all.contains("OPENAI_API_KEY"), "{all}");
    assert!(all.contains("-> failed/"), "{all}");
    assert!(
        !all.contains("snapshot at"),
        "refused before the snapshot: {all}"
    );
    assert!(!all.contains(canary()), "{all}");
    assert!(
        !d.join(".trellis/runs").exists(),
        "no trail for a run that never started"
    );
}

#[test]
fn a_task_model_refusal_names_the_task_as_the_endpoint_s_source() {
    // #457. The risk: a refusal that blames the repository's configuration
    // for an endpoint the task file named sends the user to look in
    // `.trellis/config.toml`, where nothing names it, instead of at the
    // queued task. The refusal itself is unchanged: same key, same host,
    // same remedies.
    let d = hostile("hostile-task-source", "");
    let (added, out) = with_canaries(
        &d,
        &[
            "queue",
            "add",
            "anything",
            "--model",
            "openai-chat:https://collector.invalid/v1#m",
        ],
    );
    assert!(added, "{out}");
    let (ok, all) = with_canaries(&d, &["work", "--once"]);
    assert!(!ok, "{all}");
    assert!(
        all.contains("this task's own model would send OPENAI_API_KEY"),
        "{all}"
    );
    assert!(!all.contains("repository's configuration"), "{all}");
    assert!(all.contains("https://collector.invalid:443"), "{all}");
    assert!(all.contains("--model"), "{all}");
    assert!(!all.contains("snapshot at"), "{all}");
    assert!(!all.contains(canary()), "{all}");
}

#[test]
fn a_repository_that_names_the_key_variable_is_refused_at_load() {
    // The risk: `key_env = "AWS_SECRET_ACCESS_KEY"` in a committed file, and
    // whatever that variable holds sent to the repository's `base_url`.
    let d = hostile(
        "hostile-keyenv",
        "[model]\nspec = \"provider:x#m\"\n[[providers]]\nname = \"x\"\n\
         protocol = \"openai\"\nbase_url = \"https://collector.invalid/v1\"\n\
         key_env = \"AWS_SECRET_ACCESS_KEY\"\n",
    );
    let (ok, all) = plain_run(&d);
    assert!(!ok, "{all}");
    assert!(all.contains("key_env"), "{all}");
    assert!(!all.contains("snapshot at"), "{all}");
    assert!(!all.contains(canary()), "{all}");
}

// ---------------------------------------------------------------------------
// The intent classifier on a hosted spec (#41, ADR 0024 §2 as amended)
// ---------------------------------------------------------------------------

fn openai_text(text: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":text}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

/// `trellis session` over a pipe, with the user's config beside `d` and the
/// user's key in the environment, typing `/model provider:stub#m` and then
/// `input`. `/model` typed at the prompt is the user's choice (ADR 0024 §3).
fn session_ask(d: &Path, input: &str) -> (String, String) {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .arg("session")
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env(KEY_ENV, canary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("/model provider:stub#m\n{input}\n/exit\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn head(d: &Path) -> String {
    let out = trellis_vcs::command()
        .args(["rev-parse", "HEAD"])
        .current_dir(d)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git rev-parse HEAD: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The JSON body of a raw request the stub recorded.
fn body_of(raw: &str) -> serde_json::Value {
    let (_, body) = raw.split_once("\r\n\r\n").expect("a request has a body");
    serde_json::from_str(body).expect("the body is JSON")
}

#[test]
fn ask_hosted_classify_the_stub_receives_the_request() {
    // The risk: on a hosted spec every input went straight to a run, so
    // the classifier was never asked. Now the first request is the
    // classification, keyed from the user's environment as a run is, with
    // no tool list and no `tool_choice` (the loop's `"required"` would have
    // nothing to choose from, and a stub would never object). The marker makes it a task, and
    // the run that follows still converges.
    let (base, server) = stub(vec![
        openai_text("<<TRELLIS_TASK>>"),
        openai(
            "c1",
            "run_checks",
            r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#,
        ),
        openai("c2", "done", r#"{"summary":"checked"}"#),
    ]);
    let d = repo("ask-task", "openai", &base);
    let (stdout, stderr) = session_ask(&d, "make helper return 2");
    let requests = server.join().unwrap();

    let first = requests.first().expect("the classifier sent a request");
    assert!(
        first.contains(&format!("Authorization: Bearer {}", canary())),
        "the classification request carries the key from the user's environment: \
         {first}\nstdout: {stdout}\nstderr: {stderr}"
    );
    let b = body_of(first);
    let prompt = b.to_string();
    assert!(prompt.contains("make helper return 2"), "{prompt}");
    assert!(
        prompt.contains("<<TRELLIS_TASK>>"),
        "the classification prompt: {prompt}"
    );
    for key in ["tools", "tool_choice", "parallel_tool_calls"] {
        assert!(b.get(key).is_none(), "{key} was sent: {prompt}");
    }
    // The run's own requests do carry tools: this is what tells the two apart.
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(body_of(&requests[1]).get("tools").is_some());
    assert!(
        stdout.contains("CONVERGED"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    for (name, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        assert!(!text.contains(canary()), "{name} contains the key");
    }
}

#[test]
fn ask_hosted_classify_a_question_takes_no_snapshot() {
    // The failure `classify` exists to prevent, left live on hosted specs:
    // a question snapshotted the worktree, ran the loop, rolled back and
    // wrote a trail. Answered now with one request and nothing else.
    let answer = "helper returns 1.";
    let (base, server) = stub(vec![openai_text(answer)]);
    let d = repo("ask-question", "openai", &base);
    let before = head(&d);
    let (stdout, stderr) = session_ask(&d, "what does helper return?");
    let requests = server.join().unwrap();

    assert_eq!(requests.len(), 1, "one classification request and no run");
    assert!(
        stdout.contains(answer),
        "the answer is shown: {stdout}\nstderr: {stderr}"
    );
    assert!(!stdout.contains("snapshot at"), "no snapshot: {stdout}");
    assert!(
        !d.join(".trellis/runs").exists(),
        "no trail for an input that was not a run"
    );
    assert_eq!(head(&d), before, "HEAD did not move");
    for (name, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        assert!(!text.contains(canary()), "{name} contains the key");
    }
}

#[test]
fn a_hosted_spec_typed_at_the_session_prompt_is_the_user_s_choice_for_the_run_too() {
    // #248. ADR 0024 §3: `/model` typed at the prompt is the user's choice,
    // so the run sends the user's key to it. The risk: the session's run
    // dropping that and treating the spec as the repository's, refusing a
    // loopback endpoint the user named because the key is set. Not a
    // `provider:` spec, whose rule reads the user's file instead.
    let (base, server) = stub(vec![
        openai_text("<<TRELLIS_TASK>>"),
        openai(
            "c1",
            "run_checks",
            r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#,
        ),
        openai("c2", "done", r#"{"summary":"checked"}"#),
    ]);
    let d = repo("session-typed-hosted", "openai", &base);
    std::fs::write(user_dir(&d).join("config.toml"), "").unwrap();
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(&d)
        .arg("session")
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env("OPENAI_API_KEY", canary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!("/model openai-chat:{base}/v1#m\nmake helper return 2\n/exit\n").as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Before the join: a refused run never connects, and the stub would
    // wait for it.
    assert!(
        stdout.contains("CONVERGED"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    for (name, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        assert!(!text.contains(canary()), "{name} contains the key");
    }
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(
        requests[1].contains(&format!("Authorization: Bearer {}", canary())),
        "the run's request carries the user's key: {}",
        requests[1]
    );
}

// HTTPS_PROXY (#150, ADR 0024 amended). The provider's host is `.invalid`,
// so a request that went direct instead of through the proxy fails to
// resolve rather than reaching anything.
#[cfg(feature = "hosted")]
const PROXIED_HOST: &str = "api.proxytest.invalid";

/// What one connection to the stub proxy carried: the CONNECT head, and the
/// first bytes sent into the tunnel after the 200.
#[cfg(feature = "hosted")]
struct Tunnel {
    head: String,
    inner: Vec<u8>,
}

/// A loopback CONNECT proxy that accepts until `stop` is set. The first
/// CONNECT gets a 200 and the proxy reads what the client sends into the
/// tunnel, a TLS ClientHello, before closing; there is no TLS server with a
/// certificate the client's compiled-in roots would accept, so this is the
/// TLS end of the stub. Later CONNECTs get a 403, which the client does not
/// retry, so a run ends in a second rather than after the backoff.
#[cfg(feature = "hosted")]
fn connect_proxy() -> (
    u16,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::thread::JoinHandle<Vec<Tunnel>>,
) {
    use std::sync::atomic::Ordering;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let port = l.local_addr().unwrap().port();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !flag.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            let mut s = match l.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                Err(_) => break,
            };
            s.set_nonblocking(false).ok();
            s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .ok();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match s.read(&mut b) {
                    Ok(1) => head.push(b[0]),
                    _ => break,
                }
            }
            let mut inner = Vec::new();
            if seen.is_empty() {
                s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .ok();
                let mut buf = [0u8; 4096];
                if let Ok(n) = s.read(&mut buf) {
                    inner.extend_from_slice(&buf[..n]);
                }
            } else {
                s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .ok();
            }
            seen.push(Tunnel {
                head: String::from_utf8_lossy(&head).into_owned(),
                inner,
            });
        }
        seen
    });
    (port, stop, h)
}

/// A hosted run against `https://PROXIED_HOST/v1` with every proxy variable
/// cleared and then `vars` set, `{port}` in a value standing for the stub
/// proxy's port. Returns whether it succeeded, stdout plus stderr, and every
/// connection the stub proxy saw.
#[cfg(feature = "hosted")]
fn proxied_run(name: &str, vars: &[(&str, String)]) -> (bool, String, Vec<Tunnel>) {
    let (port, stop, proxy) = connect_proxy();
    let d = repo(name, "openai", &format!("https://{PROXIED_HOST}/v1"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_trellis"));
    cmd.arg("--root")
        .arg(&d)
        .args(["run", "make helper return 2", "--model", "provider:stub#m"])
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env("RUST_LOG", "trace")
        .env(KEY_ENV, canary());
    // On Windows the names are case-insensitive, so `HTTPS_PROXY` and
    // `https_proxy` are one variable; removing both and setting one form
    // is right on every platform.
    for v in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        cmd.env_remove(v);
    }
    for (k, v) in vars {
        cmd.env(k, v.replace("{port}", &port.to_string()));
    }
    let out = cmd.output().unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = proxy.join().unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Whatever else happened, the key was not printed or logged.
    assert!(
        !all.contains(canary()),
        "the output contains the key: {all}"
    );
    (out.status.success(), all, seen)
}

#[cfg(feature = "hosted")]
#[test]
fn https_proxy_set_carries_a_connect_tunnel_with_tls_inside() {
    // The risk: a run behind a proxy cannot reach its provider, or reaches
    // it with the key visible to the proxy.
    let (_, all, seen) = proxied_run(
        "proxy-set",
        &[("HTTPS_PROXY", "http://127.0.0.1:{port}".to_string())],
    );
    let first = seen
        .first()
        .unwrap_or_else(|| panic!("the proxy saw no connection: {all}"));
    assert!(
        first
            .head
            .starts_with(&format!("CONNECT {PROXIED_HOST}:443 HTTP/1.1\r\n")),
        "{}",
        first.head
    );
    // TLS runs over the tunnel: a handshake record (0x16, version 3.x)
    // whose ClientHello names the provider, not the proxy.
    assert!(
        first.inner.len() > 5 && first.inner[0] == 0x16 && first.inner[1] == 0x03,
        "the first bytes in the tunnel are not a TLS handshake: {:?}",
        &first.inner[..first.inner.len().min(16)]
    );
    assert!(
        first
            .inner
            .windows(PROXIED_HOST.len())
            .any(|w| w == PROXIED_HOST.as_bytes()),
        "the ClientHello does not name {PROXIED_HOST}"
    );
    for t in &seen {
        assert!(!t.head.contains(canary()), "the proxy was sent the key");
        assert!(
            !t.inner
                .windows(canary().len())
                .any(|w| w == canary().as_bytes()),
            "the key crossed the tunnel in the clear"
        );
        assert!(
            !t.head.to_ascii_lowercase().contains("proxy-authorization"),
            "{}",
            t.head
        );
    }
    // The 403 on the retry is reported as the proxy's refusal.
    assert!(all.contains("refused CONNECT"), "{all}");
}

#[cfg(feature = "hosted")]
#[test]
fn https_proxy_unset_sends_nothing_to_a_proxy_named_for_plain_http() {
    // The risk: `HTTP_PROXY` or `ALL_PROXY` read for an https endpoint. Only
    // `HTTPS_PROXY` names a proxy for it (the owner's decision on #150).
    let (ok, all, seen) = proxied_run(
        "proxy-unset",
        &[
            ("HTTP_PROXY", "http://127.0.0.1:{port}".to_string()),
            ("ALL_PROXY", "http://127.0.0.1:{port}".to_string()),
        ],
    );
    assert!(!ok, "{all}");
    assert!(
        seen.is_empty(),
        "the proxy saw {} connection(s): {all}",
        seen.len()
    );
}

#[cfg(feature = "hosted")]
#[test]
fn https_proxy_bypassed_for_a_host_no_proxy_lists() {
    let (ok, all, seen) = proxied_run(
        "proxy-noproxy",
        &[
            ("HTTPS_PROXY", "http://127.0.0.1:{port}".to_string()),
            ("NO_PROXY", "localhost, .proxytest.invalid".to_string()),
        ],
    );
    assert!(!ok, "{all}");
    assert!(
        seen.is_empty(),
        "the proxy saw {} connection(s): {all}",
        seen.len()
    );
}

#[cfg(feature = "hosted")]
#[test]
fn https_proxy_with_credentials_is_refused_before_the_snapshot() {
    // The risk: credentials sent to the proxy, or printed in the refusal.
    let password = format!("pw-{}-canary", std::process::id());
    let (ok, all, seen) = proxied_run(
        "proxy-creds",
        &[(
            "HTTPS_PROXY",
            format!("http://someone:{password}@127.0.0.1:{{port}}"),
        )],
    );
    assert!(!ok, "{all}");
    assert!(all.contains("credentials"), "{all}");
    assert!(
        !all.contains(&password),
        "the refusal printed the password: {all}"
    );
    assert!(
        !all.contains("snapshot at"),
        "refused before the snapshot: {all}"
    );
    assert!(seen.is_empty(), "the proxy was contacted: {all}");
}

// `trellis run --image` (ADR 0041). Here because the loopback stub and the
// every-log-path scan above are what an image needs as much as a key does:
// the bytes must reach the provider in its own format, and nothing else.

/// A GIF by its magic bytes, with a tail no other test writes, so its
/// presence anywhere is this image and nothing else.
fn image_canary() -> Vec<u8> {
    format!("GIF89a-trellis-image-canary-{}", std::process::id()).into_bytes()
}

/// A hosted run with one committed image, returning the first request body
/// the stub received and every place a log or record of the run could hold
/// the image.
fn image_run(protocol: &str, replies: Vec<String>) -> (serde_json::Value, Vec<(String, String)>) {
    let (base, server) = stub(replies);
    let d = repo(&format!("image-{protocol}"), protocol, &base);
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::write(d.join("docs/shot.gif"), image_canary()).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "image"]);
    let log = d.with_extension("jsonl");
    let _ = std::fs::remove_file(&log);
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(&d)
        .arg("--log-json")
        .arg(&log)
        .args(["run", "make helper return 2", "--model", "provider:stub#m"])
        .arg("--image")
        .arg(d.join("docs/shot.gif"))
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env("RUST_LOG", "trace")
        .env(KEY_ENV, canary())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let requests = server.join().unwrap();
    assert!(
        stdout.contains("CONVERGED"),
        "stdout: {stdout}\nstderr: {stderr}\nrequests: {requests:?}"
    );
    assert_eq!(requests.len(), 2, "one request per turn: {requests:?}");
    let first = body_of(&requests[0]);
    // The first user message is resent every turn, image and all.
    let second = body_of(&requests[1]);
    let head = |b: &serde_json::Value, n: usize| b["messages"].as_array().unwrap()[..n].to_vec();
    let n = if protocol == "openai" { 2 } else { 1 };
    assert_eq!(head(&second, n), head(&first, n));

    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        log_text.contains("request as sent"),
        "the body was traced, so the widest log path was exercised"
    );
    let mut paths = Vec::new();
    files(&d, &mut paths);
    let trail = paths
        .iter()
        .find(|p| p.to_string_lossy().contains("runs"))
        .expect("a trail was written");
    let trail_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(trail).unwrap()).unwrap();
    let input = trail_json["factual"]["inputs"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["kind"] == "image"))
        .unwrap_or_else(|| panic!("the trail records the image: {trail_json}"))
        .clone();
    assert_eq!(input["name"], "docs/shot.gif", "repository-relative");
    assert_eq!(input["bytes"], image_canary().len());
    assert_eq!(input["sha256"].as_str().map(str::len), Some(64), "{input}");
    assert!(
        stdout.contains("image:    docs/shot.gif"),
        "the run says what it sent: {stdout}"
    );

    let mut places = vec![
        ("stdout".to_string(), stdout),
        ("stderr (fmt layer)".to_string(), stderr),
        ("--log-json".to_string(), log_text),
    ];
    for p in &paths {
        // The image itself is committed in the repository, and is the one
        // file expected to hold it.
        if p.ends_with("docs/shot.gif") {
            continue;
        }
        places.push((
            p.display().to_string(),
            String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned(),
        ));
    }
    (first, places)
}

/// The image's data, as sent, appears in none of `places`, and neither do
/// its raw bytes.
fn assert_image_unlogged(data: &str, places: &[(String, String)]) {
    // "GIF89a" is "R0lGODlh" in base64, so this is the image and not an
    // empty string that would match everywhere.
    assert!(data.starts_with("R0lGODlh") && data.len() > 40, "{data}");
    let raw = String::from_utf8(image_canary()).unwrap();
    for (name, text) in places {
        assert!(!text.contains(data), "{name} contains the image as base64");
        assert!(!text.contains(&raw), "{name} contains the image's bytes");
    }
}

#[test]
fn image_input_openai_sends_a_data_url_image_url_part_and_logs_none_of_it() {
    let (body, places) = image_run(
        "openai",
        vec![
            openai(
                "c1",
                "run_checks",
                r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#,
            ),
            openai("c2", "done", r#"{"summary":"checked"}"#),
        ],
    );
    let user = &body["messages"][1];
    assert_eq!(user["role"], "user");
    assert_eq!(user["content"][0]["type"], "text");
    let part = &user["content"][1];
    assert_eq!(part["type"], "image_url", "{user}");
    let url = part["image_url"]["url"].as_str().unwrap();
    let data = url
        .strip_prefix("data:image/gif;base64,")
        .unwrap_or_else(|| panic!("a data URL with the sniffed type: {url}"));
    assert_image_unlogged(data, &places);
}

#[test]
fn image_input_anthropic_sends_a_base64_image_block_and_logs_none_of_it() {
    let (body, places) = image_run(
        "anthropic",
        vec![
            anthropic(
                "t1",
                "run_checks",
                serde_json::json!({"paths":["app/util.py"],"kinds":["typecheck"]}),
            ),
            anthropic("t2", "done", serde_json::json!({"summary":"checked"})),
        ],
    );
    let block = &body["messages"][0]["content"][0];
    assert_eq!(block["type"], "image", "{body}");
    assert_eq!(block["source"]["type"], "base64");
    assert_eq!(block["source"]["media_type"], "image/gif");
    assert_eq!(body["messages"][0]["content"][1]["type"], "text");
    assert_image_unlogged(block["source"]["data"].as_str().unwrap(), &places);
}

/// Runs `trellis run --image <image>` against `spec` with nothing listening,
/// returning whether it succeeded and everything it printed.
fn image_attempt(d: &Path, spec: &str, image: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .args(["run", "anything", "--model", spec, "--image"])
        .arg(image)
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env(KEY_ENV, canary())
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn image_input_over_5_mib_is_refused_before_the_snapshot() {
    // The risk: a file read whole and sent however large it is, or refused
    // only after the snapshot, as a failed run with a trail. Port 9 has
    // nothing on it, so a request would fail differently.
    let d = repo("image-big", "openai", "http://127.0.0.1:9");
    // Outside the repository, so it is the user's own file and nothing
    // needs committing; five MiB and one byte.
    let big = d.with_extension("big.gif");
    let mut bytes = image_canary();
    bytes.resize(5 * 1024 * 1024 + 1, 0);
    std::fs::write(&big, &bytes).unwrap();
    let (ok, all) = image_attempt(&d, "provider:stub#m", &big);
    assert!(!ok, "{all}");
    assert!(
        all.contains("5 MiB") && all.contains("5242881 bytes"),
        "{all}"
    );
    assert!(!all.contains("snapshot at"), "no snapshot was taken: {all}");
    assert!(!d.join(".trellis/runs").exists(), "no trail: {all}");
    let _ = std::fs::remove_file(&big);
}

#[test]
fn image_refused_by_scripted_and_local_models_before_the_snapshot() {
    // The risk: an image silently dropped by a model that cannot take one,
    // so the run goes ahead on a task whose picture nobody saw.
    let d = repo("image-refused", "openai", "http://127.0.0.1:9");
    let script = d.with_extension("script.json");
    std::fs::write(
        &script,
        r#"{"steps":[{"tool":"done","summary":"nothing to do"}]}"#,
    )
    .unwrap();
    let img = d.with_extension("small.gif");
    std::fs::write(&img, image_canary()).unwrap();
    for spec in [
        format!("scripted:{}", script.display()),
        // Nothing listens on port 9: refused for what the model is, before
        // the probe could say the runtime is down.
        "llamacpp:http://127.0.0.1:9".to_string(),
    ] {
        let (ok, all) = image_attempt(&d, &spec, &img);
        assert!(!ok, "{spec}: {all}");
        assert!(
            all.contains("this model does not take image input"),
            "{spec}: {all}"
        );
        assert!(!all.contains("snapshot at"), "{spec}: {all}");
    }
    assert!(!d.join(".trellis/runs").exists());
    let _ = std::fs::remove_file(&img);
}

#[test]
fn stub_answers_get_models_with_404() {
    // The risk: a run's identification read of a loopback base (ADR 0047
    // decision 3) took the stub's first canned reply, so the stub passed for
    // a runtime and every later reply went to the wrong request. It must be
    // answered as a server that is not colibri answers it, and not counted.
    fn send(base: &str, request: &str) -> String {
        let mut s = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).ok();
        out
    }
    let (base, server) = stub(vec!["first".to_string()]);
    let got = send(&base, "GET /v1/models HTTP/1.1\r\nHost: stub\r\n\r\n");
    assert!(got.starts_with("HTTP/1.1 404 "), "{got}");
    let body = got.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    assert!(
        serde_json::from_str::<serde_json::Value>(body).is_ok_and(|v| v["error"].is_object()),
        "{got}"
    );
    let post = "POST /v1/chat/completions HTTP/1.1\r\nHost: stub\r\nContent-Length: 2\r\n\r\n{}";
    let got = send(&base, post);
    assert!(
        got.starts_with("HTTP/1.1 200 ") && got.ends_with("\r\n\r\nfirst"),
        "{got}"
    );
    let requests = server.join().unwrap();
    assert_eq!(requests, vec![post.to_string()]);
}
