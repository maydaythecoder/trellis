use super::*;

// ---------------------------------------------------------------------------
// check — the grounding contract
// ---------------------------------------------------------------------------

#[test]
fn a_check_that_could_not_run_is_not_a_check_that_passed() {
    // Constraint 2, and the single most important assertion in this file.
    // An LLM cannot verify its own work; a correctness verdict comes only
    // from a process that executed. A `check` whose tools are absent and
    // which reports success would make the whole design a formality — the
    // absence of a signal is not a pass.
    //
    // The tools are named in config rather than left to the environment, and
    // named as commands that certainly do not exist. Depending on whether the
    // developer happens to have mypy installed would make this test assert
    // something different on every machine — which is what the first version
    // of it did, passing on a `pytest` that ran and found no tests.
    let d = repo("check-no-tools");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        concat!(
            "[checks]\n",
            "typecheck = [\"trellis-no-such-typechecker\"]\n",
            "tests = [\"trellis-no-such-test-runner\"]\n"
        ),
    )
    .unwrap();

    let out = run(&d, &["check"]);
    let s = combined(&out);
    let lower = s.to_lowercase();

    assert!(
        lower.contains("failed")
            || lower.contains("could not")
            || lower.contains("failed to spawn"),
        "a check that could not run must say so: {s}"
    );
    assert!(
        !lower.contains("passed (") && !lower.contains("verified"),
        "nothing executed, so nothing may be reported as passing: {s}"
    );
    assert!(
        !out.status.success(),
        "and it must not exit 0, or a CI script reads silence as success: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_repository_with_no_tests_does_not_report_a_pass() {
    // The same contract from the other side, and the state most first-time
    // users are actually in. The fixture has a type checker's worth of Python
    // and no test suite at all. Whatever the local environment provides, the
    // one thing forbidden is an overall success.
    let d = repo("check-no-tests");
    let out = run(&d, &["check"]);
    assert!(
        !out.status.success(),
        "a repository with nothing to ground a verdict on must not pass: {}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}
