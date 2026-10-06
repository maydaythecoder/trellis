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
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn trellis(root: &Path, args: &[&str]) -> Output {
    trellis_command(root, args)
        .output()
        .expect("the binary must be built before its integration tests run")
}

fn trellis_command(root: &Path, args: &[&str]) -> Command {
    // The trust store is resolved from these (ADR 0023 §4); pointed beside
    // the fixture so `trellis trust` here never writes the developer's own.
    // The user config is resolved from TRELLIS_CONFIG_DIR first, so an
    // exported one would bypass them; it is pinned to the same place. The
    // run markers `queue note` refuses on are removed, so a suite started
    // from inside a trellis run behaves as one started from a shell.
    let home = root.with_extension("home");
    let mut c = Command::new(env!("CARGO_BIN_EXE_trellis"));
    c.args(args)
        .current_dir(root)
        .env_remove("TRELLIS_RUN_ID")
        .env_remove("TRELLIS_TASK_ID")
        .env("TRELLIS_CONFIG_DIR", home.join("trellis"))
        .env("APPDATA", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG");
    c
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
        s.contains("agent:    rev") && s.contains("CONVERGED") && !s.contains("NOT CONVERGED"),
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

/// `scripted:<fixture>/<script>`, with the path written as `fixture` writes
/// `{ROOT}`, so a spec given on the command line matches one in a file.
fn scripted(d: &Path, script: &str) -> String {
    format!(
        "scripted:{}/{script}",
        d.display().to_string().replace('\\', "/")
    )
}

#[test]
fn queue_add_model_refuses_a_spec_that_does_not_parse() {
    // ADR 0026 §1 (#346). The risk: a typo in a task's model is found by the
    // heartbeat hours later, failing the task with nobody watching, rather
    // than refused while the person who typed it is there.
    let d = fixture("task-model-parse", &[]);
    let o = trellis(&d, &["queue", "add", "x", "--model", "no-scheme-here"]);
    let s = combined(&o);
    assert!(!o.status.success(), "{s}");
    assert!(
        s.contains("`no-scheme-here` is not a model spec"),
        "refused by the spec parser `run --model` uses: {s}"
    );
    assert!(ids(&d, "pending").is_empty(), "no task file was written");

    // A spec that parses is stored on the task as given.
    let spec = scripted(&d, "converge.json");
    let id = queued_id(&ok(&d, &["queue", "add", "y", "--model", &spec]));
    assert_eq!(task(&d, "pending", &id)["model"], spec.as_str());
    // A task without one is written as before: no `model` key at all.
    let plain = queued_id(&ok(&d, &["queue", "add", "z"]));
    assert!(task(&d, "pending", &plain).get("model").is_none());
    assert_eq!(porcelain(&d), "", "the queue must never dirty the worktree");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_task_that_names_a_model_runs_on_it_over_its_profile_and_work_model() {
    // ADR 0026 §1 (#346). The profile and `work --model` both name a script
    // that stops without a change, and only the task's own converges: a run
    // on either of the others cannot pass for the task's.
    let d = fixture(
        "task-model-run",
        &[(
            ".trellis/agents/rev.toml",
            "role = \"reviewer\"\nmodel = \"scripted:{ROOT}/stop.json\"\n",
        )],
    );
    let own = scripted(&d, "converge.json");
    let id = queued_id(&ok(
        &d,
        &[
            "queue",
            "add",
            "change the greeting",
            "--assignee",
            "rev",
            "--model",
            &own,
        ],
    ));
    let o = trellis(&d, &["work", "--model", &scripted(&d, "stop.json")]);
    let s = combined(&o);
    assert!(o.status.success(), "{s}");
    assert!(
        s.contains("agent:    rev") && s.contains("CONVERGED") && !s.contains("NOT CONVERGED"),
        "{s}"
    );
    let t = task(&d, "done", &id);
    assert_eq!(t["status"], "in_review", "{t}");
    let tr = trail(&d, t["outcome"]["run_id"].as_str().unwrap());
    assert_eq!(
        tr["provenance"]["model"],
        own.as_str(),
        "the trail names the model that drove the run: {tr}"
    );
    assert_eq!(tr["factual"]["agent"], "rev");
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
    // Which pause it is decides how to lift it, so `show` names the budget.
    let s = ok(&d, &["agents", "show", "rev"]);
    assert!(s.contains("state:        paused (budget)\n"), "{s}");

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

/// The rest of the first line of `agents show <name>` that starts `prefix`.
fn shown(d: &Path, name: &str, prefix: &str) -> String {
    let s = ok(d, &["agents", "show", name]);
    s.lines()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line:\n{s}"))
        .trim()
        .to_string()
}

#[test]
fn agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked() {
    // The risk: `agents show` and `agents resume` are what a person reads
    // to lift a pause and to trust a spend figure. A file pause shown as an
    // overlay one (or the reverse), a limit not printed, a missing-count or
    // edited-trail note printed for a profile with no such trail (or not
    // printed for one with it), or "is not paused" said of a profile its
    // file pauses, each sends them the wrong way.
    // One trail for `rev`: no completion count, and a digest that does not
    // match. A fixed date: only the lifetime-wide notes are read here.
    let edited = r#"{"task":"earlier","started_at":"2020-01-01T00:00:00Z","factual":{"agent":"rev"},"factual_sha256":"0000"}"#;
    let d = fixture(
        "agents-show",
        &[
            (
                ".trellis/agents/rev.toml",
                "model = \"scripted:{ROOT}/stop.json\"\n[budget]\nmonthly_tokens = 100\n",
            ),
            (".trellis/agents/idle.toml", "role = \"idle\"\n"),
            (
                ".trellis/agents/held.toml",
                "paused = true\n[control]\nmode = \"approve\"\n",
            ),
            (
                ".trellis/agents/maker.toml",
                "command = [\"python\", \"make.py\"]\n",
            ),
            (".trellis/runs/20200101-000000-earlier.json", edited),
        ],
    );

    let rev = ok(&d, &["agents", "show", "rev"]);
    assert!(rev.contains(": 0 of 100; lifetime 0 (no limit);"), "{rev}");
    assert!(rev.contains("1 run(s) recorded no count"), "{rev}");
    assert!(rev.contains("1 trail(s) were edited"), "{rev}");
    assert!(!rev.contains("approve mode needs a person"), "{rev}");
    let idle = ok(&d, &["agents", "show", "idle"]);
    assert!(!idle.contains("recorded no count"), "{idle}");
    assert!(!idle.contains("were edited"), "{idle}");
    let held = ok(&d, &["agents", "show", "held"]);
    assert!(held.contains("approve mode needs a person"), "{held}");
    assert_eq!(shown(&d, "held", "state:"), "paused (in its file)");
    assert_eq!(shown(&d, "maker", "runs with:"), "process `python make.py`");

    // An overlay pause is not the file's.
    ok(&d, &["agents", "pause", "idle"]);
    assert_eq!(shown(&d, "idle", "state:"), "paused");

    let s = ok(&d, &["agents", "resume", "rev"]);
    assert!(s.contains("agent `rev` is not paused"), "{s}");
    let s = ok(&d, &["agents", "resume", "held"]);
    assert!(!s.contains("is not paused"), "{s}");
    assert!(
        s.contains("is also paused in .trellis/agents/held.toml"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_routine_may_assign_its_task_to_a_profile_and_not_to_a_missing_one() {
    // The risk: the assignee check is inverted, so every routine that
    // names a real profile is refused and a typo is let through to `work`.
    let d = fixture(
        "routine-assignee",
        &[
            (".trellis/agents/rev.toml", "role = \"reviews\"\n"),
            (
                ".trellis/routines/r.toml",
                "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\nassignee = \"rev\"\n",
            ),
        ],
    );
    assert!(ok(&d, &["routines", "list"]).contains("not armed yet"));
    std::fs::write(
        d.join(".trellis/routines/r.toml"),
        "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\nassignee = \"rve\"\n",
    )
    .unwrap();
    let o = trellis(&d, &["routines", "list"]);
    let s = combined(&o);
    assert!(
        !o.status.success() && s.contains("assignee `rve` is not a profile"),
        "{s}"
    );
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
            "schedule = \"0 3 * * *\"\ntimezone = \"Europe/Atlantis\"\n[task]\ntext = \"x\"\n",
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

#[test]
fn allow_write_refuses_a_process_agent_s_write_outside_it_and_names_it() {
    // The risk (#182): a process agent's writes reach the repository with
    // no `edit` call for the gate to check, so a scope enforced only there
    // held for model runs and not for the agent the profile names. The
    // judge that reads the command's changes refuses it, names it, and the
    // run rolls back. A scope that admits the write converges as before,
    // and a repository `deny_write` still wins over a scope that names it.
    let deny = "[governance]\ndeny_write = [\"app/\"]\n";
    for (name, allow, extra, refusal) in [
        (
            "proc-allow-out",
            r#"["docs/"]"#,
            "",
            Some(
                "governance: refused: app/made.cfg (outside the allow_write [`docs/`] \
                 of agent `maker` in .trellis/agents/maker.toml)",
            ),
        ),
        (
            "proc-allow-deny",
            r#"["app/"]"#,
            deny,
            Some("governance: refused: app/made.cfg (deny_write `app/`)"),
        ),
        ("proc-allow-in", r#"["app/"]"#, "", None),
    ] {
        let d = process_fixture(name, MAKES_A_FILE, PASSES);
        std::fs::write(
            d.join(".trellis/agents/maker.toml"),
            format!("command = {MAKES_A_FILE}\ntimeout_s = 60\nallow_write = {allow}\n"),
        )
        .unwrap();
        let cfg = d.join(".trellis/config.toml");
        let mut text = std::fs::read_to_string(&cfg).unwrap();
        text.push_str(extra);
        std::fs::write(&cfg, text).unwrap();
        git(&d, &["commit", "-qam", "scope"]);
        ok(&d, &["trust"]);
        ok(&d, &["queue", "add", "make a file", "--assignee", "maker"]);
        let s = ok(&d, &["work"]);
        match refusal {
            Some(r) => {
                assert!(s.contains("NOT CONVERGED"), "{name}:\n{s}");
                assert!(s.contains(r), "{name}: the refusal is named:\n{s}");
                assert!(!d.join("app/made.cfg").exists(), "{name}: rolled back");
            }
            None => {
                assert!(
                    s.contains("CONVERGED (converged: both checks passed)"),
                    "{name}:\n{s}"
                );
                assert!(d.join("app/made.cfg").exists(), "{name}");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn a_process_agent_s_write_into_the_vault_is_refused_and_named() {
    // The risk (ADR 0049 §2): a process agent's writes reach the repository
    // through its clone's carry-back, with no `edit` call for the gate to
    // check, so a vault refused only at the gate stays writable to it. The
    // refusal names `[context] vault`, the setting that put the path off
    // limits, and the run rolls back.
    let d = process_fixture("proc-vault", MAKES_A_FILE, PASSES);
    let cfg = d.join(".trellis/config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str("[context]\nvault = \"app\"\n");
    std::fs::write(&cfg, text).unwrap();
    git(&d, &["commit", "-qam", "vault"]);
    ok(&d, &["trust"]);
    ok(&d, &["queue", "add", "make a file", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(s.contains("NOT CONVERGED"), "{s}");
    assert!(
        s.lines().any(|l| l.contains("refused")
            && l.contains("app/made.cfg")
            && l.contains("[context] vault")),
        "the refusal names the path and the setting:\n{s}"
    );
    assert!(!d.join("app/made.cfg").exists(), "rolled back:\n{s}");
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
    assert!(
        s.contains("CONVERGED") && !s.contains("NOT CONVERGED"),
        "{s}"
    );
    let theirs = std::fs::read_to_string(d.join("app/pgid.txt")).unwrap();
    let ours = Command::new("ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert_eq!(theirs.trim(), String::from_utf8_lossy(&ours.stdout).trim());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(windows)]
#[test]
fn job_object_kills_a_process_agent_s_tree() {
    // The risk (ADR 0035): after the agent exited normally nothing was
    // signalled on Windows, and `taskkill /T` could not have walked the tree
    // from a process that was gone, so a grandchild it backgrounded kept
    // running and could write after the comparison and the rollback. The
    // grandchild appends to a heartbeat file outside the repository about
    // once a second, for 30 beats: bounded, so a regression fails here
    // rather than holding the test run.
    let d = process_fixture("proc-job", PASSES, PASSES);
    let beat = d.with_extension("beat");
    let _ = std::fs::remove_dir_all(&beat);
    std::fs::create_dir_all(&beat).unwrap();
    std::fs::write(
        beat.join("beat.cmd"),
        "@for /L %%i in (1,1,30) do @(echo x>>\"%~dp0hb\"& ping -n 2 127.0.0.1 >NUL)\r\n",
    )
    .unwrap();
    let script = format!(
        "start /B cmd /C {} >NUL 2>&1 & ping -n 4 127.0.0.1 >NUL & exit 0",
        beat.join("beat.cmd").display()
    );
    std::fs::write(
        d.join(".trellis/agents/maker.toml"),
        format!("command = ['cmd', '/C', '{script}']\ntimeout_s = 60\n"),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "beat"]);
    ok(&d, &["trust"]);
    ok(&d, &["queue", "add", "beat", "--assignee", "maker"]);
    // Output to a file, not a pipe: an escaped grandchild inherits the
    // pipe, and `output()` would wait for it to end, which hid the escape.
    let log = beat.join("work.log");
    let st = trellis_command(&d, &["work"])
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    let s = std::fs::read_to_string(&log).unwrap();
    assert!(st.success() && s.contains("exited 0"), "{s}");
    let hb = beat.join("hb");
    let size = || std::fs::metadata(&hb).map(|m| m.len()).unwrap_or(0);
    // Without a beat before the return, a stopped heartbeat proves nothing.
    let before = size();
    assert!(before > 0, "the grandchild never wrote its heartbeat:\n{s}");
    std::thread::sleep(std::time::Duration::from_secs(4));
    assert_eq!(
        size(),
        before,
        "the agent's backgrounded grandchild kept writing after `work` returned"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&beat);
}

/// A process agent that waits about `secs` seconds and changes nothing:
/// long enough for a service to do what it does before the task ends.
fn waits(secs: u32) -> String {
    if cfg!(windows) {
        format!("['cmd', '/C', 'ping -n {} 127.0.0.1 >NUL']", secs + 1)
    } else {
        format!("['sh', '-c', 'sleep {secs}']")
    }
}

/// `work`, with its output in a file rather than a pipe: a process that
/// escaped the kill inherits the pipe, and waiting for it to close would
/// hide the escape (as in `job_object_kills_a_process_agent_s_tree`).
fn work_to_file(d: &Path, log: &Path) -> (bool, String) {
    let st = trellis_command(d, &["work"])
        .stdout(std::fs::File::create(log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .status()
        .unwrap();
    let s = format!(
        "{}{}",
        std::fs::read_to_string(log).unwrap_or_default(),
        std::fs::read_to_string(log.with_extension("err")).unwrap_or_default()
    );
    (st.success(), s)
}

/// The `.json` records left in `<git-dir>/trellis/services/`.
fn service_records(d: &Path) -> Vec<String> {
    std::fs::read_dir(d.join(".git/trellis/services"))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn service_starts_in_the_task_s_worktree_and_the_trail_records_it_never_as_a_check() {
    // ADR 0026 §12. The risks: the service ran somewhere other than the
    // worktree the task runs in, the trail did not say it ran, or its
    // output reached the trail as though it were evidence. The service
    // proves its working directory by finding a file only the worktree has,
    // and writes outside the repository so the run's tree is untouched.
    let out = scratch("svc-start-out");
    let marker = out.join("here.txt");
    let probe = if cfg!(windows) {
        format!(
            "['cmd', '/C', 'if exist app\\core.py (echo in-the-worktree> {}) & echo %TRELLIS_SERVICE%-says-passed']",
            marker.display()
        )
    } else {
        format!(
            "['sh', '-c', 'test -f app/core.py && echo in-the-worktree > {}; echo $TRELLIS_SERVICE-says-passed']",
            marker.display()
        )
    };
    let d = fixture(
        "svc-start",
        &[(
            ".trellis/agents/maker.toml",
            &format!(
                "command = {}\ntimeout_s = 60\n[[services]]\nname = \"web\"\ncommand = {probe}\n",
                waits(3)
            ),
        )],
    );
    // Untrusted, a service does not start and its task waits (ADR 0023 §4).
    let _ = std::fs::remove_dir_all(d.with_extension("home"));
    let id = queued_id(&ok(&d, &["queue", "add", "serve", "--assignee", "maker"]));
    let s = ok(&d, &["work"]);
    assert!(!marker.exists(), "a service ran untrusted:\n{s}");
    assert_eq!(ids(&d, "pending"), vec![id.clone()], "{s}");
    let t = ok(&d, &["trust"]);
    assert!(t.contains("service maker/web"), "{t}");

    let (ran, s) = work_to_file(&d, &out.join("work.log"));
    assert!(ran, "{s}");
    assert!(s.contains("service:  web started"), "{s}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default().trim(),
        "in-the-worktree",
        "the service did not run in the task's worktree:\n{s}"
    );
    // The agent changed nothing, so the task failed; the service's own
    // "passed" is a log line and changed nothing about that.
    assert_eq!(ids(&d, "failed"), vec![id.clone()], "{s}");
    let log = std::fs::read_to_string(d.join(".git/trellis/services/maker.web.log")).unwrap();
    // Printed from its environment, so the text is in the log and not in
    // the argv the trail records.
    assert!(log.contains("web-says-passed"), "{log}");
    let task = task(&d, "failed", &id);
    let run = trail(&d, task["outcome"]["run_id"].as_str().unwrap());
    let actions = run["factual"]["actions"].as_array().unwrap();
    let svc: Vec<_> = actions.iter().filter(|a| a["tool"] == "service").collect();
    assert_eq!(svc.len(), 1, "{actions:?}");
    assert!(svc[0]["target"].as_str().unwrap().starts_with("web: "));
    assert!(
        !run.to_string().contains("web-says-passed"),
        "the service's output reached the trail"
    );
    assert!(
        run["factual"]["check_outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "service"),
        "{run}"
    );
    assert!(
        service_records(&d).is_empty(),
        "the record outlived a clean stop"
    );
    assert_eq!(porcelain(&d), "", "the trail is committed, nothing else");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn service_is_killed_with_its_tree_when_the_task_ends() {
    // ADR 0026 §12. The risk: a service, or a process it started, kept
    // running after its task, writing into the worktree after the run was
    // judged or rolled back and holding a port the next task needs. The
    // service backgrounds a heartbeat that appends to a file outside the
    // repository about once a second, for 30 beats (bounded, so a
    // regression fails here rather than holding the test run), then waits
    // 120 s and marks, in `slept`, that the wait finished.
    //
    // Measured by a second task in the same `work`, while trellis is still
    // alive: on Windows the job is also killed when trellis exits
    // (kill-on-close), so a check made only after `work` returns would pass
    // even if nothing were stopped at the end of the task.
    //
    // Killed, not waited for, is read from the service's own evidence: a
    // service `work` waited for finishes its 120 s wait and writes `slept`,
    // however fast or slow the host; one killed when its task ends never
    // does. A bound on the whole `work` run's wall clock measured host
    // load as much as the kill, and a loaded run exceeded it (#222, the same
    // class as #207). The heartbeat cannot show this
    // on its own: waited for, its 30 beats end before the next task looks.
    //
    // The task lasts until the heartbeat has beaten, not a fixed 3 s: the
    // `a > 0` premise is that event, a fixed wait could end before a loaded
    // host ran the first beat, and on a quiet one it spent seconds after it
    // (#227). Bounded at about 30 s, inside the task's 60 s timeout, so a
    // heartbeat that never beats fails on the watcher's reading of `hb`,
    // not on the task's timeout.
    // The watcher's 3 s window stays fixed: it is the absence proof (no
    // beat for about three beat periods), and absence has no event.
    let beat = scratch("svc-kill-beat");
    let hb = beat.join("hb");
    let slept = beat.join("slept");
    let (service, maker, watcher) = if cfg!(windows) {
        std::fs::write(
            beat.join("beat.cmd"),
            "@for /L %%i in (1,1,30) do @(echo x>>\"%~dp0hb\"& ping -n 2 127.0.0.1 >NUL)\r\n",
        )
        .unwrap();
        // `1%%~zA GTR 10`: a missing file has an empty size, and a bare
        // `GTR 0` would then be a syntax error that ends the script.
        std::fs::write(
            beat.join("first.cmd"),
            "@for /L %%i in (1,1,30) do @(\r\n\
             @for %%A in (\"%~dp0hb\") do @if 1%%~zA GTR 10 exit 0\r\n\
             @ping -n 2 127.0.0.1 >NUL\r\n\
             )\r\n",
        )
        .unwrap();
        std::fs::write(
            beat.join("watch.cmd"),
            "@for %%A in (\"%~dp0hb\") do @echo %%~zA> \"%~dp0a.txt\"\r\n\
             @ping -n 4 127.0.0.1 >NUL\r\n\
             @for %%A in (\"%~dp0hb\") do @echo %%~zA> \"%~dp0b.txt\"\r\n",
        )
        .unwrap();
        (
            format!(
                "['cmd', '/C', 'start /B cmd /C {} >NUL 2>&1 & ping -n 121 127.0.0.1 >NUL & echo slept> {}']",
                beat.join("beat.cmd").display(),
                slept.display()
            ),
            format!("['cmd', '/C', '{}']", beat.join("first.cmd").display()),
            format!("['cmd', '/C', '{}']", beat.join("watch.cmd").display()),
        )
    } else {
        std::fs::write(
            beat.join("beat.sh"),
            format!(
                "i=0; while [ $i -lt 30 ]; do echo x >> '{}'; i=$((i+1)); sleep 1; done\n",
                hb.display()
            ),
        )
        .unwrap();
        std::fs::write(
            beat.join("watch.sh"),
            format!(
                "wc -c < '{hb}' > '{a}'; sleep 3; wc -c < '{hb}' > '{b}'\n",
                hb = hb.display(),
                a = beat.join("a.txt").display(),
                b = beat.join("b.txt").display()
            ),
        )
        .unwrap();
        std::fs::write(
            beat.join("first.sh"),
            format!(
                "i=0; while [ ! -s '{}' ] && [ $i -lt 30 ]; do i=$((i+1)); sleep 1; done\n",
                hb.display()
            ),
        )
        .unwrap();
        (
            format!(
                "['sh', '-c', 'sh {} & sleep 120; echo slept > {}']",
                beat.join("beat.sh").display(),
                slept.display()
            ),
            format!("['sh', '{}']", beat.join("first.sh").display()),
            format!("['sh', '{}']", beat.join("watch.sh").display()),
        )
    };
    let d = fixture(
        "svc-kill",
        &[
            (
                ".trellis/agents/maker.toml",
                &format!(
                    "command = {maker}\ntimeout_s = 60\n\
                     [[services]]\nname = \"beat\"\ncommand = {service}\n"
                ),
            ),
            (
                ".trellis/agents/watcher.toml",
                &format!("command = {watcher}\ntimeout_s = 60\n"),
            ),
        ],
    );
    let _ = std::fs::remove_dir_all(d.with_extension("home"));
    ok(&d, &["trust"]);
    ok(
        &d,
        &[
            "queue",
            "add",
            "serve",
            "--assignee",
            "maker",
            "--priority",
            "high",
        ],
    );
    ok(&d, &["queue", "add", "watch", "--assignee", "watcher"]);
    let (ran, s) = work_to_file(&d, &beat.join("work.log"));
    assert!(ran && s.contains("service:  beat started"), "{s}");
    let size_at = |f: &str| -> u64 {
        std::fs::read_to_string(beat.join(f))
            .unwrap_or_else(|e| panic!("the watcher did not write {f}: {e}\n{s}"))
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{f}: {e}\n{s}"))
    };
    let (a, b) = (size_at("a.txt"), size_at("b.txt"));
    // Without a beat before the task ended, a stopped heartbeat proves
    // nothing.
    assert!(a > 0, "the service's heartbeat never beat:\n{s}");
    assert_eq!(
        a, b,
        "the service's backgrounded heartbeat kept writing after its task ended, \
         while the next task ran:\n{s}"
    );
    assert!(
        !slept.exists(),
        "`work` waited out the service's 120 s wait instead of killing it:\n{s}"
    );
    assert!(
        service_records(&d).is_empty(),
        "a killed and reaped service left its record for doctor"
    );
    let doctor = combined(&trellis(&d, &["doctor"]));
    assert!(
        doctor.contains("no recorded service outlived its task"),
        "{doctor}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&beat);
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
    assert!(
        s.contains("CONVERGED") && !s.contains("NOT CONVERGED"),
        "{s}"
    );
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
    // would roll back the worktree and leave both in place. By absolute
    // path: a relative write lands in the agent's clone (ADR 0032), and
    // this guard is for the command that finds the repository anyway.
    for (name, target) in [
        ("proc-git-config", ".git/config"),
        ("proc-git-hook", ".git/hooks/pre-commit"),
    ] {
        let cmd = format!(r#"["git", "config", "--file", "{{ROOT}}/{target}", "evil.x", "y"]"#);
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

/// `script`, run from the repository rather than from the agent's clone:
/// the command that finds the repository and writes there by absolute
/// path, which the clone does not stop and the guards on the repository
/// are kept for (ADR 0032, "Limits"). `{ROOT}` is the fixture's root,
/// unquoted under `cmd`: Rust passes an inner `"` to it as `\"`, which
/// `cmd` does not read, and the test's temporary directory has no space.
fn in_repo(script: &str) -> String {
    if cfg!(windows) {
        format!("cd /d {{ROOT}} && {script}")
    } else {
        format!(r#"cd "{{ROOT}}" && {script}"#)
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
    let d = process_fixture("proc-commit", &shell_argv(&in_repo(script)), PASSES);
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
    let d = process_fixture("proc-branch", &shell_argv(&in_repo(script)), PASSES);
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
        r#"["git", "config", "--file", "{ROOT}/.git/info/exclude", "evil.x", "y"]"#,
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

/// [`MAKES_A_FILE`], written in the repository by absolute path.
const MAKES_A_FILE_IN_REPO: &str =
    r#"["git", "config", "--file", "{ROOT}/app/made.cfg", "a.b", "c"]"#;

const PASSING_CHECKS: &str =
    "[checks]\ntypecheck = [\"git\", \"--version\"]\ntests = [\"git\", \"--version\"]\n";

#[test]
fn a_process_agent_s_ignored_writes_are_removed_on_rollback() {
    // The risk: `git status` does not list ignored files, so a command that
    // wrote only ignored paths "changed nothing", was rolled back, and left
    // what it wrote in place (`.venv/.../sitecustomize.py` runs in the next
    // check). What the command created is removed. By absolute path: in
    // the agent's clone an ignored file is never carried back at all.
    let d = ignored_fixture("proc-ignored", MAKES_A_FILE_IN_REPO, PASSING_CHECKS);
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
    let cmd = r#"["git", "config", "--file", "{ROOT}/app/old.cfg", "a.b", "c"]"#;
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
    // By absolute path: the agent's clone holds no ignored file to change.
    let rm = if cfg!(windows) {
        r#"["cmd", "/C", "cd /d {ROOT} && del app\\gone.cfg"]"#
    } else {
        r#"["rm", "{ROOT}/app/gone.cfg"]"#
    };
    for (name, cmd, config) in [
        (
            "proc-ignored-old-denied",
            r#"["git", "config", "--file", "{ROOT}/app/old.cfg", "a.b", "c"]"#,
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
    // ignored write can stop it converging. In the repository, by absolute
    // path: in the agent's clone an ignored write never comes back.
    let both = |a: &str| {
        if cfg!(windows) {
            format!(
                r#"['cmd', '/C', 'cd /d {{ROOT}} && git config --file {} a.b c && git config --file app\x.txt a.b c']"#,
                a.replace('/', r"\")
            )
        } else {
            format!(
                r#"['sh', '-c', 'cd "{{ROOT}}" && git config --file {a} a.b c && git config --file app/x.txt a.b c']"#
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

/// One script per platform: `cmd` wants backslashes where it names a file
/// itself (`echo x> app\f`, `del`), `sh` wants forward ones.
fn per_os(windows: &str, unix: &str) -> String {
    shell_argv(if cfg!(windows) { windows } else { unix })
}

#[test]
fn isolated_agent_writes_to_trellis_and_git_in_the_clone_never_reach_the_repository() {
    // The risk (ADR 0032, #175): a process agent ran in the repository, so
    // its writes under `.git/` (config, hooks, planted files) were undone
    // after the fact by a guard that reads state the writer can also write,
    // and its `.trellis/` writes were in the tree that judges it. In the
    // clone, `.git/` writes stay there and `.trellis/` is refused at the
    // carry-back, before the repository has them. The ordinary edit beside
    // them comes back and is rolled back with the refused run.
    let d = process_fixture(
        "iso-agent-judge",
        &per_os(
            r"git config --file .git/config evil.x y && echo planted> .git\planted && echo x> .git\hooks\pre-commit && echo [hooks]> .trellis\config.toml && echo x> .trellis\new.txt && echo y> app\made.txt",
            "git config --file .git/config evil.x y && echo planted > .git/planted && echo x > .git/hooks/pre-commit && echo [hooks] > .trellis/config.toml && echo x > .trellis/new.txt && echo y > app/made.txt",
        ),
        PASSES,
    );
    let git_config = std::fs::read(d.join(".git/config")).unwrap();
    let trellis_config = std::fs::read(d.join(".trellis/config.toml")).unwrap();
    ok(
        &d,
        &["queue", "add", "write the judge", "--assignee", "maker"],
    );
    let s = ok(&d, &["work"]);
    assert!(s.contains("process:  exited 0"), "{s}");
    assert!(
        s.contains("NOT CONVERGED (not converged: the command changed .trellis/"),
        "{s}"
    );
    for p in [".trellis/config.toml", ".trellis/new.txt"] {
        assert!(
            s.contains(&format!("governance: refused: {p} (under .trellis/")),
            "{p} is refused at the carry-back and named:\n{s}"
        );
    }
    // Never there, rather than put back: the git guard had nothing to do.
    assert!(!s.contains("put back"), "{s}");
    assert_eq!(std::fs::read(d.join(".git/config")).unwrap(), git_config);
    assert!(!d.join(".git/planted").exists());
    assert!(!d.join(".git/hooks/pre-commit").exists());
    assert_eq!(
        std::fs::read(d.join(".trellis/config.toml")).unwrap(),
        trellis_config
    );
    assert!(!d.join(".trellis/new.txt").exists());
    assert!(!d.join("app/made.txt").exists(), "rolled back:\n{s}");
    assert_eq!(porcelain(&d), "", "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A fixture whose agent edits `app/core.py`, creates `app/new.py`,
/// deletes the tracked `app/old.py`, and records where it ran in
/// `app/where.txt`, with `typecheck` as its typecheck command.
fn edit_create_delete(name: &str, typecheck: &str) -> PathBuf {
    let d = process_fixture(
        name,
        &per_os(
            // A space before `>`: `cmd` reads `1>` as a redirect of handle 1.
            r"echo x = 1 > app\core.py && echo y = 2 > app\new.py && del app\old.py && cd> app\where.txt",
            "echo x = 1 > app/core.py && echo y = 2 > app/new.py && rm app/old.py && pwd > app/where.txt",
        ),
        typecheck,
    );
    std::fs::write(d.join("app/old.py"), "z = 3\n").unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "old.py"]);
    ok(&d, &["trust"]);
    d
}

#[test]
fn isolated_agent_edit_new_file_and_deletion_are_carried_back() {
    // The risk: carrying back only edits, as for `bash`, would lose the
    // files an agent creates and deletes, which for a process agent are
    // part of the run's change (it has no `write_file`), so a run that
    // converged would leave half its work in a clone that is then removed.
    let d = edit_create_delete("iso-agent-back", PASSES);
    let id = queued_id(&ok(&d, &["queue", "add", "change", "--assignee", "maker"]));
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("CONVERGED (converged: both checks passed)"),
        "{s}"
    );
    assert!(
        s.contains("clone:    edited from the clone: app/core.py"),
        "{s}"
    );
    assert!(
        s.contains("clone:    created from the clone: app/new.py, app/where.txt"),
        "{s}"
    );
    assert!(
        s.contains("clone:    deleted from the clone: app/old.py"),
        "{s}"
    );
    assert!(
        std::fs::read_to_string(d.join("app/core.py"))
            .unwrap()
            .contains("x = 1")
    );
    assert!(d.join("app/new.py").exists());
    assert!(!d.join("app/old.py").exists());
    // It ran in the clone, not here.
    let at = std::fs::read_to_string(d.join("app/where.txt")).unwrap();
    assert!(at.contains("trellis-clone-"), "{at}");
    // Left for review, and the trail names all three.
    let t = task(&d, "done", &id);
    let tr = trail(&d, t["outcome"]["run_id"].as_str().unwrap());
    let proposed: Vec<&str> = tr["factual"]["proposed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for p in ["app/core.py", "app/new.py", "app/old.py"] {
        assert!(proposed.contains(&p), "{p}: {proposed:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn isolated_agent_carried_back_change_is_undone_by_rollback() {
    // The point of carrying back only what `git reset --hard` and `git
    // clean -fd` restore (ADR 0009): a run whose check fails leaves the
    // tree as it was, the new file and the deletion included.
    let d = edit_create_delete("iso-agent-rollback", r#"["git", "no-such-subcommand"]"#);
    ok(&d, &["queue", "add", "change", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("NOT CONVERGED (not converged: a check failed)"),
        "{s}"
    );
    assert_eq!(
        std::fs::read_to_string(d.join("app/core.py")).unwrap(),
        "def greet():\n    return \"hello\"\n"
    );
    assert!(!d.join("app/new.py").exists(), "{s}");
    assert_eq!(
        std::fs::read_to_string(d.join("app/old.py")).unwrap(),
        "z = 3\n"
    );
    assert_eq!(porcelain(&d), "", "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn isolated_agent_cannot_unignore_a_file_by_editing_gitignore_in_its_clone() {
    // The risk: a new file judged by the clone's `.gitignore`, which is the
    // agent's to edit. It un-ignores `*.cfg` there, and `app/made.cfg`
    // would come back as a file the repository's own rules ignore, which
    // the rollback's `git clean -fd` then leaves in place. The repository's
    // rules decide, and the `.gitignore` edit itself is refused.
    let d = ignored_fixture(
        "iso-agent-unignore",
        &per_os(
            r"echo !*.cfg>> .gitignore && git config --file app/made.cfg a.b c && echo y> app\x.txt",
            "echo !*.cfg >> .gitignore && git config --file app/made.cfg a.b c && echo y > app/x.txt",
        ),
        PASSING_CHECKS,
    );
    ok(&d, &["queue", "add", "unignore", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("clone:    left in the clone, as the repository's git ignores it: app/made.cfg"),
        "{s}"
    );
    assert!(
        s.contains("clone:    not carried back: `.gitignore` tells git what to ignore"),
        "{s}"
    );
    assert!(
        s.contains("not converged: part of what the command changed in its clone could not be"),
        "{s}"
    );
    assert!(!d.join("app/made.cfg").exists(), "{s}");
    assert!(!d.join("app/x.txt").exists(), "rolled back:\n{s}");
    assert_eq!(
        std::fs::read_to_string(d.join(".gitignore")).unwrap(),
        "*.cfg\n"
    );
    assert_eq!(porcelain(&d), "", "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn isolated_agent_that_commits_in_its_clone_leaves_head_alone_and_its_change_comes_back() {
    // What the clone changes for an agent that commits its own work (many
    // do): the commit moves the clone's HEAD, not the repository's, so it
    // is no longer a breach; its files are compared by content and come
    // back uncommitted, for review, like any other run's change.
    let d = process_fixture(
        "iso-agent-commit",
        &per_os(
            r"echo y> app\made.txt && git add -A && git -c user.email=a@example.invalid -c user.name=a commit -qm mine",
            "echo y > app/made.txt && git add -A && git -c user.email=a@example.invalid -c user.name=a commit -qm mine",
        ),
        PASSES,
    );
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    ok(&d, &["queue", "add", "commit", "--assignee", "maker"]);
    let s = ok(&d, &["work"]);
    assert!(
        s.contains("CONVERGED (converged: both checks passed)"),
        "{s}"
    );
    assert!(!s.contains("moved HEAD"), "{s}");
    assert!(d.join("app/made.txt").exists(), "{s}");
    // The trail's commit sits on the snapshot; the agent's never left the clone.
    assert_eq!(git(&d, &["rev-parse", "HEAD~1"]).trim(), head, "{s}");
    assert_eq!(porcelain(&d).trim(), "?? app/made.txt", "{s}");
    let _ = std::fs::remove_dir_all(&d);
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

#[test]
fn queue_list_filters_show_only_their_tasks_and_no_unreadable_file() {
    // #248. The risk: a filter that shows what it should hide, or a task
    // file nobody can read listed under a filter it cannot match.
    let d = fixture("list-filters", &[(".trellis/agents/rev.toml", "")]);
    let a = queued_id(&ok(
        &d,
        &[
            "queue",
            "add",
            "alpha",
            "--priority",
            "high",
            "--assignee",
            "rev",
        ],
    ));
    let b = queued_id(&ok(&d, &["queue", "add", "beta", "--backlog"]));
    std::fs::write(
        d.join(".git/trellis/queue/pending/zzz-broken.json"),
        "{not json",
    )
    .unwrap();
    let list = |args: &[&str]| {
        let mut all = vec!["queue", "list"];
        all.extend_from_slice(args);
        ok(&d, &all)
    };
    let s = list(&[]);
    assert!(
        s.contains(&a) && s.contains(&b) && s.contains("unreadable"),
        "{s}"
    );
    for (args, shown, hidden) in [
        (&["--status", "backlog"][..], &b, &a),
        (&["--assignee", "rev"][..], &a, &b),
        (&["--priority", "high"][..], &a, &b),
    ] {
        let s = list(args);
        assert!(
            s.contains(shown.as_str()) && !s.contains(hidden.as_str()),
            "{args:?}: {s}"
        );
    }
    for args in [&["--status", "todo"][..], &["--label", "x"][..]] {
        let s = list(args);
        assert!(!s.contains("unreadable"), "{args:?}: {s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn export_says_how_many_claimed_tasks_it_left_out_and_nothing_when_none() {
    // #248. The risk: a task mid-run silently missing from an export, or
    // a "0 claimed task(s)" line on every export that left nothing out.
    let d = fixture("export-claimed", &[]);
    let id = queued_id(&ok(&d, &["queue", "add", "held"]));
    let out = scratch("export-claimed-out");
    let s = ok(&d, &["queue", "export", out.join("a").to_str().unwrap()]);
    assert!(!s.contains("claimed task(s)"), "{s}");
    let q = d.join(".git/trellis/queue");
    std::fs::create_dir_all(q.join("claimed")).unwrap();
    let file = format!("{id}.json");
    std::fs::rename(q.join("pending").join(&file), q.join("claimed").join(&file)).unwrap();
    let s = ok(&d, &["queue", "export", out.join("b").to_str().unwrap()]);
    assert!(s.contains("1 claimed task(s) not exported"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn an_approve_mode_agent_waits_when_work_has_no_terminal_to_ask_on() {
    // #248, ADR 0020 §5. The risk: a profile in approve mode run by a
    // heartbeat nobody is watching, where every approval it asks for goes
    // unanswered. The test's stdin and stdout are not a terminal.
    let d = fixture(
        "approve-no-tty",
        &[(
            ".trellis/agents/rev.toml",
            "model = \"scripted:{ROOT}/converge.json\"\n[control]\nmode = \"approve\"\n",
        )],
    );
    let id = queued_id(&ok(
        &d,
        &["queue", "add", "change the greeting", "--assignee", "rev"],
    ));
    let s = ok(&d, &["work", "--once"]);
    assert!(
        s.contains("agent `rev` is in approve mode, and `work` has no terminal to ask on"),
        "{s}"
    );
    assert_eq!(ids(&d, "pending"), vec![id]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_claim_records_where_it_was_made_under_release_and_only_there() {
    // #248, ADR 0020 §5. The risk: claims under the default policy
    // recording a host nothing reads, or claims under "release" recording
    // none, so a dead holder's claim is never released. The process agent
    // prints its own claim file, found by `TRELLIS_TASK_ID`, while it
    // holds the claim.
    let cmd = per_os(
        &in_repo(r"type .git\trellis\queue\claimed\%TRELLIS_TASK_ID%.json"),
        &in_repo("cat .git/trellis/queue/claimed/$TRELLIS_TASK_ID.json"),
    );
    let d = process_fixture("claimant", &cmd, PASSES);
    ok(
        &d,
        &["queue", "add", "show the claim", "--assignee", "maker"],
    );
    let s = ok(&d, &["work", "--once"]);
    assert!(s.contains("claimed_by_pid"), "the claim was printed: {s}");
    assert!(!s.contains("claimed_host"), "{s}");

    std::fs::write(
        d.join(".trellis/config.toml"),
        format!(
            "[checks]\ntypecheck = {PASSES}\ntests = [\"git\", \"--version\"]\n\
             [work]\nstale_claims = \"release\"\n"
        ),
    )
    .unwrap();
    git(&d, &["commit", "-qam", "release"]);
    ok(&d, &["trust"]);
    ok(
        &d,
        &["queue", "add", "show it again", "--assignee", "maker"],
    );
    let s = ok(&d, &["work", "--once"]);
    assert!(s.contains("claimed_host"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}
