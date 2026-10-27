use super::*;

// ---------------------------------------------------------------------------
// the handoff to trellis-tui (ADR 0052 decision 2.2, #514)
// ---------------------------------------------------------------------------

/// This build's `trellis`, hard-linked (copied where a link cannot be made)
/// into `dir`, so a `trellis-tui` can sit beside it without touching
/// `target/debug`, which every other test shares.
fn trellis_in(dir: &Path) -> PathBuf {
    let to = dir.join(format!("trellis{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(env!("CARGO_BIN_EXE_trellis"), &to).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_trellis"), &to).unwrap();
    }
    to
}

/// Runs `trellis` (the path given) in `root` with `args` and extra
/// environment, as `run` does. A script written a moment ago can be busy
/// for exec while a concurrent test's fork holds it open (ETXTBSY), so that
/// one failure is retried.
fn run_at(trellis: &Path, root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    for _ in 0..5 {
        let mut c = Command::new(trellis);
        c.args(args).current_dir(root).env("NO_COLOR", "1").env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        );
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().expect("the binary must be built");
        if !String::from_utf8_lossy(&out.stderr).contains("Text file busy") {
            return out;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("trellis-tui stayed busy for exec");
}

#[cfg(unix)]
#[test]
fn launch_a_tui_beside_the_binary_gets_the_root_this_trellis_the_model_and_the_task() {
    // The risk: a handoff that starts some other trellis's rpc, drops the
    // user's --model or task, or loses the exit status.
    let repo = empty_repo("launch-args");
    let bin = scratch("launch-args-bin");
    let me = trellis_in(&bin);
    stub_tui(&bin);
    let out = run_at(
        &me,
        &repo,
        &[
            "--root",
            &repo.display().to_string(),
            "tui",
            "--model",
            "scripted:/x.json",
            "--",
            "-change f",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(7), "{}", combined(&out));
    let args = std::fs::read_to_string(bin.join("args.txt")).unwrap();
    let want = [
        "--root".to_string(),
        repo.display().to_string(),
        "--trellis".to_string(),
        me.display().to_string(),
        "--model".to_string(),
        "scripted:/x.json".to_string(),
        "--".to_string(),
        "-change f".to_string(),
    ];
    assert_eq!(args.lines().collect::<Vec<_>>(), want, "{args}");
    let _ = std::fs::remove_dir_all(&bin);
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn launch_a_tui_on_path_but_not_beside_the_binary_is_not_used() {
    // The risk: PATH searched, so another build, or another program called
    // trellis-tui, is handed the terminal and the user's keys.
    let repo = empty_repo("launch-path");
    let bin = scratch("launch-path-bin");
    let me = trellis_in(&bin);
    let elsewhere = scratch("launch-path-elsewhere");
    #[cfg(unix)]
    stub_tui(&elsewhere);
    #[cfg(not(unix))]
    std::fs::write(elsewhere.join("trellis-tui.exe"), b"").unwrap();
    let path = std::env::join_paths(std::iter::once(elsewhere.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let out = run_at(
        &me,
        &repo,
        &["tui", "--model", "scripted:/x.json"],
        &[("PATH", path.to_str().unwrap())],
    );
    assert!(
        !elsewhere.join("args.txt").exists(),
        "the trellis-tui on PATH ran: {}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("trellis-tui is not installed beside this trellis"),
        "{}",
        combined(&out)
    );
    for d in [&bin, &elsewhere, &repo] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn launch_without_a_tui_beside_it_refuses_naming_where_it_looked_and_the_session() {
    // The risk: a missing trellis-tui leaves `trellis tui` doing nothing,
    // or refusing without saying where to put it and what still runs. The
    // Rust TUI fallback that ran here for one release is retired (#516), so
    // nothing opens: the refusal is the whole answer, and it exits non-zero.
    let repo = empty_repo("launch-missing");
    let bin = scratch("launch-missing-bin");
    let me = trellis_in(&bin);
    let out = run_at(&me, &repo, &["tui", "--model", "scripted:/x.json"], &[]);
    let said = combined(&out);
    let looked = bin.join(format!("trellis-tui{}", std::env::consts::EXE_SUFFIX));
    assert!(
        said.contains(&format!(
            "trellis-tui is not installed beside this trellis (looked for {})",
            looked.display()
        )),
        "{said}"
    );
    assert!(
        said.contains("`trellis session` is the line interface"),
        "{said}"
    );
    assert!(!said.contains("opening"), "{said}");
    // Nothing ran after the refusal: the pipe refusal the Rust TUI gave
    // belongs to the picker now, which a `--model` never reaches.
    assert!(!said.contains("needs a terminal"), "{said}");
    assert!(!out.status.success(), "{said}");
    let _ = std::fs::remove_dir_all(&bin);
    let _ = std::fs::remove_dir_all(&repo);
}

#[cfg(unix)]
#[test]
fn launch_first_run_on_a_pipe_refuses_before_the_picker_and_the_handoff() {
    // The risk: `trellis tui` with no model configured, on a pipe, with a
    // trellis-tui beside it. The picker runs in Rust before the handoff and
    // waits on console input that a pipe never sends, so the refusal the
    // Rust TUI gave here has to survive its retirement (#516): without it
    // the command hangs, or hands a pipe to trellis-tui. Nothing may run
    // after the refusal, so the stub must not have been started.
    let repo = empty_repo("launch-pipe");
    let bin = scratch("launch-pipe-bin");
    let me = trellis_in(&bin);
    stub_tui(&bin);
    let out = run_at(&me, &repo, &["tui"], &[]);
    let said = combined(&out);
    assert!(said.contains("needs a terminal"), "{said}");
    assert!(said.contains("`trellis run`"), "{said}");
    assert!(!out.status.success(), "{said}");
    assert!(
        !bin.join("args.txt").exists(),
        "trellis-tui ran on a pipe: {said}"
    );
    assert!(
        !said.contains("No model is configured. These are running"),
        "the picker ran: {said}"
    );
    let _ = std::fs::remove_dir_all(&bin);
    let _ = std::fs::remove_dir_all(&repo);
}

#[cfg(unix)]
#[test]
fn launch_bare_with_a_terminal_and_no_tui_refuses_naming_the_session() {
    // The risk: bare `trellis` in a repository at a terminal, with no
    // desktop and no trellis-tui beside it, opening something anyway or
    // exiting 0 having opened nothing. After #516 it prints where
    // trellis-tui was looked for and that `trellis session` is the line
    // interface, after the desktop line and the overview, and exits
    // non-zero. A copy, so nothing built beside the original is found. No
    // model is configured: the refusal comes before first-run's picker,
    // which would otherwise wait here for a choice that has nowhere to go.
    let d = repo("pty-bare-no-tui");
    let (_bin, exe) = trellis_copy("pty-bare-no-tui-bin");
    let (ok, s) = under_pty_as(&exe, "pty-bare-no-tui", &d, "", "");
    assert!(!ok, "{s}");
    let desktop = s
        .find("the desktop is not installed beside this trellis")
        .unwrap_or_else(|| panic!("no desktop line: {s}"));
    let overview = s.find("recent repositories:").expect(&s);
    let looked = exe.parent().unwrap().join("trellis-tui");
    let tui = s
        .find(&format!(
            "trellis-tui is not installed beside this trellis (looked for {})",
            looked.display()
        ))
        .unwrap_or_else(|| panic!("no trellis-tui line: {s}"));
    assert!(desktop < overview && overview < tui, "{s}");
    assert!(s.contains("`trellis session` is the line interface"), "{s}");
    assert!(!s.contains("opening"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn launch_the_handoff_passes_the_environment_unchanged() {
    // The owner's answer on #514 (ADR 0052, amendment of 2026-10-07): the
    // handoff passes trellis's environment unchanged, key variables
    // included, because trellis-tui's own `trellis rpc` needs them for a
    // hosted model the user chose. Every other child stays scrubbed
    // (tests/child_env.rs).
    let repo = empty_repo("launch-env");
    let bin = scratch("launch-env-bin");
    let me = trellis_in(&bin);
    stub_tui(&bin);
    let canary = format!("canary-{}-{}", std::process::id(), "launch");
    let out = run_at(
        &me,
        &repo,
        &["tui", "--model", "scripted:/x.json"],
        &[
            ("OPENAI_API_KEY", &canary),
            ("ANTHROPIC_API_KEY", &canary),
            ("TRELLIS_LAUNCH_PROBE", "kept"),
        ],
    );
    assert_eq!(out.status.code(), Some(7), "{}", combined(&out));
    let env = std::fs::read_to_string(bin.join("env.txt")).unwrap();
    for want in [
        format!("OPENAI_API_KEY={canary}"),
        format!("ANTHROPIC_API_KEY={canary}"),
        "TRELLIS_LAUNCH_PROBE=kept".to_string(),
    ] {
        assert!(env.lines().any(|l| l == want), "{want} missing:\n{env}");
    }
    let _ = std::fs::remove_dir_all(&bin);
    let _ = std::fs::remove_dir_all(&repo);
}
