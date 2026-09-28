//! ADR 0026 through the real binary: the task model's commands, agent
//! profiles, budgets, routines, process agents and export/import.
//!
//! The decisions are unit-tested in `trellis-queue`. These check what only
//! exists in the binary: that a profile's settings reach the run, that a
//! trail records who spent what, that a process agent is judged by
//! trellis's own checks, and that none of it dirties the worktree.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-orch-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn trellis(root: &Path, args: &[&str]) -> Output {
    // The trust store is resolved from these (ADR 0023 §4); pointed beside
    // the fixture so `trellis trust` here never writes the developer's own.
    let home = root.with_extension("home");
    Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(root)
        .env("APPDATA", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .expect("the binary must be built before its integration tests run")
}

fn combined(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok(root: &Path, args: &[&str]) -> String {
    let o = trellis(root, args);
    let s = combined(&o);
    assert!(o.status.success(), "trellis {args:?}:\n{s}");
    s
}

/// A repository whose checks always pass (`git --version`), with the
/// scripts `converge.json` (edits and converges) and `stop.json` (stops at
/// once, rolled back), and `extra` files committed alongside.
fn fixture(name: &str, extra: &[(&str, &str)]) -> PathBuf {
    let d = scratch(name);
    git(&d, &["init", "-q"]);
    // CI has no global identity, and every run commits its trail.
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet():\n    return \"hello\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
    )
    .unwrap();
    std::fs::write(
        d.join("converge.json"),
        concat!(
            r#"{"steps":[{"tool":"edit","path":"app/core.py","#,
            r#""old":"return \"hello\"","new":"return \"hi\""},"#,
            r#"{"tool":"run_checks","paths":["app/core.py"],"kinds":["typecheck"]},"#,
            r#"{"tool":"done","summary":"changed"}]}"#,
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("stop.json"),
        r#"{"steps":[{"tool":"done","summary":"nothing to do"}]}"#,
    )
    .unwrap();
    for (path, body) in extra {
        let p = d.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            p,
            body.replace("{ROOT}", &d.display().to_string().replace('\\', "/")),
        )
        .unwrap();
    }
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

fn porcelain(d: &Path) -> String {
    git(d, &["status", "--porcelain"])
}

fn ids(d: &Path, state: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d.join(".git/trellis/queue").join(state))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name()
                        .to_str()?
                        .strip_suffix(".json")
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn task(d: &Path, state: &str, id: &str) -> serde_json::Value {
    let p = d
        .join(".git/trellis/queue")
        .join(state)
        .join(format!("{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn trail(d: &Path, run_id: &str) -> serde_json::Value {
    let p = d.join(".trellis/runs").join(format!("{run_id}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn queued_id(out: &str) -> String {
    out.split_whitespace()
        .skip_while(|w| *w != "queued:")
        .nth(1)
        .expect("`queued: <id>`")
        .to_string()
}

/// `YYYY-MM-DDTHH:MM:SSZ` for now, as trails write it.
fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let r = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z",
        r / 3600,
        r % 3600 / 60,
        r % 60
    )
}

#[test]
fn the_task_model_commands_change_tasks_and_every_change_is_logged() {
    let d = fixture("model", &[]);
    let a = queued_id(&ok(
        &d,
        &[
            "queue",
            "add",
            "gated",
            "--priority",
            "high",
            "--label",
            "api",
            "--needs-approval",
        ],
    ));
    let b = queued_id(&ok(&d, &["queue", "add", "follows", "--blocked-by", &a]));
    let list = ok(&d, &["queue", "list"]);
    assert!(
        list.contains("todo high") && list.contains("[labels api]"),
        "{list}"
    );
    assert!(list.contains("waiting: needs approval"), "{list}");
    assert!(
        list.contains("blocked medium")
            && list.contains("waiting: blocked until its blockers are done"),
        "{list}"
    );
    let filtered = ok(&d, &["queue", "list", "--label", "api"]);
    assert!(
        filtered.contains(&a) && !filtered.contains(&b),
        "{filtered}"
    );

    ok(&d, &["queue", "approve", &a]);
    ok(
        &d,
        &["queue", "set", &b, "--priority", "critical", "--label", "x"],
    );
    ok(&d, &["queue", "note", &b, "remember the edge case"]);
    let o = trellis(&d, &["queue", "set", &b, "--status", "blocked"]);
    assert!(
        !o.status.success() && combined(&o).contains("--reason"),
        "{}",
        combined(&o)
    );
    ok(&d, &["queue", "cancel", &b, "--reason", "superseded"]);
    assert_eq!(ids(&d, "cancelled"), vec![b.clone()]);
    let t = task(&d, "cancelled", &b);
    assert_eq!(t["priority"], "critical");
    assert_eq!(t["notes"].as_array().unwrap().len(), 2);

    let log = ok(&d, &["queue", "log", "--limit", "50"]);
    for action in [
        "task.add",
        "approval.approve",
        "task.set",
        "task.note",
        "task.cancel",
    ] {
        assert!(log.contains(action), "{action} missing:\n{log}");
    }
    assert!(log.contains("human"), "{log}");
    assert_eq!(porcelain(&d), "", "the queue must never dirty the worktree");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_profile_task_runs_with_the_profile_model_and_instructions_and_the_trail_names_the_agent() {
    let d = fixture(
        "profile-run",
        &[
            (
                ".trellis/agents/rev.toml",
                "role = \"reviewer\"\ninstructions = \"agents/rev.md\"\nmodel = \"scripted:{ROOT}/converge.json\"\n",
            ),
            (".trellis/agents/rev.md", "You are careful."),
        ],
    );
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "change the greeting", "--assignee", "rev"],
    ));
    // No --model and no [model] spec: the profile's is what runs.
    let o = trellis(&d, &["work"]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(
        s.contains("agent:    rev") && s.contains("CONVERGED"),
        "{s}"
    );
    let t = task(&d, "done", &id);
    assert_eq!(
        t["status"], "in_review",
        "a converged change awaits a person"
    );
    let tr = trail(&d, t["outcome"]["run_id"].as_str().unwrap());
    assert_eq!(tr["factual"]["agent"], "rev");
    assert!(
        tr["factual"].get("tokens").is_none(),
        "a scripted run has no count"
    );
    assert!(
        tr["task"]
            .as_str()
            .unwrap()
            .starts_with("You are careful.\n\nchange the greeting"),
        "{tr}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_paused_agent_is_skipped_until_resumed() {
    let d = fixture(
        "pause",
        &[(
            ".trellis/agents/rev.toml",
            "model = \"scripted:{ROOT}/stop.json\"\n",
        )],
    );
    ok(&d, &["queue", "add", "x", "--assignee", "rev"]);
    ok(&d, &["agents", "pause", "rev", "--reason", "on leave"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("agent `rev` is paused: on leave"), "{s}");
    assert_eq!(ids(&d, "pending").len(), 1);
    assert!(ok(&d, &["agents", "list"]).contains("paused"));
    ok(&d, &["agents", "resume", "rev"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert_eq!(ids(&d, "failed").len(), 1);
    assert_eq!(porcelain(&d), "", "overlay state lives in the git dir");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_budget_hard_stop_pauses_the_agent_and_an_approved_raise_resumes_it() {
    // Spend comes from trails. A committed trail naming `rev` with 150
    // completion tokens this month puts it over a 100-token monthly limit.
    let spent = format!(
        r#"{{"task":"earlier","started_at":"{}","factual":{{"agent":"rev","tokens":150}}}}"#,
        iso_now()
    );
    let d = fixture(
        "budget",
        &[
            (
                ".trellis/agents/rev.toml",
                "model = \"scripted:{ROOT}/stop.json\"\n[budget]\nmonthly_tokens = 100\n",
            ),
            (".trellis/runs/20260101-000000-earlier.json", &spent),
        ],
    );
    ok(&d, &["queue", "add", "x", "--assignee", "rev"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("reached its monthly limit (150 of 100"), "{s}");
    assert_eq!(ids(&d, "pending").len(), 1, "its task is skipped:\n{s}");

    let list = ok(&d, &["queue", "list"]);
    let req = list
        .split_whitespace()
        .find(|w| w.starts_with("budget-rev-"))
        .unwrap_or_else(|| panic!("a budget request is listed:\n{list}"))
        .to_string();
    // A second tick must not open a second request.
    ok(&d, &["work"]);
    assert_eq!(ok(&d, &["queue", "list"]).matches("budget-rev-").count(), 1);

    let o = trellis(&d, &["queue", "approve", &req]);
    assert!(!o.status.success() && combined(&o).contains("--raise"));
    // A raise too small to cover the spend: resumed, then paused again on
    // the next tick, which is a new pause and so a new request.
    ok(&d, &["queue", "approve", &req, "--raise", "10"]);
    assert!(ok(&d, &["agents", "show", "rev"]).contains("raised"));
    let s = ok(&d, &["work"]);
    assert!(s.contains("reached its monthly limit (150 of 110"), "{s}");
    let list = ok(&d, &["queue", "list"]);
    let req2 = list
        .split_whitespace()
        .find(|w| w.starts_with("budget-rev-"))
        .unwrap_or_else(|| panic!("a new pause, a new request:\n{list}"))
        .to_string();
    assert_ne!(req, req2);

    // A rejection stands: the next tick opens nothing new.
    ok(&d, &["queue", "reject", &req2]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("stays paused: its override was rejected"), "{s}");
    assert!(!ok(&d, &["queue", "list"]).contains("budget-rev-"));
    assert_eq!(ids(&d, "pending").len(), 1);

    // Raising the limit in the profile file lifts the budget pause.
    std::fs::write(
        d.join(".trellis/agents/rev.toml"),
        format!(
            "model = \"scripted:{}/stop.json\"\n[budget]\nmonthly_tokens = 1000\n",
            d.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "raise the budget"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("resumed") && s.contains("NOT CONVERGED"),
        "it ran:\n{s}"
    );
    let log = ok(&d, &["queue", "log", "--limit", "100"]);
    for a in [
        "approval.request",
        "approval.approve",
        "approval.reject",
        "agent.resume",
    ] {
        assert!(log.contains(a), "{a}:\n{log}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn routines_are_listed_fired_by_hand_coalesced_and_armed_by_work() {
    let d = fixture(
        "routines",
        &[(
            ".trellis/routines/nightly.toml",
            "schedule = \"0 3 * * *\"\ntimezone = \"+02:00\"\n[task]\ntext = \"triage {{date}} ({{routine}}) {{area}}\"\n[vars]\narea = \"parser\"\n",
        )],
    );
    let list = ok(&d, &["routines", "list"]);
    assert!(
        list.contains("nightly") && list.contains("0 3 * * *") && list.contains("+02:00"),
        "{list}"
    );
    assert!(list.contains("not armed yet"), "{list}");
    let s = ok(&d, &["routines", "run", "nightly"]);
    assert!(s.contains("queued"), "{s}");
    let s = ok(&d, &["routines", "run", "nightly"]);
    assert!(s.contains("coalesced"), "the default policy:\n{s}");
    assert_eq!(ids(&d, "pending").len(), 1);
    let id = &ids(&d, "pending")[0];
    let t = task(&d, "pending", id);
    assert!(
        t["task"].as_str().unwrap().contains("(nightly) parser"),
        "{t}"
    );
    assert_eq!(t["routine"], "nightly");

    let s = ok(
        &d,
        &[
            "work",
            "--model",
            &format!("scripted:{}", d.join("stop.json").display()),
        ],
    );
    assert!(s.contains("routine:  nightly armed"), "{s}");
    assert!(
        ok(&d, &["routines", "list"]).contains("armed 2"),
        "state is recorded"
    );
    assert_eq!(porcelain(&d), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_bad_routine_or_profile_is_refused_before_anything_is_claimed() {
    let d = fixture(
        "bad-defs",
        &[(
            ".trellis/routines/r.toml",
            "schedule = \"0 3 * * *\"\ntimezone = \"Europe/Amsterdam\"\n[task]\ntext = \"x\"\n",
        )],
    );
    ok(&d, &["queue", "add", "x"]);
    let o = trellis(&d, &["work", "--model", "scripted:none.json"]);
    let s = combined(&o);
    assert!(!o.status.success() && s.contains("fixed offsets"), "{s}");
    assert_eq!(ids(&d, "pending").len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

fn process_fixture(name: &str, command: &str, typecheck: &str) -> PathBuf {
    let d = fixture(
        name,
        &[(
            ".trellis/agents/maker.toml",
            &format!("command = {command}\ntimeout_s = 60\n"),
        )],
    );
    std::fs::write(
        d.join(".trellis/config.toml"),
        format!("[checks]\ntypecheck = {typecheck}\ntests = [\"git\", \"--version\"]\n"),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "checks", "--allow-empty"]);
    // A process agent runs only once trusted (ADR 0023 §4, 0026 §11). The
    // store is beside the fixture, cleared first so a stale one from an
    // earlier run cannot stand in for this decision.
    let _ = std::fs::remove_dir_all(d.with_extension("home"));
    ok(&d, &["trust"]);
    d
}

// `git config --file` creates the file: a cross-platform command that
// changes the worktree, with no shell involved.
const MAKES_A_FILE: &str = r#"["git", "config", "--file", "app/made.cfg", "a.b", "c"]"#;
const PASSES: &str = r#"["git", "--version"]"#;

#[test]
fn a_process_agent_converges_only_when_it_changed_something_and_the_checks_pass() {
    let d = process_fixture("proc-ok", MAKES_A_FILE, PASSES);
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "make a file", "--assignee", "maker"],
    ));
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("CONVERGED (converged: both checks passed)"),
        "{s}"
    );
    assert!(
        d.join("app/made.cfg").exists(),
        "the change is left for review"
    );
    let t = task(&d, "done", &id);
    let tr = trail(&d, t["outcome"]["run_id"].as_str().unwrap());
    assert_eq!(tr["factual"]["agent"], "maker");
    assert_eq!(tr["factual"]["actions"][0]["tool"], "process");
    assert_eq!(tr["factual"]["check_outcomes"].as_array().unwrap().len(), 2);
    assert!(tr["factual"].get("tokens").is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn a_process_agent_runs_in_trellis_s_own_process_group() {
    // The risk (review round 9, finding 5): in a process group of its own
    // the agent was a background job on the user's terminal, stopped by
    // SIGTTOU or SIGTTIN the moment it wrote to or read from it. That
    // cannot be staged without a terminal, so this guards the cause: the
    // agent's group is trellis's, which is this test's.
    let d = process_fixture(
        "proc-pgid",
        r#"["sh", "-c", "mkdir -p app && ps -o pgid= -p $$ > app/pgid.txt"]"#,
        PASSES,
    );
    ok(&d, &["queue", "add", "pgid", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("CONVERGED"), "{s}");
    let theirs = std::fs::read_to_string(d.join("app/pgid.txt")).unwrap();
    let ours = Command::new("ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert_eq!(theirs.trim(), String::from_utf8_lossy(&ours.stdout).trim());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_whose_checks_fail_is_rolled_back_whatever_its_exit_code() {
    // The command exits 0. That is data, not a verdict (ADR 0004): the
    // failing check decides, and the tree goes back.
    let d = process_fixture(
        "proc-fail",
        MAKES_A_FILE,
        r#"["git", "no-such-subcommand"]"#,
    );
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "make a file", "--assignee", "maker"],
    ));
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("process:  exited 0") && s.contains("NOT CONVERGED"),
        "{s}"
    );
    assert!(!d.join("app/made.cfg").exists(), "rolled back:\n{s}");
    assert_eq!(ids(&d, "failed"), vec![id.clone()]);
    let t = task(&d, "failed", &id);
    assert!(
        trail(&d, t["outcome"]["run_id"].as_str().unwrap())["factual"]["check_outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["passed"] == false)
    );
    assert_eq!(
        porcelain(&d),
        "",
        "the trail is committed, the change is gone"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_untrusted_process_agent_is_not_run_and_waits_for_trust() {
    // ADR 0023 §4 / 0026 §11. The risk: a cloned repository's
    // `.trellis/agents/*.toml` `command` ran on `trellis work` with no
    // trust decision at all, where a hook or custom tool would not.
    let d = process_fixture("proc-untrusted", MAKES_A_FILE, PASSES);
    let _ = std::fs::remove_dir_all(d.with_extension("home"));
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "make a file", "--assignee", "maker"],
    ));
    let s = ok(&d, &["work"]);
    assert!(!d.join("app/made.cfg").exists(), "it ran untrusted:\n{s}");
    assert_eq!(ids(&d, "pending"), vec![id.clone()], "{s}");
    let listed = ok(&d, &["queue", "list"]);
    assert!(
        format!("{s}{listed}").contains("not trusted: run trellis trust"),
        "{s}\n{listed}"
    );

    // `trellis trust` names the command, and then it runs.
    let t = ok(&d, &["trust"]);
    assert!(t.contains("process agent maker"), "{t}");
    let s = ok(&d, &["work"]);
    assert!(s.contains("CONVERGED"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_changes_nothing_has_nothing_to_verify() {
    let d = process_fixture("proc-noop", PASSES, PASSES);
    ok(&d, &["queue", "add", "do nothing", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("changed nothing"), "{s}");
    assert_eq!(ids(&d, "failed").len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_writes_a_denied_path_is_rolled_back_though_the_checks_pass() {
    // The risk: a process agent was judged on typecheck and tests alone, so
    // `[governance] deny_write` and `.trellis/governance/` rules that bind
    // every model run did not bind a command.
    let d = process_fixture("proc-deny", MAKES_A_FILE, PASSES);
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n\
         [governance]\ndeny_write = [\"app/\"]\n",
    )
    .unwrap();
    git(&d, &["commit", "-qam", "deny app/"]);
    // The trust fingerprint covers config.toml.
    ok(&d, &["trust"]);
    ok(&d, &["queue", "add", "make a file", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("NOT CONVERGED") && s.contains("app/made.cfg"),
        "{s}"
    );
    assert!(!d.join("app/made.cfg").exists(), "rolled back:\n{s}");
    assert_eq!(ids(&d, "failed").len(), 1, "{s}");
    let _ = std::fs::remove_dir_all(&d);

    // A `forbid` document is audited over what the command wrote.
    let d = process_fixture("proc-forbid", MAKES_A_FILE, PASSES);
    std::fs::create_dir_all(d.join(".trellis/governance")).unwrap();
    std::fs::write(
        d.join(".trellis/governance/no-c.toml"),
        "name = \"no b = c\"\nrank = \"non-negotiable\"\nforbid = { pattern = \"b = c\" }\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "rule"]);
    ok(&d, &["trust"]);
    ok(&d, &["queue", "add", "make a file", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED") && s.contains("no b = c"), "{s}");
    assert!(!d.join("app/made.cfg").exists(), "rolled back:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_rewrites_its_own_checks_does_not_converge() {
    // The risk: `[checks]`, `deny_write` and the governance documents were
    // read from the worktree after the command ran, so a command could
    // rewrite what verifies it — here, swap a failing typecheck for one that
    // passes — and converge on checks it chose. What judges the run is read
    // before it, and `.trellis/` is not the command's to change.
    let copy = if cfg!(windows) {
        r"['cmd', '/C', 'copy /Y passing.toml .trellis\config.toml']"
    } else {
        "['sh', '-c', 'cp passing.toml .trellis/config.toml']"
    };
    let d = process_fixture(
        "proc-rewrite-checks",
        copy,
        r#"["git", "no-such-subcommand"]"#,
    );
    std::fs::write(
        d.join("passing.toml"),
        "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "the file the command copies"]);
    ok(&d, &["trust"]);
    let before = std::fs::read(d.join(".trellis/config.toml")).unwrap();
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "rewrite checks", "--assignee", "maker"],
    ));
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert!(
        s.contains(".trellis/config.toml"),
        "the report names it:\n{s}"
    );
    assert_eq!(ids(&d, "failed"), vec![id], "{s}");
    assert_eq!(
        std::fs::read(d.join(".trellis/config.toml")).unwrap(),
        before,
        "rolled back:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_writes_git_config_or_a_hook_has_it_restored() {
    // `.git/` is invisible to `git status` and untouched by the rollback, so
    // a command could plant a hook or rewrite `core.hooksPath` and the run
    // would roll back the worktree and leave both in place.
    for (name, target) in [
        ("proc-git-config", ".git/config"),
        ("proc-git-hook", ".git/hooks/pre-commit"),
    ] {
        let cmd = format!(r#"["git", "config", "--file", "{target}", "evil.x", "y"]"#);
        let d = process_fixture(name, &cmd, PASSES);
        let before = std::fs::read(d.join(".git/config")).unwrap();
        ok(&d, &["queue", "add", "touch git", "--assignee", "maker"]);
        let s = ok(&d, &["work"]);
        assert!(s.contains("NOT CONVERGED"), "{s}");
        assert!(
            s.contains(target),
            "the report names what was changed:\n{s}"
        );
        assert_eq!(
            std::fs::read(d.join(".git/config")).unwrap(),
            before,
            "{name}: .git/config restored byte for byte\n{s}"
        );
        assert!(
            !d.join(".git/hooks/pre-commit").exists(),
            "{name}: a planted hook is removed\n{s}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A shell argv, as a TOML array of literal strings, running `script`:
/// `cmd /C` on Windows, `sh -c` elsewhere. The script is written in the
/// syntax both accept (`&&`, `echo x> f`, forward slashes to git).
fn shell_argv(script: &str) -> String {
    if cfg!(windows) {
        format!("['cmd', '/C', '{script}']")
    } else {
        format!("['sh', '-c', '{script}']")
    }
}

fn branch(d: &Path) -> String {
    git(d, &["rev-parse", "--symbolic-full-name", "HEAD"])
        .trim()
        .to_string()
}

#[test]
fn a_process_agent_that_commits_part_of_its_change_is_refused_and_the_commit_undone() {
    // The risk (review round 10, finding 1): touched paths came from `git
    // status` against the current HEAD, so an agent that committed its
    // `.trellis/` write hid it from the breach test, `deny_write` and the
    // governance audit, converged on the part it left uncommitted, and the
    // commit was never rolled back.
    let script = if cfg!(windows) {
        r"echo x> .trellis\sneak.txt && git add .trellis/sneak.txt && git commit -qm sneak && echo y> app\made.txt"
    } else {
        "echo x > .trellis/sneak.txt && git add .trellis/sneak.txt && git commit -qm sneak && echo y > app/made.txt"
    };
    let d = process_fixture("proc-commit", &shell_argv(script), PASSES);
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    let on = branch(&d);
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "commit a part", "--assignee", "maker"],
    ));
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert!(s.contains("moved HEAD"), "the report says why:\n{s}");
    assert!(
        s.contains(".trellis/sneak.txt"),
        "the committed path is judged too:\n{s}"
    );
    assert_eq!(ids(&d, "failed"), vec![id], "{s}");
    assert!(!d.join(".trellis/sneak.txt").exists(), "rolled back:\n{s}");
    assert!(!d.join("app/made.txt").exists(), "rolled back:\n{s}");
    assert_eq!(branch(&d), on, "{s}");
    // The trail's commit sits on top of the snapshot, not of the agent's.
    assert_eq!(
        git(&d, &["rev-parse", "HEAD~1"]).trim(),
        head,
        "the agent's commit is gone:\n{s}"
    );
    assert_eq!(porcelain(&d), "", "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_switches_branch_is_refused_and_put_back_on_the_original() {
    // The same rule for a checkout: the agent commits everything on a new
    // branch, so `git status` shows nothing. Before, that read as "changed
    // nothing", and the rollback's `reset --hard` moved the agent's branch
    // and left HEAD on it, where the trail's commit then landed.
    let script = if cfg!(windows) {
        r"git checkout -qb elsewhere && echo y> app\made.txt && git add -A && git commit -qm sneak"
    } else {
        "git checkout -qb elsewhere && echo y > app/made.txt && git add -A && git commit -qm sneak"
    };
    let d = process_fixture("proc-branch", &shell_argv(script), PASSES);
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    let on = branch(&d);
    ok(&d, &["queue", "add", "switch", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("NOT CONVERGED") && s.contains("moved HEAD"),
        "{s}"
    );
    assert_eq!(branch(&d), on, "back on the original branch:\n{s}");
    assert_eq!(git(&d, &["rev-parse", "HEAD~1"]).trim(), head, "{s}");
    assert!(!d.join("app/made.txt").exists(), "{s}");
    assert_eq!(porcelain(&d), "", "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `process_fixture`, with `*.cfg` ignored by a committed `.gitignore`
/// and `config` as `.trellis/config.toml`; trusted afterwards.
fn ignored_fixture(name: &str, command: &str, config: &str) -> PathBuf {
    let d = process_fixture(name, command, PASSES);
    std::fs::write(d.join(".gitignore"), "*.cfg\n").unwrap();
    std::fs::write(d.join(".trellis/config.toml"), config).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "ignore *.cfg"]);
    ok(&d, &["trust"]);
    d
}

#[test]
fn a_process_agent_that_writes_git_info_exclude_has_it_restored() {
    // `.git/info/exclude` decides what `git status` and the rollback's
    // `git clean` can see: a `*` there hides every later run's new files,
    // and nothing in the worktree changes to say so.
    let d = process_fixture(
        "proc-git-exclude",
        r#"["git", "config", "--file", ".git/info/exclude", "evil.x", "y"]"#,
        PASSES,
    );
    let before = std::fs::read(d.join(".git/info/exclude")).ok();
    ok(
        &d,
        &["queue", "add", "touch exclude", "--assignee", "maker"],
    );
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert!(
        s.contains("changed .git/info/exclude; put back"),
        "the report names it:\n{s}"
    );
    assert_eq!(
        std::fs::read(d.join(".git/info/exclude")).ok(),
        before,
        "restored byte for byte:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

const PASSING_CHECKS: &str =
    "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n";

#[test]
fn a_process_agent_s_ignored_writes_are_removed_on_rollback() {
    // The risk: `git status` does not list ignored files, so a command that
    // wrote only ignored paths "changed nothing", was rolled back, and left
    // what it wrote in place (`.venv/.../sitecustomize.py` runs in the next
    // check). What the command created is removed.
    let d = ignored_fixture("proc-ignored", MAKES_A_FILE, PASSING_CHECKS);
    ok(&d, &["queue", "add", "make a file", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert!(s.contains("app/made.cfg"), "the report names it:\n{s}");
    assert!(
        !d.join("app/made.cfg").exists(),
        "an ignored file the command created survived the rollback:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_process_agent_that_changed_a_pre_existing_ignored_file_is_reported_and_work_goes_on() {
    // trellis keeps no copy of ignored files, so one the command changed
    // cannot be put back, and it is named. It does not make the rollback
    // "not restored": the tracked tree is restored, which is what ADR 0009
    // promises, and an agent that runs the tests itself rewrites their
    // caches on every run, so counting it stopped `work` with exit 2 on
    // every failed run of such an agent (review round 9, finding 2).
    let cmd = r#"["git", "config", "--file", "app/old.cfg", "a.b", "c"]"#;
    let d = ignored_fixture("proc-ignored-old", cmd, PASSING_CHECKS);
    std::fs::write(d.join("app/old.cfg"), "[x]\n\ty = 1\n").unwrap();
    ok(&d, &["trust"]);
    ok(&d, &["queue", "add", "change it", "--assignee", "maker"]);
    let o = trellis(&d, &["work"]);
    let s = combined(&o);
    assert!(s.contains("app/old.cfg"), "named:\n{s}");
    assert!(!s.contains("NOT restored"), "{s}");
    assert_eq!(o.status.code(), Some(0), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_changed_or_deleted_ignored_file_the_rules_protect_leaves_the_tree_not_restored() {
    // The stricter signal is kept where it means something: a pre-existing
    // ignored file `deny_write` names that the command changed, or one it
    // deleted, is not a restored tree, and `work` stops.
    let deny = format!("{PASSING_CHECKS}[governance]\ndeny_write = [\"app/old.cfg\"]\n");
    let rm = if cfg!(windows) {
        r#"["cmd", "/C", "del app\\gone.cfg"]"#
    } else {
        r#"["rm", "app/gone.cfg"]"#
    };
    for (name, cmd, config) in [
        (
            "proc-ignored-old-denied",
            r#"["git", "config", "--file", "app/old.cfg", "a.b", "c"]"#,
            deny.as_str(),
        ),
        ("proc-ignored-gone", rm, PASSING_CHECKS),
    ] {
        let d = ignored_fixture(name, cmd, config);
        std::fs::write(d.join("app/old.cfg"), "[x]\n\ty = 1\n").unwrap();
        std::fs::write(d.join("app/gone.cfg"), "[x]\n\ty = 1\n").unwrap();
        ok(&d, &["trust"]);
        ok(&d, &["queue", "add", "change it", "--assignee", "maker"]);
        let o = trellis(&d, &["work"]);
        let s = combined(&o);
        assert!(s.contains("NOT put back"), "{name}:\n{s}");
        assert!(s.contains("left the tree NOT restored"), "{name}:\n{s}");
        assert_eq!(o.status.code(), Some(2), "{name}:\n{s}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn a_process_agent_s_ignored_write_under_trellis_or_a_denied_path_is_a_breach() {
    // An ignored path is still a path: `.trellis/` holds what judges the
    // run, and `deny_write` binds wherever git looks or not. The command
    // also writes an ordinary file, so the checks run and pass; only the
    // ignored write can stop it converging.
    let both = |a: &str| {
        if cfg!(windows) {
            format!(
                r"['cmd', '/C', 'git config --file {} a.b c && git config --file app\x.txt a.b c']",
                a.replace('/', r"\")
            )
        } else {
            format!(
                "['sh', '-c', 'git config --file {a} a.b c && git config --file app/x.txt a.b c']"
            )
        }
    };
    let mut cases = vec![
        (
            "proc-ignored-trellis",
            ".trellis/evil.cfg",
            PASSING_CHECKS.to_string(),
        ),
        (
            "proc-ignored-deny",
            "app/made.cfg",
            format!("{PASSING_CHECKS}[governance]\ndeny_write = [\"app/made.cfg\"]\n"),
        ),
    ];
    // NTFS ignores case, so a rule spelt differently still names the file
    // (finding r8-1): the audit folds as the write gate does.
    if cfg!(windows) {
        cases.push((
            "proc-ignored-deny-case",
            "app/made.cfg",
            format!("{PASSING_CHECKS}[governance]\ndeny_write = [\"./App//Made.cfg\"]\n"),
        ));
    }
    for (name, target, config) in cases {
        let d = ignored_fixture(name, &both(target), &config);
        ok(&d, &["queue", "add", "write", "--assignee", "maker"]);
        let s = ok(&d, &["work"]);
        assert!(s.contains("NOT CONVERGED"), "{name}:\n{s}");
        assert!(
            s.contains(&format!("governance: refused: {target}")),
            "{name}: the breach is named:\n{s}"
        );
        assert!(!d.join(target).exists(), "{name}: removed:\n{s}");
        assert!(!d.join("app/x.txt").exists(), "{name}: rolled back:\n{s}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn export_then_import_with_dry_run_and_rename() {
    let d = fixture(
        "export",
        &[
            (
                ".trellis/agents/rev.toml",
                "role = \"theirs\"\ninstructions = \"agents/rev.md\"\n",
            ),
            (".trellis/agents/rev.md", "notes"),
        ],
    );
    ok(&d, &["queue", "add", "exported task", "--assignee", "rev"]);
    let out = scratch("export-out").join("bundle");
    let s = ok(&d, &["queue", "export", out.to_str().unwrap()]);
    assert!(
        s.contains("1 agent(s)") && s.contains("1 task(s)") && s.contains("nothing secret"),
        "{s}"
    );

    let e = fixture(
        "import",
        &[(".trellis/agents/rev.toml", "role = \"mine\"\n")],
    );
    let dry = ok(
        &e,
        &[
            "queue",
            "import",
            out.to_str().unwrap(),
            "--dry-run",
            "--on-conflict",
            "rename",
        ],
    );
    assert!(
        dry.contains("rev -> rev-2") && dry.contains("dry run"),
        "{dry}"
    );
    assert!(
        ids(&e, "pending").is_empty() && porcelain(&e).is_empty(),
        "a dry run writes nothing"
    );
    ok(
        &e,
        &[
            "queue",
            "import",
            out.to_str().unwrap(),
            "--on-conflict",
            "rename",
        ],
    );
    let id = &ids(&e, "pending")[0];
    assert_eq!(task(&e, "pending", id)["assignee"], "rev-2");
    assert!(e.join(".trellis/agents/rev-2.toml").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&e);
    let _ = std::fs::remove_dir_all(out.parent().unwrap());
}
