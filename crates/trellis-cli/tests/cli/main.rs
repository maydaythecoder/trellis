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
        // Hermetic for the same reason: with either variable set, anyhow
        // appends a backtrace to every error the binary prints, and a test
        // that compares that text fails on a host that sets one (#783).
        // RUST_LIB_BACKTRACE is read first, so both are pinned.
        .env("RUST_BACKTRACE", "0")
        .env("RUST_LIB_BACKTRACE", "0")
        .output()
        .expect("the binary must be built before its integration tests run")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn combined(o: &Output) -> String {
    format!("{}{}", stdout(o), String::from_utf8_lossy(&o.stderr))
}

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

fn add_skill(root: &Path, dir: &str, name: &str, description: &str) {
    let d = root.join(".trellis/skills").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
    )
    .unwrap();
}

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

fn keys(v: &serde_json::Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// `get_codebase`'s answer in `d`, asked through rpc with `extra` merged
/// into the command.
fn get_codebase_in(d: &Path, extra: serde_json::Value) -> serde_json::Value {
    let mut cmd = serde_json::json!({"id": "g", "type": "get_codebase"});
    for (k, v) in extra.as_object().unwrap() {
        cmd[k] = v.clone();
    }
    rpc_answer(&rpc_records(d, None, &[cmd]), "g").clone()
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

/// A copy of the built `trellis` alone in a fresh directory: nothing is
/// beside it, whatever was built beside the original.
#[cfg(unix)]
fn trellis_copy(name: &str) -> PathBuf {
    let exe = scratch(name).join("trellis");
    std::fs::copy(env!("CARGO_BIN_EXE_trellis"), &exe).unwrap();
    wait_until_executable(&exe);
    exe
}

/// `status --porcelain` with no optional lock, so the look itself writes
/// no refreshed index.
fn porcelain_unlocked(root: &Path) -> String {
    git(root, &["--no-optional-locks", "status", "--porcelain"])
}

/// The module named `name` in a `get_codebase` answer's `data`.
fn module_in<'a>(data: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    data["modules"]
        .as_array()
        .unwrap_or_else(|| panic!("modules: {data}"))
        .iter()
        .find(|m| m["module"] == name)
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Makes `link` point at `target`; false (and the test skips, saying why)
/// where the platform will not make one.
fn link_or_skip(target: &Path, link: &Path) -> bool {
    match trellis_core::paths::symlink_for_test(target, link) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("{why}");
            false
        }
    }
}

/// A `trellis-tui` in `dir` that writes its arguments, one per line, to
/// `args.txt` and its environment to `env.txt` beside it, then exits 7, so
/// the status `trellis` ends with shows whose it is.
#[cfg(unix)]
fn stub_tui(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("trellis-tui");
    std::fs::write(
        &p,
        "#!/bin/sh\nd=$(dirname \"$0\")\n\
         for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$d/args.txt\"\n\
         env > \"$d/env.txt\"\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

// One module per command, still one test binary named `cli` (#938). A helper
// two modules use lives above; one module's own helpers stay in that module.
// Every test in `bare` drives a pseudo-terminal, which only Unix has; on
// Windows the module would be an unused `use super::*` under -D warnings.
#[cfg(unix)]
mod bare;
mod check;
mod control_dial;
mod doctor;
mod ignored_files;
mod index;
mod init;
mod model;
mod overview;
mod partitions;
mod radius;
mod read_file;
mod resume_log_why;
mod rpc_atlas;
mod rpc_contract;
mod run;
mod run_reporting;
mod session;
mod skills;
mod surface;
mod tui_handoff;
mod user_dir;
mod vault;

// The rpc helpers stay beside the contract tests they were written for, and
// come in here so every module's `use super::*` reaches them: moved above,
// they would put this file past the 600 lines #938's Verify allows.
use rpc_contract::{
    carries_each, client_contract, contract_paths, rpc_answer, rpc_records, rpc_records_env,
};
