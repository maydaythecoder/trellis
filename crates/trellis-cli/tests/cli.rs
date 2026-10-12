//! The command surface, driven as a user drives it.
//!
//! Before this file existed `crates/trellis-cli` had no `tests/` directory.
//! Every `cmd_*` entry point was called from exactly one place — the dispatch
//! in `main` — and from no test, which put `main.rs` at 39.09% line coverage
//! and made it the origin of 85 of the first 87 surviving mutants. Recorded
//! as C-001 in CORRECTNESS_LEDGER.md.
//!
//! These tests spawn the real binary rather than calling `cmd_*` directly.
//! That is the point: `main.rs` is not a library and the argument parsing,
//! the dispatch, the exit codes and the printed output are the parts with no
//! other coverage. `CARGO_BIN_EXE_trellis` is set by cargo for an integration
//! test in the binary's own package.
//!
//! What they assert is mostly *what the output may not say*. The project's two
//! constraints are both negative — the graph must never claim completeness,
//! and nothing but an executed process may assert correctness — so a test that
//! only checked exit codes would pass while the guarantee was being broken in
//! prose.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary directory unique to this process.
///
/// The process id is not decoration. Five fixtures previously used fixed
/// paths and deleted each other's repositories whenever two test processes
/// ran at once, which `cargo mutants -j 4` does by design (C-001's sibling,
/// fixed in `c526c83`).
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-cli-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `git <args>` in `root` through `trellis_vcs::command()`, as trellis runs
/// it, panicking unless it exits 0; returns what git printed. A fixture or
/// read-back that failed silently left a test asserting on a repository
/// that was not the one it built (#240).
fn git(root: &Path, args: &[&str]) -> String {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A git repository holding a two-module Python package.
///
/// Deliberately small but not trivial: `caller.py` imports and calls
/// `core.greet`, so there is a real cross-file edge for `radius` to find, and
/// `mystery()` takes an untyped parameter so there is a real unbound call for
/// the coverage report to be honest about.
fn repo(name: &str) -> PathBuf {
    repo_with_doubt(name, CALLER_ONE_REASON, None)
}

/// `repo()`'s `caller.py`: one cross-file call and one unbound call.
const CALLER_ONE_REASON: &str = concat!(
    "from app.core import greet\n\n",
    "def welcome(who):\n",
    "    return greet(who)\n\n",
    "def mystery(thing):\n",
    "    return thing.run()\n",
);

/// An empty git repository with the identity and line endings every fixture
/// here needs, so no fixture can drift from the others.
fn empty_repo(name: &str) -> PathBuf {
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    // Two fixtures once lacked this, found by the Pass 6 duplication survey.
    // Without it git normalises line endings on `add`, and
    // `a_missing_scripted_model_fails_before_anything_is_touched` asserts an
    // empty `status --porcelain` on files it wrote itself.
    git(&d, &["config", "core.autocrlf", "false"]);
    d
}

/// A one-step script that stops immediately, and the `scripted:` spec for it.
///
/// Stopping is not converging: `Done` is the model saying it has nothing more
/// to do, and the loop still decides the outcome from the checks. That makes
/// this the cheapest script that exercises the real loop without asserting
/// anything about correctness.
fn script_that_stops(root: &Path) -> String {
    let p = root.join("script.json");
    std::fs::write(
        &p,
        r#"{"steps":[{"tool":"done","summary":"nothing to do"}]}"#,
    )
    .unwrap();
    format!("scripted:{}", p.display())
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        // The TUI refuses a non-terminal, and every other command must not
        // care. Inheriting this test harness's stdio would make that
        // difference depend on how the suite was launched.
        .env("NO_COLOR", "1")
        // Hermetic: the user-level config and context files (ADR 0025 §7)
        // of whoever runs the suite must not change what it sees.
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .output()
        .expect("the binary must be built before its integration tests run")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn combined(o: &Output) -> String {
    format!("{}{}", stdout(o), String::from_utf8_lossy(&o.stderr))
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

#[test]
fn index_reports_coverage_and_never_claims_to_be_complete() {
    // Constraint 1: complete call-graph recall is not achievable in Python,
    // so the graph is high-precision and explicitly incomplete. A bare
    // "complete" in this output is the failure, whatever the numbers say.
    let d = repo("index");
    let out = run(&d, &["index"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(
        s.contains("definitions"),
        "it must report what it found: {s}"
    );
    let lower = s.to_lowercase();
    for banned in [
        "complete call graph",
        "fully resolved",
        "all call sites resolved",
    ] {
        assert!(!lower.contains(banned), "index claimed completeness: {s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn index_says_its_percentage_is_a_lower_bound_and_not_recall() {
    // The number in this output is what the resolver *bound*. Comparing it to
    // the gate thresholds in BENCHMARK_AND_EVAL.md §3 is wrong, because an
    // `obj.method()` with an unknown receiver counts as a miss here even when
    // its true callee is third-party and excluded from the gate's denominator
    // outright. A-30, and the standing warning at the top of CLAUDE.md.
    //
    // The caveat travelling with the number is the entire defence. Without it
    // the figure reads as recall to every person who sees it once.
    let d = repo("index-caveat");
    // The caveat is hand-wrapped for the terminal, so match on the words
    // rather than on where the lines happen to break.
    let s = stdout(&run(&d, &["index"]))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        s.contains("not a verified recall figure"),
        "the figure must carry its own caveat: {s}"
    );
    assert!(
        s.contains("counts agreement with itself"),
        "it must say *why* it is not recall, or the caveat is a disclaimer: {s}"
    );
    assert!(
        s.contains("hand-audited sample"),
        "it must name what *is* comparable to the gate: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn index_with_the_pyright_backstop_absent_completes_and_says_it_did_not_run() {
    // ADR 0031: with Node or Pyright missing, the index does not fail, every
    // site keeps the resolver's verdict, and the output says the backstop did
    // not run, so it cannot be read as "Pyright ran and found nothing". PATH
    // points at an empty directory so the result does not depend on whether
    // the machine running the suite has Pyright.
    let d = repo("index-backstop");
    let off = stdout(&run(&d, &["index"]));
    assert!(off.contains("backstop: off"), "{off}");

    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[index]
backstop = \"pyright\"
",
    )
    .unwrap();
    let empty = scratch("index-backstop-path");
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("index")
        .current_dir(&d)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("PATH", &empty)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("backstop: pyright did not run"), "{s}");
    assert!(s.contains("keep the resolver's verdict"), "{s}");
    assert!(
        s.contains("first-party call sites resolved"),
        "the resolver's figures still print: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn index_does_not_wait_two_minutes_on_a_backstop_server_that_never_answers_initialize() {
    // #445: `trellis index --graph` went quiet for over two minutes and was
    // stopped by hand. With `[index] backstop = "pyright"` and a server that
    // starts but never answers `initialize`, the handshake was held to the
    // 120 s meant for a first query's import analysis, then `finish` added
    // up to 7 s: 125 s measured, with nothing printed after the coverage.
    // The handshake now has its own bound, and the output names it.
    let d = repo("index-silent-backstop");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[index]\nbackstop = \"pyright\"\n",
    )
    .unwrap();
    // A `pyright-langserver` that reads its input to the end and writes
    // nothing. It exits when trellis closes its stdin, so nothing outlives
    // the test.
    let bin = scratch("index-silent-backstop-bin");
    if cfg!(windows) {
        std::fs::write(
            bin.join("pyright-langserver.cmd"),
            "@\"%SystemRoot%\\System32\\sort.exe\" > nul\r\n",
        )
        .unwrap();
    } else {
        // No `exec`: `sh` must stay alive holding the stdout pipe, as
        // cmd.exe does on Windows. With `exec cat > /dev/null` no process
        // holds it, trellis reads end of stream at once and reports that the
        // server closed its output, which is not the silent server this test
        // is about.
        let stub = bin.join("pyright-langserver");
        std::fs::write(&stub, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();

    let started = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(["index", "--graph"])
        .current_dir(&d)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("PATH", path)
        .output()
        .unwrap();
    let took = started.elapsed();

    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(
        s.contains("backstop: pyright did not run (no answer to initialize within 20s)"),
        "{s}"
    );
    assert!(s.contains("codebase graph"), "{s}");
    // 20 s for the handshake and at most 7 s for `finish`; well under the
    // 125 s it took before.
    assert!(
        took < std::time::Duration::from_secs(60),
        "index waited {took:?} on a silent server"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bin);
}

// ---------------------------------------------------------------------------
// radius
// ---------------------------------------------------------------------------

#[test]
fn radius_ships_reach_and_doubt_as_two_facts() {
    // ADR 0002 and ADR 0014: every blast radius ships with its coverage, as
    // two facts and never a ratio. `RadiusCoverage` has no `total()` and no
    // fraction because the reach counts and the doubt count come from
    // different populations — dividing them once printed "3.7% resolved" for
    // a resolver that hand-audits at 72%.
    let d = repo("radius");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("greet"), "it must name the symbol: {s}");
    assert!(
        s.contains("call site") || s.contains("caller"),
        "it must report reach: {s}"
    );
    // `repo()`'s `thing.run()` is the doubt.
    assert!(s.contains("bound to nothing"), "it must report doubt: {s}");
    assert!(!s.contains('%'), "and never a ratio of the two: {s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_on_an_unknown_symbol_fails_and_says_what_it_searched() {
    // A silent empty radius is indistinguishable from a symbol with no
    // callers, and the two mean opposite things for whether an edit is safe.
    let d = repo("radius-missing");
    let out = run(&d, &["radius", "app.core.no_such_function"]);
    assert!(!out.status.success(), "it must fail: {}", combined(&out));
    let s = combined(&out);
    assert!(
        s.contains("no_such_function"),
        "the error must name what was asked for: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

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

// ---------------------------------------------------------------------------
// run — the gates before a single token is spent
// ---------------------------------------------------------------------------

#[test]
fn run_refuses_a_dirty_worktree_and_names_the_way_out() {
    // ADR 0009: a clean worktree is required before a run, because rollback
    // is exact only if the pre-run state is a commit. Refusing is the easy
    // half; the hard half is that the refusal must not become a wall, since
    // this is the first thing most users hit.
    let d = repo("run-dirty");
    // A *valid* script, so the run gets as far as the worktree gate. With a
    // broken one the model error arrives first and this would pass without
    // ever reaching what it claims to test.
    let script = script_that_stops(&d);
    std::fs::write(d.join("app/core.py"), "def greet(name):\n    return 1\n").unwrap();

    let out = run(&d, &["run", "--model", &script, "do something"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let s = combined(&out);
    assert!(
        s.contains("uncommitted") || s.contains("clean"),
        "it must say why it refused: {s}"
    );
    assert!(
        s.contains("commit") || s.contains("stash"),
        "and how to get past it: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_prompt_template_name_may_not_climb_out_of_the_prompts_directory() {
    // `--prompt` resolves to `.trellis/prompts/<name>.md`. A name containing
    // a separator or `..` would read an arbitrary file and paste it into the
    // model's prompt, which is both a disclosure and an injection path: a
    // template cannot set the model, lift a denied path or change a budget,
    // and reading outside the directory is how it would try.
    let d = repo("prompt-traversal");
    for bad in ["../../../etc/passwd", "..", "sub/dir", "a\\b"] {
        let out = run(&d, &["run", "--prompt", bad, "--model", "scripted:x.json"]);
        assert!(!out.status.success(), "`{bad}` was accepted");
        let s = combined(&out);
        assert!(
            s.contains("not a prompt name") || s.contains("file stem"),
            "`{bad}` must be refused as a name, not merely fail to open: {s}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_scripted_model_fails_before_anything_is_touched() {
    // The model is resolved before the loop starts, so a typo costs an error
    // rather than a half-finished run to roll back.
    let d = repo("run-nomodel");
    let out = run(
        &d,
        &["run", "--model", "scripted:does-not-exist.json", "a task"],
    );
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(
        porcelain(&d).is_empty(),
        "a failed start must leave the tree exactly as it was"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// partitions
// ---------------------------------------------------------------------------

#[test]
fn partitions_states_that_it_is_not_a_correctness_guarantee() {
    // ADR 0005 and S19. Partitioning is a filter on what MAY run
    // concurrently; it is only as good as the graph's recall, which is
    // partial and known to be. The integrated check across the union still
    // runs, and the output has to say so — a plan that looks authoritative is
    // how a partial graph gets mistaken for a proof.
    let d = repo("partitions");
    let out = run(&d, &["partitions", "app.core.greet", "app.caller.welcome"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(
        s.contains("never a correctness") || s.contains("not a correctness"),
        "the plan must disclaim being a guarantee: {s}"
    );
    assert!(
        s.contains("full check suite") || s.contains("union"),
        "and must say the integrated check still runs: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn partitions_puts_contended_work_in_one_stage_before_the_rest() {
    // ADR 0016. Two symbols in one file can never be edited by two agents,
    // however disjoint their call graphs, so they must not come back as a
    // parallel pair.
    let d = repo("partitions-shared");
    let out = run(
        &d,
        &["partitions", "app.caller.welcome", "app.caller.mystery"],
    );
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    // Asserted against what the command actually prints, not against strings
    // guessed at from the outside — the first version of this test checked
    // for the absence of two phrases `explain` never emits, and would have
    // passed however the plan came out.
    assert!(
        s.contains("1 partition(s)"),
        "two symbols in one file must land in one partition: {s}"
    );
    assert!(
        s.contains("grouped because they share:") && s.contains("app/caller.py"),
        "and the plan must name the file that forced it: {s}"
    );
    for sym in ["app.caller.welcome", "app.caller.mystery"] {
        assert!(s.contains(sym), "{sym} is missing from the plan: {s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// resume, log, why
// ---------------------------------------------------------------------------

#[test]
fn resume_with_no_trail_says_so_and_names_what_writes_one() {
    let d = repo("resume-empty");
    let out = run(&d, &["resume"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("Nothing to resume"), "{s}");
    assert!(
        s.contains("trellis run"),
        "it must name what writes one: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Writes a minimal trail as `<run_id>.json`. Ids begin with a sortable
/// timestamp, so the order they are written in does not matter: `log` and
/// `resume` both order by name.
fn plant_trail(root: &Path, run_id: &str, task: &str) {
    let dir = root.join(".trellis/runs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{run_id}.json")),
        format!(r#"{{"run_id":"{run_id}","task":"{task}","factual":{{"proposed":[]}}}}"#),
    )
    .unwrap();
}

#[test]
fn resume_run_id_known_loads_that_run_rather_than_the_newest() {
    // The risk: an id that is accepted and then ignored, so the user reads
    // the newest run's facts believing they are the named run's.
    let d = repo("resume-id-known");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = run(&d, &["resume", "20260101-000000-000-older"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("previous task: the older task"), "{s}");
    assert!(
        !s.contains("the newer task"),
        "the newest run leaked in: {s}"
    );
    // `run --resume` carries the newest trail only, so offering it here
    // would name a different run than the one just shown.
    assert!(
        !s.contains("pass --resume"),
        "an older run must not be offered to `run --resume`: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_unknown_fails_naming_the_ten_most_recent_ids() {
    // The risk: an unknown id falling back to some other run. Resuming from
    // the wrong run is worse than refusing, so it fails, and names what
    // exists so the user need not run a second command to find out.
    let d = repo("resume-id-unknown");
    for i in 0..11 {
        plant_trail(&d, &format!("202601{:02}-000000-000-run", i + 1), "t");
    }
    // A prefix of a real id is still unknown: a partial match could pick
    // either of two runs sharing it.
    let out = run(&d, &["resume", "20260111"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let s = combined(&out);
    assert!(s.contains("no run `20260111`"), "{s}");
    for i in 2..=11 {
        let id = format!("202601{i:02}-000000-000-run");
        assert!(s.contains(&id), "{id} is one of the ten newest: {s}");
    }
    assert!(
        !s.contains("20260101-000000-000-run"),
        "the eleventh-newest id must not be listed: {s}"
    );
    assert!(!s.contains("previous task"), "nothing may be loaded: {s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_bare_resume_still_loads_the_newest() {
    // The risk: making the id optional changed what bare `resume` does.
    let d = repo("resume-id-bare");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = run(&d, &["resume"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("previous task: the newer task"), "{s}");
    assert!(!s.contains("the older task"), "{s}");
    assert!(s.contains("pass --resume"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_session_passes_the_id_through() {
    // `/resume <run_id>` is the session spelling of the same command; an id
    // dropped between the prompt and `cmd_resume` would show the newest run.
    let d = repo("resume-id-session");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = session_with(&d, "/resume 20260101-000000-000-older\n/exit\n");
    let s = combined(&out);
    assert!(s.contains("previous task: the older task"), "{s}");
    assert!(!s.contains("the newer task"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_with_no_trails_is_not_an_error() {
    // A fresh repository has no runs. Exiting non-zero here would break any
    // script that checks history before deciding to write more.
    let d = repo("log-empty");
    let out = run(&d, &["log"]);
    assert!(out.status.success(), "{}", combined(&out));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_actions_lists_each_recorded_action_and_only_the_outcomes_there_are() {
    // The risk (#248): `--actions` printing nothing, or a blank outcome row
    // under every action that has none, so the one that wrote is lost
    // among them.
    let d = repo("log-actions");
    let runs = d.join(".trellis/runs");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(
        runs.join("r1.json"),
        r#"{"run_id":"r1","task":"t","factual":{"proposed":[],"actions":[
            {"turn":1,"tool":"read_file","target":"a.py","outcome":"","wrote":false},
            {"turn":2,"tool":"edit","target":"a.py","outcome":"applied","wrote":true}]}}"#,
    )
    .unwrap();
    let s = stdout(&run(&d, &["log", "--actions"]));
    assert!(s.contains("actions (2), as recorded:"), "{s}");
    assert!(
        s.contains("    *  2. edit a.py\n          applied\n"),
        "{s}"
    );
    assert!(s.contains("       1. read_file a.py\n    *  2."), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_inferred_root_is_named_and_an_explicit_one_is_not() {
    // The risk (#248): acting on a tree found several levels up without
    // saying which, so a task runs against the wrong repository.
    let d = repo("root-named");
    let inferred = combined(&run(&d.join("app"), &["log"]));
    assert!(inferred.contains("(repository root)"), "{inferred}");
    let elsewhere = scratch("root-named-elsewhere");
    let root = d.to_str().unwrap();
    let explicit = combined(&run(&elsewhere, &["--root", root, "log"]));
    assert!(!explicit.contains("(repository root)"), "{explicit}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

#[test]
fn run_approve_overrides_a_repository_in_auto_mode() {
    // The risk (#248): `--approve` dropped, so a person who asked to be
    // asked before each edit gets a run that asks nothing (ADR 0018).
    let d = repo("approve-over-auto");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[control]\nmode = \"auto\"\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "auto mode"]);
    let spec = script_that_stops(&scratch("approve-over-auto-script"));
    let out = run(&d, &["run", "--approve", "--model", &spec, "t"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(
        combined(&out).contains("approve mode needs an interactive terminal"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn run_refuses_a_skill_it_cannot_find_rather_than_running_without_it() {
    // The risk (#248): `--skill` dropped on the way to the run, so the run
    // goes ahead without the instructions the person named.
    let d = repo("skill-missing");
    let spec = script_that_stops(&scratch("skill-missing-script"));
    let out = run(&d, &["run", "--skill", "nope", "--model", &spec, "t"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(combined(&out).contains("nope"), "{}", combined(&out));
    assert!(!d.join(".trellis/runs").exists(), "no trail");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_reports_absence_rather_than_inventing_a_reason() {
    // `why` answers from the trail. With no trail there is no answer, and the
    // honest output is that there is none — the alternative is a plausible
    // reconstruction, which is the one thing a decision trail exists to stop.
    let d = repo("why-empty");
    let out = run(&d, &["why", "app.core.greet"]);
    let s = combined(&out);
    assert!(out.status.success(), "{s}");
    assert!(
        s.contains("no run in the trail mentions `app.core.greet`"),
        "it must report having nothing to say: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

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

// ---------------------------------------------------------------------------
// init, and the repository gate
// ---------------------------------------------------------------------------

#[test]
fn init_scaffolds_and_a_second_init_changes_nothing() {
    let d = scratch("init");
    git(&d, &["init", "-q"]);
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(d.join(".trellis/config.toml").exists());

    std::fs::write(d.join(".trellis/config.toml"), "# mine\n").unwrap();
    assert!(run(&d, &["init"]).status.success());
    assert_eq!(
        std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap(),
        "# mine\n",
        "init must never clobber a config someone tuned"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn commands_needing_a_repository_refuse_outside_one_and_name_the_fix() {
    // The failure a new user hits first. Naming `git init` is the difference
    // between a tool that seems broken and one that seems strict.
    let d = scratch("no-repo");
    for args in [vec!["index"], vec!["init"], vec!["check"]] {
        let out = run(&d, &args);
        assert!(!out.status.success(), "{args:?} succeeded outside a repo");
        let s = combined(&out);
        assert!(
            s.contains("git init") || s.contains("git repository"),
            "{args:?} must name the missing step: {s}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_listing_works_outside_a_repository() {
    // Models are user state, not repository state. `trellis model` is how
    // someone gets a model *before* they have a project to use it on, so
    // gating it on a repository would invert the setup order.
    let d = scratch("no-repo-model");
    let out = run(&d, &["model"]);
    assert!(
        out.status.success(),
        "model list must not need a repository: {}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_init_works_outside_a_repository_and_other_vault_actions_still_refuse() {
    // ADR 0022 §3, integrated: a vault may be created before there is a
    // repository to put it in. The risk on the other side is the exemption
    // widening to every `vault` action, which would let `vault check` read
    // whatever directory someone happened to be standing in.
    let d = scratch("no-repo-vault");
    let out = run(&d, &["vault", "init", "notes"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert_eq!(
        std::fs::read_to_string(d.join("notes/manifest.json"))
            .unwrap()
            .trim(),
        "[]"
    );
    assert!(d.join("notes/layout.json").is_file());
    let out = run(&d, &["vault", "--vault", "notes", "check"]);
    assert!(!out.status.success(), "vault check needs a repository");
    let s = combined(&out);
    assert!(
        s.contains("git init") || s.contains("git repository"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

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

// ---------------------------------------------------------------------------
// radius — how much of the doubt reaches the screen
// ---------------------------------------------------------------------------

/// A repository whose doubt has a chosen shape.
///
/// `repo()` produces exactly one unbound call in one file, which is enough to
/// prove doubt is reported at all and not enough to reach any of the code
/// that decides *how much* of it to show. That code branches on how many
/// distinct reasons there are and on how many files they sit in, so the
/// fixture has to be able to sit on either side of both boundaries — hence
/// the caller body being an argument rather than three near-identical
/// fixtures differing by two lines.
fn repo_with_doubt(name: &str, caller: &str, other: Option<&str>) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    std::fs::write(d.join("app/caller.py"), caller).unwrap();
    if let Some(o) = other {
        std::fs::write(d.join("app/other.py"), o).unwrap();
    }
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// MethodDispatchUntyped, LocalBinding, UnboundName, DynamicAttribute —
/// four, so that three shown leaves a residue of exactly one.
const CALLER_FOUR_REASONS: &str = concat!(
    "from app.core import greet\n\n",
    "def welcome(who):\n    return greet(who)\n\n",
    "def mystery(thing):\n    return thing.run()\n\n",
    "def indirect(cb):\n    return cb()\n\n",
    "def missing():\n    return no_such_name()\n\n",
    "def dynamic(o):\n    return getattr(o, \"go\")()\n",
);

/// The same, minus the `getattr` call: **exactly** `SHOWN`, which is the only
/// input that tells `>` and `>=` apart on the residue row.
const CALLER_THREE_REASONS: &str = concat!(
    "from app.core import greet\n\n",
    "def welcome(who):\n    return greet(who)\n\n",
    "def mystery(thing):\n    return thing.run()\n\n",
    "def indirect(cb):\n    return cb()\n\n",
    "def missing():\n    return no_such_name()\n",
);

const OTHER_WITH_DOUBT: &str = concat!(
    "from app.core import greet\n\n",
    "def again(who):\n    return greet(who)\n\n",
    "def second_mystery(thing):\n    return thing.launch()\n\n",
    "def second_indirect(cb):\n    return cb()\n",
);

/// Four reasons across two files: past `SHOWN` and past the one-file guard,
/// so the residue row and the concentration block both render.
fn repo_with_doubt_spread(name: &str) -> PathBuf {
    repo_with_doubt(name, CALLER_FOUR_REASONS, Some(OTHER_WITH_DOUBT))
}

/// `n` files that each call `greet` and each carry one unbound call.
///
/// The concentration block shows five files and collapses the rest, so `n`
/// is what puts a run either side of that boundary. One reason throughout,
/// deliberately: this fixture is about the file list, and a second reason
/// would move the table above it as well.
fn repo_with_doubt_in_n_files(name: &str, n: usize) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    for i in 0..n {
        std::fs::write(
            d.join(format!("app/m{i}.py")),
            format!(
                "from app.core import greet\n\n\
                 def call{i}(who):\n    return greet(who)\n\n\
                 def doubt{i}(thing):\n    return thing.run()\n"
            ),
        )
        .unwrap();
    }
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// A repository with no doubt at all in the files a radius touches.
///
/// Every fixture above carries an unbound call on purpose, so the honest
/// all-clear — the branch that says every call site *was* bound — has never
/// been printed under test. Nothing here calls a method on an untyped value,
/// names an unbound name, or passes a callable, so there is nothing for the
/// resolver to miss.
fn repo_without_doubt(name: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app/caller.py"),
        "from app.core import greet\n\ndef welcome(who):\n    return greet(who)\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// The `<count> <path>` rows under "concentrated in:", residue row included.
fn file_rows(s: &str) -> Vec<u32> {
    s.lines()
        .skip_while(|l| !l.contains("concentrated in"))
        .skip(1)
        .map_while(|l| l.split_whitespace().next()?.parse::<u32>().ok())
        .collect()
}

/// The `<Name> <count>` rows of the doubt table, residue row included.
///
/// Shape-matched rather than pattern-matched, and deliberately strict about
/// what counts: the file-concentration rows below the table lead with their
/// count instead of a name, so they cannot be mistaken for reasons.
fn reason_rows(s: &str) -> Vec<(String, u32)> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?;
            let n: u32 = it.next()?.parse().ok()?;
            let looks_like_a_reason =
                name == "(other)" || name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            looks_like_a_reason.then(|| (name.to_string(), n))
        })
        .collect()
}

#[test]
fn radius_does_not_claim_every_call_site_was_bound_when_one_was_not() {
    // Constraint 1, at the surface: the graph must never claim completeness.
    // `repo()` contains `thing.run()` on an untyped parameter, so there is
    // genuinely one call site bound to nothing, and the report has a branch
    // that says "every call site ... was bound" for the case where there is
    // not. Picking that branch here would print a false all-clear over real
    // doubt, which is the one output this project cannot ship.
    //
    // The older radius test greps for "call site", which appears in *both*
    // branches and so passes either way. That is why this needed its own.
    let d = repo("radius-completeness");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(
        !s.contains("was bound"),
        "a radius with real doubt claimed every call site was bound:\n{s}"
    );
    assert!(
        s.contains("bound to nothing"),
        "and it has to say so positively, not merely omit the claim:\n{s}"
    );
    assert!(
        s.contains("MethodDispatchUntyped"),
        "with the reason, which is what tells a reader whose fault it is:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_doubt_reasons_account_for_the_whole_count() {
    // ADR 0014's residue rule. The table shows three reasons and collapses
    // the rest into "(other)", and the point of that row is that the
    // headline figure stays fully accounted for on screen. Drop it and the
    // rows silently sum to less than the number printed above them, which is
    // the same class of error as dividing two populations: every line true,
    // the screen false.
    let d = repo_with_doubt_spread("radius-residue");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let headline: u32 = s
        .lines()
        .find(|l| l.contains("bound to nothing"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline to check against:\n{s}"));

    let rows = reason_rows(&s);
    assert!(
        rows.iter().any(|(n, _)| n == "(other)"),
        "this fixture has more reasons than the table shows, so there must be a residue row:\n{s}"
    );
    let shown: u32 = rows.iter().map(|(_, n)| n).sum();
    assert_eq!(
        shown, headline,
        "the reasons on screen must account for the headline figure; rows were \
         {rows:?} against a headline of {headline}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_omits_the_residue_row_when_there_is_no_residue() {
    // The other direction, and the one a `>` -> `>=` slip breaks: with no
    // reasons beyond the three shown, "(other) 0 across 0 further reasons"
    // is a row claiming a residue that does not exist. `repo()` has exactly
    // one reason, so any residue row here is spurious.
    let d = repo("radius-no-residue");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);
    assert!(
        !s.contains("(other)"),
        "a residue row appeared with nothing to collapse into it:\n{s}"
    );
    // Same principle one block down. "concentrated in:" above a list of one
    // file distinguishes nothing; the doubt line already named the count.
    assert!(
        !s.contains("concentrated in"),
        "doubt in a single file was reported as a concentration:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_omits_the_residue_row_at_exactly_the_number_it_shows() {
    // The boundary itself, and the only input that separates `>` from `>=`.
    // With four reasons both behave alike and with one both stay silent, so
    // a fixture at exactly `SHOWN` is the whole test: `>=` here prints
    // "(other) 0 across 0 further reasons", a row whose two figures are both
    // zero, sitting under a table that already accounted for everything.
    let d = repo_with_doubt("radius-residue-boundary", CALLER_THREE_REASONS, None);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let rows = reason_rows(&s);
    assert_eq!(
        rows.len(),
        3,
        "this fixture is only meaningful at exactly three reasons; got {rows:?}\n{s}"
    );
    assert!(
        !s.contains("(other)"),
        "a residue row at exactly the number shown, with nothing left over:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_names_the_files_the_doubt_sits_in_when_it_is_spread() {
    // A count with no location is a number a reader cannot act on. The
    // concentration block is only worth printing when there is more than one
    // file to distinguish between, which is the condition being defended.
    let d = repo_with_doubt_spread("radius-concentration");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);
    assert!(s.contains("concentrated in"), "got:\n{s}");
    assert!(s.contains("app/caller.py"), "got:\n{s}");
    assert!(s.contains("app/other.py"), "got:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_offers_the_way_to_shrink_doubt_only_when_there_is_doubt_to_shrink() {
    // The closing advice — "add a test that exercises this symbol" — is
    // conditional, and both directions are wrong in their own way. Printed
    // over a clean radius it invents work and quietly contradicts the line
    // above it; withheld where there *is* doubt it leaves the reader the
    // number and not the move. Asserted in both directions, because a guard
    // tested one way is a guard that can be inverted for free.
    let dirty = repo("radius-advice-doubt");
    let s = stdout(&run(&dirty, &["radius", "app.core.greet"]));
    assert!(
        s.contains("add a test that exercises this symbol"),
        "doubt was reported with no way out of it offered:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&dirty);

    let clean = repo_without_doubt("radius-advice-clean");
    let s = stdout(&run(&clean, &["radius", "app.core.greet"]));
    assert!(
        s.contains("was bound"),
        "this fixture is only meaningful with no doubt in the touched files:\n{s}"
    );
    assert!(
        !s.contains("To shrink the doubt"),
        "a clean radius was told to go shrink doubt it does not have:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&clean);
}

#[test]
fn radius_file_list_accounts_for_the_whole_count_when_it_is_truncated() {
    // The residue rule again, one block down. Five files are listed and the
    // rest collapse into "across N further files", and the reason that row
    // exists is that the listed counts otherwise sum to less than the
    // headline — a screen that quietly stops adding up.
    let d = repo_with_doubt_in_n_files("radius-files-residue", 7);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let headline: u32 = s
        .lines()
        .find(|l| l.contains("bound to nothing"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline to check against:\n{s}"));

    assert!(
        s.contains("further files"),
        "seven files is past the five shown, so there must be a residue row:\n{s}"
    );
    let rows = file_rows(&s);
    assert_eq!(
        rows.iter().sum::<u32>(),
        headline,
        "the files on screen must account for the headline figure; rows were \
         {rows:?} against a headline of {headline}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_file_list_omits_the_residue_row_at_exactly_the_number_it_shows() {
    // Five files, the boundary — the only input that separates `>` from
    // `>=` on the file residue, where `>=` prints "0 across 0 further
    // files" under a list that already named every one of them.
    let d = repo_with_doubt_in_n_files("radius-files-boundary", 5);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert_eq!(
        file_rows(&s).len(),
        5,
        "this fixture is only meaningful at exactly five files:\n{s}"
    );
    assert!(
        !s.contains("further files"),
        "a residue row at exactly the number shown, with nothing left over:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_reason_table_lines_up() {
    // End to end, because the defect lived in the seam: `radius_of` asks for
    // `{reason:<22}` and `Reason`'s Display used `write_str`, which ignores
    // the width the caller asked for. Every reason row ran ragged while the
    // "(other)" row — a plain &str, which does pad — lined up, so the column
    // broke in exactly the place a reader compares counts. Neither side is
    // wrong alone; only running them together shows it.
    let d = repo_with_doubt_spread("radius-columns");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);

    // Where each row's *count* starts — not where its name ends, which
    // differs per row by construction and is what the padding exists to
    // absorb. The first version of this test measured the name and failed
    // against correct output.
    let counts_at: Vec<usize> = s
        .lines()
        .filter(|l| {
            let mut it = l.split_whitespace();
            let name = it.next().unwrap_or("");
            let is_reason =
                name == "(other)" || name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            is_reason && it.next().is_some_and(|t| t.parse::<u32>().is_ok())
        })
        .filter_map(|l| {
            let start = l.find(|c: char| !c.is_whitespace())?;
            let rest = &l[start..];
            let name_len = rest.find(char::is_whitespace)?;
            let gap = rest[name_len..].len() - rest[name_len..].trim_start().len();
            Some(start + name_len + gap)
        })
        .collect();

    assert!(counts_at.len() >= 2, "not enough rows to compare:\n{s}");
    assert!(
        counts_at.windows(2).all(|w| w[0] == w[1]),
        "the count column does not line up across rows (counts start at {counts_at:?}):\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// init — what "Ready" is allowed to mean
// ---------------------------------------------------------------------------

/// A repository whose configured checks are commands that certainly exist.
///
/// `git --version` rather than `mypy --version`: `version_of` runs whatever
/// is configured with `--version` and asks only whether it succeeded, and a
/// test that depends on the developer having mypy installed asserts
/// something different on every machine. That mistake is already recorded
/// against an earlier version of the check tests.
fn repo_with_checks(name: &str, typecheck: &str, tests: &str) -> PathBuf {
    let d = empty_repo(name);
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[checks]\ntypecheck = [\"{typecheck}\"]\ntests = [\"{tests}\"]\n"),
    )
    .unwrap();
    d
}

#[test]
fn init_does_not_say_ready_when_a_prerequisite_is_missing() {
    // The strongest claim this command makes. Without a typechecker there is
    // no typecheck verdict, and without a verdict no run can converge — so
    // "Ready" over a missing tool is the command reporting success for work
    // it cannot do, which CLAUDE.md names as worse than saying it is not
    // built yet.
    let d = repo_with_checks(
        "init-missing",
        "trellis-no-such-typechecker",
        "trellis-no-such-runner",
    );
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("NOT FOUND"), "got:\n{s}");
    assert!(
        !s.contains("Ready."),
        "init called a repository ready with both checks missing:\n{s}"
    );
    assert!(
        s.contains("2 prerequisite(s) missing"),
        "and it has to say how many, not merely withhold the all-clear:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_counts_each_missing_prerequisite_rather_than_noticing_one() {
    // A count that saturates at one reads as a single problem and sends the
    // user back for a second round after they fix it.
    let d = repo_with_checks("init-one-missing", "git", "trellis-no-such-runner");
    let s = stdout(&run(&d, &["init"]));
    assert!(
        s.contains("1 prerequisite(s) missing"),
        "one missing tool must count as one:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_says_ready_when_both_checks_resolve() {
    // The other direction. A command that can never say "Ready" is as
    // useless as one that always does, and a guard asserted in one direction
    // can be inverted for free.
    let d = repo_with_checks("init-ready", "git", "git");
    let out = run(&d, &["init"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("Ready."), "got:\n{s}");
    assert!(
        !s.contains("prerequisite(s) missing"),
        "nothing was missing, so nothing should be reported as missing:\n{s}"
    );
    // The *version the tool reported*, not merely that something was found.
    // Without this the line could carry any string — or an empty one — and
    // still read as present, which is the difference between "mypy 1.8.0"
    // and a blank the user cannot act on. Both checks are configured as
    // `git`, so both lines must carry git's own answer.
    assert_eq!(
        s.matches("git version").count(),
        2,
        "both check lines must show the version the command itself printed:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_reports_the_worktree_it_just_dirtied_instead_of_calling_it_clean() {
    // `init` writes into `.trellis/`, so by the time it prints this line the
    // tree it is describing is one it dirtied. Reporting "clean" there would
    // be wrong at the moment it is printed, and wrong in the direction that
    // matters: a run needs a clean tree so rollback is exact (ADR 0009), and
    // the user would find that out from the run refusing instead.
    let d = repo_with_checks("init-dirty", "git", "git");
    let s = stdout(&run(&d, &["init"]));
    assert!(
        s.contains("uncommitted change(s)"),
        "init wrote files and then called the tree clean:\n{s}"
    );
    assert!(
        !s.contains("worktree   clean"),
        "both lines cannot be true at once:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn init_calls_a_committed_worktree_clean() {
    // And the inverse, with everything init wrote committed. Without this
    // the line could be hardwired to "dirty" and still pass above.
    let d = repo_with_checks("init-clean", "git", "git");
    assert!(run(&d, &["init"]).status.success());
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "scaffold"]);

    let s = stdout(&run(&d, &["init"]));
    assert!(s.contains("worktree   clean"), "got:\n{s}");
    assert!(
        !s.contains("uncommitted change(s)"),
        "nothing is uncommitted, so nothing should be reported as such:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// the trail's radius counts — which of them sum, and which deliberately do not
// ---------------------------------------------------------------------------

/// A repository with one symbol reached by resolved edges and one reached
/// only ambiguously, plus doubt in both radii.
///
/// The override lives in its own module on purpose. A method's qualified name
/// does not carry its class, so a base and an override in one file are both
/// `app.core.execute` and `radius` refuses them as ambiguous — there is no
/// way to ask about either. Split across modules they are `app.core.execute`
/// and `app.sub.execute`, the base is nameable, and a call on a `Base`-typed
/// receiver resolves to `Candidates` over both (ADR 0012: a declared type is
/// an upper bound, not an identity).
fn repo_with_resolved_and_ambiguous_reach(name: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        concat!(
            "class Base:\n    def execute(self):\n        return 1\n\n",
            "def greet(name):\n    return f\"hello {name}\"\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/sub.py"),
        concat!(
            "from app.core import Base\n\n",
            "class Sub(Base):\n    def execute(self):\n        return 2\n\n",
            "def murky(thing):\n    return thing.spin()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        concat!(
            "from app.core import Base, greet\n\n",
            "def go(b: Base):\n    return b.execute()\n\n",
            "def also(b: Base):\n    return b.execute()\n\n",
            "def hi(w):\n    return greet(w)\n\n",
            "def hey(w):\n    return greet(w)\n\n",
            "def vague(thing):\n    return thing.run()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("script.json"),
        concat!(
            r#"{"steps":["#,
            r#"{"tool":"blast_radius","symbol":"app.core.execute"},"#,
            r#"{"tool":"blast_radius","symbol":"app.core.greet"},"#,
            r#"{"tool":"done","summary":"looked"}]}"#,
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// The `N` from one of the two doubt headlines `radius` can print.
fn unbound_reported_by_radius(root: &Path, symbol: &str) -> u32 {
    let s = stdout(&run(root, &["radius", symbol]));
    s.lines()
        .find(|l| l.contains("bound to nothing") || l.contains("was bound"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline for {symbol}:\n{s}"))
}

#[test]
fn the_trail_sums_reach_across_queries_and_takes_the_widest_doubt() {
    // Two facts in one run, because they are only interesting together.
    //
    // Reach *is* additive: each query asks about a different symbol, so "how
    // many call sites did this run examine" is the sum. Doubt is not. Two
    // radii in one repository touch overlapping files, so adding their
    // unbound counts counts the same call sites once per query — a figure
    // that grows with how many questions were asked rather than with how
    // much is unknown. The widest single query is a true statement about a
    // real radius; the sum is a statement about nothing.
    //
    // The fixture is built so the two differ: each radius reports the same
    // doubt, so a sum would be double the max and the two rules cannot be
    // confused for one another by accident.
    let d = repo_with_resolved_and_ambiguous_reach("trail-radius-counts");

    let per_query = [
        unbound_reported_by_radius(&d, "app.core.execute"),
        unbound_reported_by_radius(&d, "app.core.greet"),
    ];
    let widest = per_query.iter().copied().max().unwrap();
    let summed: u32 = per_query.iter().sum();
    assert_ne!(
        widest, summed,
        "this fixture is only meaningful when a sum and a max differ; got {per_query:?}"
    );

    let out = run(
        &d,
        &["run", "--model", "scripted:script.json", "look around"],
    );
    // A `done` with no passing check does not converge, and must not: only an
    // executed check accepts a change. The trail is written either way, and
    // is written *after* the rollback so it survives it.
    assert!(
        !out.status.success(),
        "a run that only looked around must not report convergence:\n{}",
        combined(&out)
    );

    // Read back the way the product reads it: `UntrustedTrail`, never
    // `Trail`. A trail lives in a repository that may take contributions, so
    // reading one is reading attacker-influenceable text (S10).
    let (_, trail) = trellis_trail::latest(&d)
        .expect("the trail directory must be readable")
        .expect("a run that reached its first turn writes a trail");
    let r = &trail.factual.radius;

    assert_eq!(r.radius_queries, 2, "two queries were made: {r:?}");
    assert_eq!(
        r.reached_resolved, 2,
        "`greet` has two resolved callers and `execute` none, so the sum is two: {r:?}"
    );
    assert_eq!(
        r.reached_ambiguous, 2,
        "`execute` is reached twice through Candidates and `greet` never: {r:?}"
    );
    assert_eq!(
        r.widest_unbound_in_touched_files, widest,
        "the widest single radius, not a sum — a sum here would be {summed}: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// why / log — reading the trail back
// ---------------------------------------------------------------------------

/// A repository plus a script that edits the file defining `greet`.
///
/// The run does not converge and is rolled back — a `done` with no passing
/// check never converges, because only an executed check accepts a change —
/// and that is fine here: the trail is written after the rollback and
/// survives it, which is the property `why` depends on.
fn repo_with_an_edit_script(name: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return \"hello\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        "from app.core import greet\n\ndef w(x):\n    return greet(x)\n",
    )
    .unwrap();
    std::fs::write(
        d.join("script.json"),
        concat!(
            r#"{"steps":[{"tool":"edit","path":"app/core.py","#,
            r#""old":"return \"hello\"","new":"return \"hi\""},"#,
            r#"{"tool":"done","summary":"changed the greeting"}]}"#,
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// Runs the scripted edit once, leaving one more trail behind.
fn leave_a_trail(root: &Path, task: &str) {
    let out = run(root, &["run", "--model", "scripted:script.json", task]);
    assert!(
        !out.status.success(),
        "a run with no passing check must not converge:\n{}",
        combined(&out)
    );
}

/// The newest trail file on disk.
fn newest_trail(root: &Path) -> PathBuf {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root.join(".trellis/runs"))
        .expect("a run writes .trellis/runs")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v.pop().expect("at least one trail")
}

#[test]
fn why_finds_a_run_by_the_symbol_whose_file_it_changed() {
    // The failure `03-v0.3-memory.md` names, and the one the first wedge run
    // actually hit. The factual section records FILES; a user asking "why
    // does this function look like this" holds a SYMBOL. Matching the symbol
    // against paths finds nothing unless the two happen to share a name, so
    // `why` reported "no file matching `boolean_validator` was changed"
    // about a run that had changed the file defining it — the trail written
    // faithfully and the query unable to answer the question it exists for.
    //
    // The mapping is recomputed from the index rather than recorded, which
    // is also why it works on trails written before it existed.
    let d = repo_with_an_edit_script("why-symbol");
    leave_a_trail(&d, "tweak the greeting");

    let s = stdout(&run(&d, &["why", "greet"]));
    assert!(
        s.contains("changed: app/core.py"),
        "`why` on a symbol must reach the file that defines it:\n{s}"
    );
    assert!(
        !s.contains("no run in the trail mentions"),
        "the run is there and it changed that file:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_separates_a_file_that_changed_from_a_word_in_the_task() {
    // The distinction is the whole value of this command. A run that *edited*
    // the file is evidence about why the code looks like this; a run that
    // merely named it in a task string is context. Reported alike, the
    // weaker one reads as the stronger — and the stronger one loses the fact
    // that a file actually moved.
    let d = repo_with_an_edit_script("why-mentions");
    leave_a_trail(&d, "tweak the greeting");

    let changed = stdout(&run(&d, &["why", "greet"]));
    assert!(
        !changed.contains("named in the task"),
        "a run that changed the file must not be reported as merely naming it:\n{changed}"
    );

    // "greeting" is a word in the task and not a symbol anywhere, so this is
    // the other half of the same branch.
    let mentioned = stdout(&run(&d, &["why", "greeting"]));
    assert!(
        mentioned.contains("named in the task"),
        "a run that only named it must say so rather than staying silent:\n{mentioned}"
    );
    assert!(
        !mentioned.contains("changed:"),
        "nothing matching `greeting` was changed:\n{mentioned}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_says_when_a_trail_no_longer_matches_its_digest() {
    // A trail lives in a repository that takes contributions, so it is read
    // back as untrusted data (S10). Tamper-evidence is only worth having if
    // the surface that reads it says so: a silently altered record is worse
    // than no record, because it is believed.
    let d = repo_with_an_edit_script("why-altered");
    leave_a_trail(&d, "tweak the greeting");

    let clean = stdout(&run(&d, &["why", "greet"]));
    assert!(
        !clean.contains("ALTERED"),
        "an untouched trail must not be reported as altered:\n{clean}"
    );

    // Edit inside the factual section, which is what the digest covers, and
    // deliberately not `proposed` — so the `changed:` line stays identical
    // and the only difference is the integrity verdict.
    let p = newest_trail(&d);
    let text = std::fs::read_to_string(&p).unwrap();
    let tampered = text.replace("\"radius_queries\": 0", "\"radius_queries\": 9");
    assert_ne!(text, tampered, "the tamper must actually change the file");
    std::fs::write(&p, tampered).unwrap();

    let s = stdout(&run(&d, &["why", "greet"]));
    assert!(s.contains("ALTERED"), "an edited trail must say so:\n{s}");
    assert!(
        s.contains("changed: app/core.py"),
        "and must still report what it records, flagged rather than hidden:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_lists_the_runs_it_found() {
    let d = repo_with_an_edit_script("log-lists");
    leave_a_trail(&d, "first pass");

    let s = stdout(&run(&d, &["log"]));
    assert!(s.contains("run(s), newest first"), "got:\n{s}");
    assert!(s.contains("first pass"), "the task must appear:\n{s}");
    assert!(
        !s.contains("no trails"),
        "there is a trail, so the empty message is wrong:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_says_how_many_runs_it_did_not_show() {
    // Same residue rule as the radius report. A listing that silently stops
    // at the limit tells the reader they have seen everything, and the one
    // thing a decision trail must not do is imply there is no more history.
    let d = repo_with_an_edit_script("log-residue");
    leave_a_trail(&d, "first pass");
    leave_a_trail(&d, "second pass");

    let s = stdout(&run(&d, &["log", "--limit", "1"]));
    assert!(
        s.contains("1 older"),
        "two runs shown one at a time must account for the other:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_does_not_offer_more_when_it_showed_everything() {
    // The boundary, and the only input that separates `>` from `>=`.
    let d = repo_with_an_edit_script("log-boundary");
    leave_a_trail(&d, "first pass");
    leave_a_trail(&d, "second pass");

    let s = stdout(&run(&d, &["log", "--limit", "2"]));
    assert!(
        !s.contains("older, use --limit"),
        "everything was shown, so there is nothing older to offer:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// session — the interactive surface, driven the way a user drives it
// ---------------------------------------------------------------------------

/// Runs `trellis session` over a pipe, feeding it `lines`.
///
/// `Command::output()` gives a child a null stdin, so the session sees EOF
/// and leaves before reading anything. Everything printed from inside the
/// loop — the banner, `/help`, `/status`, the coverage block — is reachable
/// only through a real stdin, which is why none of it had a test.
fn session_with(root: &Path, lines: &str) -> Output {
    session_with_env(root, lines, &[])
}

/// [`session_with`], with `vars` added to the child's environment.
fn session_with_env(root: &Path, lines: &str, vars: &[(&str, &Path)]) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .envs(vars.iter().copied())
        .arg("session")
        .current_dir(root)
        .env("NO_COLOR", "1")
        // Hermetic: the user-level config and context files (ADR 0025 §7)
        // of whoever runs the suite must not change what it sees.
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary must be built before its integration tests run");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(lines.as_bytes())
        .expect("the session must accept input");
    child.wait_with_output().expect("the session must finish")
}

#[test]
fn session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists() {
    // The risks: `!cmd` printing "exit 0" after a command that worked, or
    // nothing after one that failed; `/prompts` or `/models` printing
    // nothing; a template's description left out of `/prompts`.
    let d = repo("session-extra");
    std::fs::create_dir_all(d.join(".trellis/prompts")).unwrap();
    std::fs::write(
        d.join(".trellis/prompts/review.md"),
        "---\ndescription: Review one module\n---\nReview $1\n",
    )
    .unwrap();
    let out = session_with(
        &d,
        "!git --version\n!git no-such-command-248\n/prompts\n/models\n/exit\n",
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("exit 1"), "{s}");
    assert!(!s.contains("exit 0"), "{s}");
    assert!(s.contains("  /review\n      Review one module\n"), "{s}");
    assert!(s.contains("configured specs\n"), "{s}");
    assert!(
        s.contains("catalog (weights `trellis model pull` can fetch)"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn every_command_help_lists_is_routed_somewhere() {
    // Each of these is one match arm, and deleting any of them leaves the
    // command falling through to "unknown command" — a command the tool
    // documents in its own `/help` and then refuses. Five of the arms had no
    // test at all.
    //
    // Asserted on the *error text* rather than on success, because several
    // of these legitimately fail in a fixture with no checkers and no trail.
    // Failing for the right reason is the property; succeeding is not.
    let d = repo("session-routing");
    for cmd in ["/help", "/?", "/index", "/check", "/doctor", "/resume"] {
        let out = session_with(&d, &format!("{cmd}\n/exit\n"));
        let s = combined(&out);
        assert!(
            !s.contains("unknown command"),
            "`{cmd}` is in /help and was not routed:\n{s}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn help_lists_the_commands_rather_than_printing_nothing() {
    // `help` is a bare `fn` whose whole body is printing, so replacing it
    // with `()` leaves `/help` returning Ok and saying nothing. A help
    // command that silently succeeds is worse than a missing one: the user
    // concludes there are no commands.
    let d = repo("session-help");
    let s = stdout(&session_with(&d, "/help\n/exit\n"));

    for expected in ["/radius", "/check", "/doctor", "/accept", "/model", "/exit"] {
        assert!(
            s.contains(expected),
            "`{expected}` missing from /help:\n{s}"
        );
    }
    assert!(
        s.contains("trellis tui"),
        "the full-screen view is a command the user cannot discover any \
         other way, so /help has to name it:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_banner_reports_the_index_and_carries_its_caveat() {
    // The coverage figure is the product's honesty mechanism and does not
    // get to be absent because the surface changed (S2). The banner is also
    // a bare printing function, so `()` leaves a session that opens to a
    // blank prompt and says nothing about what it indexed.
    let d = repo("session-banner");
    let s = stdout(&session_with(&d, "/exit\n"));

    assert!(s.contains("TRELLIS"), "got:\n{s}");
    assert!(s.contains("definitions"), "it must say what it found:\n{s}");
    assert!(
        s.contains("a lower bound on recall, not recall"),
        "the figure must carry its caveat here as it does everywhere else, \
         or it reads as recall to everyone who sees it once:\n{s}"
    );
    assert!(
        s.contains("/help"),
        "a prompt with no way in is a dead end:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn status_reports_the_state_the_session_is_holding() {
    // The point of a session is the state it keeps between tasks — the
    // index, the model, the tier. `/status` is how a user sees it, and as
    // `()` it prints nothing while still returning Ok.
    let d = repo("session-status");
    let s = stdout(&session_with(&d, "/status\n/exit\n"));

    assert!(s.contains("root"), "got:\n{s}");
    assert!(s.contains("index"), "got:\n{s}");
    assert!(
        s.contains("model    none"),
        "no model is set in this fixture and status must say so rather than \
         omit the line:\n{s}"
    );
    assert!(
        s.contains("tier"),
        "the tier decides whether a run can fail on format alone:\n{s}"
    );
    // The banner and `/status` both count the population `trellis index`
    // calls "non-excluded call sites" (A-30); "first-party" there named a
    // smaller set than the number printed beside it.
    assert_eq!(
        s.matches("non-excluded call sites").count(),
        2,
        "banner and status must name the count as the index does:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unknown_command_is_refused_rather_than_run_as_a_task() {
    // The other side of the routing table. Anything beginning with `/` that
    // is not a command must be refused, not handed to the agent loop as a
    // task — `/radiuss describe` typed at a prompt would otherwise start a
    // run against a repository.
    let d = repo("session-unknown");
    let s = combined(&session_with(&d, "/nosuchcommand\n/exit\n"));
    assert!(s.contains("unknown command"), "got:\n{s}");
    assert!(
        s.contains("/help"),
        "and it must say where the list is:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn accept_reads_its_answer_from_the_next_line_of_input() {
    // #248. The risk: the approval question reading end of input, or a
    // key it never got, so `a` typed after `/accept` cancels instead.
    let d = repo("session-accept-stdin");
    let caller = d.join("app/caller.py");
    let mut body = std::fs::read_to_string(&caller).unwrap();
    body.push_str("# edited\n");
    std::fs::write(&caller, body).unwrap();

    let s = stdout(&session_with(&d, "/accept kept\na\n/exit\n"));
    assert!(s.contains("accept> "), "{s}");
    assert!(s.contains("committed 1 path(s)"), "{s}");
    assert_eq!(git(&d, &["log", "-1", "--pretty=%s"]).trim(), "kept");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_session_keeps_its_history_outside_the_repository() {
    // #248. The risk: no history file at all, which the unit tests allow
    // for a machine with no HOME or LOCALAPPDATA.
    let d = repo("session-history");
    let data = scratch("session-history-data");
    let out = session_with_env(
        &d,
        "/status\n/exit\n",
        &[("LOCALAPPDATA", &data), ("XDG_DATA_HOME", &data)],
    );
    assert!(out.status.success(), "{}", combined(&out));
    let saved: Vec<_> = std::fs::read_dir(data.join("trellis"))
        .expect("the history directory was made")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(
        saved[0].to_string_lossy().ends_with(".history"),
        "{saved:?}"
    );
    assert!(
        std::fs::read_to_string(&saved[0])
            .unwrap()
            .contains("/status"),
        "the typed line was kept"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_runtime_that_is_not_listening_is_said_at_open_and_at_model() {
    // #248. The risk: the banner, and `/model`, silent about a runtime
    // nothing is listening on, so the first task is where the user finds
    // out. Port 1 refuses at once.
    let d = repo("session-unreachable");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[model]\nspec = \"llamacpp:http://127.0.0.1:1\"\n",
    )
    .unwrap();
    let s = stdout(&session_with(
        &d,
        "/model llamacpp:http://127.0.0.1:1\n/exit\n",
    ));
    assert!(s.contains("NOT REACHABLE"), "the banner:\n{s}");
    assert!(s.contains("not reachable yet"), "`/model`:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// How many diff lines `/diff` shows before collapsing the rest.
///
/// Mirrored rather than imported — `tests/cli.rs` is a separate crate and
/// `DIFF_LINES` is private — and asserted against the shipped behaviour by
/// the boundary test, which checks both `DIFF_LINES` and `DIFF_LINES + 1`. If
/// it moves either way, that test fails rather than silently testing nothing.
const DIFF_LINES: usize = 60;

#[test]
fn index_rebuilds_rather_than_reporting_a_cached_answer() {
    // `/index` is the command whose whole job is to refresh, which is why it
    // calls `build` and not `build_cached`. As `Ok(())` it returns success
    // having done nothing, and the session then holds a stale graph while
    // reporting that it reindexed.
    let d = repo("session-reindex");
    let s = stdout(&session_with(&d, "/index\n/exit\n"));
    assert!(
        s.contains("indexed") && s.contains("files"),
        "`/index` must say what it rebuilt:\n{s}"
    );
    assert!(
        s.matches("call site").count() >= 2,
        "the coverage summary travels with it, as it does in the banner:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_shows_the_change_and_not_only_its_filename() {
    // "Acceptance is yours" is not a real offer if taking it up means
    // leaving for another terminal. The filename says a file moved; only the
    // body says whether to accept it.
    let d = repo("session-diff");
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"HELLO {name}\"\n",
    )
    .unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(s.contains("app/core.py"), "the file must be named:\n{s}");
    assert!(
        s.contains("+    return f\"HELLO {name}\""),
        "and the change itself shown:\n{s}"
    );
    assert!(
        s.contains("/accept") && s.contains("/revert"),
        "both ways out must be offered where the change is shown:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_says_how_much_of_a_long_change_it_did_not_show() {
    // The same residue rule as the radius report and the log listing. A diff
    // that stops at sixty lines without saying so shows a user part of a
    // change and lets them accept it believing they saw all of it.
    let d = repo("session-diff-long");
    let mut body = String::from("def greet(name):\n");
    for i in 0..(DIFF_LINES * 2) {
        body.push_str(&format!("    x{i} = {i}\n"));
    }
    body.push_str("    return name\n");
    std::fs::write(d.join("app/core.py"), body).unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("more lines"),
        "a change far longer than the limit must account for the rest:\n{s}"
    );
    assert!(s.contains("git diff"), "and name how to see it:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_does_not_offer_more_when_it_showed_the_whole_change() {
    // The boundary. A two-line edit followed by "... 0 more lines" is a
    // listing claiming a remainder that does not exist.
    let d = repo("session-diff-short");
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hi {name}\"\n",
    )
    .unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        !s.contains("more lines"),
        "the whole change fitted, so there is nothing more to offer:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_on_a_clean_worktree_says_so_rather_than_printing_nothing() {
    // The other branch. Silence here is indistinguishable from a command
    // that failed, and the user's next move — accept, or run again — differs
    // between those two.
    let d = repo("session-diff-clean");
    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("nothing pending") && s.contains("clean"),
        "got:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_opens_no_diff_section_for_a_file_git_has_never_seen() {
    // An untracked file is pending and has no diff: git cannot show what it
    // has never seen, so `diff` comes back empty while `pending` names the
    // file. The guard that notices this survived as `true`, and its only
    // effect is a blank line — but the rule is real, and the case is
    // reachable by the most ordinary thing a run does, which is write a new
    // file.
    //
    // Asserted as "no section was opened" rather than on exact whitespace.
    // The window starts mid-line, so the remainder of the listing line
    // counts as one: two is the clean shape, three means an empty diff block
    // was started.
    let d = repo("session-untracked");
    std::fs::write(d.join("app/fresh.py"), "def brand_new():\n    return 1\n").unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(s.contains("app/fresh.py"), "the file must be listed:\n{s}");

    let window = s
        .split_once("app/fresh.py")
        .and_then(|(_, rest)| rest.split_once("/accept"))
        .map(|(mid, _)| mid)
        .unwrap_or_else(|| panic!("the listing and the offer must both appear:\n{s}"));
    let blanks = window.lines().filter(|l| l.trim().is_empty()).count();
    assert_eq!(
        blanks, 2,
        "an empty diff section was opened for an untracked file; between the \
         listing and the offer there should be nothing but the line break \
         and one blank line, found {blanks}:\n{window:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_offers_no_remainder_at_exactly_the_number_of_lines_it_shows() {
    // The boundary, and the only input that separates `>` from `>=` on the
    // diff listing. `>=` at exactly DIFF_LINES prints "... 0 more lines —
    // `git diff` for the rest" under a diff that already showed everything.
    //
    // The fixture converges on the length rather than hardcoding a body that
    // happens to produce it: a diff's line count includes git's own headers
    // and hunk markers, so a hand-counted fixture would be pinned to one
    // git's formatting and would quietly stop testing the boundary under
    // another. Each added source line adds exactly one line to the same
    // hunk, so one correction is enough.
    let d = repo("session-diff-boundary");
    let path = d.join("app/core.py");

    let write_body = |n: usize| {
        let mut b = String::from("def greet(name):\n");
        for i in 0..n {
            b.push_str(&format!("    x{i} = {i}\n"));
        }
        b.push_str("    return name\n");
        std::fs::write(&path, b).unwrap();
    };
    let diff_lines = || trellis_vcs::diff(&d).unwrap().lines().count();

    let probe = 40;
    write_body(probe);
    let measured = diff_lines();
    let target = probe as isize + (DIFF_LINES as isize - measured as isize);
    assert!(target >= 0, "the boundary is below an empty change");
    write_body(target as usize);

    assert_eq!(
        diff_lines(),
        DIFF_LINES,
        "this test is only meaningful at exactly the limit"
    );

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        !s.contains("more lines"),
        "the diff was exactly the length shown, so there is no remainder:\n{s}"
    );

    // One line past it must offer a remainder, or a raised limit would leave
    // the assertion above passing at a length that is no longer the boundary.
    write_body(target as usize + 1);
    assert_eq!(diff_lines(), DIFF_LINES + 1);
    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("more lines"),
        "one line past the limit leaves one unshown:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_index_percentages_partition_the_call_sites_they_describe() {
    // resolved, ambiguous and unresolved are the three outcomes a call site
    // can have — ADR 0002 keeps them three precisely so none is collapsed
    // into another — so their percentages are a partition and must come to
    // a hundred. All four mutants of the arithmetic that produces them
    // survived, and each breaks that in its own way: one makes every figure
    // a multiple of the total, one adds a hundred to each, one divides them
    // to nothing.
    //
    // Asserted as the invariant rather than against fixed numbers, so the
    // test says what the figures mean rather than what this fixture happens
    // to produce.
    let d = repo("index-percentages");
    let out = run(&d, &["index"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let pct_on = |label: &str| -> f64 {
        let line = s
            .lines()
            .find(|l| l.trim_start().starts_with(label) && l.contains('%'))
            .unwrap_or_else(|| panic!("no `{label}` line with a percentage:\n{s}"));
        let open = line.find('(').unwrap_or_else(|| panic!("got {line:?}"));
        let close = line[open..]
            .find('%')
            .map(|i| open + i)
            .unwrap_or_else(|| panic!("got {line:?}"));
        line[open + 1..close]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("`{}` is not a percentage: {e}", &line[open + 1..close]))
    };

    let parts = [
        pct_on("resolved"),
        pct_on("ambiguous"),
        pct_on("unresolved"),
    ];
    let sum: f64 = parts.iter().sum();
    assert!(
        (sum - 100.0).abs() < 0.3,
        "the three outcomes must account for every first-party call site \
         and nothing else; {parts:?} came to {sum}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// run — what the run tells the user about the rules it is under
// ---------------------------------------------------------------------------

/// Writes one governance rule into a repository.
fn add_rule(root: &Path, file: &str, body: &str) {
    let dir = root.join(".trellis/governance");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), body).unwrap();
}

#[test]
fn a_run_under_governance_says_how_many_rules_it_is_under() {
    // A rule the user cannot see is a rule they will be surprised by. The
    // line is printed before the loop starts, which is the only moment it is
    // useful — after a refusal it explains a wall the contributor has
    // already hit.
    //
    // Both mutants of the condition suppress it: without the `!` the line
    // appears only when there are *no* rules, and with `&&` it needs a
    // context file as well as rules before it will say anything.
    let d = repo("run-governance");
    add_rule(
        &d,
        "no-vendor.toml",
        concat!(
            "name = \"Vendored code is not ours to edit\"\n",
            "rank = \"non-negotiable\"\n",
            "description = \"Changes go upstream.\"\n",
            "deny-write = { paths = [\"vendor/\"] }\n",
        ),
    );
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "rules"]);

    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    assert!(
        s.contains("write rule(s)"),
        "a run under a rule must say so before it starts:\n{s}"
    );
    assert!(
        s.contains("1 write rule(s)"),
        "and say how many, because one and nine are different situations:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_run_under_no_rules_does_not_announce_an_empty_rule_set() {
    // The other direction. "0 write rule(s)" on every ordinary run is a line
    // that teaches the reader to skip the place where the real one appears.
    let d = repo("run-no-governance");
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "script"]);

    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    assert!(
        !s.contains("write rule(s)"),
        "there are no rules, so there is nothing to announce:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_run_with_a_context_file_says_it_was_loaded() {
    // `.trellis/context.md` reaches the model's prompt, so it changes what a
    // run does. A run whose behaviour depends on a file the user forgot
    // they wrote is a run they cannot reproduce.
    let d = repo("run-context");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/context.md"),
        "# House style\n\nPrefer explicit returns.\n",
    )
    .unwrap();
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "context"]);

    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    assert!(
        s.contains("context.md loaded"),
        "a file that reaches the prompt must be named where the run starts:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A repository with a two-note vault under `notes/`, committed, plus the
/// stopping script. `config` is the `[context]` section's body.
fn repo_with_vault(name: &str, config: &str) -> (PathBuf, String) {
    let d = repo(name);
    let v = d.join("notes");
    std::fs::create_dir_all(&v).unwrap();
    std::fs::write(
        v.join("root.md"),
        "---\ntitle: \"Root\"\nparent: null\n---\nROOT-BODY\n",
    )
    .unwrap();
    std::fs::write(
        v.join("child.md"),
        "---\ntitle: \"Child\"\nparent: \"[[root]]\"\n---\nCHILD-BODY\n",
    )
    .unwrap();
    std::fs::write(v.join("manifest.json"), r#"["child.md","root.md"]"#).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[context]\n{config}"),
    )
    .unwrap();
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "vault"]);
    (d, script)
}

fn porcelain(d: &Path) -> String {
    git(d, &["status", "--porcelain"])
}

#[test]
fn a_run_with_a_vault_focus_names_the_note_and_its_size() {
    // The compiled note reaches the prompt, so like context.md it must be
    // named where the run starts — with the focus, because the same vault
    // gives a different prompt per note, and with the size, because that is
    // what every turn pays for it.
    //
    // `--focus` overrides the configured focus: switching note by editing
    // config.toml would dirty the worktree and the run would refuse to start.
    let (d, script) = repo_with_vault("run-vault", "vault = \"notes\"\nfocus = \"root\"\n");
    let out = run(
        &d,
        &["run", "--model", &script, "--focus", "child", "a task"],
    );
    let s = stdout(&out);
    assert!(
        s.contains("vault note `child` from notes loaded ("),
        "the flag's focus, not the config's:\n{}",
        combined(&out)
    );
    assert!(s.contains(" B)"), "the byte count is stated:\n{s}");
    assert!(
        !porcelain(&d).contains("notes"),
        "compiled in memory; nothing is written into the vault or beside it"
    );

    // Without the flag the configured focus applies.
    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    assert!(s.contains("vault note `root` from notes loaded ("), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_vault_with_no_focus_says_nothing_was_sent_rather_than_skipping_quietly() {
    let (d, script) = repo_with_vault("run-vault-nofocus", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    assert!(
        s.contains("notes configured but no focus set; no vault note sent"),
        "{s}"
    );
    assert!(!s.contains("vault note `"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_invalid_vault_refuses_the_run_before_anything_is_touched() {
    // An invalid vault is a run that cannot start as configured. It must cost
    // what an unreachable runtime costs: nothing snapshotted, no trail.
    let (d, script) = repo_with_vault("run-vault-invalid", "vault = \"notes\"\n");
    std::fs::write(
        d.join("notes/child.md"),
        "---\ntitle: \"Child\"\ntags: x\nparent: \"[[root]]\"\n---\nCHILD-BODY\n",
    )
    .unwrap();
    git(&d, &["commit", "-qam", "break the vault"]);

    let out = run(
        &d,
        &["run", "--model", &script, "--focus", "child", "a task"],
    );
    let all = combined(&out);
    assert!(!out.status.success(), "{all}");
    assert!(
        all.contains("unexpected frontmatter key \"tags\""),
        "the violation itself, not only that there was one:\n{all}"
    );
    assert!(all.contains("the run did not start"), "{all}");
    assert!(!stdout(&out).contains("snapshot at"), "{all}");
    assert!(
        !d.join(".trellis/runs").exists(),
        "no trail for a run that never began"
    );
    assert!(porcelain(&d).is_empty(), "{}", porcelain(&d));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_focus_that_names_no_note_refuses_the_run() {
    let (d, script) = repo_with_vault("run-vault-unknown", "vault = \"notes\"\n");
    let out = run(
        &d,
        &["run", "--model", &script, "--focus", "nope", "a task"],
    );
    assert!(!out.status.success());
    assert!(
        combined(&out).contains("unknown stem: nope"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failing_check_prints_what_the_checker_wrote_on_both_streams() {
    // Two guards, one per stream, and both survived. A `check` that reports
    // FAILED and nothing else gives the user the one fact they already had
    // — that something is wrong — and withholds the only one that helps.
    //
    // Both streams matter because checkers differ in which they use: mypy
    // writes its diagnostics to stdout, and a tool that cannot start writes
    // to stderr. A guard tested on one stream leaves the other free to go
    // silent.
    //
    // `git diff --exit-code` is the portable failing-with-stdout command:
    // it prints the diff and exits 1. `git nosuchsubcommand` is the
    // failing-with-stderr one. Both are on any machine that can run these
    // tests at all, which is the property C-006 was recorded for.
    let d = repo("check-streams");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        concat!(
            "[checks]\n",
            "typecheck = [\"git\", \"diff\", \"--exit-code\"]\n",
            "tests = [\"git\", \"nosuchsubcommand\"]\n",
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "config"]);
    // An uncommitted change, so `git diff --exit-code` has something to say
    // and a reason to fail.
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"CHANGED {name}\"\n",
    )
    .unwrap();

    let s = combined(&run(&d, &["check"]));

    assert!(s.contains("FAILED"), "both checks fail here:\n{s}");
    assert!(
        s.contains("+    return f\"CHANGED {name}\""),
        "the checker's stdout must reach the user; this is what mypy uses \
         for its diagnostics:\n{s}"
    );
    assert!(
        s.contains("is not a git command"),
        "and its stderr, which is what a tool that cannot start uses:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A repository whose checks always pass, so a scripted run can converge.
///
/// `git --version` rather than mypy: the run has to reach its end for the
/// post-run reporting to exist at all, and a fixture depending on the
/// developer's toolchain would decide that per machine.
fn repo_that_converges(name: &str, core: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(d.join("app/core.py"), core).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
    )
    .unwrap();
    std::fs::write(
        d.join("script.json"),
        concat!(
            r#"{"steps":[{"tool":"edit","path":"app/core.py","#,
            r#""old":"return \"hello\"","new":"return \"hi\""},"#,
            r#"{"tool":"run_checks","paths":["app/core.py"],"kinds":["typecheck"]},"#,
            r#"{"tool":"done","summary":"changed"}]}"#,
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

#[test]
fn a_converged_run_reports_the_doubt_in_what_it_changed() {
    // The moment the figure matters most. A run that has just printed
    // CONVERGED is exactly when "converged" is most likely to be read as
    // "tested", so the coverage in the files it touched is printed right
    // there rather than left for someone to go and ask for.
    //
    // The doubt is scoped to the files the run wrote, not the repository:
    // S6's severity rule, because doubt on code nobody changed is noise
    // that teaches the reader to skip the line.
    let d = repo_that_converges(
        "run-converged-doubt",
        "def greet(name):\n    return \"hello\"\n\ndef murky(thing):\n    return thing.run()\n",
    );
    let out = run(&d, &["run", "--model", "scripted:script.json", "tweak"]);
    let s = combined(&out);

    assert!(s.contains("CONVERGED"), "the fixture must converge:\n{s}");
    assert!(
        s.contains("files written: app/core.py"),
        "and must have written something:\n{s}"
    );
    assert!(
        s.contains("coverage:"),
        "a converged run that changed a file with unbound calls must say so \
         where the convergence is announced:\n{s}"
    );
    assert!(
        s.contains("may call what changed"),
        "and say what the number means, or it is a figure with no consequence:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_converged_run_over_code_with_no_doubt_reports_none() {
    // The boundary. "coverage: 0 call sites bound to nothing" after every
    // clean run is a line that trains the reader to skip the place where a
    // real one appears — the same failure as announcing an empty rule set.
    let d = repo_that_converges(
        "run-converged-clean",
        "def greet(name):\n    return \"hello\"\n",
    );
    let out = run(&d, &["run", "--model", "scripted:script.json", "tweak"]);
    let s = combined(&out);

    assert!(s.contains("CONVERGED"), "the fixture must converge:\n{s}");
    assert!(
        !s.contains("coverage:"),
        "nothing was unbound, so there is no coverage to report:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// model — the weights catalogue, which needs no network to be wrong
// ---------------------------------------------------------------------------

/// Runs a command with the user's data directory pointed at `data`.
///
/// `models_dir` reads `LOCALAPPDATA` on Windows and `XDG_DATA_HOME` (then
/// `HOME`) elsewhere. Both are set, so the test says the same thing on
/// either platform and never touches the real one — writing a fake weights
/// file into someone's actual model directory is not a thing a test does.
fn run_with_data_dir(root: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        // Hermetic: the user-level config and context files (ADR 0025 §7)
        // of whoever runs the suite must not change what it sees.
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("LOCALAPPDATA", data)
        .env("XDG_DATA_HOME", data)
        .env("HOME", data)
        .output()
        .expect("the binary must be built before its integration tests run")
}

/// Where `models_dir` will look, given that data directory.
fn models_dir_in(data: &Path) -> PathBuf {
    data.join("trellis").join("models")
}

#[test]
fn the_model_list_reports_each_weight_as_absent_partial_or_complete() {
    // Three states and a different action behind each: fetch it, resume it,
    // use it. Collapsing any two — which every mutant of the guard does —
    // makes the list say the wrong thing about work the user has already
    // paid for. On a slow link the difference between resuming and
    // restarting is hours, which is why a partial file is named rather than
    // hidden.
    //
    // No network is involved in any of this: the state is `fs::metadata`
    // against a size the catalogue already knows.
    let d = repo("model-list");
    let data = scratch("model-list-data");
    let dir = models_dir_in(&data);
    std::fs::create_dir_all(&dir).unwrap();

    // Nothing fetched yet.
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("not fetched"),
        "an empty directory means nothing is fetched:\n{s}"
    );
    assert!(
        !s.contains("downloaded") && !s.contains("partial"),
        "and nothing is downloaded or partial:\n{s}"
    );

    // The smallest entry, part-way down.
    let entry = trellis_model::CATALOG
        .iter()
        .min_by_key(|e| e.bytes)
        .expect("the catalogue is not empty");
    let path = dir.join(entry.file);
    std::fs::write(&path, vec![0u8; 1024]).unwrap();
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("partial"),
        "a file shorter than the published size is a resumable download, \
         not a usable weight:\n{s}"
    );

    // The same entry, complete. Written sparse: the file is half a gigabyte
    // and none of it is read, so `set_len` costs nothing and the point is
    // the length `metadata` reports.
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_len(entry.bytes).unwrap();
    drop(f);
    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert!(
        s.contains("downloaded"),
        "a file at or past the published size is usable:\n{s}"
    );
    assert!(
        !s.contains("partial"),
        "and must not still be offered as resumable:\n{s}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn exactly_one_catalogue_entry_is_marked_the_default() {
    // The marker is how a user picks without reading four descriptions, so
    // it is worth nothing if it is on all of them or none. Inverting the
    // comparison moves it onto every entry but the right one.
    let d = repo("model-default");
    let data = scratch("model-default-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    assert_eq!(
        s.matches("(default)").count(),
        1,
        "exactly one entry carries the marker:\n{s}"
    );
    assert!(
        s.contains(&format!("{} (default)", trellis_model::DEFAULT)),
        "and it is the one the catalogue names as the default:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_model_listing_says_where_colibri_weights_come_from() {
    // ADR 0047 decision 11: the catalogue holds one pinned GGUF file per
    // entry, so colibri's multi-shard containers are not in it. Someone
    // looking for them in `trellis model` would otherwise find nothing and
    // no hint, and might fetch shards by hand without the per-shard SHA-256
    // checks colibri's own tooling makes. One line, so the listing stays a
    // listing.
    let d = repo("model-runtime-note");
    let data = scratch("model-runtime-note-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let s = stdout(&run_with_data_dir(&d, &data, &["model"]));
    let lines: Vec<&str> = s.lines().filter(|l| l.contains("colibri")).collect();
    assert_eq!(lines.len(), 1, "exactly one line names colibri:\n{s}");
    let line = lines[0];
    for needle in [
        "not in this catalogue",
        "coli convert",
        "coli mirror verify",
        "docs/runtimes/colibri.md",
    ] {
        assert!(
            line.contains(needle),
            "the colibri line must say `{needle}`:\n{line}"
        );
    }
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_pull_colibri_2_0_0_families_get_colibris_tooling_too() {
    // #612: the list stopped at colibri 1.12.1's nine families, so
    // `model pull "MiMo-V2.6 Flash"`, a family 2.0.0 serves, got the generic
    // catalogue error, which reads as a typo of a GGUF entry. Matched as the
    // registry spells it, in any case.
    let d = repo("model-pull-colibri-2");
    let data = scratch("model-pull-colibri-2-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();
    for name in ["MiMo-V2.6 Flash", "qwen3-coder-30b-a3b"] {
        let out = run_with_data_dir(&d, &data, &["model", "pull", name]);
        assert!(
            !out.status.success(),
            "{name}: a colibri family must not pull"
        );
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            err.contains("is a colibri family") && err.contains("coli convert"),
            "{name}: {err}"
        );
        assert!(
            !err.contains("Known:"),
            "{name}: not the generic error:\n{err}"
        );
    }
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_a_colibri_family_names_colibris_tooling_and_other_names_keep_the_generic_error() {
    // ADR 0047 decision 11: `model pull` does not fetch colibri containers.
    // Someone who types a colibri family there should learn where its
    // weights come from, not read a catalogue list that never had it; and a
    // name that is neither must still get the error that lists the
    // catalogue, so a typo of a catalogue entry is not sent to colibri.
    let d = repo("model-pull-colibri");
    let data = scratch("model-pull-colibri-data");
    std::fs::create_dir_all(models_dir_in(&data)).unwrap();

    let out = run_with_data_dir(&d, &data, &["model", "pull", "Kimi K3"]);
    assert!(!out.status.success(), "a colibri family must not pull");
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let lines: Vec<&str> = err.lines().filter(|l| l.contains("colibri")).collect();
    assert_eq!(lines.len(), 1, "exactly one line names colibri:\n{err}");
    for needle in [
        "coli convert",
        "coli mirror verify",
        "docs/runtimes/colibri.md",
    ] {
        assert!(
            lines[0].contains(needle),
            "the refusal must say `{needle}`:\n{err}"
        );
    }
    assert!(!err.contains("Known:"), "not the generic error:\n{err}");

    let out = run_with_data_dir(&d, &data, &["model", "pull", "no-such-model"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("no model named `no-such-model`") && err.contains("Known:"),
        "an unrelated name keeps the generic error:\n{err}"
    );
    assert!(
        !err.contains("colibri"),
        "and is not sent to colibri:\n{err}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn model_path_names_a_real_location_under_the_data_directory() {
    // `trellis model path` exists so a user can put the file there
    // themselves — over a link the downloader cannot use, or from a copy on
    // another machine. A path that is empty, or that is not where the tool
    // will actually look, sends them to do the work twice.
    //
    // Both `models_dir` and `weights_path` can be replaced with a default
    // `PathBuf`, which is the empty one, and the command still prints a line
    // and exits zero.
    let d = repo("model-path");
    let data = scratch("model-path-data");

    let s = stdout(&run_with_data_dir(&d, &data, &["model", "path"]));
    let printed = s.trim();
    assert!(
        !printed.is_empty(),
        "an empty path is not an answer:\n{s:?}"
    );

    let expected = models_dir_in(&data).join(
        trellis_model::catalog::find(trellis_model::DEFAULT)
            .expect("the default is in the catalogue")
            .file,
    );
    assert_eq!(
        Path::new(printed),
        expected,
        "the path printed must be the one `model` itself looks in, or a \
         user who puts the file there finds it was not enough"
    );
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_stops_before_the_network_when_the_weights_are_already_here() {
    // The branch of `pull` worth the most: it is what stops a re-run of
    // `trellis model pull` from re-downloading a file that is already
    // complete. Inverting the comparison turns the guard into its opposite
    // and every repeat invocation becomes a fresh download.
    //
    // Safe precisely because it returns *before* curl.
    let d = repo("model-pull-present");
    let data = scratch("model-pull-data");
    let dir = models_dir_in(&data);
    std::fs::create_dir_all(&dir).unwrap();

    let entry = trellis_model::catalog::find(trellis_model::DEFAULT).unwrap();
    let f = std::fs::File::create(dir.join(entry.file)).unwrap();
    f.set_len(entry.bytes).unwrap();
    drop(f);

    let out = run_with_data_dir(&d, &data, &["model", "pull"]);
    let s = combined(&out);
    assert!(out.status.success(), "{s}");
    assert!(
        s.contains("is already here"),
        "a complete file must end the command, not start a download:\n{s}"
    );
    assert!(
        !s.contains("from https://"),
        "and it must not even name the URL, because nothing is being \
         fetched:\n{s}"
    );

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn pull_says_it_is_resuming_only_when_a_partial_download_is_staged() {
    // #248: on a slow link "resuming at" versus a silent restart is the
    // difference between hours, and a line that claims a resume with
    // nothing staged is as wrong as one missing. Neither case reaches
    // curl: a file one byte longer than the pin is refused as too long
    // before any fetch, whether it is staged or at the final name (which
    // `pull` moves to the staging name first, after the line is printed).
    let entry = trellis_model::catalog::find(trellis_model::DEFAULT).unwrap();
    let d = repo("model-pull-resume");
    for (staged, data) in [
        (true, scratch("model-pull-resume-staged")),
        (false, scratch("model-pull-resume-placed")),
    ] {
        let dir = models_dir_in(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if staged {
            trellis_model::pull::partial_name(entry.file)
        } else {
            entry.file.to_string()
        };
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_len(entry.bytes + 1).unwrap();
        drop(f);

        let out = run_with_data_dir(&d, &data, &["model", "pull"]);
        let s = combined(&out);
        assert!(!out.status.success(), "{s}");
        assert!(s.contains("from https://"), "{s}");
        assert!(s.contains("the pinned file is"), "{s}");
        assert_eq!(
            stdout(&out).contains("resuming at"),
            staged,
            "staged: {staged}\n{s}"
        );
        let _ = std::fs::remove_dir_all(&data);
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// control dial (ADR 0018)
// ---------------------------------------------------------------------------

/// A repository whose config puts runs in approve mode, committed.
fn repo_in_approve_mode(name: &str) -> PathBuf {
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
    let spec = format!("scripted:{}", {
        let s = scratch("approve-no-tty-script").join("s.json");
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
    let s = scratch("approve-session-script").join("s.json");
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

// ---------------------------------------------------------------------------
// skills (ADR 0019)
// ---------------------------------------------------------------------------

fn add_skill(root: &Path, dir: &str, name: &str, description: &str) {
    let d = root.join(".trellis/skills").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .unwrap();
}

#[test]
fn a_run_says_how_many_skills_it_listed_and_how_many_it_skipped() {
    let d = repo("skills-run");
    add_skill(&d, "profiling", "profiling", "How to profile a slow test.");
    add_skill(
        &d,
        "misnamed",
        "other-name",
        "Skipped: the name is not the directory.",
    );
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "skills"]);

    let out = run(
        &d,
        &[
            "run",
            "--model",
            &script_that_stops(&scratch("skills-run-script")),
            "t",
        ],
    );
    let said = stdout(&out);
    let repo_line = said
        .lines()
        .find(|l| l.starts_with("repo:"))
        .unwrap_or_else(|| panic!("no repo: line\n{}", combined(&out)));
    assert!(
        repo_line.contains("skills: 1 listed, 0 omitted (cap), 1 invalid and skipped"),
        "{repo_line}"
    );
}

#[test]
fn doctor_names_each_skipped_skill_and_why() {
    // A skill that is silently not loaded is one its author believes the
    // model can read.
    let d = repo("skills-doctor");
    add_skill(&d, "profiling", "profiling", "How to profile a slow test.");
    add_skill(&d, "misnamed", "other-name", "x");
    let out = run(&d, &["doctor"]);
    let said = stdout(&out);
    assert!(
        said.contains("skills:     2 found · 1 listed · 0 omitted (cap 4096 B) · 1 invalid"),
        "{said}"
    );
    assert!(
        said.contains(
            "skipped misnamed: frontmatter name `other-name` does not match the directory name `misnamed`"
        ),
        "{said}"
    );
}

// ---------------------------------------------------------------------------
// ignored files in a model run (ADR 0023 §5, ADR 0026 §11)
// ---------------------------------------------------------------------------

/// A repository whose `.gitignore` ignores `.venv/`, holding one ignored
/// file already, with `bash` enabled and trusted in its own trust store.
/// Returns the repository and a function running the binary against it.
fn ignored_model_fixture(name: &str) -> (PathBuf, impl Fn(&[&str]) -> Output) {
    ignored_model_fixture_with(name, "exit 0", "exit 0")
}

/// [`ignored_model_fixture`] with the two check commands given, each run
/// by the platform shell.
fn ignored_model_fixture_with(
    name: &str,
    typecheck: &str,
    tests: &str,
) -> (PathBuf, impl Fn(&[&str]) -> Output) {
    let d = empty_repo(name);
    let home = scratch(&format!("{name}-home"));
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let check = |c: &str| format!("['{shell}', '{flag}', '{c}']");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!(
            "[checks]\ntypecheck = {}\ntests = {}\n\n\
             [tools]\nbash = true\nbash_unattended = true\nshell = ['{shell}', '{flag}']\n",
            check(typecheck),
            check(tests)
        ),
    )
    .unwrap();
    std::fs::write(d.join(".gitignore"), ".venv/\n").unwrap();
    std::fs::write(d.join("a.py"), "def f():\n    return 1\n").unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    std::fs::create_dir_all(d.join(".venv")).unwrap();
    std::fs::write(d.join(".venv/old.py"), "x = 1\n").unwrap();
    let root = d.clone();
    let run = move |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_trellis"))
            .arg("--root")
            .arg(&root)
            .args(args)
            .current_dir(&root)
            .env("NO_COLOR", "1")
            .env("TRELLIS_CONFIG_DIR", home.join("trellis"))
            .env("APPDATA", &home)
            .env("XDG_CONFIG_HOME", &home)
            .output()
            .unwrap()
    };
    let t = run(&["trust"]);
    assert!(t.status.success(), "trust: {}", combined(&t));
    (d, run)
}

#[test]
fn a_model_run_s_bash_writes_to_ignored_paths_are_removed_on_rollback() {
    // The risk: `bash` in a model run writes `.venv/.../sitecustomize.py`,
    // which git ignores; the run does not converge; the rollback's `git
    // clean -fd` leaves ignored files, and the next check that starts the
    // repository's Python runs it.
    let (d, run) = ignored_model_fixture("model-ignored");
    // By absolute path: `bash` runs in a throwaway clone (ADR 0032), and a
    // command that names the repository is the writer this guard is still for.
    let p = d.display();
    let cmd = if cfg!(windows) {
        format!(
            r"mkdir {p}\.venv\lib\site && echo import os > {p}\.venv\lib\site\sitecustomize.py && echo y = 2 >> {p}\.venv\old.py"
        )
    } else {
        format!(
            "mkdir -p '{p}/.venv/lib/site' && echo 'import os' > '{p}/.venv/lib/site/sitecustomize.py' && echo 'y = 2' >> '{p}/.venv/old.py'"
        )
    };
    let script = d
        .parent()
        .unwrap()
        .join(format!("model-ignored-script-{}.json", std::process::id()));
    std::fs::write(
        &script,
        serde_json::json!({"steps": [
            {"tool": "bash", "command": cmd},
            {"tool": "done", "summary": "wrote ignored files"},
        ]})
        .to_string(),
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let out = run(&["run", "write ignored files", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert!(
        !d.join(".venv/lib/site/sitecustomize.py").exists(),
        "an ignored file the run created survived the rollback:\n{said}"
    );
    assert!(!d.join(".venv/lib").exists(), "{said}");
    assert!(
        d.join(".venv/old.py").exists(),
        "a file that was there stays"
    );
    assert!(
        said.contains("left as it is") && said.contains(".venv/old.py"),
        "a changed pre-existing ignored file is named:\n{said}"
    );
    // The tracked tree is restored, which is what a rollback promises
    // (ADR 0009). A changed pre-existing ignored file nothing protects is
    // named but is not "not restored": round 8 made it exit 2, and every
    // writer that runs pytest rewrites a cache (review round 9, finding 2).
    assert_eq!(out.status.code(), Some(1), "{said}");
}

#[test]
fn a_model_run_whose_bash_commits_does_not_converge_and_the_commit_is_undone() {
    // The risk (review round 10, finding 1): `bash`, a custom tool or a
    // hook can commit. A committed change is invisible to `git status`, so
    // the run converged with nothing to review and the commit stayed on
    // the user's branch, past any rollback.
    let (d, run) = ignored_model_fixture("model-commit");
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    // By absolute path: `bash` runs in a throwaway clone (ADR 0032), and a
    // command that names the repository is the writer this guard is still for.
    let p = d.display();
    let cmd = if cfg!(windows) {
        format!(r"echo x = 2> {p}\b.py && git -C {p} add b.py && git -C {p} commit -qm sneak")
    } else {
        format!(
            "echo 'x = 2' > '{p}/b.py' && git -C '{p}' add b.py && git -C '{p}' commit -qm sneak"
        )
    };
    let script = d
        .parent()
        .unwrap()
        .join(format!("model-commit-script-{}.json", std::process::id()));
    std::fs::write(
        &script,
        serde_json::json!({"steps": [
            {"tool": "bash", "command": cmd},
            {"tool": "run_checks", "paths": ["a.py"], "kinds": ["typecheck", "tests"]},
            {"tool": "done", "summary": "committed"},
        ]})
        .to_string(),
    )
    .unwrap();
    let spec = format!("scripted:{}", script.display());
    let out = run(&["run", "commit something", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert!(said.contains("HEAD moved"), "{said}");
    assert!(!d.join("b.py").exists(), "{said}");
    let parent = git(&d, &["rev-parse", "HEAD~1"]);
    assert_eq!(
        parent.trim(),
        head,
        "only the trail's commit sits on the snapshot:\n{said}"
    );
}

#[test]
fn a_model_run_that_changed_an_ignored_file_deny_write_names_is_not_restored() {
    // The stricter signal stays where a rule says the file is not to be
    // written: a change there cannot be put back, and the run says the
    // tree is not restored (exit 2).
    let (d, run) = ignored_model_fixture("model-ignored-denied");
    let cfg = d.join(".trellis/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str("\n[governance]\ndeny_write = [\".venv/old.py\"]\n");
    std::fs::write(&cfg, text).unwrap();
    git(&d, &["commit", "-qam", "deny"]);
    assert!(run(&["trust"]).status.success());
    // By absolute path: `bash` runs in a throwaway clone (ADR 0032), and a
    // command that names the repository is the writer this guard is still for.
    let p = d.display();
    let cmd = if cfg!(windows) {
        format!(r"echo y = 2 >> {p}\.venv\old.py")
    } else {
        format!("echo 'y = 2' >> '{p}/.venv/old.py'")
    };
    let spec = script_beside(
        &d,
        "model-ignored-denied",
        serde_json::json!([
            {"tool": "bash", "command": cmd},
            {"tool": "done", "summary": "changed it"},
        ]),
    );
    let out = run(&["run", "change it", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert!(
        said.contains("NOT put back") && said.contains(".venv/old.py"),
        "{said}"
    );
    assert_eq!(out.status.code(), Some(2), "{said}");
}

/// A scripted model's steps, written beside the repository; the `--model`
/// spec that runs them.
fn script_beside(d: &Path, name: &str, steps: serde_json::Value) -> String {
    let script = d
        .parent()
        .unwrap()
        .join(format!("{name}-script-{}.json", std::process::id()));
    std::fs::write(&script, serde_json::json!({ "steps": steps }).to_string()).unwrap();
    format!("scripted:{}", script.display())
}

/// `core.fsmonitor` as the repository's config has it, if set.
///
/// `--local`, because `trellis_vcs::command()` passes `-c core.fsmonitor=`
/// and `config --get` without a scope reports that command-line value, so
/// every call would read `Some("")` and the `None` assertions could not
/// pass. Exit 1 is git's "not set"; any other failure is the fixture's.
fn fsmonitor(d: &Path) -> Option<String> {
    let o = trellis_vcs::command()
        .args(["config", "--local", "--get", "core.fsmonitor"])
        .current_dir(d)
        .output()
        .unwrap();
    match o.status.code() {
        Some(0) => Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Some(1) => None,
        _ => panic!(
            "git config --local --get core.fsmonitor failed ({}): {}",
            o.status,
            String::from_utf8_lossy(&o.stderr)
        ),
    }
}

#[test]
fn a_model_run_s_bash_writes_to_git_config_and_hooks_are_put_back_on_rollback() {
    // The risk: `bash` sets `core.fsmonitor` or plants a hook. `git status`
    // does not show `.git/`, the rollback's reset does not restore it, and
    // trellis's next git call runs the command (review round 8, finding 2).
    let (d, run) = ignored_model_fixture("model-git-guard-rb");
    // By absolute path: `bash` runs in a throwaway clone (ADR 0032), and a
    // command that names the repository is the writer this guard is still for.
    let p = d.display();
    let cmd = if cfg!(windows) {
        format!(
            r"git -C {p} config core.fsmonitor ./evil.sh && echo x> {p}\.git\hooks\post-checkout"
        )
    } else {
        format!(
            "git -C '{p}' config core.fsmonitor ./evil.sh && echo x > '{p}/.git/hooks/post-checkout'"
        )
    };
    let spec = script_beside(
        &d,
        "model-git-guard-rb",
        serde_json::json!([
            {"tool": "bash", "command": cmd},
            {"tool": "done", "summary": "planted"},
        ]),
    );
    let out = run(&["run", "plant", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert_eq!(fsmonitor(&d), None, "core.fsmonitor survived:\n{said}");
    assert!(
        !d.join(".git/hooks/post-checkout").exists(),
        "a planted hook survived:\n{said}"
    );
}

#[test]
fn a_core_fsmonitor_bash_sets_is_never_run_by_trellis_during_or_after_the_run() {
    // The risk (review round 9, finding 4): `.git/` is put back after the
    // loop, but the digest taken around every hooked call runs `git
    // status` in between, and `git status` runs `core.fsmonitor`. So a
    // `bash` call that set it had trellis run the command at the next
    // `post_tool`, before any guard could put the config back.
    let (d, run) = ignored_model_fixture("model-fsmonitor-marker");
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let cfg = d.join(".trellis/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!(
        "\n[hooks]\npost_tool = ['{shell}', '{flag}', 'exit 0']\n"
    ));
    std::fs::write(&cfg, text).unwrap();
    git(&d, &["commit", "-qam", "a post_tool hook"]);
    assert!(run(&["trust"]).status.success());
    let parent = d.parent().unwrap();
    let pid = std::process::id();
    let mark = parent.join(format!("fsmonitor-ran-{pid}"));
    let _ = std::fs::remove_file(&mark);
    let fsm = parent.join(format!("fsmonitor-{pid}.sh"));
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    std::fs::write(
        &fsm,
        format!("#!/bin/sh\necho x >> '{}'\nexit 1\n", slash(&mark)),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fsm, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let spec = script_beside(
        &d,
        "model-fsmonitor-marker",
        serde_json::json!([
            // By absolute path (`-C`): `bash` runs in a throwaway clone
            // (ADR 0032), and this is the writer that still reaches it.
            {"tool": "bash", "command": format!("git -C {} config core.fsmonitor {}", slash(&d), slash(&fsm))},
            {"tool": "bash", "command": "echo x> a.txt"},
            {"tool": "done", "summary": "set it"},
        ]),
    );
    let out = run(&["run", "set fsmonitor", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert_eq!(fsmonitor(&d), None, "core.fsmonitor survived:\n{said}");
    assert!(
        !mark.exists(),
        "trellis ran the planted core.fsmonitor:\n{said}"
    );
    // The control: plain git in the same repository with the setting
    // back in place runs it, so the assertion above is about trellis.
    git(&d, &["config", "core.fsmonitor", &slash(&fsm)]);
    std::fs::write(d.join("a.py"), "x = 3\n").unwrap();
    // Bare `git` on purpose, the one fixture spawn that is: the control
    // needs git without trellis_vcs's overrides, which would switch the
    // fsmonitor off. scripts/checks/no-bare-git-in-cli-tests.sh allows
    // exactly this one.
    let _ = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&d)
        .output()
        .unwrap();
    assert!(mark.exists(), "control: plain `git status` runs it");
    let _ = std::fs::remove_file(&mark);
    let _ = std::fs::remove_file(&fsm);
}

#[test]
fn a_model_run_that_changed_git_config_does_not_converge() {
    // Convergence leaves the tree for review, so a `core.fsmonitor` the run
    // set would run at the reviewer's next `git status`. A change there is
    // never part of a converged run: it fails the run and is put back.
    let (d, run) = ignored_model_fixture("model-git-guard-cv");
    // By absolute path: `bash` runs in a throwaway clone (ADR 0032), and a
    // command that names the repository is the writer this guard is still for.
    let spec = script_beside(
        &d,
        "model-git-guard-cv",
        serde_json::json!([
            {"tool": "bash", "command": format!("git -C {} config core.fsmonitor ./evil.sh", d.display())},
            {"tool": "run_checks", "paths": [], "kinds": ["typecheck", "tests"]},
            {"tool": "done", "summary": "set it"},
        ]),
    );
    let out = run(&["run", "set fsmonitor", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert!(!out.status.success(), "{said}");
    assert_eq!(fsmonitor(&d), None, "core.fsmonitor survived:\n{said}");
}

#[test]
fn a_check_s_own_write_to_an_ignored_file_does_not_make_a_rollback_unrestored() {
    // The checks run the repository's tools, which rewrite their caches
    // (`.mypy_cache`, `__pycache__`) on every run, and a failed run that
    // ran a check must not exit 2 for that. Round 8 bracketed each check;
    // round 9 holds no unprotected modified ignored file against the
    // rollback, whoever changed it, and this stays as the guard.
    let append = if cfg!(windows) {
        r"echo z>> .venv\old.py"
    } else {
        "echo z >> .venv/old.py"
    };
    let (d, run) = ignored_model_fixture_with("model-ignored-check", append, "exit 1");
    let spec = script_beside(
        &d,
        "model-ignored-check",
        serde_json::json!([
            {"tool": "bash", "command": "echo hi"},
            {"tool": "run_checks", "paths": [], "kinds": ["typecheck", "tests"]},
            {"tool": "done", "summary": "tried"},
        ]),
    );
    let out = run(&["run", "try", "--model", &spec]);
    let said = combined(&out);
    assert!(said.contains("NOT CONVERGED"), "{said}");
    assert!(!said.contains("NOT put back"), "{said}");
    assert_eq!(out.status.code(), Some(1), "{said}");
}

#[test]
fn subcommand_case_does_not_matter_for_its_name() {
    // ADR 0050 section 4: "TRELLIS RUN" was refused as an unknown subcommand
    // on a first run, though nothing about it was ambiguous.
    let help = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(help(&["RUN", "--help"]), help(&["run", "--help"]));
    assert_eq!(help(&["Doctor", "--help"]), help(&["doctor", "--help"]));
}

#[test]
fn subcommand_case_leaves_a_task_text_alone() {
    // Only the subcommand's name is lowered: a task in capitals is the
    // user's text. The run writes a trail, and the trail's task is read
    // back, because no refusal message echoes the task to compare against.
    let d = repo("case-task");
    let script = scratch("case-task-script");
    let out = run(
        &d,
        &["RUN", "--model", &script_that_stops(&script), "FIX THE BUG"],
    );
    let said = combined(&out);
    assert!(!said.contains("unrecognized subcommand"), "{said}");
    let trail: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(newest_trail(&d)).unwrap()).unwrap();
    assert_eq!(trail["task"], "FIX THE BUG", "{said}");
    for p in [&d, &script] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn start_screen_is_for_bare_trellis_only_and_run_still_refuses() {
    // ADR 0050 §3: the start screen is bare `trellis`'s alone. The risk is
    // that opening it loosened root discovery for everything, and `trellis
    // run` in a home directory starts walking it instead of refusing.
    let d = scratch("start-screen-run");
    let above: Vec<_> = d.ancestors().filter(|a| a.join(".git").exists()).collect();
    assert!(
        above.is_empty(),
        "the scratch directory is inside a repository, so this proves nothing: {above:?}"
    );
    let out = run(&d, &["run", "--model", "scripted:none.json", "x"]);
    let said = combined(&out);
    assert!(!out.status.success(), "{said}");
    assert!(
        said.contains("cannot determine the repository root"),
        "{said}"
    );
    assert!(said.contains("no git repository found at"), "{said}");
    // Bare `trellis` with no terminal (output is captured here) has no one
    // to answer a screen, so it refuses as it did before.
    let bare = run(&d, &[]);
    let said = combined(&bare);
    assert!(!bare.status.success(), "{said}");
    assert!(
        said.contains("cannot determine the repository root"),
        "{said}"
    );
    assert!(!d.join(".git").exists(), "{said}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `trellis <args>` in `root`, with `user` as the user config directory and,
/// when given, `temp` as the temporary directory the binary sees.
fn run_recording(root: &Path, user: &Path, temp: Option<&Path>, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user);
    if let Some(t) = temp {
        // `std::env::temp_dir` reads TMPDIR on Unix and TMP, then TEMP, on
        // Windows.
        c.env("TMPDIR", t).env("TMP", t).env("TEMP", t);
    }
    c.output().unwrap()
}

/// The repositories `user/recent.json` lists, resolved, so a link in the
/// temporary directory's path does not decide a comparison.
fn recorded(user: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(user.join("recent.json")) else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let p = PathBuf::from(p.as_str().unwrap());
            std::fs::canonicalize(&p).unwrap_or(p)
        })
        .collect()
}

#[test]
fn start_screen_home_a_root_under_the_temp_directory_is_not_recorded() {
    // The risk (the owner's first run): test fixtures and scratch checkouts
    // under the temporary directory fill the start screen's list, and are
    // gone by the time the person sees it.
    let d = repo("home-temp-root");
    let user = scratch("home-temp-root-user");
    let out = run_recording(&d, &user, None, &[]);
    assert!(
        recorded(&user).is_empty(),
        "{}
{:?}",
        combined(&out),
        std::fs::read_to_string(user.join("recent.json"))
    );
    for p in [&d, &user] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn start_screen_home_index_does_not_record_while_bare_trellis_does() {
    // The risk: every command records its root, so a script running
    // `trellis index` over many checkouts decides what the start screen
    // offers. Only an interface start is a person opening trellis. The
    // binary is given another temporary directory, so the repository is
    // outside the one it skips.
    let d = repo("home-index");
    let user = scratch("home-index-user");
    let temp = scratch("home-index-temp");
    let want = std::fs::canonicalize(&d).unwrap();

    let out = run_recording(&d, &user, Some(&temp), &["index"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        !recorded(&user).contains(&want),
        "index recorded its root: {}",
        combined(&out)
    );

    let out = run_recording(&d, &user, Some(&temp), &[]);
    assert_eq!(recorded(&user), vec![want], "{}", combined(&out));
    for p in [&d, &user, &temp] {
        let _ = std::fs::remove_dir_all(p);
    }
}

fn trellis_out(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn interface_flag_web_with_no_desktop_beside_trellis_says_where_it_looked() {
    // ADR 0050 section 5 (#375): `--web` is the desktop serving its page,
    // so it is looked for beside the running trellis only, as `--dsk`
    // looks, and nothing on PATH is started. Outside a repository it is
    // not refused for the missing root (the desktop shows its overview).
    let exe_name = format!("trellis{}", std::env::consts::EXE_SUFFIX);
    let desktop_name = format!("trellis-desktop{}", std::env::consts::EXE_SUFFIX);
    let bin = tempfile::tempdir().unwrap();
    let on_path = tempfile::tempdir().unwrap();
    let exe = bin.path().join(&exe_name);
    std::fs::copy(env!("CARGO_BIN_EXE_trellis"), &exe).unwrap();
    wait_until_executable(&exe);
    std::fs::write(on_path.path().join(&desktop_name), b"").unwrap();
    let out = Command::new(&exe)
        .arg("--web")
        .current_dir(bin.path())
        .env("PATH", on_path.path())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "--web succeeded: {err}");
    assert!(!err.contains("not built"), "{err}");
    assert!(
        !err.contains("cannot determine the repository root"),
        "{err}"
    );
    assert!(err.contains(&desktop_name), "{err}");
    assert!(err.contains("looked for"), "{err}");
    assert!(
        !err.contains(&on_path.path().display().to_string()),
        "{err}"
    );
    assert!(
        err.contains("cargo install --locked --path desktop"),
        "{err}"
    );
}

/// Runs the executable just written at `path` with `--version` until the
/// kernel stops refusing it as busy, and at most for a few seconds (#581).
///
/// The risk: `ETXTBSY` failing a test that has nothing wrong with it. On
/// Linux, a test thread that forks while another holds the new file's write
/// handle open carries that handle into its child until the child execs, and
/// executing the file in that window fails with "Text file busy". Waiting
/// here moves the wait out of the assertion. Only `ExecutableFileBusy` is
/// retried: any other outcome, success or not, is the file being runnable,
/// and the test's own exec reports the rest.
fn wait_until_executable(path: &Path) {
    let start = std::time::Instant::now();
    loop {
        match Command::new(path).arg("--version").output() {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && start.elapsed() < std::time::Duration::from_secs(5) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => return,
        }
    }
}

#[test]
fn interface_flag_dsk_with_no_desktop_beside_trellis_says_where_it_looked() {
    // ADR 0050 section 1 (#374): the desktop is looked for beside the
    // running trellis only. The risk: a `trellis-desktop` on PATH started
    // instead, or a refusal that does not say what to install. A copy of
    // the binary in a fresh directory, so a desktop built beside the real
    // one cannot change the answer.
    let exe_name = format!("trellis{}", std::env::consts::EXE_SUFFIX);
    let desktop_name = format!("trellis-desktop{}", std::env::consts::EXE_SUFFIX);
    let bin = tempfile::tempdir().unwrap();
    let on_path = tempfile::tempdir().unwrap();
    let exe = bin.path().join(&exe_name);
    std::fs::copy(env!("CARGO_BIN_EXE_trellis"), &exe).unwrap();
    wait_until_executable(&exe);
    std::fs::write(on_path.path().join(&desktop_name), b"").unwrap();
    let out = Command::new(&exe)
        .arg("--dsk")
        .current_dir(bin.path())
        .env("PATH", on_path.path())
        // On Linux the display check comes first; nothing is started here.
        .env("DISPLAY", ":0")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "--dsk succeeded: {err}");
    assert!(err.contains(&desktop_name), "{err}");
    assert!(err.contains("looked for"), "{err}");
    assert!(
        !err.contains(&on_path.path().display().to_string()),
        "{err}"
    );
    assert!(
        err.contains("cargo install --locked --path crates/trellis-cli"),
        "{err}"
    );
    assert!(
        err.contains("cargo install --locked --path desktop"),
        "{err}"
    );
}

#[test]
fn interface_flag_two_together_are_refused_naming_both() {
    for (a, b) in [("--tui", "--dsk"), ("--tui", "--web"), ("--dsk", "--web")] {
        let out = trellis_out(&[a, b]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{a} {b} succeeded");
        assert!(err.contains(a) && err.contains(b), "{a} {b}: {err}");
    }
}

#[test]
fn interface_flag_with_a_subcommand_is_refused_naming_both() {
    let out = trellis_out(&["--tui", "run", "x"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("`--tui`") && err.contains("`run`"), "{err}");
    // And the help leads with how to start.
    let help = String::from_utf8_lossy(&trellis_out(&["--help"]).stdout).into_owned();
    let start = help.find("Start:").expect("a Start section");
    let commands = help.find("Commands:").expect("a Commands section");
    assert!(start < commands, "{help}");
    for flag in ["--tui", "--dsk", "--web"] {
        assert!(
            help[start..commands].contains(flag),
            "{flag} missing from Start: {help}"
        );
    }
}

// ---------------------------------------------------------------------------
// the user's directory and installed packages (#248)
// ---------------------------------------------------------------------------

/// `trellis <args>` in `root` with `user` as the whole user level.
fn run_as(root: &Path, user: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("TRELLIS_CONFIG_DIR", user)
        .output()
        .unwrap()
}

/// A user directory with package `kit` installed by hand: one skill,
/// `tidy`, and one template, `kitfix`.
fn user_with_kit(name: &str) -> PathBuf {
    let user = scratch(name);
    let packages = user.join("packages");
    std::fs::create_dir_all(packages.join("kit/skills/tidy")).unwrap();
    std::fs::create_dir_all(packages.join("kit/prompts")).unwrap();
    std::fs::write(
        packages.join("packages.toml"),
        format!(
            "[[package]]\nname = \"kit\"\nsource = \"/s/kit\"\nref = \"v1\"\ncommit = \"{}\"\n",
            "a".repeat(40)
        ),
    )
    .unwrap();
    std::fs::write(
        packages.join("kit/skills/tidy/SKILL.md"),
        "---\nname: tidy\ndescription: Tidy a module.\n---\n# tidy\n",
    )
    .unwrap();
    std::fs::write(
        packages.join("kit/prompts/kitfix.md"),
        "Fix the thing the kit knows about.\n",
    )
    .unwrap();
    user
}

#[test]
fn doctor_reports_the_user_s_file_and_an_installed_package_s_skills() {
    // The risk: doctor, where a person goes to find out what a run will
    // load, saying a user file is layered when it is absent or absent when
    // it is layered, or not finding a package's skills that a run loads.
    let d = repo("doctor-user-level");
    let user = user_with_kit("doctor-user-level-cfg");
    let s = stdout(&run_as(&d, &user, &["doctor"]));
    assert!(
        s.contains("config.toml — not present; the repository's config alone"),
        "{s}"
    );
    assert!(s.contains("user skills: 1 loaded"), "{s}");
    std::fs::write(user.join("config.toml"), "").unwrap();
    let s = stdout(&run_as(&d, &user, &["doctor"]));
    assert!(
        s.contains("config.toml — layered under the repository's"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn a_run_finds_a_template_only_an_installed_package_carries() {
    let d = repo("installed-template");
    let user = user_with_kit("installed-template-cfg");
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "script"]);
    let out = run_as(
        &d,
        &user,
        &["run", "--prompt", "kitfix", "--model", &script],
    );
    let s = combined(&out);
    assert!(!s.contains("no prompt"), "{s}");
    assert!(s.contains("model:"), "the run started: {s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn install_list_prints_what_the_manifest_records() {
    // The risk: `trellis install` dispatched and then doing nothing,
    // exiting 0 with no output.
    let d = scratch("install-list");
    let user = user_with_kit("install-list-cfg");
    let out = run_as(&d, &user, &["install", "--list"]);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout(&out).contains("kit  source /s/kit  ref v1  commit"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn no_model_message_is_the_same_from_run_and_from_bare_trellis_on_a_pipe() {
    // #376, ADR 0050 section 2: two commands that refuse for the same reason
    // used to say it two ways, and neither mentioned the picker. The risk is
    // drift: one wording updated and the other not. So the text `trellis
    // run` prints must appear, whole, in what bare `trellis` prints when a
    // pipe sends it to the line session.
    let d = repo("no-model-message");
    let config = std::env::temp_dir()
        .join("trellis-tests-no-user-config")
        .join("config.toml");

    let out = run(&d, &["run", "x"]);
    assert!(!out.status.success(), "a run with no model must refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    let said = err
        .trim()
        .strip_prefix("Error: ")
        .unwrap_or_else(|| panic!("an error, not:\n{err}"));
    // What it has to name: the picker, one complete `--model` form, and the
    // user's own config file.
    assert!(said.contains("  trellis "), "names bare trellis:\n{said}");
    assert!(
        said.contains("--model llamacpp:http://127.0.0.1:8080"),
        "a concrete --model:\n{said}"
    );
    assert!(
        said.contains(&config.display().to_string()),
        "the user config file:\n{said}"
    );

    let bare = {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
            .current_dir(&d)
            .env("NO_COLOR", "1")
            .env(
                "TRELLIS_CONFIG_DIR",
                std::env::temp_dir().join("trellis-tests-no-user-config"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"x\n/exit\n")
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let shown = combined(&bare);
    assert!(
        shown.contains(said),
        "bare trellis must print the same text as `trellis run`.\nrun:\n{said}\nbare:\n{shown}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

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
    let out = run(
        &d,
        &[
            "run",
            "--model",
            &script_that_stops(&scratch("skills-only-invalid-script")),
            "t",
        ],
    );
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
    let script = scratch(&format!("{name}-script")).join("s.json");
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
    let c: serde_json::Value =
        serde_json::from_str(include_str!("client_contract.json")).expect("the contract is JSON");
    let shape = &c["rpc"]["commands"]["get_state"]["data"]["tier_label"];
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

// ---------------------------------------------------------------------------
// #248, pass 3: `trellis vault`
// ---------------------------------------------------------------------------

/// `repo_with_vault`'s `root` and `child`, plus `lost`, whose parent does
/// not exist, and `a` and `b`, each the other's parent: one note with a
/// missing parent and two on a cycle, beside two with no issue.
fn vault_with_issues(name: &str) -> PathBuf {
    let (d, _) = repo_with_vault(name, "vault = \"notes\"\n");
    let v = d.join("notes");
    for (file, title, parent) in [
        ("lost.md", "Lost", "[[nowhere]]"),
        ("a.md", "A", "[[b]]"),
        ("b.md", "B", "[[a]]"),
    ] {
        std::fs::write(
            v.join(file),
            format!("---\ntitle: \"{title}\"\nparent: \"{parent}\"\n---\nBODY\n"),
        )
        .unwrap();
    }
    std::fs::write(
        v.join("manifest.json"),
        r#"["a.md","b.md","child.md","lost.md","root.md"]"#,
    )
    .unwrap();
    d
}

#[test]
fn vault_graph_lists_a_note_s_own_issues_and_no_other_note_s() {
    // #248. The risk: `graph` printing nothing, or a note's issues filtered
    // wrongly, so a note on a cycle or under a missing parent looks sound
    // and a sound note carries another note's problem.
    let d = vault_with_issues("vault-graph");
    let lost = stdout(&run(&d, &["vault", "graph", "lost"]));
    assert!(lost.contains("note:     lost.md \"Lost\""), "{lost}");
    assert!(lost.contains("issues:"), "{lost}");
    assert!(lost.contains("nowhere"), "{lost}");
    let on_cycle = stdout(&run(&d, &["vault", "graph", "a"]));
    assert!(on_cycle.contains("issues:"), "{on_cycle}");
    let sound = stdout(&run(&d, &["vault", "graph", "child"]));
    assert!(sound.contains("note:     child.md"), "{sound}");
    assert!(!sound.contains("issues:"), "{sound}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_check_lists_issues_only_when_there_are_some_and_names_the_fix_for_a_refusal() {
    // #248. The risk: `check` saying nothing, an empty "issues:" header on
    // a sound vault, or a refused vault left without the command that
    // repairs it.
    let (d, _) = repo_with_vault("vault-check", "vault = \"notes\"\n");
    let out = run(&d, &["vault", "check"]);
    let sound = stdout(&out);
    assert!(out.status.success(), "{}", combined(&out));
    assert!(sound.contains("run gate: accepts this vault"), "{sound}");
    assert!(!sound.contains("issues:"), "{sound}");

    let d2 = vault_with_issues("vault-check-issues");
    let issues = stdout(&run(&d2, &["vault", "check"]));
    assert!(issues.contains("issues:"), "{issues}");

    std::fs::write(
        d.join("notes/child.md"),
        "---\ntitle: \"Child\"\ntags: x\nparent: \"[[root]]\"\n---\nCHILD-BODY\n",
    )
    .unwrap();
    let out = run(&d, &["vault", "check"]);
    assert_eq!(out.status.code(), Some(1), "{}", combined(&out));
    assert!(
        stdout(&out).contains("`trellis vault check --fix` repairs"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn vault_search_finds_a_body_word_and_reports_no_reduction_on_a_small_vault() {
    // #248. The risk: `search` printing nothing, or saying it was reduced
    // to titles or lost its fuzzy matching on a vault far under the limits.
    let (d, _) = repo_with_vault("vault-search", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "search", "CHILD-BODY"]));
    assert!(s.contains("child.md"), "{s}");
    assert!(!s.contains("reduced"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// No raw ESC or carriage return reaches the terminal, and each shows as
/// the escape that names it.
fn assert_escaped(out: &str) {
    assert!(!out.contains('\x1b'), "{out:?}");
    assert!(!out.contains('\r'), "{out:?}");
    assert!(out.contains("\\u{1b}"), "{out:?}");
}

#[test]
fn vault_escapes_a_note_title_in_search_and_graph() {
    // #461. The risk: a note is untrusted content (a cloned repository's
    // vault), and a title holding ESC and a carriage return, printed raw,
    // can recolour or overwrite the line a person reads. The frontmatter
    // value is decoded as a JSON string, so `\u001b` and `\r` are the real
    // characters by the time the title is printed.
    let (d, _) = repo_with_vault("vault-escapes-title", "vault = \"notes\"\n");
    std::fs::write(
        d.join("notes/evil.md"),
        "---\ntitle: \"Evil\\u001b[31m\\rForged\"\nparent: \"[[root]]\"\n---\nEVIL-BODY\n",
    )
    .unwrap();
    let s = stdout(&run(&d, &["vault", "search", "EVIL-BODY"]));
    assert!(s.contains("evil.md"), "{s:?}");
    assert_escaped(&s);
    assert!(s.contains("\\r"), "{s:?}");
    for note in ["evil", "root"] {
        let g = stdout(&run(&d, &["vault", "graph", note]));
        assert!(g.contains("evil.md"), "{g:?}");
        assert_escaped(&g);
        assert!(g.contains("\\r"), "{g:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn vault_escapes_a_note_file_name_in_search_and_graph() {
    // #461. The risk: the same forged terminal through a path rather than
    // a title. Unix only: Windows refuses control characters in file names.
    let (d, _) = repo_with_vault("vault-escapes-path", "vault = \"notes\"\n");
    std::fs::write(
        d.join("notes/es\x1b[31mc.md"),
        "---\ntitle: \"Plain\"\nparent: \"[[root]]\"\n---\nPATH-BODY\n",
    )
    .unwrap();
    let s = stdout(&run(&d, &["vault", "search", "PATH-BODY"]));
    assert!(s.contains("Plain"), "{s:?}");
    assert_escaped(&s);
    let g = stdout(&run(&d, &["vault", "graph", "root"]));
    assert!(g.contains("Plain"), "{g:?}");
    assert_escaped(&g);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_new_says_what_it_created() {
    // #248. The risk: an edit that reports nothing, so the person cannot
    // tell which file the title became.
    let (d, _) = repo_with_vault("vault-new", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "new", "Third", "--parent", "root"]));
    assert!(s.contains("created third.md (\"Third\")"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files() {
    // #248. The risk: `dates` silent about a missing dates.json, `--seed`
    // refusing to create one, or the seeded entries carrying no day where
    // the file times give one.
    let (d, _) = repo_with_vault("vault-dates", "vault = \"notes\"\n");
    let s = stdout(&run(&d, &["vault", "dates"]));
    assert!(s.contains("no dates.json in"), "{s}");
    assert!(s.contains("--seed"), "{s}");
    let out = run(&d, &["vault", "dates", "--seed"]);
    let s = stdout(&out);
    assert!(s.contains("dates.json: 2 seeded"), "{}", combined(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("notes/dates.json")).unwrap())
            .unwrap();
    // Written moments ago, so the file times name a day; which day depends
    // on the host's zone, so only its shape is asserted.
    let day = doc["notes"]["root.md"]["created"]["day"].as_str();
    assert!(day.is_some_and(|d| d.len() == 10), "{doc}");
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// `trellis overview` (ADR 0050 §3, ADR 0025 §4, #558)
// ---------------------------------------------------------------------------

/// What `gh` the overview finds on PATH.
enum Gh {
    /// None: every PATH entry holding a `gh` is left out, so the real one on
    /// a developer's machine is never run by the suite.
    Absent,
    /// A stub, first on PATH, whose body is `windows` (a `.cmd`) or `unix`
    /// (a shell script), with `{dir}` replaced by its own directory.
    Stub {
        windows: &'static str,
        unix: &'static str,
    },
}

/// A `gh` that prints fixed JSON for `pr list` and `run list`, and records
/// the directory it was started in.
const GH_OK: Gh = Gh::Stub {
    windows: "@echo off\r\ncd > \"{dir}\\cwd.txt\"\r\n\
              if \"%1\"==\"pr\" type \"{dir}\\pr.json\"\r\n\
              if \"%1\"==\"run\" type \"{dir}\\run.json\"\r\n",
    unix: "#!/bin/sh\npwd > '{dir}/cwd.txt'\n\
           case \"$1\" in pr) /bin/cat '{dir}/pr.json';; run) /bin/cat '{dir}/run.json';; esac\n",
};

/// PATH without any directory holding a `gh`, so `git` is still found.
fn path_without_gh() -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|d| {
            !["gh", "gh.exe", "gh.cmd"]
                .iter()
                .any(|n| d.join(n).is_file())
        })
        .collect()
}

/// On Unix `gh` often shares `/usr/bin` with `git` and `kill`, so leaving
/// out the directories that hold a `gh` loses them too: the queue needs
/// `git`, and a timed-out `gh`'s group is killed with `kill`. A directory
/// holding only wrappers that run the real ones puts them back; the stubs
/// name `/bin/cat` and `/bin/sleep` for the same reason. `None` on Windows,
/// where `gh` has a directory of its own.
fn tools_beside_gh(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let kept = path_without_gh();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dir = scratch(&format!("{name}-tools"));
    for tool in ["git", "kill"] {
        if kept.iter().any(|d| d.join(tool).is_file()) {
            continue;
        }
        let Some(real) = std::env::split_paths(&path)
            .map(|d| d.join(tool))
            .find(|p| p.is_file())
        else {
            continue;
        };
        let p = dir.join(tool);
        let body = format!("#!/bin/sh\nexec '{}' \"$@\"\n", real.display());
        std::fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    Some(dir)
}

/// `scratch`'s path for `name`, without emptying it.
fn scratch_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("trellis-cli-it-{name}-{}", std::process::id()))
}

/// The stub's directory: `gh.cmd` or `gh`, beside the JSON it prints.
fn gh_stub(name: &str, windows: &str, unix: &str) -> PathBuf {
    let dir = scratch(&format!("{name}-gh"));
    std::fs::write(
        dir.join("pr.json"),
        r#"[{"number":42,"title":"teach the board","headRefName":"t558-overview","isDraft":true,
            "statusCheckRollup":[
              {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
              {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE"},
              {"__typename":"StatusContext","context":"ci/x","state":"PENDING"}]}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("run.json"),
        r#"[{"databaseId":9001,"workflowName":"CI","headBranch":"t558-overview",
             "status":"completed","conclusion":"success","createdAt":"2026-10-07T10:00:00Z"}]"#,
    )
    .unwrap();
    let d = dir.display().to_string();
    if cfg!(windows) {
        std::fs::write(dir.join("gh.cmd"), windows.replace("{dir}", &d)).unwrap();
    } else {
        let p = dir.join("gh");
        std::fs::write(&p, unix.replace("{dir}", &d)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    dir
}

/// `trellis <args>` in `cwd`, with `user` as its user directory and the
/// `gh` that `gh` names on PATH.
fn overview_in(name: &str, cwd: &Path, user: &Path, gh: Gh, args: &[&str]) -> Output {
    let mut dirs = path_without_gh();
    if let Some(tools) = tools_beside_gh(name) {
        dirs.insert(0, tools);
    }
    if let Gh::Stub { windows, unix } = gh {
        dirs.insert(0, gh_stub(name, windows, unix));
    }
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env("TRELLIS_CONFIG_DIR", user)
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .expect("the binary is built before its tests")
}

/// The one JSON object `--json overview` prints: the whole of stdout.
fn overview_json(o: &Output) -> serde_json::Value {
    assert!(o.status.success(), "{}", combined(o));
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON value ({e}):\n{}", combined(o)))
}

/// A user directory whose `recent.json` lists `repos`, newest first.
fn user_dir_listing(name: &str, repos: &[&Path]) -> PathBuf {
    let dir = scratch(&format!("{name}-user"));
    let list: Vec<String> = repos.iter().map(|r| r.display().to_string()).collect();
    std::fs::write(
        dir.join("recent.json"),
        serde_json::json!({"version": 1, "repositories": list}).to_string(),
    )
    .unwrap();
    dir
}

/// A trail at `<root>/.trellis/runs/<stamp>-run.json` asking `task`: two
/// passing checks, two files proposed, five turns, 3 verified and 1
/// reviewed, tier 2.
fn write_trail(root: &Path, stamp: &str, task: &str) {
    let runs = root.join(".trellis").join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let trail = serde_json::json!({
        "schema": 1,
        "run_id": format!("{stamp}-run"),
        "task": task,
        "base_commit": "abc1234",
        "started_at": "2026-10-07T09:30:00Z",
        "provenance": {"trellis_version": "0.0.1", "model": null, "tier": "2"},
        "factual": {
            "proposed": ["app/a.py", "app/b.py"],
            "radius": {"reached_resolved": 0, "reached_ambiguous": 0,
                       "widest_unbound_in_touched_files": 0, "radius_queries": 0},
            "checks": [],
            "check_outcomes": [{"kind": "Typecheck", "passed": true},
                               {"kind": "Tests", "passed": true}],
            "approved_by": "Human",
            "mode_counts": {"verified": 3, "reviewed": 1},
            "context": {"first_prompt_bytes": 1, "peak_prompt_bytes": 2, "turns_measured": 5},
            "actions": []
        },
        "factual_sha256": "0000",
        "judgment": {"tech_debt_projection": null, "rationale": null}
    });
    std::fs::write(runs.join(format!("{stamp}-run.json")), trail.to_string()).unwrap();
}

fn keys(v: &serde_json::Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

#[test]
fn overview_outside_any_repository_exits_0_and_prints_only_recent() {
    // ADR 0050 §3, amended: the overview runs with or without a repository.
    // Outside one there is no current root, so the list is all there is.
    let here = scratch("overview-outside");
    let listed = repo("overview-outside-listed");
    write_trail(&listed, "20261007-093000-000", "rename the parser");
    let user = user_dir_listing("overview-outside", &[&listed]);
    let o = overview_in(
        "overview-outside",
        &here,
        &user,
        Gh::Absent,
        &["--json", "overview"],
    );
    let v = overview_json(&o);
    assert_eq!(keys(&v), ["recent"], "{v}");
    assert_eq!(v["recent"].as_array().unwrap().len(), 1, "{v}");
    let text = overview_in("overview-outside", &here, &user, Gh::Absent, &["overview"]);
    assert!(text.status.success(), "{}", combined(&text));
    let s = stdout(&text);
    assert!(s.contains("rename the parser"), "{s}");
    assert!(s.contains("not inside a git repository"), "{s}");
    assert!(!s.contains("board"), "no root, so no board: {s}");
    for d in [&here, &listed, &user] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn overview_treats_a_root_without_git_as_no_root_and_reads_nothing_under_it() {
    // The `current_dir()` fallback from a home directory, here named with
    // `--root`: neither the trail nor the config under it may be read, the
    // config invalid so that reading it would fail the command.
    let tree = scratch("overview-no-git");
    write_trail(
        &tree,
        "20261007-093000-000",
        "a trail that must stay unread",
    );
    std::fs::write(tree.join(".trellis/config.toml"), "this is = = not toml").unwrap();
    let user = scratch("overview-no-git-user");
    let root = tree.display().to_string();
    let args = ["--root", root.as_str(), "--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-git",
        &tree,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(keys(&v), ["recent"], "{v}");
    let o = overview_in("overview-no-git", &tree, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        !combined(&o).contains("must stay unread"),
        "{}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_never_creates_the_index_and_says_the_repository_is_not_indexed() {
    // The overview reads `.trellis/index.bin` only when it is current, and
    // never builds or saves one: printing a picture must not write the tree.
    let d = repo("overview-no-index");
    let user = scratch("overview-no-index-user");
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-index",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["codebase"].is_null(), "{v}");
    assert_eq!(
        v["codebase_reason"], "not indexed; run trellis index",
        "{v}"
    );
    let o = overview_in("overview-no-index", &d, &user, Gh::Absent, &["overview"]);
    assert!(
        stdout(&o).contains("not indexed; run trellis index"),
        "{}",
        combined(&o)
    );
    assert!(
        !d.join(".trellis/index.bin").exists(),
        "the overview wrote an index"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_reads_a_current_index_and_calls_a_changed_tree_stale() {
    // The cache is shown only while its fingerprint matches the sources; a
    // stale one is named, never shown as current, never rebuilt.
    let d = repo("overview-index");
    let user = scratch("overview-index-user");
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-index", &d, &user, Gh::Absent, &args));
    assert!(
        v["codebase"]["modules_indexed"]
            .as_u64()
            .is_some_and(|n| n > 0),
        "{v}"
    );
    assert!(v["codebase"]["bound_call_sites"].is_u64(), "{v}");
    let s = stdout(&overview_in(
        "overview-index",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("module(s) indexed"), "{s}");
    let before = std::fs::read(d.join(".trellis/index.bin")).unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(who):\n    return who + '!'\n",
    )
    .unwrap();
    let v = overview_json(&overview_in("overview-index", &d, &user, Gh::Absent, &args));
    assert!(v["codebase"].is_null(), "{v}");
    assert_eq!(
        v["codebase_reason"], "index is stale; run trellis index",
        "{v}"
    );
    assert_eq!(
        std::fs::read(d.join(".trellis/index.bin")).unwrap(),
        before,
        "the overview rewrote the index"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_shows_a_listed_repository_s_last_outcome_and_the_current_last_run() {
    // The recent list carries each repository's last outcome (ADR 0021
    // rows 44-47, amended). The current root's last run keeps the run's
    // outcome and its checks' results under separate labels, and coverage
    // as two counts.
    let d = repo("overview-outcome");
    write_trail(&d, "20261007-080000-000", "an older run");
    write_trail(&d, "20261007-093000-000", "rename the parser");
    let user = user_dir_listing("overview-outcome", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-outcome",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    let last = &v["recent"][0]["last_run"];
    assert_eq!(last["run_id"], "20261007-093000-000-run", "{v}");
    assert_eq!(last["outcome"], "converged", "{v}");
    assert_eq!(last["task"], "rename the parser", "{v}");
    assert_eq!(last["started_at"], "2026-10-07T09:30:00Z", "{v}");
    assert!(last["integrity"].is_string(), "{v}");
    let run = &v["last_run"];
    assert_eq!(run["turns"], 5, "{v}");
    assert_eq!(run["edits"], 2, "{v}");
    assert_eq!(run["verified"], 3, "{v}");
    assert_eq!(run["reviewed"], 1, "{v}");
    assert_eq!(run["tier"], "2", "{v}");
    assert_eq!(
        run["checks"][1],
        serde_json::json!({"kind": "Tests", "passed": true}),
        "{v}"
    );
    let s = stdout(&overview_in(
        "overview-outcome",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("outcome: converged"), "{s}");
    assert!(s.contains("checks: Typecheck passed, Tests passed"), "{s}");
    assert!(
        s.contains("3 verified by an executed check, 1 reviewed only"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_shows_null_for_a_repository_with_no_runs() {
    let d = repo("overview-no-runs");
    let user = user_dir_listing("overview-no-runs", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-runs",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["recent"][0]["last_run"].is_null(), "{v}");
    assert!(v["last_run"].is_null(), "{v}");
    let s = stdout(&overview_in(
        "overview-no-runs",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("no runs yet"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_reports_a_corrupt_newest_trail_as_an_error_and_still_exits_0() {
    // One unreadable file must not hide the rest of the picture, nor be
    // passed over for an older trail as if it were the last run.
    let d = repo("overview-corrupt");
    write_trail(&d, "20261007-080000-000", "an older run");
    std::fs::write(
        d.join(".trellis/runs/20261007-093000-000-run.json"),
        "{ not a trail",
    )
    .unwrap();
    let user = user_dir_listing("overview-corrupt", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-corrupt",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["recent"][0]["last_run"]["error"].is_string(), "{v}");
    assert!(v["last_run"]["error"].is_string(), "{v}");
    let o = overview_in("overview-corrupt", &d, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        stdout(&o).contains("newest trail unreadable"),
        "{}",
        combined(&o)
    );
    assert!(!stdout(&o).contains("an older run"), "{}", combined(&o));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_strips_an_escape_from_trail_text_and_escapes_it_in_json() {
    // A trail is untrusted (ADR 0007): ESC and the C1 CSI in a task must not
    // reach the terminal raw, nor leave the JSON unescaped.
    let d = repo("overview-esc");
    write_trail(&d, "20261007-093000-000", "fix\u{1b}[2Jthis\u{9b}2J");
    let user = user_dir_listing("overview-esc", &[&d]);
    let o = overview_in("overview-esc", &d, &user, Gh::Absent, &["overview"]);
    let s = stdout(&o);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(!s.contains('\u{1b}') && !s.contains('\u{9b}'), "{s:?}");
    assert!(s.contains("fix\\u{1b}[2Jthis"), "{s}");
    let j = overview_in(
        "overview-esc",
        &d,
        &user,
        Gh::Absent,
        &["--json", "overview"],
    );
    let raw = String::from_utf8_lossy(&j.stdout).into_owned();
    assert!(
        !raw.contains('\u{1b}') && !raw.contains('\u{9b}'),
        "{raw:?}"
    );
    assert!(raw.contains("\\u001b") && raw.contains("\\u009b"), "{raw}");
    assert_eq!(
        overview_json(&j)["last_run"]["task"],
        "fix\u{1b}[2Jthis\u{9b}2J"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_lists_pull_requests_and_runs_from_a_gh_on_path() {
    // PR and CI state come live from the user's own `gh` (owner,
    // 2026-10-07), started in the current root.
    let d = repo("overview-gh");
    let user = scratch("overview-gh-user");
    let o = overview_in("overview-gh", &d, &user, GH_OK, &["--json", "overview"]);
    let v = overview_json(&o);
    let pr_ci = &v["board"]["pr_ci"];
    assert_eq!(pr_ci["available"], true, "{v}");
    let pr = &pr_ci["pull_requests"][0];
    assert_eq!(pr["number"], 42, "{v}");
    assert_eq!(pr["title"], "teach the board", "{v}");
    assert_eq!(pr["head"], "t558-overview", "{v}");
    assert_eq!(pr["draft"], true, "{v}");
    assert_eq!(
        pr["checks"],
        serde_json::json!({"passed": 1, "failed": 1, "pending": 1, "other": 0}),
        "{v}"
    );
    let r = &pr_ci["runs"][0];
    assert_eq!(r["id"], 9001, "{v}");
    assert_eq!(r["workflow"], "CI", "{v}");
    assert_eq!(r["branch"], "t558-overview", "{v}");
    assert_eq!(r["conclusion"], "success", "{v}");
    let cwd = std::fs::read_to_string(scratch_path("overview-gh-gh").join("cwd.txt")).unwrap();
    let name = d.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        cwd.trim().ends_with(&name),
        "gh started in {cwd}, not the root"
    );
    let s = stdout(&overview_in("overview-gh", &d, &user, GH_OK, &["overview"]));
    assert!(s.contains("#42 teach the board"), "{s}");
    assert!(s.contains("CI  t558-overview  completed/success"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
    let _ = std::fs::remove_dir_all(scratch_path("overview-gh-gh"));
}

#[test]
fn wait_line_overview_json_stdout_stays_one_json_object_while_gh_is_asked() {
    // #650 says what bare `trellis` waits on, on stderr and only at a
    // terminal. The risk is the desktop's `--json overview`: a line on its
    // stdout would leave a body no client can parse.
    let d = repo("overview-wait-line");
    let user = scratch("overview-wait-line-user");
    let o = overview_in(
        "overview-wait-line",
        &d,
        &user,
        GH_OK,
        &["--json", "overview"],
    );
    let v = overview_json(&o);
    assert_eq!(v["board"]["pr_ci"]["available"], true, "gh was asked: {v}");
    assert!(
        !String::from_utf8_lossy(&o.stderr).contains("asking gh"),
        "stderr is a pipe here, so nothing is said: {}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
    let _ = std::fs::remove_dir_all(scratch_path("overview-wait-line-gh"));
}

#[test]
fn overview_without_gh_on_path_says_pr_ci_is_unavailable_and_exits_0() {
    let d = repo("overview-gh-absent");
    let user = scratch("overview-gh-absent-user");
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-gh-absent",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("not found")),
        "{v}"
    );
    assert!(
        v["board"]["tasks"].is_array(),
        "the rest is unaffected: {v}"
    );
    let o = overview_in("overview-gh-absent", &d, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        stdout(&o).contains("PR and CI state unavailable"),
        "{}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_says_gh_is_not_logged_in_when_it_asks_for_auth() {
    let d = repo("overview-gh-auth");
    let user = scratch("overview-gh-auth-user");
    let gh = Gh::Stub {
        windows: "@echo off\r\n\
                  echo To get started with GitHub CLI, please run:  gh auth login 1>&2\r\n\
                  exit /b 4\r\n",
        unix: "#!/bin/sh\n\
               echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n",
    };
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-gh-auth", &d, &user, gh, &args));
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("not logged in")),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
    let _ = std::fs::remove_dir_all(scratch_path("overview-gh-auth-gh"));
}

#[test]
fn overview_gives_up_on_a_gh_that_runs_past_its_bound() {
    // Each `gh` is bounded to 10 s; one that sleeps past it is killed and
    // the board says it timed out, well before the stub would have ended.
    let d = repo("overview-gh-slow");
    let user = scratch("overview-gh-slow-user");
    let gh = Gh::Stub {
        windows: "@echo off\r\nping -n 60 127.0.0.1 >nul\r\n",
        unix: "#!/bin/sh\n/bin/sleep 60\n",
    };
    let started = std::time::Instant::now();
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-gh-slow", &d, &user, gh, &args));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("timed out")),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_board_lists_the_queue_s_tasks() {
    let d = repo("overview-queue");
    let user = scratch("overview-queue-user");
    let o = run(&d, &["queue", "add", "tidy the parser"]);
    assert!(o.status.success(), "{}", combined(&o));
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-queue", &d, &user, Gh::Absent, &args));
    assert_eq!(v["board"]["tasks"][0]["task"], "tidy the parser", "{v}");
    let s = stdout(&overview_in(
        "overview-queue",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("tidy the parser"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

/// Whether `record` carries `path`: dotted keys, with `[]` meaning every
/// element of a non-empty array. A key present with `null` is carried. The
/// reading `tests/modes.rs` gives the rpc and `log_json` entries.
fn contract_carries(record: &serde_json::Value, path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    let (key, each) = match head.strip_suffix("[]") {
        Some(k) => (k, true),
        None => (head, false),
    };
    let Some(v) = record.get(key) else {
        return false;
    };
    match (each, rest) {
        (false, None) => true,
        (false, Some(r)) => contract_carries(v, r),
        (true, r) => v.as_array().is_some_and(|a| {
            !a.is_empty() && a.iter().all(|e| r.is_none_or(|r| contract_carries(e, r)))
        }),
    }
}

#[test]
fn client_contract_overview_lists_what_the_overview_object_carries() {
    // The risk: a field the desktop reads from `--json overview` (ADR 0048
    // §2, amended) is renamed or dropped, and its view goes blank with
    // nothing failing.
    let c: serde_json::Value =
        serde_json::from_str(include_str!("client_contract.json")).expect("the contract is JSON");
    let entry = &c["overview"];
    let d = repo("overview-contract");
    write_trail(&d, "20261007-093000-000", "rename the parser");
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let o = run(&d, &["queue", "add", "tidy the parser"]);
    assert!(o.status.success(), "{}", combined(&o));
    let user = user_dir_listing("overview-contract", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-contract", &d, &user, GH_OK, &args));
    let always = entry["always"]
        .as_array()
        .expect("overview.always is a list");
    assert!(always.len() > 10, "{entry}");
    let gaps: Vec<&str> = always
        .iter()
        .map(|p| p.as_str().expect("a path"))
        .filter(|p| !contract_carries(&v, p))
        .collect();
    assert!(
        gaps.is_empty(),
        "--json overview lacks what tests/client_contract.json lists: {gaps:?}\n{v}"
    );
    let outside: Vec<&str> = entry["outside_a_repository"]
        .as_array()
        .expect("overview.outside_a_repository lists the keys without a root")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    let here = scratch("overview-contract-outside");
    let v = overview_json(&overview_in(
        "overview-contract-outside",
        &here,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(keys(&v), outside, "{v}");
    for p in [&d, &user, &here] {
        let _ = std::fs::remove_dir_all(p);
    }
    let _ = std::fs::remove_dir_all(scratch_path("overview-contract-gh"));
}

// ---------------------------------------------------------------------------
// bare `trellis` at a terminal (ADR 0050 §3), Unix only
// ---------------------------------------------------------------------------
//
// Every other test here runs the binary on pipes, which is the one case bare
// `trellis` never routes to a terminal surface. util-linux `script` gives it a
// pseudo-terminal instead, and a shell redirection inside the command takes
// one stream back off it. Windows has no std route to a ConPTY and a pty crate
// needs an ADR, so there `bare_route`'s unit tests are the coverage (#556).

/// Runs `'<trellis>'<redirect>` under `script -qfec … /dev/null` in `cwd`,
/// feeding `input` to the terminal, and returns the exit status (`-e` passes
/// trellis's own through) with what the terminal showed.
///
/// The user level is a fresh directory, so the start screen's list is empty
/// whoever runs the suite. A deadline rather than a bare wait: a mutant that
/// opens the wrong surface fails here in seconds instead of holding the
/// mutants job until its per-mutant limit.
#[cfg(unix)]
fn under_pty(name: &str, cwd: &Path, redirect: &str, input: &str) -> (bool, String) {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_trellis"));
    under_pty_as(&exe, name, cwd, redirect, input)
}

/// [`under_pty`] running `exe`, followed by `after` (arguments or a
/// redirection). `DISPLAY` is set, so whether bare `trellis` opens the
/// desktop turns on what is beside `exe` alone, never on the machine
/// running the suite (#561).
#[cfg(unix)]
fn under_pty_as(exe: &Path, name: &str, cwd: &Path, after: &str, input: &str) -> (bool, String) {
    use std::io::Write as _;
    use std::process::Stdio;
    let user = scratch(&format!("{name}-user"));
    let shown = user.join("terminal.txt");
    let cmd = format!("'{}'{after}", exe.display());
    let mut child = Command::new("script")
        .args(["-qfec", &cmd, "/dev/null"])
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("DISPLAY", ":0")
        // Plain line input: no editor escape sequences to read around.
        .env("TERM", "dumb")
        .env("SHELL", "/bin/sh")
        .env("TRELLIS_CONFIG_DIR", &user)
        .stdin(Stdio::piped())
        .stdout(std::fs::File::create(&shown).unwrap())
        .stderr(Stdio::null())
        .spawn()
        .expect("util-linux `script` must be on PATH for the pty tests");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`{cmd}` did not exit within 60 s: {}",
                std::fs::read_to_string(&shown).unwrap_or_default()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let s = std::fs::read_to_string(&shown).unwrap();
    let _ = std::fs::remove_dir_all(&user);
    (status.success(), s)
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_trellis_outside_a_repository_asks_where_to_go() {
    // The start screen, not the refusal: with a person at a terminal, bare
    // `trellis` outside a repository offers somewhere to go, and `q` leaves.
    let d = scratch("pty-no-repo");
    let (ok, s) = under_pty("pty-no-repo", &d, "", "q\n");
    assert!(ok, "quitting the start screen is not a failure: {s}");
    assert!(s.contains("is not inside a git repository"), "{s}");
    assert!(!s.contains("no git repository found"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_trellis_with_stderr_redirected_refuses() {
    // The start screen prompts on stderr. Sent nowhere, the prompt would
    // wait for an answer to a question nobody saw, so this is the refusal.
    let d = scratch("pty-no-stderr");
    let (ok, s) = under_pty("pty-no-stderr", &d, " 2>/dev/null", "q\n");
    assert!(!ok, "bare trellis with no one to ask must refuse: {s}");
    assert!(!s.contains("is not inside a git repository"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_trellis_with_stdout_redirected_in_a_repository_opens_the_session() {
    // stdin is a terminal and stdout a file: the full-screen interface
    // cannot draw, so the line session runs and its banner lands in the
    // file.
    let d = repo("pty-session");
    let out = scratch("pty-session-out").join("stdout.txt");
    let (ok, s) = under_pty(
        "pty-session",
        &d,
        &format!(" >'{}'", out.display()),
        "/exit\n",
    );
    assert!(ok, "{s}");
    let banner = std::fs::read_to_string(&out).unwrap();
    assert!(banner.contains("TRELLIS  ·"), "{banner}\n{s}");
    assert!(banner.contains("/exit to leave"), "{banner}\n{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(out.parent().unwrap());
}

// ---------------------------------------------------------------------------
// bare `trellis` opens the desktop (ADR 0050 §1, as built 2026-10-07, #561)
// ---------------------------------------------------------------------------

/// A copy of the built `trellis` alone in a fresh directory: nothing is
/// beside it, whatever was built beside the original.
#[cfg(unix)]
fn trellis_copy(name: &str) -> PathBuf {
    let exe = scratch(name).join("trellis");
    std::fs::copy(env!("CARGO_BIN_EXE_trellis"), &exe).unwrap();
    wait_until_executable(&exe);
    exe
}

/// [`trellis_copy`] with a stub `trellis-desktop` beside it that writes its
/// arguments, one per line, to `args.txt` there (through a rename, so a
/// reader never sees half of it). A copy, so no stub ever sits beside the
/// build's own binary.
#[cfg(unix)]
fn trellis_with_stub_desktop(name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let exe = trellis_copy(name);
    let stub = exe.parent().unwrap().join("trellis-desktop");
    std::fs::write(
        &stub,
        "#!/bin/sh\nd=$(dirname \"$0\")\n\
         printf '%s\n' \"$@\" > \"$d/args.tmp\" && mv \"$d/args.tmp\" \"$d/args.txt\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    // The probe runs the stub, which records `--version` as its arguments;
    // removed so the test reads only what trellis passed.
    wait_until_executable(&stub);
    let _ = std::fs::remove_file(exe.parent().unwrap().join("args.txt"));
    exe
}

/// The stub's arguments, or `None` when it has not written them within
/// `wait`: it runs detached, so trellis may exit before it writes.
#[cfg(unix)]
fn stub_args(exe: &Path, wait: std::time::Duration) -> Option<Vec<String>> {
    let file = exe.parent().unwrap().join("args.txt");
    let start = std::time::Instant::now();
    loop {
        if let Ok(t) = std::fs::read_to_string(&file) {
            return Some(t.lines().map(str::to_string).collect());
        }
        if start.elapsed() > wait {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Whether two paths name the same file, whichever way each was spelled.
#[cfg(unix)]
fn same_file(a: &str, b: &Path) -> bool {
    Path::new(a).canonicalize().ok() == b.canonicalize().ok()
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_opens_overview() {
    // The owner's goal (ADR 0050 §1, 2026-10-07): bare `trellis` in a
    // repository at a terminal starts the desktop on that repository,
    // prints the overview and exits 0, leaving the terminal free.
    let d = repo("pty-bare-desktop");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    // A model configured, so first-run's picker does not run first.
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[model]\nspec = \"llamacpp:http://127.0.0.1:1\"\n",
    )
    .unwrap();
    let exe = trellis_with_stub_desktop("pty-bare-desktop-bin");
    let (ok, s) = under_pty_as(&exe, "pty-bare-desktop", &d, "", "");
    assert!(ok, "{s}");
    assert!(s.contains("recent repositories:"), "{s}");
    assert!(s.contains("repository: "), "{s}");
    assert!(s.contains("last run: no runs yet"), "{s}");
    assert!(!s.contains("falling back"), "{s}");
    let args = stub_args(&exe, std::time::Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the stub desktop never ran: {s}"));
    assert_eq!(args.len(), 4, "{args:?}");
    assert_eq!(args[0], "--trellis");
    assert!(same_file(&args[1], &exe), "{args:?}");
    assert_eq!(args[2], "--root");
    assert!(same_file(&args[3], &d), "{args:?}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_outside_a_repository_opens_the_desktop_with_no_root() {
    // With a desktop to open, the start screen is skipped: the desktop
    // shows its own, and is given no root it did not find.
    let d = scratch("pty-bare-desktop-no-repo");
    let exe = trellis_with_stub_desktop("pty-bare-desktop-no-repo-bin");
    let (ok, s) = under_pty_as(&exe, "pty-bare-desktop-no-repo", &d, "", "");
    assert!(ok, "{s}");
    assert!(s.contains("so only the recent list is shown"), "{s}");
    assert!(!s.contains("or q to quit"), "{s}");
    let args = stub_args(&exe, std::time::Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the stub desktop never ran: {s}"));
    assert_eq!(args.len(), 2, "{args:?}");
    assert_eq!(args[0], "--trellis");
    assert!(same_file(&args[1], &exe), "{args:?}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_with_no_desktop_says_so_then_falls_back() {
    // No desktop beside the binary: one line saying where it looked and
    // what to install, the overview, then today's start screen. A copy, so
    // a desktop built beside the original cannot open a window here.
    let d = scratch("pty-bare-no-desktop");
    let exe = trellis_copy("pty-bare-no-desktop-bin");
    let (ok, s) = under_pty_as(&exe, "pty-bare-no-desktop", &d, "", "q\n");
    assert!(ok, "{s}");
    let line = s
        .find("the desktop is not installed beside this trellis")
        .unwrap_or_else(|| panic!("no fallback line: {s}"));
    let overview = s.find("recent repositories:").expect(&s);
    let screen = s.find("or q to quit").expect(&s);
    assert!(line < overview && overview < screen, "{s}");
    assert!(s.contains("cargo install --locked --path desktop"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn launch_route_tui_flag_keeps_the_terminal() {
    // The risk: `--tui`, the way back to the terminal interface, opening
    // the desktop anyway. Outside a repository it is today's start screen,
    // with no overview and no desktop started.
    let d = scratch("pty-tui-flag");
    let exe = trellis_with_stub_desktop("pty-tui-flag-bin");
    let (ok, s) = under_pty_as(&exe, "pty-tui-flag", &d, " --tui", "q\n");
    assert!(ok, "{s}");
    assert!(s.contains("or q to quit"), "{s}");
    assert!(!s.contains("recent repositories:"), "{s}");
    assert!(stub_args(&exe, std::time::Duration::from_secs(2)).is_none());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

// ---------------------------------------------------------------------------
// rpc's client contract: the fields the desktop's four views read (#559)
// ---------------------------------------------------------------------------

/// Every record `trellis [--root <root>] rpc` writes for `commands`, run in
/// `cwd`, with stdin closed after the last one. Bounded: a command that
/// never answers (a walk blocked on a file it should not have opened) fails
/// the test instead of hanging it.
fn rpc_records(
    cwd: &Path,
    root: Option<&Path>,
    commands: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    rpc_records_env(cwd, root, &[], commands)
}

/// [`rpc_records`], with `env` set on the child.
fn rpc_records_env(
    cwd: &Path,
    root: Option<&Path>,
    env: &[(&str, &Path)],
    commands: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use std::io::{BufRead, Write};
    use std::process::Stdio;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_trellis"));
    if let Some(r) = root {
        cmd.arg("--root").arg(r);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .arg("rpc")
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary must be built before its integration tests run");
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        std::io::BufReader::new(stdout)
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(&l.unwrap()).expect("stdout is JSONL")
            })
            .collect::<Vec<_>>()
    });
    {
        let mut stdin = child.stdin.take().unwrap();
        for c in commands {
            writeln!(stdin, "{c}").unwrap();
        }
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "rpc exited {status}");
            break;
        }
        if std::time::Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rpc did not answer {commands:?} within 60 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    reader.join().unwrap()
}

/// The response to the command sent with `id`.
fn rpc_answer<'a>(records: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    records
        .iter()
        .find(|r| r["type"] == "response" && r["id"] == id)
        .unwrap_or_else(|| panic!("no response to {id}: {records:?}"))
}

/// Whether `record` carries `path` as the contract writes one: dotted keys,
/// `[]` meaning every element of an array. Unlike `contract_carries`, an
/// empty array carries every path under it, since a module that calls
/// nothing has no edge to carry fields; the tests below show the elements
/// exist where they assert on them.
fn carries_each(record: &serde_json::Value, path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    let (key, each) = match head.strip_suffix("[]") {
        Some(k) => (k, true),
        None => (head, false),
    };
    let Some(v) = record.get(key) else {
        return false;
    };
    match (each, rest) {
        (false, None) => true,
        (false, Some(r)) => carries_each(v, r),
        (true, r) => v
            .as_array()
            .is_some_and(|a| a.iter().all(|e| r.is_none_or(|r| carries_each(e, r)))),
    }
}

/// The paths under one of the contract's lists.
fn contract_paths(list: &serde_json::Value) -> Vec<&str> {
    list.as_array()
        .unwrap_or_else(|| panic!("a contract list, not {list}"))
        .iter()
        .map(|p| p.as_str().expect("a path"))
        .collect()
}

fn client_contract() -> serde_json::Value {
    serde_json::from_str(include_str!("client_contract.json")).expect("the contract is JSON")
}

#[test]
fn rpc_contract_get_codebase_modules_carry_their_path_relative_to_the_root() {
    // The risk: the desktop's graph view (ADR 0048 §10, amended for #557)
    // opens a module's file from `path`, and a module named only by its
    // dotted name leaves it to guess `app/core.py` from `app.core`, which is
    // wrong for every package `__init__.py` and every namespace layout.
    let d = repo("rpc-contract-codebase");
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "g", "type": "get_codebase"})],
    );
    let r = rpc_answer(&records, "g");
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    let modules = data["modules"].as_array().expect("modules");
    assert!(!modules.is_empty(), "{data}");
    for m in modules {
        let p = m["path"].as_str().unwrap_or_else(|| panic!("no path: {m}"));
        assert!(!Path::new(p).is_absolute() && !p.contains('\\'), "{m}");
        assert!(d.join(p).is_file(), "{p} is not a file under the root: {m}");
    }
    let core = modules
        .iter()
        .find(|m| m["module"] == "app.core")
        .unwrap_or_else(|| panic!("app.core: {data}"));
    assert_eq!(core["path"], "app/core.py", "{core}");
    let init = modules
        .iter()
        .find(|m| m["module"] == "app")
        .unwrap_or_else(|| panic!("app: {data}"));
    assert_eq!(init["path"], "app/__init__.py", "{init}");
    assert!(
        modules
            .iter()
            .any(|m| m["calls"].as_array().is_some_and(|c| !c.is_empty())),
        "the fixture's cross-module call is drawn: {data}"
    );

    let c = client_contract();
    let always = contract_paths(&c["rpc"]["responses"]["get_codebase"]["always"]);
    assert!(always.contains(&"modules[].path"), "{always:?}");
    let gaps: Vec<&&str> = always.iter().filter(|p| !carries_each(data, p)).collect();
    assert!(
        gaps.is_empty(),
        "get_codebase lacks what tests/client_contract.json lists: {gaps:?}\n{data}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_get_queue_tasks_carry_blockers_and_parent() {
    // The risk: the desktop's board (ADR 0048 §10, amended for #557) draws
    // which task waits on which, and with neither field on the wire it can
    // only parse `queue list`'s `[blocked by ...]` text, or draw no chains.
    // Both are always present, so a client can tell "none" from "not sent".
    let d = repo("rpc-contract-queue");
    let added = |args: &[&str]| -> String {
        let o = run(&d, args);
        assert!(o.status.success(), "{}", combined(&o));
        let s = stdout(&o);
        let line = s.lines().find(|l| l.starts_with("queued:")).expect(&s);
        line.split_whitespace().nth(1).unwrap().to_string()
    };
    let a = added(&["queue", "add", "fix the parser"]);
    let b = added(&["queue", "add", "write the docs"]);
    let child = added(&[
        "queue",
        "add",
        "then ship",
        "--parent",
        &a,
        "--blocked-by",
        &a,
        "--blocked-by",
        &b,
    ]);
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "q", "type": "get_queue"})],
    );
    let r = rpc_answer(&records, "q");
    assert_eq!(r["success"], true, "{r}");
    let tasks = r["data"]["tasks"].as_array().expect("tasks");
    let task = |id: &str| {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("{id}: {tasks:?}"))
    };
    let t = task(&child);
    assert_eq!(t["parent"], a.as_str(), "{t}");
    assert_eq!(t["blockers"], serde_json::json!([a, b]), "{t}");
    for id in [&a, &b] {
        let t = task(id);
        assert_eq!(t.get("parent"), Some(&serde_json::Value::Null), "{t}");
        assert_eq!(t["blockers"], serde_json::json!([]), "{t}");
    }

    let c = client_contract();
    let entry = &c["rpc"]["responses"]["get_queue"];
    let listed: Vec<&str> = contract_paths(&entry["always"])
        .into_iter()
        .chain(contract_paths(&entry["readable_only"]))
        .collect();
    for f in ["tasks[].blockers", "tasks[].parent"] {
        assert!(listed.contains(&f), "{entry}");
    }
    let gaps: Vec<&&str> = listed
        .iter()
        .filter(|p| !carries_each(&r["data"], p))
        .collect();
    assert!(
        gaps.is_empty(),
        "get_queue lacks what tests/client_contract.json lists: {gaps:?}\n{r}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_get_codebase_and_get_radius_refuse_a_root_with_no_git_and_index_nothing() {
    // The risk: `trellis --root ~ rpc` from the desktop's start screen, then
    // `get_codebase`, walking the whole home folder (25,147 files the last
    // time an index did it) and answering with a graph of it. The refusal is
    // answered on the reader thread, so it precedes the `get_state` sent
    // after it; that no indexing thread starts at all is held by
    // `rpc_contract_refusal_starts_no_indexing_thread` in `src/rpc.rs`.
    let d = scratch("rpc-contract-no-git");
    std::fs::write(d.join("loose.py"), "def f():\n    return g()\n").unwrap();
    let records = rpc_records(
        &d,
        Some(&d),
        &[
            serde_json::json!({"id": "g", "type": "get_codebase"}),
            serde_json::json!({"id": "r", "type": "get_radius", "symbol": "f"}),
            serde_json::json!({"id": "s", "type": "get_state"}),
        ],
    );
    let at = |id: &str| {
        records
            .iter()
            .position(|r| r["type"] == "response" && r["id"] == id)
            .unwrap_or_else(|| panic!("no response to {id}: {records:?}"))
    };
    assert!(at("g") < at("s") && at("r") < at("s"), "{records:?}");
    for (id, command) in [("g", "get_codebase"), ("r", "get_radius")] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["command"], command, "{r}");
        assert_eq!(r["success"], false, "{r}");
        assert!(r.get("data").is_none(), "{r}");
        let e = r["error"].as_str().unwrap_or_else(|| panic!("{r}"));
        assert!(e.contains(command), "names the command: {e}");
        assert!(e.contains("no .git"), "names the rule: {e}");
        assert!(e.contains(&d.display().to_string()), "names the root: {e}");
    }
    assert!(
        !d.join(".trellis").exists(),
        "nothing was written under the root"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_lists_every_field_get_state_answers() {
    // The risk: the desktop's run view and the TUI's status bar read
    // `get_state` fields the contract never promised, so a rename passes
    // every contract test. Both directions: each field answered is listed,
    // and each field listed is answered.
    let d = repo("rpc-contract-state");
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "s", "type": "get_state"})],
    );
    let r = rpc_answer(&records, "s");
    assert_eq!(r["success"], true, "{r}");
    let answered: std::collections::BTreeSet<&str> = r["data"]
        .as_object()
        .expect("get_state's data is an object")
        .keys()
        .map(String::as_str)
        .collect();
    let c = client_contract();
    let listed: std::collections::BTreeSet<&str> =
        contract_paths(&c["rpc"]["commands"]["get_state"]["data"]["always"])
            .into_iter()
            .collect();
    assert_eq!(
        answered, listed,
        "get_state's fields and tests/client_contract.json's get_state.data.always differ"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// rpc for the desktop's Atlas section, the owner's request of 2026-10-11:
// "the current view is nice in the trellis atlas, could you add that to the
// current view without removing the other ones"
// ---------------------------------------------------------------------------

/// Every file under `dir`, `.git` included, by relative path with its
/// bytes: the before and after of a read that must write nothing.
fn bytes_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(p.strip_prefix(dir).unwrap().to_path_buf(), bytes);
            }
        }
    }
    out
}

/// `status --porcelain` with no optional lock, so the look itself writes
/// no refreshed index.
fn porcelain_unlocked(root: &Path) -> String {
    git(root, &["--no-optional-locks", "status", "--porcelain"])
}

/// `git <args>` in `root` with the committer date set, through
/// `trellis_vcs::command()`.
fn git_dated(root: &Path, date: &str, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The committer date of `v1` in [`atlas_repo`], in a zone other than UTC.
const V1_DATE: &str = "2024-06-01T00:00:00+02:00";

/// A repository with two commits. The first, tagged `v1` (annotated),
/// holds `app/old.py`; the second deletes it and adds `app/new.py`. In
/// both, `hub.py` calls into twelve modules, two more than `get_codebase`
/// draws per module by default. Returns the root and `v1`'s commit.
fn atlas_repo(name: &str) -> (PathBuf, String) {
    let d = empty_repo(name);
    let mut hub = String::new();
    let mut body = String::from("\ndef hub():\n    return 0");
    for i in 0..12 {
        std::fs::write(
            d.join(format!("m{i}.py")),
            format!("def f{i}():\n    return {i}\n"),
        )
        .unwrap();
        hub.push_str(&format!("from m{i} import f{i}\n"));
        body.push_str(&format!(" + f{i}()"));
    }
    std::fs::write(d.join("hub.py"), format!("{hub}{body}\n")).unwrap();
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(d.join("app/core.py"), "def greet(who):\n    return who\n").unwrap();
    std::fs::write(
        d.join("app/old.py"),
        "from app.core import greet\n\ndef legacy():\n    return greet('old')\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git_dated(&d, V1_DATE, &["commit", "-qm", "first"]);
    git(&d, &["tag", "-a", "v1", "-m", "release"]);
    let v1 = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    git(&d, &["rm", "-q", "app/old.py"]);
    std::fs::write(
        d.join("app/new.py"),
        "from app.core import greet\n\ndef modern():\n    return greet('new')\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git_dated(
        &d,
        "2024-07-01T12:00:00+00:00",
        &["commit", "-qm", "second"],
    );
    (d, v1)
}

/// The module named `name` in a `get_codebase` answer's `data`.
fn module_in<'a>(data: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    data["modules"]
        .as_array()
        .unwrap_or_else(|| panic!("modules: {data}"))
        .iter()
        .find(|m| m["module"] == name)
}

#[test]
fn rpc_atlas_get_codebase_edges_all_lists_every_module_edge() {
    // The risk: the Atlas drawing a module's edges from a list cut at ten,
    // so a hub module looks coupled to ten modules when it calls twelve,
    // and package edges summed from it undercount. Without the argument
    // the answer stays what it was (#292).
    let (d, _) = atlas_repo("rpc-atlas-edges");
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "d", "type": "get_codebase", "limit": 1000}),
            serde_json::json!({"id": "a", "type": "get_codebase", "limit": 1000, "edges": "all"}),
        ],
    );
    let drawn = rpc_answer(&records, "d");
    let all = rpc_answer(&records, "a");
    assert_eq!(drawn["success"], true, "{drawn}");
    assert_eq!(all["success"], true, "{all}");
    let (drawn, all) = (&drawn["data"], &all["data"]);
    let hub = module_in(drawn, "hub").unwrap();
    assert_eq!(hub["calls"].as_array().unwrap().len(), 10, "{hub}");
    assert_eq!(hub["calls_not_drawn"], 2, "{hub}");
    let hub = module_in(all, "hub").unwrap();
    assert_eq!(hub["calls"].as_array().unwrap().len(), 12, "{hub}");
    let mut sum = 0;
    for m in all["modules"].as_array().unwrap() {
        assert_eq!(m["calls_not_drawn"], 0, "{m}");
        for c in m["calls"].as_array().unwrap() {
            sum += c["call_sites"].as_u64().unwrap();
        }
    }
    assert_eq!(serde_json::json!(sum), all["bound_call_sites"], "{all}");
    for k in [
        "modules_indexed",
        "bound_call_sites",
        "ambiguous",
        "unresolved",
        "modules_not_drawn",
        "commit",
        "date",
    ] {
        assert_eq!(drawn[k], all[k], "{k}");
    }
    let names = |v: &serde_json::Value| -> Vec<String> {
        v["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["module"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(drawn),
        names(all),
        "the same modules in the same order"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_ref_answers_the_codebase_at_that_commit_and_writes_nothing() {
    // The risks: a snapshot indexed from the worktree rather than the
    // commit; one taken by a checkout, which rewrites the repository's
    // index, or a `worktree add`, which writes `.git/worktrees`; a copy
    // left behind in the temporary directory; and a date in this
    // machine's zone rather than the commit's.
    let (d, v1) = atlas_repo("rpc-atlas-ref");
    let tmp = scratch("rpc-atlas-ref-tmp");
    // A status first, so an index refresh it might make is not counted.
    assert_eq!(porcelain_unlocked(&d), "");
    let before = bytes_under(&d);
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    // trellis answers UTC as `+00:00` whichever spelling this machine's git
    // uses (some write `Z`), so the expectation is put in that form too.
    let head_date = git(&d, &["log", "-1", "--format=%cI", "HEAD"]);
    let head_date = match head_date.trim().strip_suffix('Z') {
        Some(rest) => format!("{rest}+00:00"),
        None => head_date.trim().to_string(),
    };
    let records = rpc_records_env(
        &d,
        None,
        &[("TMPDIR", &tmp), ("TMP", &tmp), ("TEMP", &tmp)],
        &[
            serde_json::json!({"id": "r", "type": "get_codebase", "limit": 1000, "ref": "v1"}),
            serde_json::json!({"id": "c", "type": "get_codebase", "limit": 1000, "ref": v1}),
            serde_json::json!({"id": "h", "type": "get_codebase", "limit": 1000}),
        ],
    );
    assert_eq!(bytes_under(&d), before, "the repository changed");
    assert_eq!(porcelain_unlocked(&d), "");
    let left: Vec<_> = std::fs::read_dir(&tmp).unwrap().collect();
    assert!(left.is_empty(), "left in the temporary directory: {left:?}");

    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], true, "{r}");
    let at = &r["data"];
    assert_eq!(at["ref"], "v1", "{at}");
    assert_eq!(at["commit"], v1.as_str(), "the commit, not the tag object");
    assert_eq!(at["date"], V1_DATE, "{at}");
    let old = module_in(at, "app.old").unwrap_or_else(|| panic!("app.old at v1: {at}"));
    assert_eq!(old["path"], "app/old.py", "{old}");
    assert!(module_in(at, "app.new").is_none(), "{at}");
    for m in at["modules"].as_array().unwrap() {
        let p = m["path"].as_str().unwrap();
        assert!(!Path::new(p).is_absolute() && !p.contains('\\'), "{m}");
        git(&d, &["cat-file", "-e", &format!("{v1}:{p}")]);
    }
    let core = module_in(at, "app.old").unwrap()["calls"].clone();
    assert_eq!(
        core,
        serde_json::json!([{"module": "app.core", "call_sites": 1}]),
        "indexed, not only listed"
    );
    let by_id = rpc_answer(&records, "c");
    assert_eq!(by_id["data"]["commit"], v1.as_str(), "{by_id}");
    assert_eq!(by_id["data"]["modules"], at["modules"], "{by_id}");

    let h = rpc_answer(&records, "h");
    let now = &h["data"];
    assert!(now.get("ref").is_none(), "no ref was asked for: {now}");
    assert_eq!(now["commit"], head.as_str(), "{now}");
    assert_eq!(now["date"], head_date.as_str(), "{now}");
    assert!(module_in(now, "app.new").is_some(), "{now}");
    assert!(module_in(now, "app.old").is_none(), "{now}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rpc_atlas_get_codebase_refuses_an_option_an_unknown_ref_and_edges_other_than_all() {
    // The risks: a client's `ref` read by git as an option (`--output`
    // writes a file); an unknown ref answered as HEAD's codebase, which
    // the Atlas would draw as that snapshot; an argument it does not know
    // taken as the default.
    let (d, _) = atlas_repo("rpc-atlas-refuse");
    let tree = git(&d, &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    let cases = [
        (serde_json::json!("-n"), "`-n`"),
        (serde_json::json!("--output=out.txt"), "`--output=out.txt`"),
        (serde_json::json!("no-such-ref"), "`no-such-ref`"),
        (serde_json::json!(tree), tree.as_str()),
        (serde_json::json!(""), "`ref`"),
        (serde_json::json!("v1\nHEAD"), "`ref`"),
        (serde_json::json!(7), "`ref`"),
    ];
    let mut commands: Vec<serde_json::Value> = cases
        .iter()
        .enumerate()
        .map(|(i, (r, _))| serde_json::json!({"id": format!("r{i}"), "type": "get_codebase", "ref": r}))
        .collect();
    commands.push(serde_json::json!({"id": "e1", "type": "get_codebase", "edges": "some"}));
    commands.push(serde_json::json!({"id": "e2", "type": "get_codebase", "edges": true}));
    let before = bytes_under(&d);
    let records = rpc_records(&d, None, &commands);
    assert_eq!(bytes_under(&d), before, "a refusal wrote something");
    for (i, (_, named)) in cases.iter().enumerate() {
        let r = rpc_answer(&records, &format!("r{i}"));
        assert_eq!(r["command"], "get_codebase", "{r}");
        assert_eq!(r["success"], false, "{r}");
        assert!(r.get("data").is_none(), "{r}");
        let e = r["error"].as_str().unwrap();
        assert!(e.contains(named), "names {named}: {e}");
    }
    for id in ["e1", "e2"] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains("`edges`"), "{r}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_with_no_commit_or_no_git_says_so() {
    // The risks: a repository with no commit yet answered with an invented
    // commit, or refused outright, when its worktree can still be drawn;
    // and `ref` opening a way past #559's refusal of a root with no `.git`.
    let d = empty_repo("rpc-atlas-unborn");
    std::fs::write(d.join("a.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "g", "type": "get_codebase"}),
            serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"}),
        ],
    );
    let g = rpc_answer(&records, "g");
    assert_eq!(g["success"], true, "{g}");
    assert_eq!(g["data"]["commit"], serde_json::Value::Null, "{g}");
    assert_eq!(g["data"]["date"], serde_json::Value::Null, "{g}");
    assert!(module_in(&g["data"], "a").is_some(), "{g}");
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("`HEAD`"), "{r}");

    let bare = scratch("rpc-atlas-no-git");
    std::fs::write(bare.join("loose.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"})],
    );
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("no .git"), "{r}");
    assert!(!bare.join(".trellis").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

/// A repository whose `app/shapes.py` has a class hierarchy (so a call
/// through a declared base is ambiguous between two overrides), a helper
/// called from two places in it, and `app/use.py` calling the helper from
/// another module.
fn symbols_repo(name: &str) -> PathBuf {
    let d = empty_repo(name);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/shapes.py"),
        concat!(
            "class Base:\n",
            "    def run(self):\n",
            "        return helper()\n",
            "\n",
            "\n",
            "class Child(Base):\n",
            "    def run(self):\n",
            "        return 2\n",
            "\n",
            "\n",
            "def helper():\n",
            "    return 1\n",
            "\n",
            "\n",
            "def go(b: Base):\n",
            "    return b.run() + helper() + b.missing()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        "from app.shapes import helper\n\n\ndef use():\n    return helper()\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "symbols"]);
    d
}

#[test]
fn rpc_atlas_get_symbols_lists_each_definition_with_its_lines_and_call_site_counts() {
    // The risks: the Atlas's near zoom reading symbol lines from a second
    // parser that disagrees with the index (the prototype used Python's
    // ast); a call through a declared base counted as bound to one
    // override; a count folded into another, or a share of them; and a
    // read that writes the index cache.
    let d = symbols_repo("rpc-atlas-symbols");
    let before = bytes_under(&d);
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "all", "type": "get_symbols"}),
            serde_json::json!({"id": "one", "type": "get_symbols", "module": "app.use"}),
        ],
    );
    assert_eq!(bytes_under(&d), before, "get_symbols wrote something");
    let all = rpc_answer(&records, "all");
    assert_eq!(all["command"], "get_symbols", "{all}");
    assert_eq!(all["success"], true, "{all}");
    let data = &all["data"];
    assert_eq!(data["modules_indexed"], 3, "{data}");
    let row = |module: &str, name: &str, kind: &str, lines: (u64, u64), counts: (u64, u64, u64)| {
        serde_json::json!({
            "module": module,
            "path": format!("{}.py", module.replace('.', "/")),
            "name": name,
            "kind": kind,
            "line": lines.0,
            "end_line": lines.1,
            "calls_in": counts.0,
            "calls_in_ambiguous": counts.1,
            "calls_out": counts.2,
        })
    };
    let want = serde_json::json!([
        row("app.shapes", "Base", "class", (1, 3), (0, 0, 0)),
        row("app.shapes", "run", "method", (2, 3), (0, 1, 1)),
        row("app.shapes", "Child", "class", (6, 8), (0, 0, 0)),
        row("app.shapes", "run", "method", (7, 8), (0, 1, 0)),
        row("app.shapes", "helper", "function", (11, 12), (3, 0, 0)),
        row("app.shapes", "go", "function", (15, 16), (0, 0, 1)),
        row("app.use", "use", "function", (4, 5), (0, 0, 1)),
    ]);
    assert_eq!(data["symbols"], want, "{data}");
    let mut all_keys = Vec::new();
    for s in data["symbols"].as_array().unwrap() {
        all_keys.extend(s.as_object().unwrap().keys().cloned());
    }
    for k in &all_keys {
        for banned in ["ratio", "percent", "fraction", "total", "share"] {
            assert!(!k.contains(banned), "key `{k}`");
        }
    }

    let one = rpc_answer(&records, "one");
    assert_eq!(one["success"], true, "{one}");
    assert_eq!(one["data"]["modules_indexed"], 3, "{one}");
    assert_eq!(
        one["data"]["symbols"],
        serde_json::json!([want[6]]),
        "{one}"
    );

    // Last: `index` writes the cache, which is its job. Every definition
    // it counts is listed, and no other.
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let s = stdout(&o);
    let defs: u64 = s
        .lines()
        .find_map(|l| l.trim().strip_prefix("definitions:"))
        .unwrap_or_else(|| panic!("{s}"))
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        data["symbols"].as_array().unwrap().len() as u64,
        defs,
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_symbols_refuses_an_unknown_module_a_non_string_and_a_root_with_no_git() {
    // The risks: a module the index never saw answered as one with no
    // definitions, which the Atlas would draw as empty; and #559's
    // refusal skipped by a new read, walking a home folder.
    let d = symbols_repo("rpc-atlas-symbols-refuse");
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "u", "type": "get_symbols", "module": "app.nowhere"}),
            serde_json::json!({"id": "n", "type": "get_symbols", "module": 3}),
        ],
    );
    let u = rpc_answer(&records, "u");
    assert_eq!(u["success"], false, "{u}");
    assert!(
        u["error"].as_str().unwrap().contains("`app.nowhere`"),
        "{u}"
    );
    let n = rpc_answer(&records, "n");
    assert_eq!(n["success"], false, "{n}");
    assert!(n["error"].as_str().unwrap().contains("`module`"), "{n}");

    let bare = scratch("rpc-atlas-symbols-no-git");
    std::fs::write(bare.join("loose.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[
            serde_json::json!({"id": "s", "type": "get_symbols"}),
            serde_json::json!({"id": "t", "type": "get_state"}),
        ],
    );
    let at = |id: &str| {
        records
            .iter()
            .position(|r| r["type"] == "response" && r["id"] == id)
            .unwrap()
    };
    assert!(
        at("s") < at("t"),
        "refused on the reader thread: {records:?}"
    );
    let s = rpc_answer(&records, "s");
    assert_eq!(s["success"], false, "{s}");
    let e = s["error"].as_str().unwrap();
    assert!(e.contains("get_symbols") && e.contains("no .git"), "{e}");
    assert!(!bare.join(".trellis").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn rpc_atlas_get_radius_lists_the_symbols_reached_and_the_sites_beside_the_counts() {
    // The risks: the Atlas tinting reached modules from a list trellis
    // never gave (the prototype decoded index.bin for it); an ambiguous
    // site listed as certain; and the lists replacing or changing the
    // counts, which stay whole and separate (ADR 0014).
    let d = empty_repo("rpc-atlas-radius");
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/shapes.py"),
        concat!(
            "class Base:\n",
            "    def area(self):\n",
            "        return 0\n",
            "\n",
            "\n",
            "class Square(Base):\n",
            "    def side(self):\n",
            "        return 1\n",
            "\n",
            "    def area(self):\n",
            "        return self.side() * self.side()\n",
            "\n",
            "\n",
            "def measure(s: Base):\n",
            "    return s.area()\n",
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "shapes"]);
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "r", "type": "get_radius", "symbol": "side"})],
    );
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(
        data["reach"],
        serde_json::json!({"resolved": 2, "ambiguous": 1, "symbols_reached": 2}),
        "{data}"
    );
    assert_eq!(
        data["reached"],
        serde_json::json!([
            {"symbol": "app.shapes.area", "module": "app.shapes", "path": "app/shapes.py",
             "kind": "method", "line": 10},
            {"symbol": "app.shapes.measure", "module": "app.shapes", "path": "app/shapes.py",
             "kind": "function", "line": 14},
        ]),
        "{data}"
    );
    assert_eq!(
        data["sites"],
        serde_json::json!([
            {"path": "app/shapes.py", "line": 11, "certain": true},
            {"path": "app/shapes.py", "line": 11, "certain": true},
            {"path": "app/shapes.py", "line": 15, "certain": false},
        ]),
        "{data}"
    );
    assert_eq!(data["reached_not_listed"], 0, "{data}");
    assert_eq!(data["sites_not_listed"], 0, "{data}");
    let _ = std::fs::remove_dir_all(&d);
}

/// [`history_repo`]'s commits, by the names its comment gives them.
struct History {
    root: PathBuf,
    c: std::collections::BTreeMap<&'static str, String>,
}

/// A repository whose first-parent line is c0 (2023-06-01, tagged `old`),
/// c1 (2024-01-01), c2 (2024-03-01, tagged `v1`, annotated), c3
/// (2025-01-15), c5 (2025-02-10, merging `side`'s c4, tagged `side-tag`)
/// and c6 (2025-02-20, HEAD); `elsewhere` tags a commit HEAD cannot reach.
/// All in UTC but c6, whose offset is +01:00.
fn history_repo(name: &str) -> History {
    let d = empty_repo(name);
    let main = git(&d, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_string();
    let mut c = std::collections::BTreeMap::new();
    let commit = |c: &mut std::collections::BTreeMap<&'static str, String>,
                  id: &'static str,
                  date: &str,
                  subject: &str,
                  files: &[(&str, &str)]| {
        for (p, text) in files {
            std::fs::write(d.join(p), text).unwrap();
        }
        git(&d, &["add", "-A"]);
        git_dated(&d, date, &["commit", "-qm", subject]);
        c.insert(id, git(&d, &["rev-parse", "HEAD"]).trim().to_string());
    };
    commit(
        &mut c,
        "c0",
        "2023-06-01T00:00:00+00:00",
        "start",
        &[
            ("a.py", "A = 0\n"),
            ("b.py", "B = 0\n"),
            ("e.py", "E = 0\n"),
        ],
    );
    git(&d, &["tag", "old"]);
    commit(
        &mut c,
        "c1",
        "2024-01-01T00:00:00+00:00",
        "e once",
        &[("e.py", "E = 1\n")],
    );
    commit(
        &mut c,
        "c2",
        "2024-03-01T00:00:00+00:00",
        "a once",
        &[("a.py", "A = 1\n")],
    );
    git(&d, &["tag", "-a", "v1", "-m", "release"]);
    commit(
        &mut c,
        "c3",
        "2025-01-15T00:00:00+00:00",
        "b and c",
        &[("b.py", "B = 1\n"), ("c.py", "C = 0\n")],
    );
    git(&d, &["checkout", "-qb", "side"]);
    commit(
        &mut c,
        "c4",
        "2025-02-01T00:00:00+00:00",
        "d on a side",
        &[("d.py", "D = 0\n")],
    );
    git(&d, &["tag", "side-tag"]);
    git(&d, &["checkout", "-q", &main]);
    git_dated(
        &d,
        "2025-02-10T00:00:00+00:00",
        &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
    );
    c.insert("c5", git(&d, &["rev-parse", "HEAD"]).trim().to_string());
    commit(
        &mut c,
        "c6",
        "2025-02-20T01:00:00+01:00",
        "a\tagain <b>",
        &[("a.py", "A = 2\n")],
    );
    git(&d, &["checkout", "-qb", "other", &c["c0"]]);
    commit(
        &mut c,
        "c7",
        "2025-03-01T00:00:00+00:00",
        "unmerged",
        &[("z.py", "Z = 0\n")],
    );
    git(&d, &["tag", "elsewhere"]);
    git(&d, &["checkout", "-q", &main]);
    assert_eq!(porcelain_unlocked(&d), "");
    History { root: d, c }
}

#[test]
fn rpc_atlas_get_history_measures_churn_back_from_head_and_lists_commits_and_tags() {
    // The risks: windows measured from the wall clock, so the same
    // repository answers differently tomorrow; a merge's changes missed or
    // a side branch's commits counted on the first-parent line; a tag HEAD
    // cannot reach offered as a snapshot; churn dropping a path changed
    // since the oldest tag but not in the last year; and a read that
    // writes.
    let h = history_repo("rpc-atlas-history");
    let c = |id: &str| h.c[id].as_str();
    let before = bytes_under(&h.root);
    let records = rpc_records(
        &h.root,
        None,
        &[
            serde_json::json!({"id": "d", "type": "get_history"}),
            serde_json::json!({"id": "n", "type": "get_history", "commits": 2, "tags": 2}),
            serde_json::json!({"id": "z", "type": "get_history", "commits": 0, "tags": 0}),
        ],
    );
    assert_eq!(bytes_under(&h.root), before, "get_history wrote something");
    let d = rpc_answer(&records, "d");
    assert_eq!(d["command"], "get_history", "{d}");
    assert_eq!(d["success"], true, "{d}");
    let data = &d["data"];
    assert_eq!(
        data["head"],
        serde_json::json!({"commit": c("c6"), "date": "2025-02-20T01:00:00+01:00"}),
        "{data}"
    );
    assert_eq!(data["windows_days"], serde_json::json!([30, 90, 365]));
    assert_eq!(
        data["tags"],
        serde_json::json!([
            {"name": "side-tag", "commit": c("c4"), "date": "2025-02-01T00:00:00+00:00"},
            {"name": "v1", "commit": c("c2"), "date": "2024-03-01T00:00:00+00:00"},
            {"name": "old", "commit": c("c0"), "date": "2023-06-01T00:00:00+00:00"},
        ]),
        "{data}"
    );
    let logged = |id: &str, date: &str, subject: &str, paths: &[&str]| serde_json::json!({"commit": c(id), "date": date, "subject": subject, "paths": paths});
    assert_eq!(
        data["commits"],
        serde_json::json!([
            logged("c6", "2025-02-20T01:00:00+01:00", "a\tagain <b>", &["a.py"]),
            logged("c5", "2025-02-10T00:00:00+00:00", "merge side", &["d.py"]),
            logged(
                "c3",
                "2025-01-15T00:00:00+00:00",
                "b and c",
                &["b.py", "c.py"]
            ),
            logged("c2", "2024-03-01T00:00:00+00:00", "a once", &["a.py"]),
            logged("c1", "2024-01-01T00:00:00+00:00", "e once", &["e.py"]),
            logged(
                "c0",
                "2023-06-01T00:00:00+00:00",
                "start",
                &["a.py", "b.py", "e.py"]
            ),
        ]),
        "{data}"
    );
    let row = |path: &str, d: [u64; 3], commits: serde_json::Value, last: &str, date: &str| {
        serde_json::json!({
            "path": path, "d30": d[0], "d90": d[1], "d365": d[2], "commits": commits,
            "last_commit": c(last), "last_date": date,
        })
    };
    let n = |v: u64| serde_json::json!(v);
    // 30 days back from c6 reaches c5; 90 reaches c3; 365 reaches c2; the
    // oldest tag, `old` at c0, puts c1 in range too, so e.py is a row.
    assert_eq!(
        data["churn"],
        serde_json::json!([
            row("a.py", [1, 1, 2], n(2), "c6", "2025-02-20T01:00:00+01:00"),
            row("b.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("c.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("d.py", [1, 1, 1], n(1), "c5", "2025-02-10T00:00:00+00:00"),
            row("e.py", [0, 0, 0], n(1), "c1", "2024-01-01T00:00:00+00:00"),
        ]),
        "{data}"
    );

    // Two commits and two tags: the oldest returned is v1 (c2), so
    // `commits` counts c6, c5 and c3, and e.py is out of range.
    let s = &rpc_answer(&records, "n")["data"];
    assert_eq!(s["commits"].as_array().unwrap().len(), 2, "{s}");
    assert_eq!(s["commits"][1]["commit"], c("c5"), "{s}");
    assert_eq!(s["tags"].as_array().unwrap().len(), 2, "{s}");
    assert_eq!(
        s["churn"],
        serde_json::json!([
            row("a.py", [1, 1, 2], n(1), "c6", "2025-02-20T01:00:00+01:00"),
            row("b.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("c.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("d.py", [1, 1, 1], n(1), "c5", "2025-02-10T00:00:00+00:00"),
        ]),
        "{s}"
    );

    // No tag asked for: there is no tag to count from, so `commits` is
    // null, never 0, and the year still sets the rows.
    let z = &rpc_answer(&records, "z")["data"];
    assert_eq!(z["commits"], serde_json::json!([]), "{z}");
    assert_eq!(z["tags"], serde_json::json!([]), "{z}");
    let null = serde_json::Value::Null;
    assert_eq!(
        z["churn"],
        serde_json::json!([
            row(
                "a.py",
                [1, 1, 2],
                null.clone(),
                "c6",
                "2025-02-20T01:00:00+01:00"
            ),
            row(
                "b.py",
                [0, 1, 1],
                null.clone(),
                "c3",
                "2025-01-15T00:00:00+00:00"
            ),
            row(
                "c.py",
                [0, 1, 1],
                null.clone(),
                "c3",
                "2025-01-15T00:00:00+00:00"
            ),
            row("d.py", [1, 1, 1], null, "c5", "2025-02-10T00:00:00+00:00"),
        ]),
        "{z}"
    );
    let _ = std::fs::remove_dir_all(&h.root);
}

#[test]
fn rpc_atlas_get_history_of_no_commit_is_empty_and_bad_counts_and_no_git_are_refused() {
    // The risks: a repository with no commit answered with an error the
    // Atlas shows as a failure, when the truth is "no history yet"; a
    // count that is not one taken as the default; and #559's refusal
    // skipped by a new read.
    let d = empty_repo("rpc-atlas-history-unborn");
    std::fs::write(d.join("a.py"), "A = 0\n").unwrap();
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "u", "type": "get_history"}),
            serde_json::json!({"id": "a", "type": "get_history", "commits": "20"}),
            serde_json::json!({"id": "b", "type": "get_history", "tags": -1}),
            serde_json::json!({"id": "c", "type": "get_history", "commits": 2.5}),
        ],
    );
    let u = rpc_answer(&records, "u");
    assert_eq!(u["success"], true, "{u}");
    assert_eq!(
        u["data"],
        serde_json::json!({"head": null, "windows_days": [30, 90, 365],
                           "churn": [], "commits": [], "tags": []}),
        "{u}"
    );
    for (id, field) in [("a", "`commits`"), ("b", "`tags`"), ("c", "`commits`")] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains(field), "{r}");
    }

    let bare = scratch("rpc-atlas-history-no-git");
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[serde_json::json!({"id": "h", "type": "get_history"})],
    );
    let r = rpc_answer(&records, "h");
    assert_eq!(r["success"], false, "{r}");
    let e = r["error"].as_str().unwrap();
    assert!(e.contains("get_history") && e.contains("no .git"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

/// Every dotted path in `v` as the contract writes them: a key, `a.b`
/// under it, and `a[].b` under each element of an array.
fn answered_paths(v: &serde_json::Value, at: &str, out: &mut std::collections::BTreeSet<String>) {
    match v {
        serde_json::Value::Object(o) => {
            for (k, child) in o {
                let p = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                out.insert(p.clone());
                answered_paths(child, &p, out);
            }
        }
        serde_json::Value::Array(a) => {
            for e in a {
                if e.is_object() {
                    answered_paths(e, &format!("{at}[]"), out);
                }
            }
        }
        _ => {}
    }
}

#[test]
fn rpc_atlas_contract_lists_every_field_the_atlas_commands_answer() {
    // The risk (ADR 0048 §6): the desktop's Atlas reads these answers by
    // name, and a field renamed here, or answered and never promised,
    // passes every other test. Both directions: each field the contract
    // lists is answered, and each field answered is listed.
    let (d, _) = atlas_repo("rpc-atlas-contract");
    // An unbound call, so the radius's doubt lists carry entries.
    std::fs::write(
        d.join("app/core.py"),
        "def greet(who):\n    return who.strip()\n",
    )
    .unwrap();
    git(&d, &["commit", "-qam", "doubt"]);
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "g", "type": "get_codebase", "limit": 1000, "edges": "all"}),
            serde_json::json!({"id": "r", "type": "get_codebase", "ref": "v1"}),
            serde_json::json!({"id": "s", "type": "get_symbols"}),
            serde_json::json!({"id": "x", "type": "get_radius", "symbol": "greet"}),
            serde_json::json!({"id": "h", "type": "get_history"}),
        ],
    );
    let c = client_contract();
    let responses = &c["rpc"]["responses"];
    for (id, command, lists) in [
        ("g", "get_codebase", &["always"][..]),
        ("r", "get_codebase", &["always", "ref_only"][..]),
        ("s", "get_symbols", &["always"][..]),
        ("x", "get_radius", &["always"][..]),
        ("h", "get_history", &["always", "head_only"][..]),
    ] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], true, "{r}");
        let data = &r["data"];
        let entry = &responses[command];
        let promised: Vec<&str> = lists
            .iter()
            .flat_map(|l| contract_paths(&entry[*l]))
            .collect();
        let gaps: Vec<&&str> = promised.iter().filter(|p| !carries_each(data, p)).collect();
        assert!(
            gaps.is_empty(),
            "{command} ({id}) lacks what tests/client_contract.json lists: {gaps:?}\n{data}"
        );
        let mut listed: std::collections::BTreeSet<String> = Default::default();
        for (k, l) in entry.as_object().unwrap() {
            if k == "always" || k.ends_with("_only") {
                listed.extend(contract_paths(l).into_iter().map(str::to_string));
            }
        }
        let mut answered = Default::default();
        answered_paths(data, "", &mut answered);
        let unlisted: Vec<&String> = answered.difference(&listed).collect();
        assert!(
            unlisted.is_empty(),
            "{command} ({id}) answers what tests/client_contract.json does not list: {unlisted:?}"
        );
    }
    // The fixture gives every list something to carry, so an element's
    // fields are checked rather than passed by an empty array.
    let x = &rpc_answer(&records, "x")["data"];
    for list in ["reached", "sites", "doubt.reasons", "doubt.files"] {
        let v = list.split('.').fold(x, |v, k| &v[k]);
        assert!(v.as_array().is_some_and(|a| !a.is_empty()), "{list}: {x}");
    }
    let h = &rpc_answer(&records, "h")["data"];
    for list in ["churn", "commits", "tags"] {
        assert!(
            h[list].as_array().is_some_and(|a| !a.is_empty()),
            "{list}: {h}"
        );
    }
    assert!(h["churn"][0]["commits"].is_u64(), "a tag is listed: {h}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_answers_its_modules_when_head_cannot_be_read() {
    // The risk: `commit` and `date`, metadata the Atlas added, failing a
    // read that needs no git: the desktop's own codebase test gives the
    // root a hand-made `.git`, which git refuses, and the index never
    // asks git anything. A git error is said in `head_error`; a
    // repository with no commit yet is not an error, so it has none.
    // Discovery is held to the scratch directory, so a `.git` git refuses
    // is not passed over for a repository above it.
    let ceiling = std::env::temp_dir();
    let env: &[(&str, &Path)] = &[("GIT_CEILING_DIRECTORIES", &ceiling)];
    let refused = scratch("rpc-atlas-head-refused");
    std::fs::create_dir_all(refused.join(".git")).unwrap();
    let unborn = scratch("rpc-atlas-head-unborn");
    for d in [".git/objects", ".git/refs"] {
        std::fs::create_dir_all(unborn.join(d)).unwrap();
    }
    std::fs::write(unborn.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    for root in [&refused, &unborn] {
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/a.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(
            root.join("app/b.py"),
            "from app.a import f\n\ndef g():\n    return f()\n",
        )
        .unwrap();
    }
    let ask = [serde_json::json!({"id": "g", "type": "get_codebase"})];

    let r = rpc_answer(&rpc_records_env(&refused, Some(&refused), env, &ask), "g").clone();
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(data["modules_indexed"], 2, "{data}");
    assert!(module_in(data, "app.b").is_some(), "{data}");
    assert_eq!(data["commit"], serde_json::Value::Null, "{data}");
    assert_eq!(data["date"], serde_json::Value::Null, "{data}");
    let e = data["head_error"]
        .as_str()
        .unwrap_or_else(|| panic!("git's refusal is said: {data}"));
    assert!(e.contains("not a git repository"), "{e}");
    assert_eq!(e, e.trim(), "trimmed");

    let r = rpc_answer(&rpc_records_env(&unborn, Some(&unborn), env, &ask), "g").clone();
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(data["modules_indexed"], 2, "{data}");
    assert_eq!(data["commit"], serde_json::Value::Null, "{data}");
    assert!(
        data.get("head_error").is_none(),
        "no commit is not an error: {data}"
    );

    // A ref still needs git, and a ref git cannot resolve is still refused.
    let r = rpc_records_env(
        &refused,
        Some(&refused),
        env,
        &[serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"})],
    );
    assert_eq!(rpc_answer(&r, "r")["success"], false, "{r:?}");
    let _ = std::fs::remove_dir_all(&refused);
    let _ = std::fs::remove_dir_all(&unborn);
}
