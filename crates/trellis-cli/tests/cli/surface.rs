use super::*;

// ---------------------------------------------------------------------------
// the surface itself
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_command_fails_rather_than_falling_through_to_a_session() {
    // `trellis` with no subcommand opens an interactive session. A typo'd
    // subcommand must not do that, because a session started by accident
    // reads as a hang in CI.
    let d = repo("unknown-cmd");
    let out = run(&d, &["inndex"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn help_describes_the_commands_without_marketing_them() {
    // The house style, applied to the one text every user reads. State what
    // is true and what is not known.
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--help")
        .output()
        .unwrap();
    let s = stdout(&out).to_lowercase();
    for word in [
        "blazing",
        "seamless",
        "effortless",
        "revolutionary",
        "powerful",
        "cutting-edge",
    ] {
        assert!(!s.contains(word), "marketing language in --help: {word}");
    }
    for cmd in ["index", "radius", "run", "check", "doctor", "partitions"] {
        assert!(s.contains(cmd), "--help omits `{cmd}`");
    }
}

#[test]
fn help_current_tui_help_does_not_call_the_session_the_default() {
    // #662. The risk: help describing a default that changed. Bare
    // `trellis` stopped opening the line session (ADR 0050 §1, #561), and
    // `tui --help` still said "The line-based session stays the default".
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(["tui", "--help"])
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(!s.contains("stays the default"), "{s}");
    assert!(s.contains("`trellis --tui` opens it too"), "{s}");
}

#[test]
fn help_current_describes_bare_trellis_as_the_desktop_and_the_overview() {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--help")
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(
        s.contains("trellis          open the desktop window and print the overview"),
        "{s}"
    );
    assert!(!s.contains("interactive session"), "{s}");
}

#[test]
fn an_unreachable_runtime_costs_nothing() {
    // The failure a user actually hit, four times in one sitting.
    //
    // `Driver::build` parses a URL; it does not connect. So a runtime that
    // was not listening used to be discovered by the loop's *first* model
    // call — after the git snapshot — and the run would roll back and write
    // a trail for work that never began. Four dead attempts looked exactly
    // like four real failures and left four trail files behind.
    //
    // A refused connection is not a failed run. It is a run that could not
    // start, and it must cost nothing.
    let d = repo("dead-runtime");

    let out = run(
        &d,
        &["run", "--model", "llamacpp:http://127.0.0.1:1", "a task"],
    );
    let s = combined(&out);

    assert!(
        !out.status.success(),
        "a dead runtime must fail the run: {s}"
    );
    assert!(
        s.contains("cannot reach the runtime"),
        "it must name what it could not reach: {s}"
    );
    assert!(
        s.contains("Nothing was snapshotted"),
        "and say the run did not start, so the user knows there is nothing \
         to clean up: {s}"
    );

    // The two things that must not have happened.
    assert!(
        !d.join(".trellis/runs").exists()
            || std::fs::read_dir(d.join(".trellis/runs"))
                .map(|mut r| r.next().is_none())
                .unwrap_or(true),
        "a run that never started must not write a trail"
    );
    assert!(
        porcelain(&d).trim().is_empty(),
        "the worktree must be untouched"
    );

    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failed_check_says_why_it_failed() {
    // Reported from a real session: three turns printed "check FAILED" and
    // nothing else, so the one thing a user needs — what the checker
    // objected to — was the one thing not shown. The observation already
    // held it; it was what the model was given and the user was not.
    let d = repo("why-failed");
    std::fs::write(
        d.join("app/core.py"),
        "def add(a: int, b: int) -> int:\n    return a + b\n\nx: str = add(1, 2)\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"trellis-no-such-typechecker\"]\n",
    )
    .unwrap();
    let script = d.join("s.json");
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
            "make it typecheck",
        ],
    );
    let s = combined(&out);

    assert!(
        s.contains("check FAILED"),
        "the turn list still marks it: {s}"
    );
    assert!(
        s.contains("why the check failed:"),
        "and the run must say why, not only that: {s}"
    );
    assert!(
        s.contains("command:"),
        "naming the exact command is half the answer — it is how someone \
         reruns it by hand: {s}"
    );
    assert!(
        s.contains("trellis-no-such-typechecker"),
        "the checker's own output must reach the user: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
