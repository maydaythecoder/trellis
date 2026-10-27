use super::*;

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_without_a_model_reports_unknown_rather_than_guessing() {
    // S17 and S18. The difference between adapter tier 2 and tier 4 is
    // invisible to a user, and so is whether the backend can batch at all.
    // An optimistic guess about either is exactly the failure both soft spots
    // describe, so absent a probe the answer is "unknown" and multi-agent
    // work is treated as single-stream.
    let d = repo("doctor");
    let out = run(&d, &["doctor"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(
        s.contains("unknown") || s.contains("NOT"),
        "an unprobed runtime must be reported as unknown: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_reports_a_missing_test_suite_as_the_grounding_problem_it_is() {
    // The fixture has no test suite. Without one there is nothing for a
    // verdict to be grounded on, which makes it a finding rather than a
    // detail — `doctor` exists so that lands before a run does.
    // A runner configured by name and certainly absent, so the missing
    // branch is the one taken whether or not this machine has pytest.
    let d = repo_with_checks("doctor-notests", "git", "trellis-no-such-test-runner");
    let s = stdout(&run(&d, &["doctor"]));
    assert!(
        s.contains("tests:      NOT FOUND — the grounding contract has nothing to ground on"),
        "doctor must report the missing suite as a grounding problem: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_coverage_probe_never_appends_help_to_a_tests_command_that_is_not_pytest() {
    // #572. The risk: doctor appending `--help` to whatever the tests command
    // is. Fixtures configure `git --version`, and `git --version --help`
    // opens Git's HTML help in a browser on Windows, once per doctor run. A
    // stub first on PATH records every argv it is given; doctor must run it
    // (for the version) and never with `--help`.
    let bin = scratch("doctor-coverage-probe-bin");
    let log = bin.join("calls.log");
    let stub = if cfg!(windows) {
        let p = bin.join("trellis-stub-runner.cmd");
        std::fs::write(
            &p,
            "@echo off\r\necho %*>>\"%~dp0calls.log\"\r\necho stub-runner 1.0\r\n",
        )
        .unwrap();
        "trellis-stub-runner.cmd"
    } else {
        let p = bin.join("trellis-stub-runner");
        std::fs::write(
            &p,
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/calls.log\"\necho stub-runner 1.0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        "trellis-stub-runner"
    };
    let d = repo_with_checks("doctor-coverage-probe", "git", stub);
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("doctor")
        .current_dir(&d)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", combined(&out));
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("--version"),
        "the stub was never run, so this test proves nothing:\n{calls}\n{}",
        combined(&out)
    );
    assert!(
        !calls.contains("--help"),
        "doctor appended --help to a command that is not pytest:\n{calls}"
    );
    let s = stdout(&out);
    assert!(
        s.contains("coverage: not probed (the tests command is not pytest)"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
fn doctor_floor_line_states_four_configurations_and_the_tier_4_result() {
    // #542. The risk: the line a person reads before trusting a local model
    // keeps saying every measured configuration failed after #144 measured
    // one that repairs 5 of 20 at tier 4, so doctor contradicts its own
    // evidence. Both branches print the line: without --model, and with a
    // runtime that cannot be reached (a port bound and released is closed).
    let addr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let d = repo("doctor-floor-line");
    let spec = format!("llamacpp:http://{addr}");
    for args in [vec!["doctor"], vec!["doctor", "--model", &spec]] {
        let out = run(&d, &args);
        let s = stdout(&out);
        // The line wraps, so take `floor:` and its indented continuation.
        let block: Vec<&str> = s
            .lines()
            .skip_while(|l| !l.starts_with("floor:"))
            .enumerate()
            .take_while(|(i, l)| *i == 0 || l.starts_with(' '))
            .map(|(_, l)| l.trim())
            .collect();
        assert!(!block.is_empty(), "no floor line\n{}", combined(&out));
        let floor = block.join(" ");
        let lower = floor.to_lowercase();
        assert!(lower.contains("four configurations"), "{args:?}: {floor}");
        assert!(
            lower.contains("located only at tier 4"),
            "{args:?}: {floor}"
        );
        assert!(
            floor.contains("qwen3:8b Q4_K_M via ollama repaired 5/20"),
            "{args:?}: {floor}"
        );
        assert!(
            floor.contains("BENCHMARK_AND_EVAL.md §5.7"),
            "{args:?}: {floor}"
        );
        assert!(!lower.contains("all failed"), "{args:?}: {floor}");
        assert!(!lower.contains("all below"), "{args:?}: {floor}");
        assert!(!lower.contains("three configurations"), "{args:?}: {floor}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
