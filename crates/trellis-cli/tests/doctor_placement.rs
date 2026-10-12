//! `trellis doctor` against a server that answers like Ollama (#633): where
//! the configured model sits, as Ollama reports it on `GET /api/ps`.
//!
//! Driven through the real binary against a loopback stub. The `/api/ps`
//! body is the shape Ollama's docs/api.md documents for "List Running
//! Models" (`size`, `size_vram`, `details.quantization_level`), not captured
//! from a running server. Nothing here connects anywhere but 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Ollama's `/api/ps` with one model loaded, 4,880,000,000 of its
/// 5,137,025,024 bytes in VRAM: the split that runs slowly and that
/// nothing else in doctor would show.
const PS_SPLIT: &str = r#"{"models":[{"name":"qwen3:8b","model":"qwen3:8b","size":5137025024,"digest":"2ae6f6dd","details":{"parent_model":"","format":"gguf","family":"qwen3","families":["qwen3"],"parameter_size":"8.2B","quantization_level":"Q4_K_M"},"expires_at":"2026-10-11T14:38:31.83753-07:00","size_vram":4880000000}]}"#;

/// Ollama's `/api/ps` with nothing loaded.
const PS_EMPTY: &str = r#"{"models":[]}"#;

/// Ollama's OpenAI-compatible `/v1/models`, which is not colibri's.
const MODELS_OLLAMA: &str = r#"{"object":"list","data":[{"id":"qwen3:8b","object":"model","created":1758800000,"owned_by":"library"}]}"#;

/// A loopback stub answering `GET /api/ps` with `ps`, `GET /v1/models` as
/// Ollama does, and any POST with an empty completion, on every connection
/// until the test process ends.
fn stub(ps: &'static str) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
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
            let body = if request.starts_with("GET /api/ps ") {
                ps
            } else if request.starts_with("GET /v1/models ") {
                MODELS_OLLAMA
            } else {
                r#"{"choices":[{"text":""}]}"#
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    format!("http://{addr}")
}

/// A git repository with one Python file.
fn repo(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-placement-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let out = trellis_vcs::command()
        .args(["init", "-q"])
        .current_dir(&d)
        .output()
        .expect("git on PATH");
    assert!(out.status.success());
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
    d
}

/// `trellis doctor --model <spec>` in `root`, with no user config and no
/// key in the environment.
fn doctor(root: &Path, spec: &str) -> Output {
    let user = std::env::temp_dir().join("trellis-placement-no-user-config");
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(root)
        .args(["doctor", "--model", spec])
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", &user)
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("the binary is built")
}

/// Stdout of a doctor that exited 0: doctor never fails because of what a
/// runtime said.
fn ok(o: &Output) -> String {
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "doctor failed: {s}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    s
}

fn placement_line(s: &str) -> &str {
    s.lines()
        .find(|l| l.starts_with("placement:"))
        .unwrap_or_else(|| panic!("no `placement:` line in:\n{s}"))
}

#[test]
fn doctor_placement_prints_the_model_s_quantization_and_bytes_as_ollama_s_report() {
    // The risk: a model split between VRAM and RAM runs several times
    // slower with nothing in doctor saying so. The figures are Ollama's two
    // byte counts and its quantization string, each said to be its report;
    // no fit score is derived from them (CLAUDE.md, "Before claiming a
    // number").
    let base = stub(PS_SPLIT);
    let d = repo("split");
    let s = ok(&doctor(&d, &format!("vllm:{base}#qwen3:8b")));
    let line = placement_line(&s);
    assert!(line.contains("qwen3:8b"), "{line}");
    assert!(line.contains("Q4_K_M"), "{line}");
    assert!(line.contains("5137025024 bytes"), "{line}");
    assert!(line.contains("4880000000 bytes in VRAM"), "{line}");
    assert!(line.contains("split between VRAM and RAM"), "{line}");
    assert!(line.ends_with("as reported by ollama"), "{line}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_placement_with_nothing_loaded_says_placement_is_unknown_until_a_run() {
    // Ollama loads a model on its first request, so before a run the list
    // is empty; zeros here would read as a model that takes no memory.
    let base = stub(PS_EMPTY);
    let d = repo("empty");
    let s = ok(&doctor(&d, &format!("vllm:{base}#qwen3:8b")));
    assert_eq!(
        placement_line(&s),
        "placement:  not loaded; placement unknown until a run"
    );
    assert!(!s.contains("as reported by ollama"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_placement_with_no_runtime_prints_nothing_new_and_exits_as_before() {
    // A port nothing listens on: the `/api/ps` read fails like the tier
    // probe does, and doctor says what it said before this line existed.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let d = repo("absent");
    let s = ok(&doctor(
        &d,
        &format!("vllm:http://127.0.0.1:{port}#qwen3:8b"),
    ));
    assert!(!s.contains("placement:"), "{s}");
    assert!(s.contains("Could not probe the runtime"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}
