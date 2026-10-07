//! `trellis run` identifies and probes a loopback colibri endpoint before
//! its snapshot (#268, ADR 0047 decisions 3 and 4), and `trellis run` and
//! `trellis ask` send colibri's request profile to an endpoint so
//! identified, and to no other (#270, decision 5).
//!
//! Driven through the real binary against a loopback stub, which routes by
//! request: the identification read (`GET /v1/models`), the tools probe (a
//! chat request with `max_tokens: 1` and the one tool `noop`), and the run's
//! own chat requests. Bodies are transcribed from colibri's source at
//! ce370e8 (the same fixtures as `trellis_model::colibri`'s tests), not
//! captured from a running server. Nothing here connects anywhere but
//! 127.0.0.1.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// ce370e8, c/openai_server.py:3885-3886 inside the list sent at :4360-4362.
fn models_colibri(id: &str) -> String {
    serde_json::json!({"object":"list","data":[{"id":id,"object":"model",
        "created":1758800000,"owned_by":"colibri"}]})
    .to_string()
}

/// The same list shape with another owner, as vLLM answers.
const MODELS_VLLM: &str = r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"vllm"}]}"#;

/// ce370e8, c/openai_server.py:4190-4193 (`require_auth`), with a marker
/// word the run must not print.
const UNAUTHORIZED: &str = r#"{"error":{"message":"Invalid or missing API key. canary-body-word","type":"authentication_error","param":null,"code":"invalid_api_key"}}"#;

/// ce370e8, c/openai_server.py:4366 (an unknown GET path).
const NOT_FOUND: &str = r#"{"error":{"message":"Not found.","type":"invalid_request_error","param":null,"code":"not_found"}}"#;

/// ce370e8, c/openai_server.py:1364-1367 (OLMoE refuses `tools` while
/// rendering, naming the fallback setting). The wording is reconstructed, as
/// in `trellis_model::colibri`'s tests; only `param` and the setting's name
/// are read.
const TOOLS_REFUSED_OLMOE: &str = r#"{"error":{"message":"OLMoE has no tool-calling chat template; set COLI_TOOL_FALLBACK=1 to render tools as text.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

/// ce370e8, c/openai_server.py:1707-1708 (Inkling, no fallback named).
const TOOLS_REFUSED_INKLING: &str = r#"{"error":{"message":"Inkling does not support tools.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

/// ce370e8, c/openai_server.py:5169-5241, cut at one token.
const TOOLS_ACCEPTED: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1758800000,"model":"glm-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"I"},"finish_reason":"length"}],"usage":{"prompt_tokens":40,"completion_tokens":1,"total_tokens":41}}"#;

/// Built at run time, not written as a literal: a key-shaped string in the
/// source trips secret scanners (scripts/gates/security.sh).
fn canary() -> String {
    format!("sk-{}-colibri-run-canary-{}", "trellis", std::process::id())
}

/// What the stub does with one kind of request.
#[derive(Clone)]
enum Answer {
    /// `status` is the status line's tail (`"200 OK"`).
    Reply(&'static str, String),
    /// Reads the request and never answers, until the client closes.
    Hang,
}

fn reply(status: &'static str, body: &str) -> Answer {
    Answer::Reply(status, body.to_string())
}

/// One request the stub received, and for one it held unanswered, how long
/// after it arrived the client closed the connection.
#[derive(Clone, Debug)]
struct Seen {
    raw: String,
    closed_after: Option<Duration>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn is_models_get(raw: &str) -> bool {
    raw.starts_with("GET ") && raw.lines().next().is_some_and(|l| l.contains("/models "))
}

/// The tools probe: one chat request with `max_tokens: 1` (ADR 0047
/// decision 4). The run's own requests ask for far more.
fn is_probe(raw: &str) -> bool {
    raw.starts_with("POST ")
        && raw
            .split_once("\r\n\r\n")
            .and_then(|(_, b)| serde_json::from_str::<serde_json::Value>(b).ok())
            .is_some_and(|v| v["max_tokens"] == 1)
}

fn openai_call(id: &str, name: &str, args: &str) -> String {
    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]}}],
        "usage":{"completion_tokens":3}})
    .to_string()
}

/// A loopback stub on its own thread, one thread per connection so a held
/// request blocks nothing else. The run's chat requests get `run_checks`
/// first and `done` after, so a run that goes ahead converges.
fn stub(models: Answer, probe: Answer) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let chats = Arc::new(Mutex::new(0usize));
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { break };
            let (models, probe, seen, chats) =
                (models.clone(), probe.clone(), seen.clone(), chats.clone());
            std::thread::spawn(move || serve(s, &models, &probe, &seen, &chats));
        }
    });
    (format!("http://{addr}"), log)
}

fn serve(mut s: TcpStream, models: &Answer, probe: &Answer, log: &Log, chats: &Mutex<usize>) {
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let raw = read_request(&mut s);
    let arrived = Instant::now();
    let answer = if is_models_get(&raw) {
        models.clone()
    } else if is_probe(&raw) {
        probe.clone()
    } else {
        let mut n = chats.lock().unwrap();
        *n += 1;
        Answer::Reply(
            "200 OK",
            if *n == 1 {
                openai_call(
                    "c1",
                    "run_checks",
                    r#"{"paths":["app/util.py"],"kinds":["typecheck"]}"#,
                )
            } else {
                openai_call("c2", "done", r#"{"summary":"checked"}"#)
            },
        )
    };
    let index = {
        let mut l = log.lock().unwrap();
        l.push(Seen {
            raw,
            closed_after: None,
        });
        l.len() - 1
    };
    match answer {
        Answer::Reply(status, body) => respond(&mut s, status, &body),
        Answer::Hang => {
            // Held until the client gives up: a read of 0 bytes, or an error,
            // is the connection closed.
            let mut buf = [0u8; 1024];
            loop {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            log.lock().unwrap()[index].closed_after = Some(arrived.elapsed());
        }
    }
}

fn respond(s: &mut TcpStream, status: &str, body: &str) {
    let _ = s.write_all(
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

fn read_request(s: &mut TcpStream) -> String {
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
                return text;
            }
        }
        if n == 0 {
            return text;
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-colibri-run-{name}-{}", std::process::id()));
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

/// A committed repository whose typecheck exits 0, with `model` appended to
/// its `.trellis/config.toml`, and `user` as the user's own config.
fn repo(name: &str, model: &str, user: &str) -> PathBuf {
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
        format!("[checks]\ntypecheck = {check}\n{model}"),
    )
    .unwrap();
    let u = user_dir(&d);
    let _ = std::fs::remove_dir_all(&u);
    std::fs::create_dir_all(&u).unwrap();
    std::fs::write(u.join("config.toml"), user).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

/// The user's own `[[providers]]` entry for colibri at `base`, keyed by
/// `COLI_API_KEY` as ADR 0047 decision 2 suggests.
fn coli_provider(base: &str) -> String {
    format!(
        "[[providers]]\nname = \"coli\"\nprotocol = \"openai\"\nbase_url = \"{base}/v1\"\n\
         key_env = \"COLI_API_KEY\"\n"
    )
}

struct Ran {
    out: Output,
    all: String,
    stdout: String,
}

/// `trellis run` in `d`, with `--model spec` when given, no provider key in
/// the environment but `key` as `COLI_API_KEY`, and `extra` arguments.
fn run(d: &Path, spec: Option<&str>, key: Option<&str>, extra: &[&str]) -> Ran {
    run_with(d, spec, key, extra, &[])
}

/// [`run`], with `after` passed to `run` itself (`--image <path>`).
fn run_with(
    d: &Path,
    spec: Option<&str>,
    key: Option<&str>,
    extra: &[&str],
    after: &[&str],
) -> Ran {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.arg("--root").arg(d);
    c.args(extra);
    c.args(["run", "make helper return 2"]);
    if let Some(s) = spec {
        c.args(["--model", s]);
    }
    c.args(after);
    c.current_dir(d)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("COLI_API_KEY");
    if let Some(k) = key {
        c.env("COLI_API_KEY", k);
    }
    let out = c.output().expect("the binary is built");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let all = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    Ran { out, all, stdout }
}

/// A run with `--model openai-chat:<base>/v1#<id>` against a stub.
fn chat_run(name: &str, base: &str, id: &str) -> (PathBuf, Ran) {
    let d = repo(name, "", "");
    let r = run(&d, Some(&format!("openai-chat:{base}/v1#{id}")), None, &[]);
    (d, r)
}

/// The run stopped before its snapshot: non-zero, no `snapshot at`, no trail.
fn refused(d: &Path, r: &Ran) {
    assert!(!r.out.status.success(), "{}", r.all);
    assert!(!r.all.contains("snapshot at"), "no snapshot: {}", r.all);
    assert!(!d.join(".trellis/runs").exists(), "no trail: {}", r.all);
}

/// The run went ahead: it took its snapshot and converged.
fn went_ahead(r: &Ran) {
    assert!(r.stdout.contains("snapshot at"), "{}", r.all);
    assert!(r.stdout.contains("CONVERGED"), "{}", r.all);
}

fn requests(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

fn probes(log: &Log) -> usize {
    requests(log).iter().filter(|s| is_probe(&s.raw)).count()
}

/// The run's own chat bodies, the probe excluded.
fn chat_bodies(log: &Log) -> Vec<serde_json::Value> {
    requests(log)
        .iter()
        .filter(|s| s.raw.starts_with("POST ") && !is_probe(&s.raw))
        .map(|s| serde_json::from_str(s.raw.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect()
}

/// The run continued as it does without identification: no probe, no
/// `runtime:` line, and chat bodies without colibri's profile.
fn continued_as_today(log: &Log, r: &Ran) {
    went_ahead(r);
    assert!(
        requests(log).iter().any(|s| is_models_get(&s.raw)),
        "the endpoint was asked whether it is colibri"
    );
    assert_eq!(probes(log), 0, "no tools probe: {}", r.all);
    assert!(!r.stdout.contains("runtime:"), "{}", r.stdout);
    let bodies = chat_bodies(log);
    assert!(!bodies.is_empty());
    for b in bodies {
        assert!(b.get("temperature").is_none(), "{b}");
        assert!(b.get("enable_thinking").is_none(), "{b}");
    }
}

fn snapshot_line_at(s: &str) -> usize {
    s.find("snapshot at")
        .unwrap_or_else(|| panic!("no snapshot line in:\n{s}"))
}

#[test]
fn colibri_run_a_port_that_refuses_the_connection_stops_before_the_snapshot() {
    // The risk: a hosted endpoint that is not listening found by the first
    // model call, after the snapshot, as a failed run with a trail.
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", free.local_addr().unwrap());
    drop(free);
    let (d, r) = chat_run("refused", &base, "glm-5.2");
    refused(&d, &r);
}

#[test]
fn colibri_run_an_identification_left_unanswered_stops_and_says_the_endpoint_did_not_answer() {
    let (base, _log) = stub(Answer::Hang, Answer::Hang);
    let (d, r) = chat_run("silent", &base, "glm-5.2");
    refused(&d, &r);
    assert!(r.all.contains("did not answer"), "{}", r.all);
}

#[test]
fn colibri_run_a_401_without_a_key_names_the_status_and_says_no_key_was_sent() {
    let (base, _log) = stub(reply("401 Unauthorized", UNAUTHORIZED), Answer::Hang);
    let (d, r) = chat_run("401", &base, "glm-5.2");
    refused(&d, &r);
    assert!(r.all.contains("401"), "{}", r.all);
    assert!(r.all.contains("no key was sent"), "{}", r.all);
    assert!(!r.all.contains("canary-body-word"), "{}", r.all);
}

#[test]
fn colibri_run_a_403_with_a_key_names_the_variable_and_holds_neither_key_nor_body() {
    // The risk: a refusal printing the key it carried, or a body that echoes
    // it.
    let (base, _log) = stub(reply("403 Forbidden", UNAUTHORIZED), Answer::Hang);
    let d = repo("403", "", &coli_provider(&base));
    let key = canary();
    let r = run(&d, Some("provider:coli#glm-5.2"), Some(&key), &[]);
    refused(&d, &r);
    assert!(r.all.contains("403"), "{}", r.all);
    assert!(r.all.contains("COLI_API_KEY"), "{}", r.all);
    assert!(!r.all.contains(&key), "{}", r.all);
    assert!(!r.all.contains("canary-body-word"), "{}", r.all);
}

#[test]
fn colibri_run_a_404_identification_continues_as_today() {
    let (base, log) = stub(reply("404 Not Found", NOT_FOUND), Answer::Hang);
    let (_d, r) = chat_run("404", &base, "glm-5.2");
    continued_as_today(&log, &r);
}

#[test]
fn colibri_run_a_500_identification_continues_as_today() {
    // The risk: a server that is not colibri, answering the read with an
    // error, refused when the run would have worked.
    let (base, log) = stub(
        reply("500 Internal Server Error", r#"{"error":"boom"}"#),
        Answer::Hang,
    );
    let (_d, r) = chat_run("500", &base, "glm-5.2");
    continued_as_today(&log, &r);
}

#[test]
fn colibri_run_a_405_identification_continues_as_today() {
    let (base, log) = stub(reply("405 Method Not Allowed", ""), Answer::Hang);
    let (_d, r) = chat_run("405", &base, "glm-5.2");
    continued_as_today(&log, &r);
}

#[test]
fn colibri_run_a_200_identification_that_is_not_json_continues_as_today() {
    let (base, log) = stub(reply("200 OK", "<html>hello</html>"), Answer::Hang);
    let (_d, r) = chat_run("notjson", &base, "glm-5.2");
    continued_as_today(&log, &r);
}

#[test]
fn colibri_run_a_model_list_owned_by_vllm_continues_as_today() {
    // The risk: any OpenAI-compatible server taken for colibri and probed.
    let (base, log) = stub(reply("200 OK", MODELS_VLLM), Answer::Hang);
    let (_d, r) = chat_run("vllm", &base, "glm-5.2");
    continued_as_today(&log, &r);
}

#[test]
fn colibri_run_a_spec_naming_another_id_stops_naming_both() {
    // colibri answers 404 to every request naming another id.
    let (base, log) = stub(reply("200 OK", &models_colibri("a")), Answer::Hang);
    let (d, r) = chat_run("mismatch", &base, "b");
    refused(&d, &r);
    assert!(r.all.contains("`a`") && r.all.contains("`b`"), "{}", r.all);
    assert_eq!(probes(&log), 0, "no probe of a model that is not served");
}

#[test]
fn colibri_run_an_olmoe_style_refusal_names_the_fallback_and_the_tier_4_spec() {
    let (base, _log) = stub(
        reply("200 OK", &models_colibri("olmoe")),
        reply("400 Bad Request", TOOLS_REFUSED_OLMOE),
    );
    let (d, r) = chat_run("olmoe", &base, "olmoe");
    refused(&d, &r);
    assert!(r.all.contains("COLI_TOOL_FALLBACK=1"), "{}", r.all);
    assert!(r.all.contains(&format!("vllm:{base}#olmoe")), "{}", r.all);
}

#[test]
fn colibri_run_an_inkling_style_refusal_names_the_tier_4_spec_and_not_the_fallback() {
    // The risk: telling an Inkling user to set a variable that cannot help.
    let (base, _log) = stub(
        reply("200 OK", &models_colibri("inkling")),
        reply("400 Bad Request", TOOLS_REFUSED_INKLING),
    );
    let (d, r) = chat_run("inkling", &base, "inkling");
    refused(&d, &r);
    assert!(r.all.contains(&format!("vllm:{base}#inkling")), "{}", r.all);
    assert!(!r.all.contains("COLI_TOOL_FALLBACK"), "{}", r.all);
    assert!(!r.all.contains("sends none"), "no key was sent: {}", r.all);
}

#[test]
fn colibri_run_a_refusal_after_a_key_was_sent_says_the_vllm_spec_sends_none() {
    // The risk: a tier-4 hint that a keyed colibri will refuse, unexplained.
    let (base, _log) = stub(
        reply("200 OK", &models_colibri("inkling")),
        reply("400 Bad Request", TOOLS_REFUSED_INKLING),
    );
    let d = repo("keyed-refusal", "", &coli_provider(&base));
    let key = canary();
    let r = run(&d, Some("provider:coli#inkling"), Some(&key), &[]);
    refused(&d, &r);
    assert!(r.all.contains(&format!("vllm:{base}#inkling")), "{}", r.all);
    assert!(r.all.contains("sends none"), "{}", r.all);
    assert!(!r.all.contains(&key), "{}", r.all);
}

#[test]
fn colibri_run_an_accepting_colibri_prints_tools_probed_before_the_snapshot() {
    let (base, log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    let (_d, r) = chat_run("accepted", &base, "glm-5.2");
    went_ahead(&r);
    let line = "runtime:  colibri, model glm-5.2, tools probed";
    let at = r
        .stdout
        .find(line)
        .unwrap_or_else(|| panic!("no `{line}` in:\n{}", r.stdout));
    assert!(at < snapshot_line_at(&r.stdout), "{}", r.stdout);
    assert_eq!(probes(&log), 1, "exactly one probe");
}

#[test]
fn colibri_run_a_probe_answered_500_is_not_verified_and_the_run_continues() {
    let (base, _log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply(
            "500 Internal Server Error",
            r#"{"error":"canary-body-word"}"#,
        ),
    );
    let (_d, r) = chat_run("probe500", &base, "glm-5.2");
    went_ahead(&r);
    let line = "runtime:  colibri, model glm-5.2, tools not verified: HTTP 500";
    let at = r
        .stdout
        .find(line)
        .unwrap_or_else(|| panic!("no `{line}` in:\n{}", r.stdout));
    assert!(at < snapshot_line_at(&r.stdout), "{}", r.stdout);
    assert!(!r.all.contains("canary-body-word"), "{}", r.all);
}

#[test]
fn colibri_run_a_probe_left_unanswered_is_closed_within_15_s_and_the_run_continues() {
    // The risk: a probe bound by the model call's 300 s timeout, so a busy
    // server holds the run before it starts.
    let (base, log) = stub(reply("200 OK", &models_colibri("glm-5.2")), Answer::Hang);
    let (_d, r) = chat_run("probehang", &base, "glm-5.2");
    went_ahead(&r);
    assert!(
        r.stdout
            .contains("runtime:  colibri, model glm-5.2, tools not verified: "),
        "{}",
        r.stdout
    );
    let probe = requests(&log)
        .into_iter()
        .find(|s| is_probe(&s.raw))
        .expect("the probe arrived");
    let closed = probe.closed_after.expect("the probe's connection closed");
    assert!(closed < Duration::from_secs(15), "closed after {closed:?}");
}

#[test]
fn colibri_run_a_spec_whose_model_carries_an_esc_byte_is_refused_before_any_read() {
    // The risk: a server's id repainting the user's terminal. A served id
    // reaches the runtime line only when it equals the spec's model, and
    // since #499 a spec carrying a control byte is refused at parse, so
    // that line cannot hold one; the mismatch message, where the served id
    // need not match, is covered below.
    let id = "glm\u{1b}[2J";
    let (base, log) = stub(
        reply("200 OK", &models_colibri(id)),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    // In the repository's config, as TOML can carry the byte and a command
    // line cannot be relied on to.
    let d = repo(
        "esc",
        &format!("[model]\nspec = \"openai-chat:{base}/v1#glm\\u001b[2J\"\n"),
        "",
    );
    let r = run(&d, None, None, &[]);
    refused(&d, &r);
    assert!(r.all.contains("control character U+001B"), "{}", r.all);
    assert!(!r.all.contains('\x1b'), "{:?}", r.all);
    assert!(requests(&log).is_empty(), "the endpoint was never read");
}

#[test]
fn colibri_run_a_served_id_with_an_esc_byte_is_escaped_in_the_mismatch_message() {
    let (base, _log) = stub(reply("200 OK", &models_colibri("a\u{1b}[2J")), Answer::Hang);
    let (d, r) = chat_run("esc-mismatch", &base, "b");
    refused(&d, &r);
    assert!(r.all.contains("a\\u{1b}[2J"), "{}", r.all);
    assert!(!r.all.contains('\x1b'), "{:?}", r.all);
}

#[test]
fn colibri_run_a_repository_chosen_stub_is_sent_no_key_with_the_variable_unset() {
    // The risk: a port the repository chose given a key (ADR 0024 §3).
    let (base, log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    let d = repo(
        "repo-chosen",
        &format!("[model]\nspec = \"openai-chat:{base}/v1#glm-5.2\"\n"),
        "",
    );
    let r = run(&d, None, None, &[]);
    went_ahead(&r);
    let got = requests(&log);
    let ident = got
        .iter()
        .find(|s| is_models_get(&s.raw))
        .expect("the identification read arrived");
    let lower = ident.raw.to_ascii_lowercase();
    assert!(
        !lower.contains("authorization") && !lower.contains("x-api-key"),
        "{}",
        ident.raw
    );
}

/// Every file under `dir` but git's object store.
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

#[test]
fn colibri_run_a_user_provider_s_key_reaches_the_identification_and_is_recorded_nowhere() {
    let (base, log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    let d = repo("user-key", "", &coli_provider(&base));
    let log_json = d.with_extension("jsonl");
    let _ = std::fs::remove_file(&log_json);
    let key = canary();
    let log_arg = log_json.display().to_string();
    let r = run(
        &d,
        Some("provider:coli#glm-5.2"),
        Some(&key),
        &["--log-json", &log_arg],
    );
    went_ahead(&r);
    // The positive half first: a leak test whose key was never sent proves
    // nothing.
    let got = requests(&log);
    let ident = got
        .iter()
        .find(|s| is_models_get(&s.raw))
        .expect("the identification read arrived");
    assert!(
        ident
            .raw
            .contains(&format!("\r\nAuthorization: Bearer {key}\r\n")),
        "{}",
        ident.raw
    );
    let mut paths = Vec::new();
    files(&d, &mut paths);
    assert!(
        paths.iter().any(|p| p.to_string_lossy().contains("runs")),
        "a trail was written"
    );
    let mut places = vec![
        ("stdout and stderr".to_string(), r.all.clone()),
        (
            "--log-json".to_string(),
            std::fs::read_to_string(&log_json).unwrap_or_default(),
        ),
    ];
    for p in &paths {
        places.push((
            p.display().to_string(),
            String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned(),
        ));
    }
    for (name, text) in &places {
        assert!(!text.contains(&key), "{name} contains the key");
    }
}

/// The profile's two keys on an openai-chat body with thinking off (ADR
/// 0047 decision 5).
fn has_profile(b: &serde_json::Value) -> bool {
    b["temperature"] == 0 && b["enable_thinking"] == false
}

fn has_neither(b: &serde_json::Value) -> bool {
    b.get("temperature").is_none() && b.get("enable_thinking").is_none()
}

#[test]
fn colibri_profile_a_run_against_colibri_sends_temperature_0_and_thinking_off() {
    // The risk: an identified colibri left sampling at 0.7, its Qwen3.8
    // thinking at `xhigh` because the run never said otherwise.
    let (base, log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    let (_d, r) = chat_run("profile-on", &base, "glm-5.2");
    went_ahead(&r);
    let bodies = chat_bodies(&log);
    assert!(bodies.len() >= 2, "the run's own requests: {}", r.all);
    for b in &bodies {
        assert!(has_profile(b), "{b}");
    }
}

#[test]
fn colibri_profile_a_run_against_a_vllm_owned_list_sends_neither_key() {
    // The risk: the profile sent to a server that is not colibri, whose body
    // must stay byte for byte as it was.
    let (base, log) = stub(reply("200 OK", MODELS_VLLM), Answer::Hang);
    let (_d, r) = chat_run("profile-vllm", &base, "glm-5.2");
    went_ahead(&r);
    let bodies = chat_bodies(&log);
    assert!(!bodies.is_empty(), "{}", r.all);
    for b in &bodies {
        assert!(has_neither(b), "{b}");
    }
}

#[test]
fn colibri_image_over_budget_is_refused_before_the_snapshot_and_no_chat_is_sent() {
    // The risk (#527): an image checked only against the 400,000-byte hosted
    // budget, before `preflight` identifies colibri, so one between 25,088
    // and 400,000 bytes passes and every request then goes over colibri's
    // budget. 100,000 bytes is 133,336 of base64.
    let (base, log) = stub(
        reply("200 OK", &models_colibri("glm-5.2")),
        reply("200 OK", TOOLS_ACCEPTED),
    );
    let d = repo("image-over-budget", "", "");
    // Outside the repository, so nothing needs committing.
    let img = d.with_extension("big.gif");
    let mut bytes = b"GIF89a".to_vec();
    bytes.resize(100_000, 0);
    std::fs::write(&img, &bytes).unwrap();
    let r = run_with(
        &d,
        Some(&format!("openai-chat:{base}/v1#glm-5.2")),
        None,
        &[],
        &["--image", &img.to_string_lossy()],
    );
    refused(&d, &r);
    assert!(
        r.all.contains("25088 bytes") && r.all.contains("[model] context_bytes"),
        "names the budget and the setting: {}",
        r.all
    );
    assert!(
        requests(&log).iter().any(|s| is_models_get(&s.raw)),
        "colibri was identified: {}",
        r.all
    );
    assert!(chat_bodies(&log).is_empty(), "no chat request: {}", r.all);
    let _ = std::fs::remove_file(&img);
}

/// What the classifier reads as an answer, so no run follows it.
const ANSWER: &str = r#"{"choices":[{"message":{"role":"assistant","content":"helper returns 1."}}],"usage":{"completion_tokens":4}}"#;

/// A stub for `trellis ask`: `models` to the identification read, and
/// [`ANSWER`] to every chat request.
fn ask_stub(models: Answer) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            s.set_read_timeout(Some(Duration::from_secs(30))).ok();
            let raw = read_request(&mut s);
            let answer = if is_models_get(&raw) {
                models.clone()
            } else {
                reply("200 OK", ANSWER)
            };
            seen.lock().unwrap().push(Seen {
                raw,
                closed_after: None,
            });
            if let Answer::Reply(status, body) = answer {
                respond(&mut s, status, &body);
            }
        }
    });
    (format!("http://{addr}"), log)
}

/// One question typed into `trellis session`, which classifies it through
/// `ask` before anything else: after `/model spec` when given, else with
/// the repository's `[model] spec`. No provider key is in the environment.
fn ask(d: &Path, spec: Option<&str>) -> Ran {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(d)
        .arg("session")
        .current_dir(d)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user_dir(d))
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("COLI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let typed = match spec {
        Some(s) => format!("/model {s}\nwhat does helper return?\n/exit\n"),
        None => "what does helper return?\n/exit\n".to_string(),
    };
    child
        .stdin
        .take()
        .unwrap()
        .write_all(typed.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let all = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    Ran { out, all, stdout }
}

/// The one classification body, after the identification read: `ask`
/// answered the question and nothing else was sent.
fn classification(log: &Log, r: &Ran) -> serde_json::Value {
    assert!(r.stdout.contains("helper returns 1."), "{}", r.all);
    let got = requests(log);
    let ident = got
        .iter()
        .position(|s| is_models_get(&s.raw))
        .unwrap_or_else(|| panic!("no identification read: {got:?}\n{}", r.all));
    let chats: Vec<usize> = (0..got.len())
        .filter(|&i| got[i].raw.starts_with("POST "))
        .collect();
    assert_eq!(chats.len(), 1, "one classification call: {got:?}");
    assert!(ident < chats[0], "identified before classifying: {got:?}");
    serde_json::from_str(got[chats[0]].raw.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn colibri_profile_ask_classifies_on_colibri_at_temperature_0_with_thinking_off() {
    // The risk: one question on Qwen3.8 thinking at `xhigh` and sampling at
    // 0.7, because the classifier built its own client without the profile.
    let (base, log) = ask_stub(reply("200 OK", &models_colibri("glm-5.2")));
    let d = repo("ask-profile", "", "");
    let r = ask(&d, Some(&format!("openai-chat:{base}/v1#glm-5.2")));
    let b = classification(&log, &r);
    assert!(has_profile(&b), "{b}");
    assert_eq!(probes(&log), 0, "ask sends no tools probe");
}

#[test]
fn colibri_profile_ask_sends_the_classification_as_today_when_not_identified() {
    // The risk: a 404, a 5xx, a body that is not JSON or another server's
    // list taken for colibri, and the profile sent where it was not.
    for (name, models) in [
        ("404", reply("404 Not Found", NOT_FOUND)),
        (
            "500",
            reply("500 Internal Server Error", r#"{"error":"boom"}"#),
        ),
        ("notjson", reply("200 OK", "<html>hello</html>")),
        ("vllm", reply("200 OK", MODELS_VLLM)),
    ] {
        let (base, log) = ask_stub(models);
        let d = repo(&format!("ask-{name}"), "", "");
        let r = ask(&d, Some(&format!("openai-chat:{base}/v1#glm-5.2")));
        let b = classification(&log, &r);
        assert!(has_neither(&b), "{name}: {b}");
    }
}

#[test]
fn colibri_profile_ask_identifies_a_repository_chosen_stub_without_a_key() {
    // The risk: the identification read giving a port the repository chose
    // a key the classification itself would not send (ADR 0024 §3).
    let (base, log) = ask_stub(reply("200 OK", &models_colibri("glm-5.2")));
    let d = repo(
        "ask-repo-chosen",
        &format!("[model]\nspec = \"openai-chat:{base}/v1#glm-5.2\"\n"),
        "",
    );
    let r = ask(&d, None);
    let b = classification(&log, &r);
    assert!(has_profile(&b), "{b}");
    let got = requests(&log);
    let ident = got.iter().find(|s| is_models_get(&s.raw)).unwrap();
    let lower = ident.raw.to_ascii_lowercase();
    assert!(
        !lower.contains("authorization") && !lower.contains("x-api-key"),
        "{}",
        ident.raw
    );
}
