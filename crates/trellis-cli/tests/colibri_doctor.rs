//! `trellis doctor` against a server that answers like colibri (#267, ADR
//! 0047 decision 7).
//!
//! Driven through the real binary against a loopback stub. The stub's
//! bodies are transcribed from colibri's source at ce370e8 (the same
//! fixtures as `trellis_model::colibri`'s tests), not captured from a
//! running server. Nothing here connects anywhere but 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

/// ce370e8, c/openai_server.py:3885-3886 inside the list sent at :4360-4362.
const MODELS_COLIBRI: &str = r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#;

/// ce370e8, c/openai_server.py:4318-4331 (authed `/health`), with
/// `scheduler` from :211-218, `tiers` from :3634-3636 and `hwinfo` from
/// :3608-3614.
const HEALTH_FULL: &str = r#"{"status":"ok","scheduler":{"active":0,"queued":0,"capacity":16,"max_queue":8,"queue_timeout_seconds":300,"admitted":3,"completed":3,"failed":0,"rejected":0,"timed_out":0,"cancelled":0},"kv_slots":16,"continue_assistant":true,"tiers":{"vram":120,"ram":2400,"disk":17000,"vram_gb":22.5,"ram_gb":96.0},"hwinfo":{"cores":32,"ram_total_gb":128.0,"ram_avail_gb":101.3,"gpus":1,"vram_total_gb":24.0,"cpu":"AMD Ryzen 9 7950X","gpu":"NVIDIA GeForce RTX 4090"}}"#;

/// ce370e8, c/openai_server.py:4322 (public liveness, unauthenticated).
const HEALTH_LIVENESS: &str = r#"{"status":"ok"}"#;

/// Built at run time, not written as a literal: a key-shaped string in the
/// source trips secret scanners (scripts/gates/security.sh).
fn canary() -> String {
    format!("sk-{}-colibri-canary-{}", "trellis", std::process::id())
}

/// ce370e8, c/openai_server.py:1364-1367 (OLMoE refuses `tools` while
/// rendering, naming the fallback setting), as `tests/colibri_run.rs` has it.
const TOOLS_REFUSED_OLMOE: &str = r#"{"error":{"message":"OLMoE has no tool-calling chat template; set COLI_TOOL_FALLBACK=1 to render tools as text.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

/// ce370e8, c/openai_server.py:1707-1708 (Inkling, no fallback named).
const TOOLS_REFUSED_INKLING: &str = r#"{"error":{"message":"Inkling does not support tools.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

/// ce370e8, c/openai_server.py:4190-4193 (`require_auth`).
const UNAUTHORIZED: &str = r#"{"error":{"message":"Invalid or missing API key.","type":"authentication_error","param":null,"code":"invalid_api_key"}}"#;

/// A status line and a body.
type Answer = (&'static str, String);

fn ok200(body: &str) -> Answer {
    ("200 OK", body.to_string())
}

/// A loopback stub answering `GET /v1/models` with `models`, `GET /health`
/// with `health`, and any POST with an empty completion, on every
/// connection until the test process ends; and the raw requests it got.
fn stub(models: String, health: String) -> (String, Arc<Mutex<Vec<String>>>) {
    stub_with(
        ok200(&models),
        health,
        ok200(r#"{"choices":[{"text":""}]}"#),
    )
}

/// [`stub`], with the status of the `/v1/models` answer and the answer to a
/// POST (the tools probe) given.
fn stub_with(models: Answer, health: String, post: Answer) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let got = Arc::new(Mutex::new(Vec::new()));
    let seen = got.clone();
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
            let (status, body) = if request.starts_with("GET /v1/models ") {
                models.clone()
            } else if request.starts_with("GET /health ") {
                ok200(&health)
            } else {
                post.clone()
            };
            seen.lock().unwrap().push(request);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (format!("http://{addr}"), got)
}

fn colibri() -> (String, Arc<Mutex<Vec<String>>>) {
    stub(MODELS_COLIBRI.into(), HEALTH_FULL.into())
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-colibri-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A git repository with one Python file, and `config` as its
/// `.trellis/config.toml` when given.
fn repo(name: &str, config: Option<&str>) -> PathBuf {
    let d = scratch(name);
    let out = trellis_vcs::command()
        .args(["init", "-q"])
        .current_dir(&d)
        .output()
        .expect("git on PATH");
    assert!(out.status.success());
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
    if let Some(c) = config {
        std::fs::create_dir_all(d.join(".trellis")).unwrap();
        std::fs::write(d.join(".trellis/config.toml"), c).unwrap();
    }
    d
}

/// `trellis doctor <args>` in `root`, with no user config and no key in
/// the environment unless `key` sets `OPENAI_API_KEY`.
fn doctor(root: &Path, args: &[&str], key: Option<&str>) -> Output {
    // Never created or removed: tests run in parallel, and a missing
    // directory is a user config that is not there.
    let user = std::env::temp_dir().join("trellis-colibri-no-user-config");
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.arg("--root")
        .arg(root)
        .arg("doctor")
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", &user)
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY");
    if let Some(k) = key {
        c.env("OPENAI_API_KEY", k);
    }
    c.output().expect("the binary is built")
}

/// Stdout of a doctor that exited 0: doctor never fails because of what
/// colibri said.
fn ok(o: &Output) -> String {
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "doctor failed: {s}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    s
}

fn chat(base: &str, id: &str) -> String {
    format!("openai-chat:{base}/v1#{id}")
}

fn line<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line in:\n{s}"))
}

#[test]
fn colibri_doctor_names_colibri_and_the_served_id_and_says_the_spec_matches() {
    let (base, _) = colibri();
    let d = repo("match", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let rt = line(&s, "runtime:");
    assert!(rt.contains("colibri"), "{rt}");
    assert!(rt.contains("`glm-5.2`"), "{rt}");
    assert!(rt.contains("matches"), "{rt}");
    assert!(!rt.contains("NOT"), "{rt}");
}

#[test]
fn colibri_doctor_names_both_ids_when_the_spec_s_model_is_not_the_served_one() {
    // The risk: a spec naming another id, to which colibri answers 404 on
    // every POST, reported as fine.
    let (base, _) = colibri();
    let d = repo("mismatch", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "kimi-k3")], None));
    let rt = line(&s, "runtime:");
    assert!(rt.contains("`glm-5.2`") && rt.contains("`kimi-k3`"), "{rt}");
    assert!(rt.contains("does NOT match"), "{rt}");
}

#[test]
fn colibri_doctor_says_the_version_is_not_reported_over_http() {
    let (base, _) = colibri();
    let d = repo("version", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(
        s.lines()
            .any(|l| l.starts_with("version:") && l.contains("not reported over HTTP")),
        "{s}"
    );
}

#[test]
fn colibri_doctor_reports_sixteen_kv_slots_as_measured_concurrency_said_to_be_configured() {
    let (base, _) = colibri();
    let d = repo("slots16", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(
        line(&s, "batching:").contains("continuous, measured concurrency 16"),
        "{s}"
    );
    assert!(s.contains("colibri's configured KV slot count"), "{s}");
}

#[test]
fn colibri_doctor_reports_one_kv_slot_as_single_stream() {
    let (base, _) = stub(
        MODELS_COLIBRI.into(),
        HEALTH_FULL.replace(r#""kv_slots":16"#, r#""kv_slots":1"#),
    );
    let d = repo("slots1", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(line(&s, "batching:").contains("single-stream"), "{s}");
    // Not llama.cpp's advice: colibri has no --parallel.
    assert!(!s.contains("--parallel"), "{s}");
}

#[test]
fn colibri_doctor_reports_memory_tiers_and_host_as_colibri_s_own_report() {
    let (base, _) = colibri();
    let d = repo("tiers", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let experts = line(&s, "experts:");
    for want in [
        "120",
        "22.5 GB",
        "2400",
        "96.0 GB",
        "17000",
        "reported by colibri",
    ] {
        assert!(experts.contains(want), "{want}: {experts}");
    }
    let host = line(&s, "host:");
    for want in [
        "32 cores",
        "128.0 GB RAM",
        "1 GPU",
        "NVIDIA GeForce RTX 4090",
        "AMD Ryzen 9 7950X",
        "reported by colibri",
    ] {
        assert!(host.contains(want), "{want}: {host}");
    }
}

#[test]
fn colibri_doctor_says_a_liveness_only_health_hides_these_from_an_unauthenticated_request() {
    let (base, _) = stub(MODELS_COLIBRI.into(), HEALTH_LIVENESS.into());
    let d = repo("liveness", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(s.contains("only to an authenticated request"), "{s}");
    assert!(!s.lines().any(|l| l.starts_with("experts:")), "{s}");
}

#[test]
fn colibri_doctor_on_a_vllm_spec_reports_validate_and_retry_and_names_the_tier_1_spec() {
    // ADR 0047 finding 1: colibri's grammar never constrains sampling, so
    // a schema probe that happened to pass would claim a tier colibri does
    // not have.
    let (base, _) = colibri();
    let d = repo("vllm", None);
    let s = ok(&doctor(
        &d,
        &["--model", &format!("vllm:{base}#glm-5.2")],
        None,
    ));
    assert!(line(&s, "tier:").contains("ValidateAndRetry"), "{s}");
    assert!(s.contains("grammar never constrains sampling"), "{s}");
    assert!(s.contains("without a chat template"), "{s}");
    assert!(
        s.contains(&format!("`openai-chat:{base}/v1#glm-5.2`")),
        "{s}"
    );
}

#[test]
fn colibri_doctor_escapes_an_esc_byte_in_the_served_id_and_the_cpu_name() {
    // The risk: a server's string repainting the terminal doctor prints to.
    let (base, _) = stub(
        MODELS_COLIBRI.replace("glm-5.2", r"glm\u001b[2Kfake"),
        HEALTH_FULL.replace("AMD Ryzen", r"AMD\u001b[31mRyzen"),
    );
    let d = repo("esc", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(!s.contains('\x1b'), "{s:?}");
    assert!(line(&s, "runtime:").contains(r"glm\u{1b}[2Kfake"), "{s}");
    assert!(line(&s, "host:").contains(r"AMD\u{1b}[31mRyzen"), "{s}");
}

#[test]
fn colibri_doctor_sends_no_key_to_a_loopback_spec_the_repository_chose() {
    // ADR 0024 §3: the repository's `[model] spec` gets no key, and
    // identifying colibri never changes that (ADR 0047 decision 3).
    let (base, got) = colibri();
    let d = repo(
        "repo-spec",
        Some(&format!("[model]\nspec = \"{}\"\n", chat(&base, "glm-5.2"))),
    );
    let s = ok(&doctor(&d, &[], None));
    assert!(line(&s, "runtime:").contains("colibri"), "{s}");
    let got = got.lock().unwrap().clone();
    let reads: Vec<&String> = got
        .iter()
        .filter(|r| r.starts_with("GET /v1/models ") || r.starts_with("GET /health "))
        .collect();
    assert_eq!(reads.len(), 2, "{got:?}");
    // The tools probe (#269) carries no key either.
    let probes: Vec<&String> = got.iter().filter(|r| r.starts_with("POST ")).collect();
    assert_eq!(probes.len(), 1, "{got:?}");
    for r in reads.into_iter().chain(probes) {
        let lower = r.to_ascii_lowercase();
        assert!(
            !lower.contains("authorization") && !lower.contains("x-api-key"),
            "{r}"
        );
    }
}

#[test]
fn colibri_doctor_sends_the_user_s_key_as_bearer_and_prints_none_of_it() {
    let (base, got) = colibri();
    let d = repo("user-key", None);
    let key = canary();
    let o = doctor(&d, &["--model", &chat(&base, "glm-5.2")], Some(&key));
    let s = ok(&o);
    let all = format!("{s}{}", String::from_utf8_lossy(&o.stderr));
    assert!(!all.contains(&key), "{all}");
    let got = got.lock().unwrap().clone();
    for path in ["GET /v1/models ", "GET /health "] {
        let r = got
            .iter()
            .find(|r| r.starts_with(path))
            .unwrap_or_else(|| panic!("no {path}: {got:?}"));
        assert!(
            r.contains(&format!("\r\nAuthorization: Bearer {key}\r\n")),
            "{r}"
        );
    }
}

#[test]
fn colibri_doctor_does_not_fail_when_nothing_is_listening() {
    // doctor says what it saw; a stopped server is something it saw.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let d = repo("absent", None);
    let s = ok(&doctor(
        &d,
        &[
            "--model",
            &chat(&format!("http://127.0.0.1:{port}"), "glm-5.2"),
        ],
        None,
    ));
    assert!(s.contains("whether it is colibri"), "{s}");
}

#[test]
fn colibri_doctor_says_a_server_owned_by_another_is_not_colibri() {
    let (base, _) = stub(
        MODELS_COLIBRI.replace(r#""owned_by":"colibri""#, r#""owned_by":"vllm""#),
        HEALTH_FULL.into(),
    );
    let d = repo("not-colibri", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(!line(&s, "runtime:").contains("colibri"), "{s}");
    assert!(s.contains("owned by `vllm`"), "{s}");
    assert!(
        line(&s, "batching:").contains("concurrency not reported"),
        "{s}"
    );
}

// ---- #269: the tier from colibri's tools probe (ADR 0047 decision 4) ----

#[test]
fn colibri_doctor_tier_is_native_probed_when_colibri_accepts_tools() {
    // The risk: doctor claiming tier 1 from the protocol without asking, as
    // it did, for a family that may refuse tools.
    let (base, got) = colibri();
    let d = repo("tier-probed", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(line(&s, "tier:").contains("Native (probed)"), "{s}");
    let got = got.lock().unwrap().clone();
    let probes: Vec<&String> = got.iter().filter(|r| r.starts_with("POST ")).collect();
    assert_eq!(probes.len(), 1, "{got:?}");
    assert!(
        probes[0].starts_with("POST /v1/chat/completions ")
            && probes[0].contains(r#""max_tokens":1"#)
            && probes[0].contains(r#""tools""#),
        "{}",
        probes[0]
    );
}

#[test]
fn colibri_doctor_tier_refused_is_unavailable_naming_the_vllm_spec_and_the_fallback() {
    let (base, _) = stub_with(
        ok200(MODELS_COLIBRI),
        HEALTH_FULL.into(),
        ("400 Bad Request", TOOLS_REFUSED_OLMOE.into()),
    );
    let d = repo("tier-refused", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let tier = line(&s, "tier:");
    assert!(tier.starts_with("tier:       1, unavailable"), "{s}");
    assert!(!tier.contains("Native"), "{s}");
    assert!(s.contains(&format!("vllm:{base}#glm-5.2")), "{s}");
    assert!(s.contains("COLI_TOOL_FALLBACK=1"), "{s}");
}

#[test]
fn colibri_doctor_tier_refused_without_a_named_fallback_never_names_one() {
    // Inkling has no fallback: naming one sends the user to restart colibri
    // with a setting that changes nothing.
    let (base, _) = stub_with(
        ok200(MODELS_COLIBRI),
        HEALTH_FULL.into(),
        ("400 Bad Request", TOOLS_REFUSED_INKLING.into()),
    );
    let d = repo("tier-refused-inkling", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(
        line(&s, "tier:").starts_with("tier:       1, unavailable"),
        "{s}"
    );
    assert!(s.contains(&format!("vllm:{base}#glm-5.2")), "{s}");
    assert!(!s.contains("COLI_TOOL_FALLBACK"), "{s}");
}

#[test]
fn colibri_doctor_tier_not_verified_is_native_assumed_with_the_reason() {
    let (base, _) = stub_with(
        ok200(MODELS_COLIBRI),
        HEALTH_FULL.into(),
        (
            "500 Internal Server Error",
            r#"{"error":"canary-body-word"}"#.into(),
        ),
    );
    let d = repo("tier-assumed", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    assert!(
        line(&s, "tier:").contains("Native (assumed: HTTP 500)"),
        "{s}"
    );
    // The reason is TRELLIS's, never the server's body.
    assert!(!s.contains("canary-body-word"), "{s}");
}

#[test]
fn colibri_doctor_tier_of_a_server_that_is_not_colibri_sends_no_probe() {
    let (base, got) = stub(
        MODELS_COLIBRI.replace(r#""owned_by":"colibri""#, r#""owned_by":"vllm""#),
        HEALTH_FULL.into(),
    );
    let d = repo("tier-not-colibri", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let tier = line(&s, "tier:");
    assert!(tier.contains("Native") && !tier.contains("probed"), "{s}");
    let got = got.lock().unwrap().clone();
    assert!(!got.iter().any(|r| r.starts_with("POST ")), "{got:?}");
}

#[test]
fn colibri_doctor_tier_401_names_the_status_the_authenticated_listing_and_the_key_s_variable() {
    let (base, got) = stub_with(
        ("401 Unauthorized", UNAUTHORIZED.into()),
        HEALTH_LIVENESS.into(),
        ok200("{}"),
    );
    let d = repo("tier-401", None);
    let key = canary();
    let o = doctor(&d, &["--model", &chat(&base, "glm-5.2")], Some(&key));
    let s = ok(&o);
    let note = s
        .lines()
        .find(|l| l.contains("/v1/models answered"))
        .unwrap_or_else(|| panic!("no identification note:\n{s}"));
    assert!(note.contains("HTTP 401"), "{note}");
    assert!(
        note.contains("lists its model only to an authenticated request"),
        "{note}"
    );
    assert!(note.contains("the key from OPENAI_API_KEY"), "{note}");
    assert!(!s.contains(&key), "{s}");
    // Not identified, so not probed.
    let got = got.lock().unwrap().clone();
    assert!(!got.iter().any(|r| r.starts_with("POST ")), "{got:?}");
}

#[test]
fn colibri_doctor_tier_403_without_a_key_says_none_was_sent() {
    let (base, _) = stub_with(
        ("403 Forbidden", UNAUTHORIZED.into()),
        HEALTH_LIVENESS.into(),
        ok200("{}"),
    );
    let d = repo("tier-403", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let note = s
        .lines()
        .find(|l| l.contains("/v1/models answered"))
        .unwrap_or_else(|| panic!("no identification note:\n{s}"));
    assert!(note.contains("HTTP 403"), "{note}");
    assert!(
        note.contains("lists its model only to an authenticated request"),
        "{note}"
    );
    assert!(
        note.contains("without a key (OPENAI_API_KEY is not set)"),
        "{note}"
    );
}

/// The `context:` line about the model's window, not the one about the
/// repository's context files, which doctor prints under the same label.
fn budget_line(s: &str) -> &str {
    s.lines()
        .find(|l| l.starts_with("context:") && l.contains("window"))
        .unwrap_or_else(|| panic!("no `context:` line about the window in:\n{s}"))
}

#[test]
fn colibri_context_doctor_says_colibri_does_not_report_its_window_and_names_the_budget() {
    // The risk: a user whose colibri was started with a small `--ctx` told
    // nothing about the budget a run will hold the prompt to, so the first
    // they learn of it is colibri refusing a prompt or cutting a reply
    // (ADR 0047 decision 6).
    let (base, _) = colibri();
    let d = repo("context", None);
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let c = budget_line(&s);
    assert!(c.contains("colibri does not report its window"), "{c}");
    assert!(c.contains("25,088 bytes"), "{c}");
    assert!(
        c.contains("8,192 tokens") && c.contains("1,024 left for the reply"),
        "{c}"
    );
    assert!(c.contains("3.5 bytes per token"), "{c}");
    assert!(
        c.contains("[model] context_bytes") && c.contains("--ctx"),
        "{c}"
    );
}

#[test]
fn colibri_context_doctor_names_an_explicit_context_bytes_as_the_run_s_budget() {
    // The risk: doctor printing the default when the user's own value is
    // what the run will use.
    let (base, _) = colibri();
    let d = repo("context-set", Some("[model]\ncontext_bytes = 30000\n"));
    let s = ok(&doctor(&d, &["--model", &chat(&base, "glm-5.2")], None));
    let c = budget_line(&s);
    assert!(c.contains("colibri does not report its window"), "{c}");
    assert!(c.contains("30000 bytes"), "{c}");
    assert!(!c.contains("25,088"), "{c}");
}
