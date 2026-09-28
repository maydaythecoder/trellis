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
    // Every other git fixture in this workspace pins this and these two did
    // not — found by the Pass 6 duplication survey, which is the argument
    // for that pass in one line. Without it git normalises line endings on
    // `add`, and `a_missing_scripted_model_fails_before_anything_is_touched`
    // asserts an empty `status --porcelain` on files it wrote itself.
    git(&d, &["config", "core.autocrlf", "false"]);

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
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&d)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);
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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_trellis"))
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

/// How many diff lines `/diff` shows before collapsing the rest.
///
/// Mirrored rather than imported — `tests/cli.rs` is a separate crate and
/// `DIFF_LINES` is private — and asserted against the shipped behaviour by
/// the two tests below sitting either side of it. If it moves, the boundary
/// test fails rather than silently testing nothing.
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
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(d)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
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
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);

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
    // The only branch of `pull` that can be tested without fetching half a
    // gigabyte, and the one worth the most: it is what stops a re-run of
    // `trellis model pull` from re-downloading a file that is already
    // complete. Inverting the comparison turns the guard into its opposite
    // and every repeat invocation becomes a fresh download.
    //
    // Safe precisely because it returns *before* curl. Nothing else here is
    // — see the ledger entry on why the rest of this function is triaged
    // rather than tested.
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
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&d)
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );

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
