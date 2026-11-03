use super::*;

// ---------------------------------------------------------------------------
// ignored files in a model run (ADR 0023 §5, ADR 0026 §11)
// ---------------------------------------------------------------------------

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
