//! `[model] timeout_s` is the timeout of every model request (ADR 0047
//! decision 9, #266).
//!
//! Driven through the real binary against a loopback stub that answers each
//! model request 2 s after reading it, because the risk is in the hand-off:
//! both clients already took a timeout, and nothing passed the configured
//! one to them, so every request kept the fixed 300 s. A run with
//! `timeout_s = 5` must get the slow reply, and one with `timeout_s = 1`
//! must end on a 1 s timeout rather than wait for it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How long the stub waits before answering a model request.
const DELAY: Duration = Duration::from_secs(2);

/// The variable the `provider:` spec's key is read from. The stub checks
/// nothing about the key; its value is built at run time.
const KEY_ENV: &str = "TRELLIS_TEST_PROVIDER_KEY";

mod common;
use common::Scratch;

fn scratch(name: &str) -> Scratch {
    common::scratch_dir(&format!("trellis-mto-{name}"))
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

fn user_dir(repo: &Path) -> PathBuf {
    repo.with_extension("user")
}

/// A committed repository whose typecheck exits 0 and whose config sets
/// `[model] timeout_s`, with a user config naming one openai-chat provider
/// at `base`. Committed with the config, so the run starts on a clean tree.
fn repo(name: &str, timeout_s: u64, base: &str) -> Scratch {
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
        format!("[checks]\ntypecheck = {check}\n\n[model]\ntimeout_s = {timeout_s}\n"),
    )
    .unwrap();
    let user = user_dir(&d);
    let _ = std::fs::remove_dir_all(&user);
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("config.toml"),
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"openai\"\n\
             base_url = \"{base}/v1\"\nkey_env = \"{KEY_ENV}\"\n"
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

/// Serves `replies` in order on a loopback port, each [`DELAY`] after the
/// whole request was read.
///
/// A connection that sends nothing is not answered and takes no reply: a
/// run opens one to see whether a local runtime is listening. A `GET` of
/// `/models`, the identification read of a loopback base (ADR 0047 decision
/// 3), is answered 404 at once and takes no reply: it is a probe with its
/// own bound, not a model request. A reply written after the client gave up
/// is still counted, so the thread ends once every reply is spent.
fn stub(replies: Vec<String>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for r in replies {
            loop {
                let Ok((mut s, _)) = l.accept() else { return };
                s.set_read_timeout(Some(Duration::from_secs(10))).ok();
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
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_string)
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
                if raw.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(&raw);
                let mut line = text.lines().next().unwrap_or_default().split_whitespace();
                if line.next() == Some("GET") && line.next().is_some_and(|p| p.ends_with("/models"))
                {
                    let body = r#"{"error":{"message":"not found","type":"not_found_error"}}"#;
                    let out = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    s.write_all(out.as_bytes()).ok();
                    continue;
                }
                std::thread::sleep(DELAY);
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{r}",
                    r.len()
                );
                s.write_all(out.as_bytes()).ok();
                break;
            }
        }
    });
    format!("http://{addr}")
}

fn openai_call(id: &str, name: &str, args: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

fn openai_text(text: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":text}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

fn vllm(text: &str) -> String {
    serde_json::json!({"choices":[{"text":text}],"usage":{"completion_tokens":3}}).to_string()
}

const CHECK: &str = r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#;
const DONE: &str = r#"{"summary":"checked"}"#;

/// The replies that converge a run on an `openai-chat:` spec.
fn openai_run() -> Vec<String> {
    vec![
        openai_call("c1", "run_checks", CHECK),
        openai_call("c2", "done", DONE),
    ]
}

/// The replies that converge a run on a `vllm:` spec.
fn vllm_run() -> Vec<String> {
    vec![
        vllm(r#"{"tool":"run_checks","paths":["app/util.py"],"kinds":["typecheck"]}"#),
        vllm(r#"{"tool":"done","summary":"checked"}"#),
    ]
}

/// `trellis run` with `model_args`; stdout and stderr together.
fn run(d: &Path, model_args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .args(["run", "make helper return 2"])
        .args(model_args)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn converged(all: &str) -> bool {
    all.contains("CONVERGED") && !all.contains("NOT CONVERGED")
}

/// The run ended on a 1 s request timeout: `RuntimeError::Timeout` prints
/// the duration that fired, so a request still bounded by 300 s would say
/// `300s` here, after waiting for it.
fn timed_out_after_1_s(all: &str) -> bool {
    all.contains("did not respond within 1s")
}

#[test]
fn model_timeout_applies_to_an_openai_chat_run_that_waits_under_it() {
    let base = stub(openai_run());
    let d = repo("chat-5", 5, &base);
    let all = run(&d, &["--model", &format!("openai-chat:{base}/v1#m")]);
    assert!(converged(&all), "a 2 s reply is inside 5 s: {all}");
}

#[test]
fn model_timeout_applies_to_an_openai_chat_run_that_waits_past_it() {
    let base = stub(openai_run());
    let d = repo("chat-1", 1, &base);
    let all = run(&d, &["--model", &format!("openai-chat:{base}/v1#m")]);
    assert!(!converged(&all), "a 2 s reply is past 1 s: {all}");
    assert!(timed_out_after_1_s(&all), "{all}");
}

#[test]
fn model_timeout_applies_to_a_vllm_run_that_waits_under_it() {
    // `--unconstrained`, so no schema probe reaches the stub.
    let base = stub(vllm_run());
    let d = repo("vllm-5", 5, &base);
    let all = run(&d, &["--model", &format!("vllm:{base}"), "--unconstrained"]);
    assert!(converged(&all), "a 2 s reply is inside 5 s: {all}");
}

#[test]
fn model_timeout_applies_to_a_vllm_run_that_waits_past_it() {
    let base = stub(vllm_run());
    let d = repo("vllm-1", 1, &base);
    let all = run(&d, &["--model", &format!("vllm:{base}"), "--unconstrained"]);
    assert!(!converged(&all), "a 2 s reply is past 1 s: {all}");
    assert!(timed_out_after_1_s(&all), "{all}");
}

#[test]
fn model_timeout_applies_to_the_ask_classification() {
    // `ask::classify` over `trellis session`, as hosted_keys.rs drives it.
    // With the timeout ignored the classifier waits the 2 s and prints the
    // answer. With it applied the classification times out, the session
    // falls through to a run (its existing handling of a classifier that
    // fails), and that run's request times out too.
    use std::process::Stdio;
    let answer = "helper returns 1.";
    let base = stub(vec![openai_text(answer), openai_text(answer)]);
    let d = repo("ask-1", 1, &base);
    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(&d)
        .arg("session")
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(&d))
        .env(KEY_ENV, format!("test-{}", std::process::id()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/model provider:stub#m\nwhat does helper return?\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let all = format!(
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !stdout.contains(answer),
        "the classifier waited past 1 s for the reply: {all}"
    );
    assert!(timed_out_after_1_s(&all), "{all}");
}
