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

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repository holding a two-module Python package.
///
/// Deliberately small but not trivial: `caller.py` imports and calls
/// `core.greet`, so there is a real cross-file edge for `radius` to find, and
/// `mystery()` takes an untyped parameter so there is a real unbound call for
/// the coverage report to be honest about.
fn repo(name: &str) -> PathBuf {
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app/caller.py"),
        concat!(
            "from app.core import greet\n\n",
            "def welcome(who):\n",
            "    return greet(who)\n\n",
            "def mystery(thing):\n",
            "    return thing.run()\n",
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
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
        std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&d)
            .output()
            .unwrap()
            .stdout
            .is_empty(),
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
fn why_reports_absence_rather_than_inventing_a_reason() {
    // `why` answers from the trail. With no trail there is no answer, and the
    // honest output is that there is none — the alternative is a plausible
    // reconstruction, which is the one thing a decision trail exists to stop.
    let d = repo("why-empty");
    let out = run(&d, &["why", "app.core.greet"]);
    let s = combined(&out);
    assert!(
        s.contains("no ") || s.contains("No ") || s.contains("nothing") || s.contains("Nothing"),
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
    let d = repo("doctor-notests");
    let s = stdout(&run(&d, &["doctor"]));
    assert!(
        s.contains("tests:"),
        "doctor must report on the test suite: {s}"
    );
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
