use super::*;

// ---------------------------------------------------------------------------
// #248, pass 3: run_body's reporting, and doctor's slot count
// ---------------------------------------------------------------------------

#[test]
fn a_run_with_only_skipped_skills_still_says_so() {
    // #248. The risk: the `repo:` line printed only when some skill was
    // valid, so a repository whose one skill is malformed hears nothing
    // about it, and its author believes the model can read it.
    let d = repo("skills-only-invalid");
    add_skill(
        &d,
        "misnamed",
        "other-name",
        "Skipped: the name is not the directory.",
    );
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "skills"]);
    let scripts = scratch("skills-only-invalid-script");
    let out = run(&d, &["run", "--model", &script_that_stops(&scripts), "t"]);
    let said = stdout(&out);
    let repo_line = said
        .lines()
        .find(|l| l.starts_with("repo:"))
        .unwrap_or_else(|| panic!("no repo: line\n{}", combined(&out)));
    assert!(
        repo_line.contains("skills: 0 listed, 0 omitted (cap), 1 invalid and skipped"),
        "{repo_line}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// What `trellis run` printed for a run whose typecheck prints `n` lines
/// and fails.
fn check_failure_said(name: &str, n: usize) -> String {
    let d = empty_repo(name);
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    let body: String = (1..=n).map(|i| format!("line{i}\n")).collect();
    std::fs::write(d.join("out.txt"), body).unwrap();
    let check = if cfg!(windows) {
        "['cmd', '/C', 'type out.txt & exit 1']"
    } else {
        "['sh', '-c', 'cat out.txt; exit 1']"
    };
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[checks]\ntypecheck = {check}\n"),
    )
    .unwrap();
    std::fs::write(d.join("a.py"), "x = 1\n").unwrap();
    let scripts = scratch(&format!("{name}-script"));
    let script = scripts.join("s.json");
    std::fs::write(
        &script,
        r#"{"steps":[{"tool":"run_checks","paths":[],"kinds":["typecheck"]}]}"#,
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    let out = run(
        &d,
        &[
            "run",
            "--model",
            &format!("scripted:{}", script.display()),
            "t",
        ],
    );
    let said = stdout(&out);
    assert!(said.contains("why the check failed:"), "{}", combined(&out));
    let _ = std::fs::remove_dir_all(&d);
    said
}

/// The `K` of "… K more line(s)", if the run said it.
fn more_lines(said: &str) -> Option<usize> {
    let (_, rest) = said.split_once("  … ")?;
    rest.split_whitespace().next()?.parse().ok()
}

#[test]
fn a_failed_check_s_output_is_cut_at_twenty_four_lines_and_says_what_it_cut() {
    // #248. The boundary of "why the check failed": an observation of
    // exactly 24 lines is shown whole, with no "0 more", and one of 25
    // says it left one out. The observation carries a header around the
    // checker's output, so the fixture first measures that header rather
    // than hardcoding it.
    const SHOWN: usize = 24;
    let probe = 40;
    let total = SHOWN
        + more_lines(&check_failure_said("check-cut-probe", probe))
            .expect("40 lines of output are more than are shown");
    let header = total - probe;
    assert!(header < SHOWN, "the header alone fills the block: {header}");
    let exact = check_failure_said("check-cut-exact", SHOWN - header);
    assert_eq!(more_lines(&exact), None, "{exact}");
    let over = check_failure_said("check-cut-over", SHOWN - header + 1);
    assert_eq!(more_lines(&over), Some(1), "{over}");
}

/// A model run in `ignored_model_fixture` that runs `cmd` through `bash`,
/// then checks, then stops; what it printed.
fn converge_after_bash(name: &str, cmd: impl Fn(&Path) -> String) -> String {
    let (d, run) = ignored_model_fixture(name);
    let script = d
        .parent()
        .unwrap()
        .join(format!("{name}-script-{}.json", std::process::id()));
    std::fs::write(
        &script,
        serde_json::json!({"steps": [
            {"tool": "bash", "command": cmd(&d)},
            {"tool": "run_checks", "paths": ["a.py"], "kinds": ["typecheck", "tests"]},
            {"tool": "done", "summary": "done"},
        ]})
        .to_string(),
    )
    .unwrap();
    let out = run(&[
        "run",
        "a task",
        "--model",
        &format!("scripted:{}", script.display()),
    ]);
    let said = combined(&out);
    assert!(
        said.contains("CONVERGED") && !said.contains("NOT CONVERGED"),
        "{said}"
    );
    said
}

#[test]
fn a_converged_run_names_the_ignored_files_it_changed_and_says_nothing_when_none() {
    // #248. A converged run leaves its changes for review, and a change
    // to an ignored file is one `git status` does not show. The risk: that
    // line dropped, or printed with an empty list on every run that could
    // have written one.
    let said = converge_after_bash("converged-ignored", |d| {
        let p = d.display();
        if cfg!(windows) {
            format!(r"echo x = 1> {p}\.venv\new.py")
        } else {
            format!("echo 'x = 1' > '{p}/.venv/new.py'")
        }
    });
    assert!(
        said.contains("ignored:  the run changed files git ignores")
            && said.contains(".venv/new.py"),
        "{said}"
    );
    let said = converge_after_bash("converged-no-ignored", |_| "echo hello".to_string());
    assert!(!said.contains("ignored:"), "{said}");
}

/// A loopback llama.cpp stand-in reporting `slots` in `/props`, answering
/// every other request with an empty completion. Serves until the process
/// ends.
fn llamacpp_with_slots(slots: u32) -> String {
    llamacpp_answering(slots, "")
}

/// [`llamacpp_with_slots`], answering every completion with `content`.
fn llamacpp_answering(slots: u32, content: &'static str) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
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
            let request = String::from_utf8_lossy(&raw);
            let body = if request.starts_with("GET /props") {
                format!("{{\"total_slots\":{slots}}}")
            } else {
                serde_json::json!({ "content": content }).to_string()
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
    format!("llamacpp:http://{addr}")
}

#[test]
fn doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency() {
    // #248. S18: a runtime started without --parallel serialises agents,
    // and doctor is where a person finds out. The risk: the slot count
    // misread at the boundary between one and more than one.
    let d = repo("doctor-slots");
    let one = stdout(&run(&d, &["doctor", "--model", &llamacpp_with_slots(1)]));
    assert!(one.contains("batching:   single-stream"), "{one}");
    assert!(one.contains("reports a single slot"), "{one}");
    let two = stdout(&run(&d, &["doctor", "--model", &llamacpp_with_slots(2)]));
    assert!(
        two.contains("batching:   continuous, measured concurrency 2"),
        "{two}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The head `line` (a doctor `tier:` line) begins with, when it is one of
/// the heads `tests/client_contract.json` documents for `get_state`'s tier
/// label, followed by the documented separator and some explanation.
fn documented_tier_head(line: &str) -> Option<String> {
    let c = client_contract();
    let shape = &c["rpc"]["responses"]["get_state"]["tier_label"];
    let sep = shape["detail_separator"]
        .as_str()
        .expect("the contract names the separator");
    let body = line.strip_prefix("tier:")?.trim_start();
    let (head, detail) = body.split_once(sep)?;
    let listed = shape["heads"]
        .as_array()
        .expect("the contract lists the label heads")
        .iter()
        .any(|h| h.as_str() == Some(head));
    (listed && !detail.trim().is_empty()).then(|| head.to_string())
}

#[test]
fn doctor_tier_lines_begin_with_a_head_the_status_bar_contract_documents() {
    // #523. The risk: the status bar and `get_state` say `4, no schema`
    // while doctor, where a person goes to find out why, says
    // `ValidateAndRetry — …` about the same runtime, and a reader (or a
    // client reading by head) cannot tell the two are the same answer.
    // Every backend kind doctor can be pointed at here, by name; colibri's
    // own lines are checked in tests/colibri_doctor.rs.
    let d = repo("doctor-tier-lines");
    let slots = llamacpp_with_slots(1);
    let base = slots.trim_start_matches("llamacpp:").to_string();
    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        ("no --model", vec![], "unknown"),
        ("llama.cpp, schema ignored", vec![slots.clone()], "4"),
        (
            "llama.cpp, schema honoured",
            vec![llamacpp_answering(2, r#"{"trellis_probe_ok":true}"#)],
            "2",
        ),
        (
            "runtime that cannot be reached",
            vec!["llamacpp:http://127.0.0.1:1".into()],
            "unknown",
        ),
        (
            "hosted, not colibri",
            vec![format!("openai-chat:{base}/v1#m")],
            "1",
        ),
    ];
    for (kind, spec, want) in cases {
        let mut args = vec!["doctor"];
        if let Some(s) = spec.first() {
            args.extend(["--model", s.as_str()]);
        }
        let out = run(&d, &args);
        let s = stdout(&out);
        let tiers: Vec<&str> = s.lines().filter(|l| l.starts_with("tier:")).collect();
        assert!(
            !tiers.is_empty(),
            "{kind}: no tier line\n{}",
            combined(&out)
        );
        for t in tiers {
            assert_eq!(
                documented_tier_head(t).as_deref(),
                Some(want),
                "{kind}: {t:?} does not begin with the head `{want}, `"
            );
        }
    }
    // A scripted spec has no runtime to probe, and doctor refuses it rather
    // than print a tier line outside the vocabulary.
    let out = run(&d, &["doctor", "--model", "scripted:/tmp/x.json"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(
        !stdout(&out).lines().any(|l| l.starts_with("tier:")),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_unreachable_runtime_says_it_could_not_be_reached_not_pass_model() {
    // #526. The risk: a person who passed `--model` is told to "pass
    // --model to find out" when the runtime is down or on another port, so
    // the advice sends them back to the step they already took instead of
    // to the runtime. A port bound and released is closed on loopback, so
    // the connection is refused at once.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let d = repo("doctor-unreachable-runtime");
    let out = run(
        &d,
        &["doctor", "--model", &format!("llamacpp:http://{addr}")],
    );
    let s = stdout(&out);
    let tier = s
        .lines()
        .find(|l| l.starts_with("tier:"))
        .unwrap_or_else(|| panic!("no tier line\n{}", combined(&out)));
    assert!(tier.starts_with("tier:       unknown, "), "{tier}");
    assert!(tier.contains("could not reach the runtime"), "{tier}");
    assert!(tier.contains(&addr.to_string()), "{tier}");
    assert!(tier.contains("start it, or check the port"), "{tier}");
    assert!(!tier.contains("pass --model"), "{tier}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wait_line_doctor_names_the_address_it_probes_before_the_wait() {
    // #650. The risk: every probe ran before any output, with a 30 s bound
    // on the runtime's, so `doctor --model` against a runtime that does not
    // answer was half a minute of blank terminal while a person diagnosed
    // it. The line goes to stderr, and the report on stdout is unchanged.
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let d = repo("doctor-wait-line");
    let spec = format!("llamacpp:http://{addr}");
    let out = run(&d, &["doctor", "--model", &spec]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.lines()
            .any(|l| l == format!("probing {spec} (up to 30 s)")),
        "{}",
        combined(&out)
    );
    assert!(!stdout(&out).contains("probing "), "{}", combined(&out));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_repository_model_spec_with_esc_and_a_c1_control_never_reaches_the_banner() {
    // #492. The risk: a cloned repository chooses `[model] spec`, and the
    // banner's `model:` line echoed its model id raw, so ESC or a C1 CSI in
    // it could repaint the line a person reads before the run starts. TOML
    // decodes `\u001b` and `\u009b` to the real characters, which a command
    // line cannot be relied on to carry. Since #499 such a spec is refused
    // at parse, before any banner, and the refusal names the character
    // without repeating it.
    let d = repo("banner-escapes-spec");
    let base = llamacpp_with_slots(1).replace("llamacpp:", "");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[model]\nspec = \"openai-chat:{base}/v1#glm\\u001b[2J\\u009b31m\"\n"),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "spec with controls"]);
    let out = run(&d, &["run", "make greet louder"]);
    let said = combined(&out);
    assert!(!out.status.success(), "{said:?}");
    assert!(said.contains("control character U+001B"), "{said:?}");
    assert!(!said.contains("snapshot at"), "{said:?}");
    assert!(!said.contains('\x1b'), "{said:?}");
    assert!(!said.contains('\u{9b}'), "{said:?}");
    let _ = std::fs::remove_dir_all(&d);
}
