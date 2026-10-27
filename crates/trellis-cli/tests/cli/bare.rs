use super::*;

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

#[cfg(unix)]
#[test]
fn startup_pty_bare_trellis_outside_a_repository_asks_where_to_go() {
    // The start screen, not the refusal: with a person at a terminal, bare
    // `trellis` outside a repository offers somewhere to go, and `q` leaves.
    // A trellis-tui beside the binary, since the screen's answer is handed
    // to it and without one the refusal comes first (#516).
    let d = scratch("pty-no-repo");
    let (_bin, exe) = trellis_with_stub_tui("pty-no-repo-bin");
    let (ok, s) = under_pty_as(&exe, "pty-no-repo", &d, "", "q\n");
    assert!(ok, "quitting the start screen is not a failure: {s}");
    assert!(s.contains("is not inside a git repository"), "{s}");
    assert!(!s.contains("no git repository found"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn startup_pty_bare_trellis_outside_a_repository_with_no_tui_refuses_before_asking() {
    // The risk: the start screen asking where to go, then the handoff
    // refusing with the answer in hand (#516). With no desktop and no
    // trellis-tui beside the binary the refusal comes first, names where
    // trellis-tui was looked for and `trellis session`, and no screen is
    // offered.
    let d = scratch("pty-no-repo-no-tui");
    let (_bin, exe) = trellis_copy("pty-no-repo-no-tui-bin");
    let (ok, s) = under_pty_as(&exe, "pty-no-repo-no-tui", &d, "", "q\n");
    assert!(!ok, "{s}");
    assert!(
        s.contains("trellis-tui is not installed beside this trellis"),
        "{s}"
    );
    assert!(s.contains("`trellis session` is the line interface"), "{s}");
    assert!(!s.contains("or q to quit"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
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
    let out_dir = scratch("pty-session-out");
    let out = out_dir.join("stdout.txt");
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

/// [`trellis_copy`] with a stub `trellis-tui` beside it (the one
/// `stub_tui` writes), so the start screen has something to hand its answer
/// to and is offered rather than refused (#516).
#[cfg(unix)]
fn trellis_with_stub_tui(name: &str) -> (Scratch, PathBuf) {
    let (dir, exe) = trellis_copy(name);
    stub_tui_ready(&dir);
    (dir, exe)
}

/// [`stub_tui`] in `dir`, run once so it is executable, with what that run
/// wrote (`--version` as its arguments) removed, so a reader of `args.txt`
/// there sees only what trellis passed.
#[cfg(unix)]
fn stub_tui_ready(dir: &Path) {
    let stub = stub_tui(dir);
    wait_until_executable(&stub);
    let _ = std::fs::remove_file(dir.join("args.txt"));
    let _ = std::fs::remove_file(dir.join("env.txt"));
}

/// [`trellis_copy`] with a stub `trellis-desktop` beside it that writes its
/// arguments, one per line, to `args.txt` there (through a rename, so a
/// reader never sees half of it). A copy, so no stub ever sits beside the
/// build's own binary.
#[cfg(unix)]
fn trellis_with_stub_desktop(name: &str) -> (Scratch, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (dir, exe) = trellis_copy(name);
    let stub = dir.join("trellis-desktop");
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
    let _ = std::fs::remove_file(dir.join("args.txt"));
    (dir, exe)
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
    let (_bin, exe) = trellis_with_stub_desktop("pty-bare-desktop-bin");
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
    let (_bin, exe) = trellis_with_stub_desktop("pty-bare-desktop-no-repo-bin");
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
    // a desktop built beside the original cannot open a window here; a
    // stub trellis-tui beside it, so the screen is offered (#516).
    let d = scratch("pty-bare-no-desktop");
    let (_bin, exe) = trellis_with_stub_tui("pty-bare-no-desktop-bin");
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
    // with no overview and no desktop started. A stub trellis-tui beside
    // the binary, so the screen is offered rather than refused (#516).
    let d = scratch("pty-tui-flag");
    let (_bin, exe) = trellis_with_stub_desktop("pty-tui-flag-bin");
    stub_tui_ready(exe.parent().unwrap());
    let (ok, s) = under_pty_as(&exe, "pty-tui-flag", &d, " --tui", "q\n");
    assert!(ok, "{s}");
    assert!(s.contains("or q to quit"), "{s}");
    assert!(!s.contains("recent repositories:"), "{s}");
    assert!(stub_args(&exe, std::time::Duration::from_secs(2)).is_none());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}
