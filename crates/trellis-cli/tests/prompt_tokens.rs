//! Prompt tokens reach the trail from each protocol (ADR 0027, #106).
//!
//! Driven through the real binary against loopback stubs, because the risk
//! is in the hand-offs: the adapter reads the figure, the loop sums it, and
//! `cmd_run` copies it into `factual.prompt_tokens`. A unit test of any one
//! of those says nothing about the copy, which is where it read `None` until
//! now.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The variable the stub provider's key is read from. The stubs check nothing
/// about the key; its value is built at run time, not written as a literal.
const KEY_ENV: &str = "TRELLIS_TEST_PROVIDER_KEY";

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-prompt-{name}-{}", std::process::id()));
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

fn user_dir(repo: &Path) -> PathBuf {
    repo.with_extension("user")
}

/// A committed repository whose typecheck runs and exits 0, and a user
/// config naming one provider at `base` when `protocol` is given.
fn repo(name: &str, protocol: Option<&str>, base: &str) -> PathBuf {
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
    let providers = protocol.map_or(String::new(), |p| {
        format!(
            "[[providers]]\nname = \"stub\"\nprotocol = \"{p}\"\n\
             base_url = \"{base}\"\nkey_env = \"{KEY_ENV}\"\n"
        )
    });
    std::fs::write(user.join("config.toml"), providers).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

/// Serves `replies` in order on a loopback port.
///
/// A connection that sends nothing is not answered and takes no reply:
/// `cmd_run` opens one to see whether a local runtime is listening before
/// its first request.
fn stub(replies: Vec<String>) -> (String, std::thread::JoinHandle<usize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let mut answered = 0;
        let mut replies = replies.into_iter();
        let mut next = replies.next();
        while let Some(r) = &next {
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
            if raw.is_empty() {
                continue;
            }
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{r}",
                r.len()
            );
            s.write_all(out.as_bytes()).ok();
            answered += 1;
            next = replies.next();
        }
        answered
    });
    (format!("http://{addr}"), h)
}

/// Runs `trellis run` with `model_args` and returns the trail's `factual`.
fn run_and_read(
    d: &Path,
    model_args: &[&str],
    server: std::thread::JoinHandle<usize>,
    n: usize,
) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .args(["run", "make helper return 2"])
        .args(model_args)
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env(KEY_ENV, format!("test-{}", std::process::id()))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        server.join().unwrap(),
        n,
        "every scripted reply was requested.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("CONVERGED"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    let runs = d.join(".trellis/runs");
    let trail = std::fs::read_dir(&runs)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap_or_else(|| panic!("a trail under {}", runs.display()));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&trail).unwrap()).unwrap();
    v["factual"].clone()
}

fn hosted(protocol: &str, name: &str, replies: Vec<String>) -> serde_json::Value {
    let n = replies.len();
    let (base, server) = stub(replies);
    let d = repo(name, Some(protocol), &base);
    run_and_read(&d, &["--model", "provider:stub#m"], server, n)
}

fn openai(id: &str, name: &str, args: &str, prompt: Option<u64>) -> String {
    let mut usage = serde_json::json!({"completion_tokens": 3});
    if let Some(p) = prompt {
        usage["prompt_tokens"] = p.into();
    }
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]}}],
        "usage": usage})
    .to_string()
}

const CHECK: &str = r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#;
const DONE: &str = r#"{"summary":"checked"}"#;

#[test]
fn openai_prompt_tokens_are_summed_into_the_trail() {
    let f = hosted(
        "openai",
        "openai",
        vec![
            openai("c1", "run_checks", CHECK, Some(100)),
            openai("c2", "done", DONE, Some(150)),
        ],
    );
    assert_eq!(f["prompt_tokens"], 250, "{f}");
    // `tokens` still means completion tokens: 3 + 3, not 256.
    assert_eq!(f["tokens"], 6, "{f}");
}

#[test]
fn anthropic_prompt_tokens_in_the_trail_include_the_cache_fields() {
    // Chosen so each wrong reading gives a different number: `input_tokens`
    // alone is 15, and the cache fields alone 235.
    let reply = |id: &str, name: &str, input: serde_json::Value, usage: serde_json::Value| {
        serde_json::json!({"content":[{"type":"tool_use","id":id,"name":name,"input":input}],
            "usage": usage})
        .to_string()
    };
    let f = hosted(
        "anthropic",
        "anthropic",
        vec![
            reply(
                "t1",
                "run_checks",
                serde_json::from_str(CHECK).unwrap(),
                serde_json::json!({"input_tokens":10,"cache_creation_input_tokens":20,
                    "cache_read_input_tokens":70,"output_tokens":3}),
            ),
            reply(
                "t2",
                "done",
                serde_json::from_str(DONE).unwrap(),
                serde_json::json!({"input_tokens":5,"cache_read_input_tokens":145,
                    "output_tokens":3}),
            ),
        ],
    );
    assert_eq!(f["prompt_tokens"], 250, "{f}");
    assert_eq!(f["tokens"], 6, "{f}");
}

#[test]
fn llamacpp_prompt_tokens_evaluated_are_summed_into_the_trail() {
    // `--unconstrained` so no schema probe is sent: the probe goes through
    // the client, not the loop, and is not part of the run's total.
    let reply = |call: &str, evaluated: u64| {
        serde_json::json!({"content": call, "tokens_predicted": 3,
            "tokens_evaluated": evaluated, "timings": {"prompt_n": 1}})
        .to_string()
    };
    let replies = vec![
        reply(
            r#"{"tool":"run_checks","paths":["app/util.py"],"kinds":["typecheck"]}"#,
            100,
        ),
        reply(r#"{"tool":"done","summary":"checked"}"#, 150),
    ];
    let n = replies.len();
    let (base, server) = stub(replies);
    let d = repo("llamacpp", None, &base);
    let spec = format!("llamacpp:{base}");
    let f = run_and_read(&d, &["--model", &spec, "--unconstrained"], server, n);
    assert_eq!(f["prompt_tokens"], 250, "{f}");
    assert_eq!(f["tokens"], 6, "{f}");
}

#[test]
fn one_reply_without_a_prompt_figure_leaves_prompt_tokens_out_of_the_trail() {
    // The partial sum would be 100, a cheaper run than the one that
    // happened. Absent is what unknown looks like in the trail; the
    // completion count, which both replies carried, is still recorded.
    let f = hosted(
        "openai",
        "missing",
        vec![
            openai("c1", "run_checks", CHECK, Some(100)),
            openai("c2", "done", DONE, None),
        ],
    );
    assert!(f.get("prompt_tokens").is_none(), "{f}");
    assert_eq!(f["tokens"], 6, "{f}");
}
