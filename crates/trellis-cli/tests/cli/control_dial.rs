use super::*;

// ---------------------------------------------------------------------------
// control dial (ADR 0018)
// ---------------------------------------------------------------------------

/// A repository whose config puts runs in approve mode, committed.
fn repo_in_approve_mode(name: &str) -> Scratch {
    let d = repo(name);
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[control]\nmode = \"approve\"\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "approve mode"]);
    d
}

#[test]
fn approve_mode_without_a_terminal_refuses_before_anything_is_touched() {
    // `.output()` gives the child a pipe for stdin, which is exactly the case
    // the ADR names: nobody to ask. Starting and then denying every edit
    // would be a run that looks as though it worked without doing the work.
    let d = repo_in_approve_mode("approve-no-tty");
    let s_dir = scratch("approve-no-tty-script");
    let spec = format!("scripted:{}", {
        let s = s_dir.join("s.json");
        std::fs::write(
            &s,
            r#"{"steps":[{"tool":"edit","path":"app/core.py","old":"hello","new":"hi"}]}"#,
        )
        .unwrap();
        s.display().to_string()
    });
    let out = run(&d, &["run", "--model", &spec, "change greet"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(
        combined(&out).contains("approve mode needs an interactive terminal; pass --auto"),
        "{}",
        combined(&out)
    );
    assert!(!stdout(&out).contains("snapshot at"), "{}", combined(&out));
    assert!(
        !d.join(".trellis/runs").exists(),
        "no trail for a run that never began"
    );
    let status = porcelain(&d);
    assert!(status.is_empty(), "{status}");

    // And `--auto` is the way out it names.
    let out = run(&d, &["run", "--auto", "--model", &spec, "change greet"]);
    assert!(
        stdout(&out).contains("control:  auto"),
        "{}",
        combined(&out)
    );
}

#[test]
fn approve_and_auto_cannot_both_be_asked_for() {
    let d = repo("approve-and-auto");
    let out = run(
        &d,
        &[
            "run",
            "--approve",
            "--auto",
            "--model",
            "scripted:x.json",
            "t",
        ],
    );
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("cannot be used with"),
        "{}",
        combined(&out)
    );
}

#[test]
fn a_session_on_a_pipe_in_approve_mode_refuses_the_task_and_stays_open() {
    // The session is what runs without a terminal, so it is where approve
    // mode meets a pipe most often. The task is refused before a snapshot;
    // the session itself carries on, because a refused task is not an exit.
    let d = repo_in_approve_mode("approve-session");
    let s_dir = scratch("approve-session-script");
    let s = s_dir.join("s.json");
    std::fs::write(
        &s,
        r#"{"steps":[{"tool":"edit","path":"app/core.py","old":"hello","new":"hi"}]}"#,
    )
    .unwrap();
    let out = session_with(
        &d,
        &format!("/model scripted:{}\nchange greet\n/status\n", s.display()),
    );
    let said = combined(&out);
    assert!(
        said.contains("approve mode needs an interactive terminal"),
        "{said}"
    );
    assert!(!said.contains("snapshot at"), "{said}");
    assert!(!d.join(".trellis/runs").exists(), "{said}");
    assert!(out.status.success(), "{said}");
}
