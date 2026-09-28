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
    /// is context read top down (ADR 0023 §5).
    head: Vec<u8>,
}

impl Tail {
    /// The kept bytes as text, with a line saying what was cut. A silent
    /// cut reads as the whole output.
    pub(crate) fn render(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        if self.total > self.bytes.len() {
            format!(
                "...({} earlier bytes cut; the last {} are shown)...\n{}",
                self.total - self.bytes.len(),
                self.bytes.len(),
                text
            )
        } else {
            text.into_owned()
        }
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
}

/// Runs `spec`, writing stdin and reading both streams concurrently.
///
/// Concurrently because a child that fills its stdout pipe while the parent
/// is still writing its stdin waits forever, and so does the parent. On a
/// timeout the child is killed; anything it started may outlive it, which
/// ADR 0023 says aloud. `Err` only when the process could not be started.
pub(crate) async fn run(spec: Spec<'_>) -> Result<Captured, String> {
    let Some((program, args)) = spec.argv.split_first() else {
        return Err("empty command".into());
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(spec.cwd)
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(if spec.stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start `{program}`: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let mut out = Captured::default();
    let cap = spec.cap;
    let input = spec.stdin;

    let finished = {
        let (so, se) = (&mut out.stdout, &mut out.stderr);
        let work = async {
            let write = async {
                if let (Some(mut pipe), Some(bytes)) = (stdin, input) {
                    // A child that exits without reading its input closes the
                    // pipe; that is its business, not an error here.
                    let _ = pipe.write_all(&bytes).await;
                    let _ = pipe.shutdown().await;
                }
            };
            tokio::join!(
                write,
                read_tail(stdout, so, cap),
                read_tail(stderr, se, cap)
            );
            child.wait().await
        };
        tokio::time::timeout(spec.timeout, work).await
    };
    match finished {
        Ok(Ok(status)) => out.code = status.code(),
        Ok(Err(e)) => return Err(format!("`{program}` could not be waited on: {e}")),
        Err(_) => {
            out.timed_out = true;
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
    Ok(out)
}

async fn read_tail<R: AsyncRead + Unpin>(stream: Option<R>, into: &mut Tail, cap: usize) {
    let Some(mut s) = stream else { return };
    let mut buf = [0u8; 8192];
    loop {
        match s.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                into.total += n;
                let room = cap.saturating_sub(into.head.len());
                into.head.extend_from_slice(&buf[..n.min(room)]);
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
