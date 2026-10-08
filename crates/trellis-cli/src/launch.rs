//! Starting the desktop from `trellis --dsk` (ADR 0050 section 1, ADR 0048).
//!
//! The desktop is looked for beside the running `trellis` and nowhere else:
//! the two are installed together, and a `trellis-desktop` found on PATH
//! could be another build speaking another client contract (ADR 0048
//! section 6), or another program with the name.
//!
//! The child is started detached and not waited for: `trellis` returns once
//! it has started, and the window outlives the terminal it was started
//! from. Its environment is trellis's own, unchanged, because the desktop
//! starts its own `trellis rpc`, which needs the user's key variables for a
//! hosted model the user chose (ADR 0048 section 3).

use anyhow::{Context, Result};
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

/// Starts `desktop` detached and returns without waiting for it. `--root`
/// is passed only when there is a root to pass; without one the desktop
/// shows its overview (ADR 0050 section 3).
pub(crate) fn spawn_desktop(desktop: &Path, trellis: &Path, root: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new(desktop); // key-exempt: trellis-desktop
    cmd.arg("--trellis").arg(trellis);
    if let Some(r) = root {
        cmd.arg("--root").arg(r);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
        // Its own process group, so the terminal's signals stay with trellis.
        cmd.process_group(0);
    }
    // Dropped, not waited for: no job object ties the window to trellis.
    cmd.spawn()
        .with_context(|| format!("cannot start {}", desktop.display()))?;
    Ok(())
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
