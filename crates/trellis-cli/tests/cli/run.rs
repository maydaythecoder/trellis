use super::*;

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
fn sandbox_is_named_on_the_repo_line_and_by_doctor_alike() {
    // ADR 0055 (#621). A sandbox that is missing, or present and failing,
    // is a warning and not a refusal, so what trellis prints is the only
    // place a person learns that `bash` ran unconfined. The risk: a run
    // that says nothing, or names a sandbox doctor does not, or the
    // reverse.
    let d = repo("run-sandbox-line");
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "script"]);
    let s = stdout(&run(&d, &["run", "--model", &script, "a task"]));
    let label = s
        .lines()
        .find_map(|l| l.strip_prefix("repo:     sandbox: "))
        .unwrap_or_else(|| panic!("no sandbox on the repo: line:\n{s}"))
        .to_string();
    let doctor = stdout(&run(&d, &["doctor"]));
    let line = doctor
        .lines()
        .find(|l| l.starts_with("sandbox:"))
        .unwrap_or_else(|| panic!("doctor names no sandbox:\n{doctor}"));
    let expected = if cfg!(target_os = "linux") {
        "bwrap"
    } else if cfg!(target_os = "macos") {
        "sandbox-exec"
    } else {
        "Windows"
    };
    if label.starts_with("none (") {
        assert!(label.contains(expected), "{label}");
        assert!(
            line.starts_with(&format!("sandbox:    {label}")),
            "{line}\n{label}"
        );
    } else {
        assert_eq!(label, expected);
        // The binary doctor found, by path.
        assert!(
            line.starts_with(&format!("sandbox:    {label} at /")),
            "{line}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(target_os = "linux")]
#[test]
fn sandbox_missing_or_failing_is_a_warning_that_names_the_reason() {
    // The owner's decision on #621: `bwrap` absent, or present and failing
    // (Ubuntu 24.04's apparmor_restrict_unprivileged_userns refuses the
    // user namespace), is a warning, never a refusal, and the reason is
    // printed. A stand-in `bwrap` first on PATH fails as that one does.
    use std::os::unix::fs::PermissionsExt;
    let d = repo("run-sandbox-failing");
    let script = script_that_stops(&d);
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "script"]);
    let bin = d.with_extension("bin");
    let _ = std::fs::remove_dir_all(&bin);
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("bwrap");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'bwrap: setting up uid map: Permission denied' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let with_path = |args: &[&str], path: &str| {
        Command::new(env!("CARGO_BIN_EXE_trellis"))
            .args(args)
            .current_dir(&d)
            .env("NO_COLOR", "1")
            .env(
                "TRELLIS_CONFIG_DIR",
                std::env::temp_dir().join("trellis-tests-no-user-config"),
            )
            .env("PATH", path)
            .output()
            .unwrap()
    };
    let failing = "none (bwrap failed: bwrap: setting up uid map: Permission denied)";
    let doctor = stdout(&with_path(&["doctor"], &path));
    assert!(
        doctor.contains(&format!("sandbox:    {failing}")),
        "{doctor}"
    );
    let out = with_path(&["run", "--model", &script, "a task"], &path);
    let s = stdout(&out);
    assert!(s.contains(&format!("repo:     sandbox: {failing}")), "{s}");
    // Warned, not refused: the run went ahead to its end.
    assert!(s.contains("NOT CONVERGED"), "{}", combined(&out));

    let empty = d.with_extension("empty");
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let doctor = stdout(&with_path(&["doctor"], &empty.display().to_string()));
    assert!(
        doctor.contains("sandbox:    none (bwrap not found)"),
        "{doctor}"
    );
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
