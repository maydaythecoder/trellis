//! Running a process whose output becomes an observation (ADR 0023).
//!
//! Used by `bash`, custom tools, hooks and the git-backed listings. Never
//! `.output()`: that buffers everything a child prints, and a runaway `yes`
//! under a 120-second timeout is gigabytes. Output is kept as a bounded tail
//! instead, because the tail is where a failure summary lives and the
//! adapter shows observations from the tail anyway.

use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// How many bytes of each stream an observation keeps: the same bound the
/// orchestrator puts on a file excerpt.
pub(crate) const OUTPUT_CAP: usize = 8_192;

/// The last `cap` bytes of a stream, and how many there were in all.
#[derive(Debug, Default, Clone)]
pub(crate) struct Tail {
    bytes: Vec<u8>,
    pub(crate) total: usize,
    /// The first `cap` bytes as well, for the one reader that wants the
    /// start of a stream rather than its end: `before_run`, whose output
    /// is context read top down (ADR 0023 §5). Kept only when the
    /// [`Spec`] asks: for `find` at a 64 MiB cap it doubled the memory.
    head: Vec<u8>,
    /// The stream was still open [`DRAIN_GRACE`] after the process exited,
    /// because something it started inherited the pipe. What was written
    /// after that is not here.
    pub(crate) unfinished: bool,
}

/// How long the streams are read after the process has exited. A
/// backgrounded grandchild that inherited a pipe holds it open for its
/// whole life; waiting for EOF made a finished command look timed out.
pub(crate) const DRAIN_GRACE: Duration = Duration::from_secs(2);

impl Tail {
    /// The kept bytes as text, with a line saying what was cut. A silent
    /// cut reads as the whole output.
    pub(crate) fn render(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let mut s = if self.total > self.bytes.len() {
            format!(
                "...({} earlier bytes cut; the last {} are shown)...\n{}",
                self.total - self.bytes.len(),
                self.bytes.len(),
                text
            )
        } else {
            text.into_owned()
        };
        if self.unfinished {
            s.push_str(&format!(
                "\n...(the process exited but something it started still held this \
                 stream {}s later; output after that point is not shown)...",
                DRAIN_GRACE.as_secs()
            ));
        }
        s
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The first bytes of the stream, up to the cap, as text.
    pub(crate) fn head(&self) -> String {
        String::from_utf8_lossy(&self.head).into_owned()
    }
}

/// What a finished (or stopped) process left behind.
#[derive(Debug, Default, Clone)]
pub(crate) struct Captured {
    /// `None` when there is no code to report: a timeout, or a signal.
    pub(crate) code: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) stdout: Tail,
    pub(crate) stderr: Tail,
}

/// One process to run.
pub(crate) struct Spec<'a> {
    pub(crate) argv: &'a [String],
    pub(crate) cwd: &'a Path,
    pub(crate) env: &'a [(String, String)],
    /// Written to stdin and then closed; `None` gives the child no stdin.
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) timeout: Duration,
    pub(crate) cap: usize,
    /// Keep the first `cap` bytes of each stream too ([`Tail::head`]).
    pub(crate) head: bool,
}

/// Runs `spec`, writing stdin and reading both streams concurrently.
///
/// Concurrently because a child that fills its stdout pipe while the parent
/// is still writing its stdin waits forever, and so does the parent. The
/// run ends when the process exits, plus [`DRAIN_GRACE`] for its pipes; on a
/// timeout the child and what it started are killed as far as
/// [`kill_tree`] reaches, which ADR 0023 §3 states. `Err` only when the
/// process could not be started.
pub(crate) async fn run(spec: Spec<'_>) -> Result<Captured, String> {
    let Some((program, args)) = spec.argv.split_first() else {
        return Err("empty command".into());
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(spec.cwd)
        // No `__pycache__` from what trellis runs: under `.trellis/` it is in
        // the trust digest (bytecode can be code, so it is not skipped), and
        // anywhere git does not ignore it moves the per-call worktree digest
        // and voids a pass (ADR 0023 §4, §5).
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(if spec.stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // After `envs`, so nothing in `spec.env` can put a key back. Every
    // `bash`, custom tool, hook and listing command is spawned here, and its
    // output is an observation the log and the trail both record.
    trellis_core::secrets::scrub(cmd.as_std_mut());
    // Its own process group, so a timeout can kill what it started too
    // (`kill_tree`). Safe std; the signal goes through `kill(1)`.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start `{program}`: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let mut out = Captured::default();
    let cap = spec.cap;
    let input = spec.stdin;

    let keep_head = spec.head;

    let finished = {
        let (so, se) = (&mut out.stdout, &mut out.stderr);
        let mut write = std::pin::pin!(async {
            if let (Some(mut pipe), Some(bytes)) = (stdin, input) {
                // A child that exits without reading its input closes the
                // pipe; that is its business, not an error here.
                let _ = pipe.write_all(&bytes).await;
                let _ = pipe.shutdown().await;
            }
        });
        let mut reading = std::pin::pin!(async {
            tokio::join!(
                read_tail(stdout, so, cap, keep_head),
                read_tail(stderr, se, cap, keep_head)
            );
        });
        let (mut written, mut read) = (false, false);
        // The process's exit ends the wait, not EOF on its pipes: anything
        // it backgrounded inherited them and may hold them for hours. The
        // pipes are still read meanwhile, or a child that fills one blocks.
        let exited = tokio::time::timeout(spec.timeout, async {
            loop {
                tokio::select! {
                    s = child.wait() => break s,
                    _ = &mut write, if !written => written = true,
                    _ = &mut reading, if !read => read = true,
                }
            }
        })
        .await;
        // Then what is left in the pipes, for a short grace. Dropping the
        // readers keeps every byte they already stored.
        let drained = match &exited {
            Ok(Ok(_)) if !read => tokio::time::timeout(DRAIN_GRACE, &mut reading)
                .await
                .is_ok(),
            _ => true,
        };
        (exited, drained)
    };
    match finished {
        (Ok(Ok(status)), drained) => {
            out.code = status.code();
            if drained {
                out.stdout.unfinished = false;
                out.stderr.unfinished = false;
            }
        }
        (Ok(Err(e)), _) => return Err(format!("`{program}` could not be waited on: {e}")),
        (Err(_), _) => {
            out.timed_out = true;
            // The timeout is the report; "still held after exit" is not.
            out.stdout.unfinished = false;
            out.stderr.unfinished = false;
            kill_tree(&mut child).await;
        }
    }
    Ok(out)
}

/// Kills the child and what it started, as far as the platform allows
/// without `unsafe`: the child leads its own process group on Unix, which
/// `kill -KILL -- -<pgid>` signals whole; on Windows `taskkill /T` walks the
/// tree from the child. A process that left the group (`setsid`) or whose
/// parent already exited on Windows (the tree link is the parent pid) is
/// not reached. Then the child itself, directly, whatever that did.
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let mut k = if cfg!(windows) {
            let mut c = tokio::process::Command::new("taskkill");
            c.args(["/T", "/F", "/PID", &pid.to_string()]);
            c
        } else {
            let mut c = tokio::process::Command::new("kill");
            c.args(["-KILL", "--", &format!("-{pid}")]);
            c
        };
        k.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // `taskkill` and `kill` need no key, and are given none.
        trellis_core::secrets::scrub(k.as_std_mut());
        if let Ok(mut k) = k.spawn() {
            let _ = tokio::time::timeout(Duration::from_secs(2), k.wait()).await;
        }
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}

async fn read_tail<R: AsyncRead + Unpin>(
    stream: Option<R>,
    into: &mut Tail,
    cap: usize,
    keep_head: bool,
) {
    let Some(mut s) = stream else { return };
    // Until EOF; a reader dropped before it leaves this set.
    into.unfinished = true;
    let mut buf = [0u8; 8192];
    loop {
        match s.read(&mut buf).await {
            Ok(0) | Err(_) => {
                into.unfinished = false;
                return;
            }
            Ok(n) => {
                into.total += n;
                if keep_head {
                    let room = cap.saturating_sub(into.head.len());
                    into.head.extend_from_slice(&buf[..n.min(room)]);
                }
                into.bytes.extend_from_slice(&buf[..n]);
                if into.bytes.len() > cap {
                    let excess = into.bytes.len() - cap;
                    into.bytes.drain(..excess);
                }
            }
        }
    }
}

/// The shell `bash` runs through when `[tools] shell` is not set.
///
/// `sh -c` on Unix. On Windows, Git's `bash.exe` where Git for Windows
/// installs it, then an `sh.exe` on `PATH`, then `cmd /C`. `System32` is
/// skipped when scanning `PATH`: the `bash.exe` there is WSL's, which runs
/// the command in a different filesystem from the repository.
pub fn default_shell() -> Vec<String> {
    if !cfg!(windows) {
        return vec!["sh".into(), "-c".into()];
    }
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var) {
            let p = Path::new(&pf).join("Git").join("bin").join("bash.exe");
            if p.is_file() {
                return vec![p.display().to_string(), "-c".into()];
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("system32")
            {
                continue;
            }
            let p = dir.join("sh.exe");
            if p.is_file() {
                return vec![p.display().to_string(), "-c".into()];
            }
        }
    }
    vec!["cmd".into(), "/C".into()]
}

#[cfg(test)]
pub(crate) fn shell_argv(script: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), script.into()]
    } else {
        vec!["sh".into(), "-c".into(), script.into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(argv: &'a [String], cwd: &'a Path, timeout: Duration, cap: usize) -> Spec<'a> {
        Spec {
            argv,
            cwd,
            env: &[],
            stdin: None,
            timeout,
            cap,
            head: false,
        }
    }

    #[tokio::test]
    async fn a_process_that_prints_forever_is_bounded_by_the_tail() {
        // `.output()` would buffer every byte until the timeout. The cap is
        // what keeps a runaway command from becoming a memory problem.
        let d = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "for /L %i in (1,1,3000) do @echo line %i"
        } else {
            "i=0; while [ $i -lt 3000 ]; do echo line $i; i=$((i+1)); done"
        };
        let argv = shell_argv(script);
        let c = run(spec(&argv, d.path(), Duration::from_secs(60), 256))
            .await
            .unwrap();
        assert_eq!(c.code, Some(0));
        assert!(c.stdout.total > 256);
        let r = c.stdout.render();
        assert!(r.contains("earlier bytes cut"), "{r}");
        assert!(r.contains("line 3000") || r.contains("line 2999"), "{r}");
        assert!(!r.contains("line 1\n") && !r.contains("line 1\r"), "{r}");
    }

    #[tokio::test]
    async fn a_timeout_is_reported_as_one_and_has_no_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "ping -n 30 127.0.0.1 >NUL"
        } else {
            "sleep 30"
        };
        let argv = shell_argv(script);
        let started = std::time::Instant::now();
        let c = run(spec(&argv, d.path(), Duration::from_millis(500), 1024))
            .await
            .unwrap();
        assert!(c.timed_out);
        assert_eq!(c.code, None);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[tokio::test]
    async fn a_command_that_exits_while_a_background_child_holds_its_pipes_returns_promptly() {
        // Waiting for EOF on both pipes before `wait()` made a command that
        // had finished look timed out: the backgrounded process inherited
        // stdout and held it open for its whole life.
        let d = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "echo before& start /B ping -n 30 127.0.0.1 >NUL & exit 0"
        } else {
            "echo before; sleep 30 & exit 0"
        };
        let argv = shell_argv(script);
        let started = std::time::Instant::now();
        let c = run(spec(&argv, d.path(), Duration::from_secs(20), 1024))
            .await
            .unwrap();
        assert!(!c.timed_out, "the command finished: {c:?}");
        assert_eq!(c.code, Some(0));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(
            c.stdout.render().contains("before"),
            "{}",
            c.stdout.render()
        );
    }

    #[tokio::test]
    async fn every_command_starts_with_python_bytecode_writes_off() {
        // The risk: a Python hook importing code under `.trellis/` wrote
        // `__pycache__/*.pyc` there, which the trust digest covers (bytecode
        // can be code: ADR 0023 §4), so running a trusted hook untrusted the
        // repository; and one written anywhere git does not ignore showed in
        // the per-call worktree digest and voided the pass.
        let d = tempfile::tempdir().unwrap();
        let argv = shell_argv(if cfg!(windows) {
            "echo [%PYTHONDONTWRITEBYTECODE%]"
        } else {
            "echo [$PYTHONDONTWRITEBYTECODE]"
        });
        let c = run(spec(&argv, d.path(), Duration::from_secs(20), 1024))
            .await
            .unwrap();
        assert!(c.stdout.render().contains("[1]"), "{}", c.stdout.render());
    }

    #[tokio::test]
    async fn only_a_caller_that_asks_for_the_head_keeps_one() {
        // Every run kept a second buffer of up to `cap` bytes for the head,
        // which only `before_run` reads: 64 MiB more for `find`.
        let d = tempfile::tempdir().unwrap();
        let argv = shell_argv("echo hello");
        let c = run(spec(&argv, d.path(), Duration::from_secs(20), 1024))
            .await
            .unwrap();
        assert!(c.stdout.render().contains("hello"));
        assert!(c.stdout.head().is_empty(), "{:?}", c.stdout.head());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_what_the_command_started_not_only_the_shell() {
        let d = tempfile::tempdir().unwrap();
        let argv = shell_argv("sleep 30 & echo $! > bg.pid; wait");
        let c = run(spec(&argv, d.path(), Duration::from_millis(800), 1024))
            .await
            .unwrap();
        assert!(c.timed_out);
        let pid = std::fs::read_to_string(d.path().join("bg.pid")).unwrap();
        // Give the kill a moment to land, then ask whether it is still there.
        let mut alive = true;
        for _ in 0..20 {
            let st = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            if !st.success() {
                alive = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive, "background sleep {pid} survived the timeout");
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_is_an_error_not_an_empty_success() {
        let d = tempfile::tempdir().unwrap();
        let argv = vec!["trellis-no-such-program-x".to_string()];
        assert!(
            run(spec(&argv, d.path(), Duration::from_secs(5), 64))
                .await
                .is_err()
        );
    }
}
