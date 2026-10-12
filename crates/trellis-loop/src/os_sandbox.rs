//! An OS sandbox around the command that runs in a throwaway clone
//! (ADR 0055, amending ADR 0032).
//!
//! The clone (ADR 0032) isolates a writer that edits where it stands. It
//! does not stop one that opens a socket, reads the home directory, or
//! writes by absolute path. Here the clone's command is started under
//! `bwrap` on Linux or `sandbox-exec` on macOS, when the binary is present
//! and works, so that:
//!
//! - it has no network: a network namespace of its own (`bwrap`), or every
//!   `network*` operation denied (Seatbelt);
//! - the home directory reads as empty (`bwrap`: a fresh tmpfs over it) or
//!   is unreadable (Seatbelt), apart from the clone if it lies inside;
//! - it can write only into the clone and the system's temporary
//!   directory (and `/dev`), the rest of the filesystem being read-only;
//! - on Linux it sees only its own processes (a pid namespace), so it can
//!   reach neither trellis's `/proc/<pid>/environ` nor its `/proc/<pid>/cwd`.
//!
//! When the binary is missing, or is present and fails a trial run (Ubuntu
//! 24.04's `apparmor_restrict_unprivileged_userns`, a kernel without user
//! namespaces), the command runs as before and the run says so: `sandbox:
//! none (<reason>)` on its `repo:` line and in `trellis doctor`. The owner
//! chose a warning over a refusal (issue #621, 2026-10-12). Windows stays
//! on ADR 0035's job objects.
//!
//! No crate is added: the sandbox is a child process, spawned through the
//! same key scrub as every other (`trellis_core::secrets::scrub`), and the
//! command it wraps inherits that scrubbed environment.
//!
//! The `bwrap` argument list and the Seatbelt rules follow the approach of
//! anthropic-experimental/sandbox-runtime (Apache-2.0, README "How It
//! Works"); the text here is written for trellis, not copied.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Which OS mechanism confines the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsSandboxKind {
    /// bubblewrap, Linux.
    Bwrap,
    /// Seatbelt through `sandbox-exec`, macOS.
    SandboxExec,
}

impl OsSandboxKind {
    /// The binary's name, as it is looked up on `PATH` and as the run's
    /// `repo:` line names it.
    pub fn binary_name(self) -> &'static str {
        match self {
            OsSandboxKind::Bwrap => "bwrap",
            OsSandboxKind::SandboxExec => "sandbox-exec",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// The binary was found and a trial run under it exited 0.
    Ready {
        kind: OsSandboxKind,
        binary: PathBuf,
    },
    /// No sandbox: why, in words the `repo:` line prints.
    Absent(String),
}

/// The OS sandbox a clone's command runs under, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsSandbox {
    state: State,
    /// Hidden from the command. `None` when `HOME` is unset, relative,
    /// `/`, or does not exist: there is then no home directory to hide,
    /// and `trellis doctor` says so.
    home: Option<PathBuf>,
    /// Writable inside, beside the clone: the system's temporary
    /// directory, which compilers and test runners write to.
    temp: PathBuf,
}

/// How long the trial run may take before the sandbox is taken to be
/// unusable. A working `bwrap` or `sandbox-exec` running `true` takes
/// milliseconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest reason kept from a failed trial run's stderr.
const REASON_CAP: usize = 160;

impl OsSandbox {
    /// The sandbox for this machine, detected once per process: the
    /// platform's binary looked up on `PATH` and given a trial run, with
    /// `HOME` and the temporary directory taken from trellis's own
    /// environment.
    pub fn detected() -> &'static OsSandbox {
        static DETECTED: OnceLock<OsSandbox> = OnceLock::new();
        DETECTED.get_or_init(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let temp = std::env::temp_dir();
            if cfg!(target_os = "linux") {
                OsSandbox::probe(OsSandboxKind::Bwrap, home.as_deref(), &temp)
            } else if cfg!(target_os = "macos") {
                OsSandbox::probe(OsSandboxKind::SandboxExec, home.as_deref(), &temp)
            } else if cfg!(windows) {
                OsSandbox::none(
                    "Windows: the command is held by a job object (ADR 0035), not sandboxed",
                )
            } else {
                OsSandbox::none("no OS sandbox is built for this platform")
            }
        })
    }

    /// No sandbox, for `reason`. The command runs as it did before ADR
    /// 0055, in its clone, with the limits ADR 0032 lists.
    pub fn none(reason: impl Into<String>) -> OsSandbox {
        OsSandbox {
            state: State::Absent(reason.into()),
            home: None,
            temp: std::env::temp_dir(),
        }
    }

    /// Looks `kind`'s binary up on `PATH` and runs `true` under it with
    /// the arguments a command would get, hiding `home` and keeping `temp`
    /// writable. Ready only if that exits 0: a `bwrap` that is installed
    /// but may not create a user namespace fails here, once, rather than
    /// on every command.
    pub fn probe(kind: OsSandboxKind, home: Option<&Path>, temp: &Path) -> OsSandbox {
        let name = kind.binary_name();
        let home = home.and_then(hideable_home);
        let temp = std::fs::canonicalize(temp).unwrap_or_else(|_| temp.to_path_buf());
        let Some(binary) = on_path(name) else {
            return OsSandbox {
                state: State::Absent(format!("{name} not found")),
                home,
                temp,
            };
        };
        let candidate = OsSandbox {
            state: State::Ready { kind, binary },
            home,
            temp,
        };
        match candidate.trial() {
            Ok(()) => candidate,
            Err(why) => OsSandbox {
                state: State::Absent(format!("{name} failed: {why}")),
                ..candidate
            },
        }
    }

    /// Whether a command will run under the sandbox.
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Ready { .. })
    }

    /// What the run's `repo:` line says: `bwrap`, `sandbox-exec`, or
    /// `none (<reason>)`.
    pub fn label(&self) -> String {
        match &self.state {
            State::Ready { kind, .. } => kind.binary_name().to_string(),
            State::Absent(why) => format!("none ({why})"),
        }
    }

    /// What `trellis doctor` says: the binary and what it confines, or the
    /// reason there is none and what that leaves.
    pub fn doctor_line(&self) -> String {
        match &self.state {
            State::Ready { kind, binary } => format!(
                "sandbox:    {} at {}: bash, custom tools, hooks and process agents run with no \
                 network, {}, and writes only to their clone and {}",
                kind.binary_name(),
                binary.display(),
                match &self.home {
                    Some(h) => format!("{} hidden", h.display()),
                    None => "no home directory to hide (HOME unset or not a directory)".into(),
                },
                self.temp.display()
            ),
            State::Absent(why) => format!(
                "sandbox:    none ({why}); bash, custom tools, hooks and process agents run in \
                 their clone with the network, reads and writes the user has (ADR 0032, Limits)"
            ),
        }
    }

    /// The argv that runs `argv` under the sandbox, confined to `clone`;
    /// `argv` itself when there is no sandbox. The caller spawns it with
    /// `clone` as its working directory and scrubs it as any other spawn.
    pub fn wrap(&self, argv: &[String], clone: &Path) -> Vec<OsString> {
        let State::Ready { kind, binary } = &self.state else {
            return argv.iter().map(OsString::from).collect();
        };
        let clone = std::fs::canonicalize(clone).unwrap_or_else(|_| clone.to_path_buf());
        let mut out = vec![binary.clone().into_os_string()];
        match kind {
            OsSandboxKind::Bwrap => {
                out.extend(bwrap_args(self.home.as_deref(), &self.temp, &clone))
            }
            OsSandboxKind::SandboxExec => {
                out.extend(seatbelt_args(self.home.as_deref(), &self.temp, &clone))
            }
        }
        // `--` ends the sandbox's own options for both: a program whose
        // name begins with `-` is a program, never an option of theirs.
        out.push("--".into());
        out.extend(argv.iter().map(OsString::from));
        out
    }

    /// Runs `true` under the sandbox in the temporary directory. `Err`
    /// with the first line the sandbox printed, or its exit status.
    fn trial(&self) -> Result<(), String> {
        let argv = self.wrap(&["true".to_string()], &self.temp);
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&self.temp)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        // A trial run is a spawn like any other: no key reaches it.
        trellis_core::secrets::scrub(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "a trial run did not finish in {}s",
                        PROBE_TIMEOUT.as_secs()
                    ));
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        if status.success() {
            return Ok(());
        }
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = e.by_ref().take(4096).read_to_end(&mut buf);
            stderr = String::from_utf8_lossy(&buf).into_owned();
        }
        Err(first_line(&stderr).unwrap_or_else(|| match status.code() {
            Some(c) => format!("a trial run exited {c}"),
            None => "a trial run was ended by a signal".into(),
        }))
    }
}

/// `home` as a directory the sandbox can hide: absolute, not `/`, and
/// existing (canonicalized, since `bwrap` mounts on and Seatbelt matches
/// real paths, and `/home` or `/Users` may be a link).
fn hideable_home(home: &Path) -> Option<PathBuf> {
    if !home.is_absolute() {
        return None;
    }
    let real = std::fs::canonicalize(home).ok()?;
    if real.parent().is_none() || !real.is_dir() {
        return None;
    }
    Some(real)
}

/// The first non-empty line of `s`, with control characters removed (it
/// is printed to the terminal) and cut to [`REASON_CAP`] characters.
fn first_line(s: &str) -> Option<String> {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty())?;
    let clean: String = line
        .chars()
        .filter(|c| !c.is_control())
        .take(REASON_CAP)
        .collect();
    Some(clean)
}

/// `name` in a directory on `PATH`, as a regular file.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// `bwrap`'s options, before the `--` and the command.
///
/// Order matters, since each mount lands on top of the ones before it:
/// the whole filesystem read-only first, then the home directory and the
/// temporary directory (the outer of the two first, so the inner one is
/// not covered by it), then the clone last, so it is writable wherever it
/// lies, inside the home directory included.
///
/// - `--unshare-net`: a network namespace with only a loopback device of
///   its own, so neither the internet nor a port trellis or anything else
///   on the machine listens on is reachable.
/// - `--unshare-pid` with a fresh `/proc`: the command sees only its own
///   processes, so `/proc/<trellis>/environ` (the provider key) and
///   `/proc/<trellis>/cwd` (the repository, writable through the host's
///   mounts) are not there. When the command exits, everything it left
///   running in the namespace is killed with it.
/// - `--dev /dev`: a minimal `/dev` (`null`, `zero`, `urandom`, a `pts`).
/// - `--die-with-parent`: killing `bwrap` (a process agent's timeout or
///   interrupt kills the process trellis spawned, which is now `bwrap`)
///   kills the command and everything in its namespace. Without it the
///   command outlived the kill and wrote after its timeout. The signal is
///   also sent when the thread that spawned `bwrap` exits; trellis spawns
///   from a thread that waits for the command (a process agent's run) or
///   from a tokio worker, which lives as long as the runtime.
/// - No `--new-session`: a process agent stays in the terminal's
///   foreground group (ADR 0026 §11, round 9).
fn bwrap_args(home: Option<&Path>, temp: &Path, clone: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = [
        "--die-with-parent",
        "--unshare-net",
        "--unshare-pid",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let hide = |a: &mut Vec<OsString>, h: &Path| {
        a.push("--tmpfs".into());
        a.push(h.into());
    };
    let bind = |a: &mut Vec<OsString>, p: &Path| {
        a.push("--bind".into());
        a.push(p.into());
        a.push(p.into());
    };
    match home {
        // The temporary directory inside the home directory: hide the
        // home, then open the temporary directory again inside it.
        Some(h) if temp.starts_with(h) => {
            hide(&mut a, h);
            bind(&mut a, temp);
        }
        // A home inside the temporary directory (a test's fake `HOME`):
        // the other way round, or the bind would uncover it.
        Some(h) => {
            bind(&mut a, temp);
            hide(&mut a, h);
        }
        None => bind(&mut a, temp),
    }
    bind(&mut a, clone);
    a.push("--chdir".into());
    a.push(clone.into());
    a
}

/// The Seatbelt profile `sandbox-exec -p` is given. Paths reach it only as
/// parameters (`-D`), never spliced into the text, so a path holding a
/// quote or a parenthesis cannot change a rule.
///
/// Every rule is a `deny` whose filter excludes what stays allowed
/// (`require-not`), on top of `(allow default)`, so no rule depends on the
/// order Seatbelt resolves overlapping rules in.
pub(crate) fn seatbelt_profile(hide_home: bool) -> String {
    let mut p = String::from(
        "(version 1)\n\
         (allow default)\n\
         (deny network*)\n\
         (deny file-write*\n  \
           (require-not\n    \
             (require-any\n      \
               (subpath (param \"CLONE\"))\n      \
               (subpath (param \"TEMP\"))\n      \
               (subpath \"/dev\"))))\n",
    );
    if hide_home {
        p.push_str(
            "(deny file-read*\n  \
               (require-all\n    \
                 (subpath (param \"HOME\"))\n    \
                 (require-not (subpath (param \"CLONE\")))\n    \
                 (require-not (subpath (param \"TEMP\")))))\n",
        );
    }
    p
}

/// `sandbox-exec`'s options, before the `--` and the command.
fn seatbelt_args(home: Option<&Path>, temp: &Path, clone: &Path) -> Vec<OsString> {
    let param = |k: &str, v: &Path| {
        let mut s = OsString::from(format!("{k}="));
        s.push(v.as_os_str());
        s
    };
    let mut a: Vec<OsString> = vec![
        "-p".into(),
        seatbelt_profile(home.is_some()).into(),
        "-D".into(),
        param("CLONE", clone),
        "-D".into(),
        param("TEMP", temp),
    ];
    if let Some(h) = home {
        a.push("-D".into());
        a.push(param("HOME", h));
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    fn ready(kind: OsSandboxKind, home: Option<&str>, temp: &str) -> OsSandbox {
        OsSandbox {
            state: State::Ready {
                kind,
                binary: PathBuf::from(format!("/usr/bin/{}", kind.binary_name())),
            },
            home: home.map(PathBuf::from),
            temp: PathBuf::from(temp),
        }
    }

    #[test]
    fn os_sandbox_without_a_binary_leaves_the_command_unchanged_and_says_why() {
        // The risk: a missing sandbox that reads as a present one, or a
        // wrap that runs something other than the command.
        let none = OsSandbox::none("bwrap not found");
        let argv = vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()];
        assert_eq!(strs(&none.wrap(&argv, Path::new("/tmp/c"))), argv);
        assert!(!none.is_active());
        assert_eq!(none.label(), "none (bwrap not found)");
        assert!(
            none.doctor_line()
                .starts_with("sandbox:    none (bwrap not found)")
        );
    }

    #[test]
    fn os_sandbox_probe_of_a_binary_not_on_path_is_none_naming_it() {
        // Not a real kind's binary: whatever this machine has installed,
        // a name nothing provides must come back absent and named.
        let p = OsSandbox::probe(OsSandboxKind::Bwrap, None, &std::env::temp_dir());
        if !p.is_active() {
            assert!(p.label().starts_with("none (bwrap "), "{}", p.label());
        }
        assert_eq!(on_path("trellis-no-such-sandbox-binary-621"), None);
    }

    #[test]
    fn os_sandbox_bwrap_argv_cuts_the_network_hides_home_and_binds_the_clone_last() {
        let s = ready(OsSandboxKind::Bwrap, Some("/home/u"), "/tmp");
        let argv = vec!["-weird".to_string(), "x".to_string()];
        let got = strs(&s.wrap(&argv, Path::new("/tmp/clone-1")));
        let joined = got.join(" ");
        assert_eq!(got[0], "/usr/bin/bwrap");
        assert!(joined.contains("--unshare-net"), "{joined}");
        assert!(joined.contains("--unshare-pid"), "{joined}");
        assert!(joined.contains("--ro-bind / /"), "{joined}");
        assert!(joined.contains("--proc /proc"), "{joined}");
        assert!(joined.contains("--tmpfs /home/u"), "{joined}");
        // The clone is the last mount, so it is writable even inside home.
        assert!(
            joined.ends_with("--bind /tmp/clone-1 /tmp/clone-1 --chdir /tmp/clone-1 -- -weird x"),
            "{joined}"
        );
        assert!(joined.contains("--die-with-parent"), "{joined}");
    }

    #[test]
    fn os_sandbox_bwrap_mounts_the_outer_of_home_and_temp_first() {
        // A fake HOME inside the temporary directory: binding the temporary
        // directory after hiding HOME would uncover HOME again.
        let inner_home = strs(&bwrap_args(
            Some(Path::new("/tmp/h")),
            Path::new("/tmp"),
            Path::new("/tmp/c"),
        ))
        .join(" ");
        let bind = inner_home.find("--bind /tmp /tmp").unwrap();
        let hide = inner_home.find("--tmpfs /tmp/h").unwrap();
        assert!(bind < hide, "{inner_home}");
        // A temporary directory inside HOME: the other way round.
        let inner_temp = strs(&bwrap_args(
            Some(Path::new("/home/u")),
            Path::new("/home/u/tmp"),
            Path::new("/home/u/tmp/c"),
        ))
        .join(" ");
        let hide = inner_temp.find("--tmpfs /home/u").unwrap();
        let bind = inner_temp.find("--bind /home/u/tmp /home/u/tmp").unwrap();
        assert!(hide < bind, "{inner_temp}");
    }

    #[test]
    fn os_sandbox_seatbelt_profile_denies_network_home_reads_and_writes_outside() {
        // The macOS path cannot run here; its text and argv are what can be
        // held. The risk: a path spliced into the profile, where a quote in
        // it would rewrite a rule.
        let p = seatbelt_profile(true);
        assert!(p.starts_with("(version 1)\n(allow default)\n"), "{p}");
        assert!(p.contains("(deny network*)"), "{p}");
        assert!(p.contains("(deny file-write*"), "{p}");
        assert!(p.contains("(subpath (param \"CLONE\"))"), "{p}");
        assert!(p.contains("(subpath (param \"TEMP\"))"), "{p}");
        assert!(p.contains("(deny file-read*"), "{p}");
        assert!(p.contains("(subpath (param \"HOME\"))"), "{p}");
        // Every rule is a deny: none depends on rule order to take effect.
        assert_eq!(p.matches("(allow").count(), 1, "{p}");
        // Parentheses balance, or sandbox-exec refuses the profile.
        assert_eq!(p.matches('(').count(), p.matches(')').count(), "{p}");
        let no_home = seatbelt_profile(false);
        assert!(!no_home.contains("HOME"), "{no_home}");
        assert_eq!(
            no_home.matches('(').count(),
            no_home.matches(')').count(),
            "{no_home}"
        );

        let s = ready(
            OsSandboxKind::SandboxExec,
            Some("/Users/u\"x"),
            "/private/tmp",
        );
        let got = strs(&s.wrap(&["true".to_string()], Path::new("/private/tmp/c")));
        assert_eq!(got[0], "/usr/bin/sandbox-exec");
        assert_eq!(got[1], "-p");
        assert!(
            !got[2].contains("/Users"),
            "a path reached the profile text"
        );
        let rest = &got[3..];
        assert_eq!(
            rest,
            [
                "-D",
                "CLONE=/private/tmp/c",
                "-D",
                "TEMP=/private/tmp",
                "-D",
                "HOME=/Users/u\"x",
                "--",
                "true"
            ]
        );
        assert_eq!(s.label(), "sandbox-exec");
    }

    #[test]
    fn os_sandbox_failed_trial_reason_is_one_clean_line() {
        assert_eq!(
            first_line("\n  bwrap: setting up uid map: Permission denied\nmore\n").as_deref(),
            Some("bwrap: setting up uid map: Permission denied")
        );
        assert_eq!(first_line("a\u{1b}[31mb").as_deref(), Some("a[31mb"));
        assert_eq!(
            first_line("x".repeat(500).as_str()).map(|s| s.len()),
            Some(REASON_CAP)
        );
        assert_eq!(first_line("  \n"), None);
    }

    #[test]
    fn os_sandbox_home_that_cannot_be_hidden_is_none() {
        assert_eq!(hideable_home(Path::new("relative")), None);
        assert_eq!(hideable_home(Path::new("/")), None);
        assert_eq!(hideable_home(Path::new("/no/such/home/621")), None);
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            hideable_home(d.path()),
            Some(std::fs::canonicalize(d.path()).unwrap())
        );
    }
}
