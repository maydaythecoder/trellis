//! Starting the desktop from `trellis --dsk` and `trellis --web` (ADR 0050
//! sections 1 and 5, ADR 0048).
//!
//! The desktop is looked for beside the running `trellis` and nowhere else:
//! the two are installed together, and a `trellis-desktop` found on PATH
//! could be another build speaking another client contract (ADR 0048
//! section 6), or another program with the name.
//!
//! For `--dsk` the child is started detached and not waited for: `trellis`
//! returns once it has started, and the window outlives the terminal it was
//! started from. For `--web` it is started with `--web` and waited for: it
//! serves the page on 127.0.0.1 and prints the URL, which `trellis` reads
//! from its stdout, prints and opens in the browser; Ctrl-C closes the
//! child's stdin, which is how the server is told to stop, and `trellis`
//! waits for it to exit, so the rpc child it runs is sent `quit` and
//! finishes rolling back (ADR 0048 section 9). In both the environment is
//! trellis's own, unchanged, because the desktop starts its own `trellis
//! rpc`, which needs the user's key variables for a hosted model the user
//! chose (ADR 0048 section 3); the one spawn below that is not the desktop,
//! the browser opener, is scrubbed like every other child.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The desktop's file name on this platform.
const DESKTOP: &str = if cfg!(windows) {
    "trellis-desktop.exe"
} else {
    "trellis-desktop"
};

/// The desktop's path beside `exe`, the running `trellis`, or the places
/// looked in. PATH is never searched.
pub(crate) fn find_desktop(exe: &Path) -> Result<PathBuf, Vec<PathBuf>> {
    let beside = exe.parent().unwrap_or(Path::new("")).join(DESKTOP);
    if beside.is_file() {
        Ok(beside)
    } else {
        Err(vec![beside])
    }
}

/// What `trellis --dsk` says when there is no desktop to start.
fn not_found(tried: &[PathBuf]) -> String {
    let places: Vec<String> = tried
        .iter()
        .map(|p| format!("    {}", p.display()))
        .collect();
    format!(
        "the desktop is not installed beside this trellis; looked for:\n{}\n\
         Install both from a checkout of the repository:\n    \
         cargo install --locked --path crates/trellis-cli\n    \
         cargo install --locked --path desktop",
        places.join("\n")
    )
}

/// How every line bare `trellis` prints instead of starting the desktop
/// ends: what it does next (ADR 0050 section 1, #561).
const FALLING_BACK: &str = "falling back to the terminal";

/// [`not_found`] on one line, for bare `trellis`, where the overview and
/// the terminal interface follow it and a paragraph would scroll away.
fn not_found_line(tried: &[PathBuf]) -> String {
    let places: Vec<String> = tried.iter().map(|p| p.display().to_string()).collect();
    format!(
        "the desktop is not installed beside this trellis (looked for {}); install both with \
         `cargo install --locked --path crates/trellis-cli` and \
         `cargo install --locked --path desktop`; {FALLING_BACK}",
        places.join(", ")
    )
}

/// Whether there is a display to open the desktop on: on Linux, `DISPLAY`
/// or `WAYLAND_DISPLAY`; elsewhere there always is.
pub(crate) fn display() -> bool {
    !cfg!(target_os = "linux")
        || display_present(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        )
}

/// Bare `trellis` at a terminal (ADR 0050 section 1, as built 2026-10-07,
/// #561): starts `found` detached, as `--dsk` does, or returns the one line
/// saying why it did not, for the caller to print before falling back.
pub(crate) fn launch_bare(
    found: &Result<PathBuf, Vec<PathBuf>>,
    display: bool,
    trellis: &Path,
    root: Option<&Path>,
) -> Result<(), String> {
    if !display {
        return Err(format!(
            "no display to open the desktop on: neither DISPLAY nor WAYLAND_DISPLAY is set; \
             {FALLING_BACK}"
        ));
    }
    let desktop = found.as_ref().map_err(|tried| not_found_line(tried))?;
    spawn_desktop(desktop, trellis, root).map_err(|e| format!("{e:#}; {FALLING_BACK}"))
}

/// Whether a Linux desktop session is there to show a window on.
fn display_present(display: Option<&std::ffi::OsStr>, wayland: Option<&std::ffi::OsStr>) -> bool {
    display.is_some_and(|v| !v.is_empty()) || wayland.is_some_and(|v| !v.is_empty())
}

/// The one command line the desktop is started with, for the window or
/// (`web`) the web view: `--trellis <this trellis>`, `--root` only when
/// there is a root to pass (without one the desktop shows its overview,
/// ADR 0050 section 3), and its own process group, so the terminal's
/// signals stay with trellis: the window outlives the terminal, and the
/// web view is told to stop by its stdin, not by a signal that would reach
/// its rpc child too.
fn desktop_command(desktop: &Path, trellis: &Path, root: Option<&Path>, web: bool) -> Command {
    let mut cmd = Command::new(desktop); // key-exempt: trellis-desktop
    cmd.arg("--trellis").arg(trellis);
    if let Some(r) = root {
        cmd.arg("--root").arg(r);
    }
    if web {
        cmd.arg("--web");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Win32 DETACHED_PROCESS and CREATE_NEW_PROCESS_GROUP: no console of
        // ours, and a Ctrl+C in the terminal that started it does not reach it.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Starts `desktop` detached and returns without waiting for it.
pub(crate) fn spawn_desktop(desktop: &Path, trellis: &Path, root: Option<&Path>) -> Result<()> {
    let mut cmd = desktop_command(desktop, trellis, root, false);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Dropped, not waited for: no job object ties the window to trellis.
    cmd.spawn()
        .with_context(|| format!("cannot start {}", desktop.display()))?;
    Ok(())
}

/// Starts `desktop --web` with its stdin and stdout piped: the URL comes
/// out of the one, and closing the other stops it. Its stderr is the
/// terminal's, so what it says is seen. Not killed on drop: the server is
/// stopped by its stdin, so its rpc child is sent `quit` and waited for.
pub(crate) fn spawn_web(
    desktop: &Path,
    trellis: &Path,
    root: Option<&Path>,
) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::from(desktop_command(desktop, trellis, root, true));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(false);
    cmd.spawn()
        .with_context(|| format!("cannot start {}", desktop.display()))
}

/// The URL the web view printed: the `url` of the one JSON object on the
/// first line of its stdout, or why that line is not one.
pub(crate) fn web_url(first_line: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(first_line.trim()).with_context(|| {
        format!(
            "the web view printed something other than its URL: {}",
            printable(first_line)
        )
    })?;
    let url = v
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|u| u.starts_with("http://127.0.0.1:"))
        .with_context(|| {
            format!(
                "the web view printed no 127.0.0.1 URL: {}",
                printable(first_line)
            )
        })?;
    Ok(url.to_string())
}

/// A line the desktop printed, as it is shown: printable ASCII, cut short.
fn printable(line: &str) -> String {
    line.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(200)
        .collect()
}

/// The program this platform opens a URL with, and its arguments before
/// the URL. On Windows `rundll32 url.dll,FileProtocolHandler` takes the URL
/// as one argument, where `cmd /c start` would read it as a command line.
fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else if cfg!(target_os = "macos") {
        ("open", &[])
    } else {
        ("xdg-open", &[])
    }
}

/// Opens `url` in the user's browser through the platform's opener, which
/// is given none of the key variables (ADR 0024 §3). `Err` names the
/// opener, so the URL printed beside it can be opened by hand.
pub(crate) fn open_in_browser(url: &str) -> Result<()> {
    let (program, args) = opener();
    let mut cmd = Command::new(program);
    trellis_core::secrets::scrub(&mut cmd);
    cmd.args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = cmd
        .status()
        .with_context(|| format!("cannot run {program} to open the browser"))?;
    if !status.success() {
        anyhow::bail!("{program} exited with {status}");
    }
    Ok(())
}

/// What `attend` says on the first interrupt, before it closes the web
/// view's stdin.
const STOPPING: &str =
    "stopping the web view; a run in flight is rolled back and its trail written";

/// Waits for the web view. The first press on `presses` (Ctrl-C) closes
/// its stdin, which stops it, and the wait goes on so the rpc child it
/// runs is sent `quit` and finishes (ADR 0048 section 9); a second press
/// gives up waiting and returns `None`. `Some` is how the web view exited.
async fn attend(
    mut child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    mut presses: tokio::sync::mpsc::Receiver<()>,
) -> Result<Option<std::process::ExitStatus>> {
    let mut stdin = Some(stdin);
    loop {
        tokio::select! {
            status = child.wait() => {
                return Ok(Some(status.context("waiting for the web view")?));
            }
            press = presses.recv() => {
                if press.is_none() {
                    // No more presses can come: nothing is left to do but
                    // wait for the exit.
                    let status = child.wait().await.context("waiting for the web view")?;
                    return Ok(Some(status));
                }
                match stdin.take() {
                    Some(s) => {
                        eprintln!("{STOPPING}");
                        drop(s);
                    }
                    None => {
                        eprintln!("not waiting for the web view any longer");
                        return Ok(None);
                    }
                }
            }
        }
    }
}

/// One Ctrl-C after another, as a future that completes once per press.
async fn ctrl_c_presses(tx: tokio::sync::mpsc::Sender<()>) {
    while tokio::signal::ctrl_c().await.is_ok() {
        if tx.send(()).await.is_err() {
            return;
        }
    }
}

/// `trellis --dsk`: finds the desktop beside this executable and starts it.
pub(crate) fn launch_desktop(root: Option<&Path>) -> Result<()> {
    #[cfg(target_os = "linux")]
    if !display_present(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    ) {
        anyhow::bail!(
            "no display to open the desktop on: neither DISPLAY nor WAYLAND_DISPLAY is set"
        );
    }
    let exe = std::env::current_exe().context("cannot find the running trellis executable")?;
    let desktop = find_desktop(&exe).map_err(|tried| anyhow::anyhow!(not_found(&tried)))?;
    spawn_desktop(&desktop, &exe, root)
}

/// `trellis --web` (ADR 0050 section 5): finds the desktop beside this
/// executable, starts it as the web view, prints and opens the URL it
/// printed, and waits for it. Ctrl-C stops it and waits; a second Ctrl-C
/// stops waiting.
pub(crate) fn launch_web(root: Option<&Path>) -> Result<()> {
    use tokio::io::AsyncBufReadExt;
    let exe = std::env::current_exe().context("cannot find the running trellis executable")?;
    let desktop = find_desktop(&exe).map_err(|tried| anyhow::anyhow!(not_found(&tried)))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the runtime that waits for the web view")?;
    rt.block_on(async {
        let mut child = spawn_web(&desktop, &exe, root)?;
        let stdout = child.stdout.take().context("the web view has no stdout")?;
        let stdin = child.stdin.take().context("the web view has no stdin")?;
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let url = match lines.next_line().await? {
            Some(line) => web_url(&line)?,
            None => {
                let status = child.wait().await?;
                anyhow::bail!("the web view exited ({status}) before printing its URL");
            }
        };
        {
            let mut out = std::io::stdout().lock();
            writeln!(out, "serving the desktop's page at {url}")?;
            match open_in_browser(&url) {
                Ok(()) => writeln!(out, "opened in the browser; Ctrl-C stops the web view")?,
                Err(e) => writeln!(
                    out,
                    "could not open the browser ({e:#}); open the URL by hand; Ctrl-C stops the web view"
                )?,
            }
            out.flush()?;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(ctrl_c_presses(tx));
        match attend(child, stdin, rx).await? {
            Some(status) if status.success() => Ok(()),
            Some(status) => anyhow::bail!("the web view exited with {status}"),
            None => Ok(()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};

    /// A fake `trellis` in a fresh directory: the finder only reads its path.
    fn fake_exe(dir: &Path) -> PathBuf {
        let exe = dir.join(format!("trellis{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&exe, b"").unwrap();
        exe
    }

    #[test]
    fn launch_desktop_found_beside_the_binary() {
        // The risk: an installed pair that `--dsk` cannot start.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let desktop = d.path().join(DESKTOP);
        std::fs::write(&desktop, b"").unwrap();
        assert_eq!(find_desktop(&exe), Ok(desktop));
    }

    #[test]
    fn launch_desktop_a_path_copy_is_ignored() {
        // The risk: a `trellis-desktop` on PATH, another build or another
        // program, started with the user's keys in its environment.
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join(DESKTOP), b"").unwrap();
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let tried = find_desktop(&exe).unwrap_err();
        assert_eq!(tried, vec![d.path().join(DESKTOP)]);
        let said = not_found(&tried);
        assert!(!said.contains(&bin.path().display().to_string()), "{said}");
    }

    #[test]
    fn launch_desktop_not_found_names_where_it_looked_and_both_commands() {
        // The risk: a refusal that leaves someone guessing what to build.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let said = not_found(&find_desktop(&exe).unwrap_err());
        assert!(
            said.contains(&d.path().join(DESKTOP).display().to_string()),
            "{said}"
        );
        assert!(
            said.contains("cargo install --locked --path crates/trellis-cli"),
            "{said}"
        );
        assert!(
            said.contains("cargo install --locked --path desktop"),
            "{said}"
        );
    }

    #[test]
    fn launch_desktop_display_needs_either_variable() {
        // The risk: a spawn on a headless Linux box that fails out of sight.
        let x = Some(OsStr::new(":0"));
        let none = None;
        let empty = Some(OsStr::new(""));
        assert!(display_present(x, none));
        assert!(display_present(none, Some(OsStr::new("wayland-0"))));
        assert!(!display_present(none, none));
        assert!(!display_present(empty, empty));
    }

    /// A stub desktop that writes its arguments, one per line, to `args.txt`
    /// beside it (through a rename, so a reader never sees half of it).
    fn stub_desktop(dir: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let p = dir.join("stub.cmd");
            std::fs::write(
                &p,
                "@echo off\r\n\
                 (for %%a in (%*) do @echo %%~a) > \"%~dp0args.tmp\"\r\n\
                 move /y \"%~dp0args.tmp\" \"%~dp0args.txt\" >nul\r\n",
            )
            .unwrap();
            p
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("stub");
            std::fs::write(
                &p,
                "#!/bin/sh\nd=$(dirname \"$0\")\n\
                 printf '%s\\n' \"$@\" > \"$d/args.tmp\" && mv \"$d/args.tmp\" \"$d/args.txt\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }
    }

    /// The stub's arguments, once it has written them.
    fn args_written(dir: &Path) -> Vec<String> {
        let file = dir.join("args.txt");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            if let Ok(t) = std::fs::read_to_string(&file) {
                return t.lines().map(|l| l.trim().to_string()).collect();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the stub desktop never wrote {}", file.display());
    }

    #[test]
    fn launch_desktop_spawn_passes_trellis_and_root_only_when_found() {
        // The risk: a desktop started without the trellis it should run, or
        // with a `--root` naming a directory nobody found.
        let trellis = Path::new("trellis-under-test");
        let root = Path::new("repo-under-test");

        let with = tempfile::tempdir().unwrap();
        spawn_desktop(&stub_desktop(with.path()), trellis, Some(root)).unwrap();
        assert_eq!(
            args_written(with.path()),
            [
                "--trellis",
                "trellis-under-test",
                "--root",
                "repo-under-test"
            ]
        );

        let without = tempfile::tempdir().unwrap();
        spawn_desktop(&stub_desktop(without.path()), trellis, None).unwrap();
        assert_eq!(
            args_written(without.path()),
            ["--trellis", "trellis-under-test"]
        );
    }

    #[test]
    fn launch_bare_passes_the_trellis_and_the_root_it_found() {
        // The risk: bare `trellis` in a repository opening a window on no
        // repository, or one that runs some other trellis.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let repo = d.path().join("repo");
        let found = Ok(stub_desktop(d.path()));
        launch_bare(&found, true, &exe, Some(&repo)).unwrap();
        assert_eq!(
            args_written(d.path()),
            [
                "--trellis".to_string(),
                exe.display().to_string(),
                "--root".to_string(),
                repo.display().to_string()
            ]
        );
    }

    #[test]
    fn launch_bare_with_no_root_passes_no_root() {
        // The risk: outside a repository, a `--root` naming the directory
        // trellis fell back to, which the desktop would then read.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let found = Ok(stub_desktop(d.path()));
        launch_bare(&found, true, &exe, None).unwrap();
        assert_eq!(
            args_written(d.path()),
            ["--trellis".to_string(), exe.display().to_string()]
        );
    }

    #[test]
    fn launch_bare_not_found_is_one_line_naming_where_and_both_commands() {
        // The risk: a fallback that scrolls away above the overview, or that
        // leaves someone guessing what to install.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let said = launch_bare(&find_desktop(&exe), true, &exe, None).unwrap_err();
        assert!(!said.contains('\n'), "{said}");
        assert!(
            said.contains(&d.path().join(DESKTOP).display().to_string()),
            "{said}"
        );
        assert!(
            said.contains("cargo install --locked --path crates/trellis-cli"),
            "{said}"
        );
        assert!(
            said.contains("cargo install --locked --path desktop"),
            "{said}"
        );
    }

    #[test]
    fn launch_bare_with_no_display_starts_nothing_and_says_so() {
        // The risk: a spawn on a headless Linux box, the stub's argv file
        // proving it ran when it must not have.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let found = Ok(stub_desktop(d.path()));
        let said = launch_bare(&found, false, &exe, None).unwrap_err();
        assert!(!said.contains('\n'), "{said}");
        assert!(said.contains("DISPLAY"), "{said}");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!d.path().join("args.txt").exists());
    }

    /// A stub web view: writes its arguments to `args.txt` beside it as
    /// `stub_desktop` does, prints the URL line `trellis --web` reads, then
    /// waits for its stdin to end, as the real one does, and exits 0.
    fn stub_web(dir: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            // The wait is cmd's own `set /p`, which reads the stdin trellis
            // gave cmd.exe. A console program started from here would not:
            // cmd.exe runs DETACHED_PROCESS, so Windows gives its console
            // children a console of their own, whose keyboard is their
            // stdin. `more` waited there, past the close of our pipe.
            let p = dir.join("stub-web.cmd");
            std::fs::write(
                &p,
                "@echo off\r\n\
                 (for %%a in (%*) do @echo %%~a) > \"%~dp0args.tmp\"\r\n\
                 move /y \"%~dp0args.tmp\" \"%~dp0args.txt\" >nul\r\n\
                 echo {\"url\":\"http://127.0.0.1:7/?t=stub\"}\r\n\
                 set /p line=\r\n\
                 exit /b 0\r\n",
            )
            .unwrap();
            p
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("stub-web");
            std::fs::write(
                &p,
                "#!/bin/sh\nd=$(dirname \"$0\")\n\
                 printf '%s\\n' \"$@\" > \"$d/args.tmp\" && mv \"$d/args.tmp\" \"$d/args.txt\"\n\
                 printf '%s\\n' '{\"url\":\"http://127.0.0.1:7/?t=stub\"}'\n\
                 cat > /dev/null\n",
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn launch_web_passes_web_and_the_root_and_reads_the_url_it_prints() {
        // The risk (#375): a web view started as a window, on no root, or
        // whose URL trellis reads from the wrong line; and a server that
        // outlives `trellis --web` because its stdin was never closed.
        use tokio::io::AsyncBufReadExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let status = runtime().block_on(async {
            let mut child = spawn_web(
                &stub_web(d.path()),
                Path::new("trellis-under-test"),
                Some(&root),
            )
            .unwrap();
            let stdout = child.stdout.take().unwrap();
            let stdin = child.stdin.take().unwrap();
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let first = lines.next_line().await.unwrap().expect("a URL line");
            assert_eq!(web_url(&first).unwrap(), "http://127.0.0.1:7/?t=stub");
            // The stub waits on its stdin: the first press closes it, and
            // `attend` returns the exit, not `None`.
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(()).await.unwrap();
            // Bounded: a stub that misses the end of its stdin is a failure
            // here, not a wait that lasts until something else ends it.
            tokio::time::timeout(Duration::from_secs(60), attend(child, stdin, rx))
                .await
                .expect("the web view did not exit within 60 s of its stdin closing")
                .unwrap()
        });
        assert!(status.is_some_and(|s| s.success()), "{status:?}");
        assert_eq!(
            args_written(d.path()),
            [
                "--trellis".to_string(),
                "trellis-under-test".to_string(),
                "--root".to_string(),
                root.display().to_string(),
                "--web".to_string()
            ]
        );
    }

    #[test]
    fn launch_web_a_second_press_stops_waiting() {
        // The risk: a web view that does not stop on its stdin holding the
        // terminal for ever, with no way out but a kill from elsewhere.
        let d = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let stub = {
            use std::os::unix::fs::PermissionsExt;
            let p = d.path().join("stub-deaf");
            // Ignores its stdin, and ends on its own after a while, so a
            // failed test does not leave it behind.
            std::fs::write(
                &p,
                "#!/bin/sh\nprintf '%s\\n' '{\"url\":\"http://127.0.0.1:7/?t=stub\"}'\nsleep 30\n",
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        #[cfg(windows)]
        let stub = {
            let p = d.path().join("stub-deaf.cmd");
            std::fs::write(&p, "@echo off\r\necho {\"url\":\"http://127.0.0.1:7/?t=stub\"}\r\nping -n 30 127.0.0.1 >nul\r\n").unwrap();
            p
        };
        let (status, elapsed) = runtime().block_on(async {
            let mut child = spawn_web(&stub, Path::new("trellis-under-test"), None).unwrap();
            let stdin = child.stdin.take().unwrap();
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.send(()).await.unwrap();
            tx.send(()).await.unwrap();
            let start = Instant::now();
            (attend(child, stdin, rx).await.unwrap(), start.elapsed())
        });
        assert_eq!(status, None);
        assert!(
            elapsed < Duration::from_secs(20),
            "{elapsed:?}: waited for the deaf stub"
        );
    }

    #[test]
    fn launch_web_url_takes_only_a_127_0_0_1_url_from_the_first_line() {
        // The risk: trellis opening, in the user's browser, a URL the
        // desktop did not print as its own, or one on another address.
        assert_eq!(
            web_url("{\"url\":\"http://127.0.0.1:4321/?t=abc\"}\n").unwrap(),
            "http://127.0.0.1:4321/?t=abc"
        );
        for bad in [
            "",
            "not json",
            "{\"url\":\"http://0.0.0.0:4321/?t=abc\"}",
            "{\"url\":\"https://example.com/\"}",
            "{\"url\":\"http://localhost:4321/?t=abc\"}",
            "{\"other\":\"http://127.0.0.1:4321/\"}",
            "trellis-desktop: cannot start",
        ] {
            let said = format!("{:#}", web_url(bad).unwrap_err());
            assert!(said.contains("the web view printed"), "{bad}: {said}");
            assert!(!said.contains('\u{1b}'), "{bad}: {said}");
        }
        let said = format!("{:#}", web_url("\u{1b}[31mred\u{1b}[0m").unwrap_err());
        assert!(
            !said.contains('\u{1b}'),
            "a control sequence reached the terminal: {said}"
        );
    }

    #[test]
    fn launch_bare_spawn_failure_is_one_line() {
        // The risk: a desktop that is there but cannot start (not an
        // executable) ending bare `trellis` instead of falling back.
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let broken = d.path().join("not-a-directory").join(DESKTOP);
        let said = launch_bare(&Ok(broken.clone()), true, &exe, None).unwrap_err();
        assert!(!said.contains('\n'), "{said}");
        assert!(said.contains(&broken.display().to_string()), "{said}");
    }
}
