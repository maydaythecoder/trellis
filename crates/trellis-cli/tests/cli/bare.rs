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
#[cfg_attr(not(target_os = "linux"), ignore = "only Linux asks for a display")]
#[test]
fn startup_pty_bare_with_no_display_says_so_and_starts_no_desktop() {
    // The risk (#248): `launch::display` is the one place that reads
    // DISPLAY and WAYLAND_DISPLAY, and read as always true it starts the
    // desktop on a headless Linux box, where the window fails out of sight
    // and the terminal is left with nothing. With neither variable set,
    // bare `trellis` says so in one line and the desktop beside it, a stub
    // that records its arguments, is never run; a stub trellis-tui beside
    // it too, so the fallback offers the start screen and `q` leaves.
    let d = scratch("pty-bare-no-display");
    let (_bin, exe) = trellis_with_stub_desktop("pty-bare-no-display-bin");
    stub_tui_ready(exe.parent().unwrap());
    let (ok, s) = under_pty_with_display(&exe, "pty-bare-no-display", &d, "", "q\n", false);
    assert!(ok, "{s}");
    assert!(
        s.contains("no display to open the desktop on: neither DISPLAY nor WAYLAND_DISPLAY is set"),
        "{s}"
    );
    assert!(!s.contains("the desktop is not installed"), "{s}");
    assert!(s.contains("or q to quit"), "{s}");
    assert_eq!(
        stub_args(&exe, std::time::Duration::from_secs(3)),
        None,
        "the desktop was started with no display: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn launch_dsk_and_web_hand_the_desktop_a_root_only_when_one_was_found() {
    // ADR 0050 §1 and §5: `--dsk` and `--web` pass `--root` to the desktop
    // when the root was named (`--root`) or found (a `.git` at or above the
    // working directory), and not when trellis fell back to the working
    // directory outside any repository (`explicit || root.join(".git")
    // .exists()`, once for each flag). The `&&` mutant of either line drops
    // `--root` for a found repository and for a named plain tree, so the
    // desktop opens on its overview instead of the tree the person is in.
    // Read from the stub desktop's recorded arguments, under pipes, with
    // the display the Linux check asks for; the stub writes its arguments
    // and exits, which for `--web` is "exited before printing its URL", so
    // only the arguments are asserted.
    let (_bin, exe) = trellis_with_stub_desktop("dsk-web-root-bin");
    let found = repo("dsk-web-root-found");
    let plain = scratch("dsk-web-root-plain");
    let outside = scratch("dsk-web-root-outside");
    let user = scratch("dsk-web-root-user");
    let args_file = exe.parent().unwrap().join("args.txt");
    // The stub runs detached and the suite may be under load (cargo mutants
    // runs test binaries side by side), the same 20 s the pty tests allow.
    let wait = std::time::Duration::from_secs(20);
    for flag in ["--dsk", "--web"] {
        // (name, where it runs, extra arguments, the --root the desktop gets)
        let cases: [(&str, &Path, Vec<String>, Option<&Path>); 3] = [
            ("found", &found, vec![], Some(&found)),
            (
                "named",
                &plain,
                vec!["--root".into(), plain.display().to_string()],
                Some(&plain),
            ),
            ("outside", &outside, vec![], None),
        ];
        for (name, cwd, extra, want) in cases {
            let _ = std::fs::remove_file(&args_file);
            let out = Command::new(&exe)
                .args(&extra)
                .arg(flag)
                .current_dir(cwd)
                .env("NO_COLOR", "1")
                .env_remove("RUST_LOG")
                // The user's own file is read on this path too; a scratch
                // one, so the suite runner's config cannot change the answer.
                .env("TRELLIS_CONFIG_DIR", &user)
                .env("DISPLAY", ":0")
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            let args = stub_args(&exe, wait)
                .unwrap_or_else(|| panic!("{flag} {name}: the stub desktop never ran: {err}"));
            let root = args
                .iter()
                .position(|a| a == "--root")
                .map(|i| args[i + 1].clone());
            match want {
                Some(w) => {
                    let got =
                        root.unwrap_or_else(|| panic!("{flag} {name}: no --root in {args:?}"));
                    assert!(
                        same_file(&got, w),
                        "{flag} {name}: --root {got} is not {}",
                        w.display()
                    );
                }
                None => assert!(
                    root.is_none(),
                    "{flag} {name}: --root passed outside a repository: {args:?}"
                ),
            }
        }
    }
    for d in [&found, &plain, &outside, &user] {
        let _ = std::fs::remove_dir_all(d);
    }
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

// ---------------------------------------------------------------------------
// `trellis --web` waited for, and the human stream's colour (#248, Unix only)
// ---------------------------------------------------------------------------

/// [`trellis_copy`] with a stub `trellis-desktop` beside it that prints the
/// web view's URL line and then runs `then`, and a directory holding only a
/// stub browser opener (`open` on macOS, `xdg-open` elsewhere, as
/// `launch::opener` names them) that exits `opener_exit`. That directory is
/// the whole of the PATH the caller gives trellis, so no real browser opens
/// and nothing else on the host is started. The stubs use only `sh`
/// builtins for the same reason.
#[cfg(unix)]
fn trellis_web(name: &str, then: &str, opener_exit: i32) -> (Scratch, PathBuf, Scratch) {
    use std::os::unix::fs::PermissionsExt;
    let (dir, exe) = trellis_copy(name);
    let stub = dir.join("trellis-desktop");
    std::fs::write(
        &stub,
        format!("#!/bin/sh\nprintf '%s\\n' '{{\"url\":\"http://127.0.0.1:7/?t=stub\"}}'\n{then}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(&stub);
    let tools = scratch(&format!("{name}-path"));
    let opener = tools.join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    std::fs::write(&opener, format!("#!/bin/sh\nexit {opener_exit}\n")).unwrap();
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(&opener);
    (dir, exe, tools)
}

/// `trellis --web` from `exe`, outside any repository, with `path` as its
/// whole PATH and a scratch user directory.
#[cfg(unix)]
fn web_command(exe: &Path, path: &Path, user: &Path) -> Command {
    let outside = exe.parent().unwrap();
    let mut c = Command::new(exe);
    c.arg("--web")
        .current_dir(outside)
        .env("PATH", path)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env("TRELLIS_CONFIG_DIR", user);
    c
}

#[cfg(unix)]
#[test]
fn launch_web_says_the_browser_opened_and_exits_as_the_web_view_did() {
    // The risk (#248, `open_in_browser` and `launch_web`): a browser that
    // could not be opened reported as opened, so the person waits for a
    // page that never comes instead of opening the URL by hand; an opener
    // that worked reported as failed; and `trellis --web` exiting 0 when the
    // web view failed, or 1 when it ended cleanly. Neither the opener nor
    // the web view's exit was reached by a test: the unit tests drive
    // `attend` and `web_url`, not `launch_web`.
    let user = scratch("web-exit-user");

    let (bin, exe, path) = trellis_web("web-exit-ok", "exit 0", 0);
    let o = web_command(&exe, &path, &user).output().unwrap();
    let said = combined(&o);
    assert!(o.status.success(), "{said}");
    assert!(
        said.contains("serving the desktop's page at http://127.0.0.1:7/?t=stub"),
        "{said}"
    );
    assert!(said.contains("opened in the browser"), "{said}");
    assert!(!said.contains("could not open the browser"), "{said}");
    drop((bin, path));

    let (bin, exe, path) = trellis_web("web-exit-fail", "exit 3", 1);
    let o = web_command(&exe, &path, &user).output().unwrap();
    let said = combined(&o);
    assert!(!o.status.success(), "{said}");
    assert!(said.contains("the web view exited with"), "{said}");
    assert!(said.contains("could not open the browser"), "{said}");
    assert!(said.contains("exited with exit status: 1"), "{said}");
    assert!(!said.contains("opened in the browser"), "{said}");
    drop((bin, path));
    let _ = std::fs::remove_dir_all(&user);
}

#[cfg(unix)]
#[test]
fn launch_web_ctrl_c_stops_the_web_view_and_waits_for_it() {
    // The risk (#248, `ctrl_c_presses`): Ctrl-C not reaching `attend`, so
    // the signal's default action kills trellis with the web view still
    // holding its rpc child, which is never sent `quit` (ADR 0048 §9).
    // `kill -INT`, as tests/interrupt.rs sends it. The stub web view stops
    // when its stdin closes, as the desktop does.
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let user = scratch("web-ctrl-c-user");
    let (_bin, exe, path) = trellis_web("web-ctrl-c", "read -r line\nexit 0", 0);
    let mut child = web_command(&exe, &path, &user)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            all.push_str(&line);
            all.push('\n');
            let _ = tx.send(line);
        }
        all
    });
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(l) if l.contains("Ctrl-C stops the web view") => break,
            Ok(_) => {}
            Err(_) => {
                let _ = child.kill();
                panic!("`trellis --web` never said Ctrl-C stops the web view");
            }
        }
    }
    // The listener is spawned just after that line; the pause covers the
    // runtime polling it once, which registers the handler.
    std::thread::sleep(Duration::from_millis(500));
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill(1) is on PATH")
        .success();
    assert!(sent, "kill -INT failed");
    let until = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > until {
            let _ = child.kill();
            panic!("`trellis --web` did not stop within 20 s of Ctrl-C");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let o = child.wait_with_output().unwrap();
    let said = reader.join().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        o.status.success(),
        "stopped by Ctrl-C, not by the signal ({:?}):\n{said}{err}",
        o.status
    );
    assert!(err.contains("stopping the web view"), "{said}{err}");
    let _ = std::fs::remove_dir_all(&user);
}

#[cfg(unix)]
#[test]
fn startup_pty_the_human_stream_is_coloured_at_a_terminal_without_no_color() {
    // The risk (#248, `output::human_colour`): a terminal with NO_COLOR
    // unset losing the colour of a warning's level, so a WARN reads like
    // any other line. `colour`'s own test holds the rule; this holds that
    // the stream's real terminal answer reaches it. A user config with an
    // `[agents]` table gives a WARN on any command (#958). util-linux
    // `script` on Linux and the BSD one on macOS take different arguments.
    use std::process::Stdio;
    let d = repo("colour-pty");
    let user = scratch("colour-pty-user");
    std::fs::write(user.join("config.toml"), "[agents]\ncount = 2\n").unwrap();
    let exe = env!("CARGO_BIN_EXE_trellis");
    let args: Vec<String> = if cfg!(target_os = "linux") {
        vec!["-qfec".into(), format!("'{exe}' log"), "/dev/null".into()]
    } else {
        vec!["-q".into(), "/dev/null".into(), exe.into(), "log".into()]
    };
    let run = |no_color: bool| {
        let mut c = Command::new("script");
        c.args(&args)
            .current_dir(&d)
            .env_remove("RUST_LOG")
            .env("TRELLIS_CONFIG_DIR", &user)
            .stdin(Stdio::null());
        if no_color {
            c.env("NO_COLOR", "1");
        } else {
            c.env_remove("NO_COLOR");
        }
        let o = c
            .output()
            .expect("`script` must be on PATH for the pty tests");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let shown = run(false);
    assert!(shown.contains("has an [agents] table"), "{shown:?}");
    assert!(
        shown.contains("\u{1b}[33m"),
        "no colour at a terminal: {shown:?}"
    );
    let shown = run(true);
    assert!(shown.contains("has an [agents] table"), "{shown:?}");
    assert!(
        !shown.contains("\u{1b}["),
        "colour under NO_COLOR: {shown:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}
