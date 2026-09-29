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
    /// A ring: bytes past the cap leave from the front in O(1), where a
    /// `Vec` moved all `cap` kept bytes on every read, and at `find`'s
    /// 64 MiB cap that was a timeout.
    bytes: std::collections::VecDeque<u8>,
    pub(crate) total: usize,
    /// Nothing is kept, because the stream passed the cap and the reader
    /// asked for all of it or none ([`Spec::whole`]); `total` still counts.
    dropped: bool,
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
        let kept: Vec<u8> = self.bytes.iter().copied().collect();
        let text = String::from_utf8_lossy(&kept);
        let mut s = if self.dropped {
            format!(
                "...({} bytes; none kept: past the cap, and this output is read whole or \
                 not at all)...",
                self.total
            )
        } else if self.total > self.bytes.len() {
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

    #[cfg(test)]
    fn kept(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
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

/// Why [`run`] returned no [`Captured`]. The messages are the ones it
/// returned as strings before (review round 10). Public because a hook's
/// error carries it.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("empty command")]
    Empty,
    #[error("could not start `{program}`: {source}")]
    Start {
        program: String,
        source: std::io::Error,
    },
    #[error("`{program}` could not be waited on: {source}")]
    Wait {
        program: String,
        source: std::io::Error,
    },
}

/// One process to run.
pub(crate) struct Spec<'a> {
    pub(crate) argv: &'a [String],
    pub(crate) cwd: &'a Path,
    pub(crate) env: &'a [(String, String)],
    /// Removed from the inherited environment: the variables that would
    /// point a `git` run in a throwaway clone at the real repository
    /// ([`crate::isolate::GIT_LOCATION_VARS`]).
    pub(crate) env_remove: &'a [&'a str],
    /// Written to stdin and then closed; `None` gives the child no stdin.
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) timeout: Duration,
    pub(crate) cap: usize,
    /// Keep the first `cap` bytes of each stream too ([`Tail::head`]).
    pub(crate) head: bool,
    /// Stdout is wanted whole or not at all: past the cap nothing more is
    /// stored and what was is let go, and only `total` keeps counting. For
    /// `find`, which refuses a list past the cap rather than show part of
    /// one, so storing its tail was work for nothing.
    pub(crate) whole: bool,
}

/// Runs `spec`, writing stdin and reading both streams concurrently.
///
/// Concurrently because a child that fills its stdout pipe while the parent
/// is still writing its stdin waits forever, and so does the parent. The
/// run ends when the process exits, plus [`DRAIN_GRACE`] for its pipes; on a
/// timeout the child and what it started are killed as far as
/// [`kill_tree`] reaches, which ADR 0023 §3 states. `Err` only when the
/// process could not be started.
pub(crate) async fn run(spec: Spec<'_>) -> Result<Captured, ExecError> {
    let Some((program, args)) = spec.argv.split_first() else {
        return Err(ExecError::Empty);
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
    // After `envs`, so nothing in `spec.env` can put a key back. Every
    // `bash`, custom tool, hook and listing command is spawned here, and its
    // output is an observation the log and the trail both record.
    trellis_core::secrets::scrub(cmd.as_std_mut());
    for v in spec.env_remove {
        cmd.env_remove(v);
    }
    // No `__pycache__` from anything trellis runs: under `.trellis/` it is
    // in the trust digest (bytecode can be code, so it is not skipped), and
    // where git does not ignore it, it moves the per-call worktree digest
    // and voids a pass (ADR 0023 §4). Last, so `spec.env` cannot undo it.
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    // Its own process group, so a timeout can kill what it started too
    // (`kill_tree`). Safe std; the signal goes through `kill(1)`.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|source| ExecError::Start {
        program: program.clone(),
        source,
    })?;
    // Taken now: tokio's `id()` is `None` once the child has been reaped,
    // and the group is killed after that (`kill_group`).
    let pid = child.id();
    // A member of the group that trellis holds and does not reap until the
    // group has been signalled: alive or a zombie, it keeps the group id
    // from being reused, so the group kill after a normal exit (which is
    // only observable by reaping the command) cannot reach a stranger's
    // group that took the id (review round 9). `None` on Windows, or when
    // it could not be started; the post-exit kill is then skipped.
    #[cfg(unix)]
    let mut holder = pid.and_then(hold_group);
    #[cfg(not(unix))]
    let mut holder: Option<tokio::process::Child> = None;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let mut out = Captured::default();
    let cap = spec.cap;
    let input = spec.stdin;

    let keep_head = spec.head;
    let whole = spec.whole;

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
                read_tail(stdout, so, cap, keep_head, whole),
                read_tail(stderr, se, cap, keep_head, false)
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
        // A normal exit ends what the command left running too: a
        // backgrounded `sleep 1; touch x` would otherwise write after this
        // call returns, after a convergence judged the tree or a rollback
        // restored it. Before the drain, so the pipes it held close. Only
        // while `holder` keeps the group id ours.
        if let Ok(Ok(_)) = &exited {
            if holder.is_some() {
                Box::pin(kill_group(pid)).await;
            } else if cfg!(unix) {
                tracing::warn!(
                    "the group holder did not start; what the command left running is not \
                     killed"
                );
            }
        }
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
        (Ok(Err(source)), _) => {
            return Err(ExecError::Wait {
                program: program.clone(),
                source,
            });
        }
        (Err(_), _) => {
            out.timed_out = true;
            // The timeout is the report; "still held after exit" is not.
            out.stdout.unfinished = false;
            out.stderr.unfinished = false;
            Box::pin(kill_tree(&mut child)).await;
        }
    }
    // Reaped last, after every group signal: until now it held the id.
    if let Some(h) = &mut holder {
        let _ = h.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), h.wait()).await;
    }
    Ok(out)
}

/// Starts `cat` in process group `pgid`, reading a pipe trellis holds so it
/// ends with trellis. Joining needs the group to exist, and it does: the
/// command leads it and has not been reaped yet (a zombie still counts).
#[cfg(unix)]
fn hold_group(pgid: u32) -> Option<tokio::process::Child> {
    let mut c = tokio::process::Command::new("cat");
    c.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .process_group(i32::try_from(pgid).ok()?);
    trellis_core::secrets::scrub(c.as_std_mut());
    c.spawn().ok()
}

/// Kills the child and what it started ([`trellis_core::process::kill_tree`],
/// whose limits ADR 0023 §3 states), then the child itself, directly,
/// whatever that did. Only while the child is unreaped: its pid is still
/// its own until then.
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // The kill commands are std and blocking; off the runtime's threads.
        let _ = tokio::task::spawn_blocking(move || trellis_core::process::kill_tree(pid)).await;
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}

/// Kills the process group the child led, after the child has exited
/// ([`trellis_core::process::kill_group`]). Called only while the holder
/// `run` keeps unreaped is a member, so the group id is still ours.
/// Nothing on Windows (ADR 0023 §3).
///
/// Awaited boxed, as `kill_tree` is: inline, their state made `run`'s
/// future larger, and a debug build's deepest call path (a `pre_tool` hook
/// before a custom tool, `child_env.rs`) overflowed the main thread's 1 MiB
/// stack on Windows.
async fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let _ = tokio::task::spawn_blocking(move || trellis_core::process::kill_group(pid)).await;
}

/// Reads `stream` to EOF into `into`: the last `cap` bytes, the total, and
/// the first `cap` when `keep_head`. With `whole`, once the total passes
/// the cap nothing is stored any more ([`Spec::whole`]). Linear in the
/// stream's length whatever the cap.
async fn read_tail<R: AsyncRead + Unpin>(
    stream: Option<R>,
    into: &mut Tail,
    cap: usize,
    keep_head: bool,
    whole: bool,
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
                if whole && into.total > cap {
                    if !into.dropped {
                        into.dropped = true;
                        into.bytes = Default::default();
                    }
                    continue;
                }
                // Only the part of this read that can survive is copied in;
                // then the front goes, which a ring does without moving
                // what stays.
                let fresh = &buf[n.saturating_sub(cap)..n];
                into.bytes.extend(fresh);
                let excess = into.bytes.len().saturating_sub(cap);
                into.bytes.drain(..excess);
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
            env_remove: &[],
            stdin: None,
            timeout,
            cap,
            head: false,
            whole: false,
        }
    }

    #[tokio::test]
    async fn a_stream_far_past_a_large_cap_is_read_in_linear_time() {
        // The risk: each read past the cap drained the front of the kept
        // bytes, moving `cap` bytes per 8 KiB read, so `find`'s 64 MiB cap
        // turned a long listing into minutes of memmove and a timeout.
        use tokio::io::AsyncReadExt;
        let cap = 64 << 20;
        let total = 192u64 << 20;
        let stream = tokio::io::repeat(b'x').take(total);
        let mut t = Tail::default();
        let read = tokio::time::timeout(
            Duration::from_secs(10),
            read_tail(Some(stream), &mut t, cap, false, false),
        )
        .await;
        assert!(
            read.is_ok(),
            "reading 192 MiB through a 64 MiB cap took over 10 s"
        );
        assert_eq!(t.total, total as usize);
        assert_eq!(t.kept().len(), cap);
    }

    #[tokio::test]
    async fn the_tail_keeps_the_last_bytes_in_order_across_the_wrap() {
        let bytes: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let mut t = Tail::default();
        read_tail(Some(&bytes[..]), &mut t, 1000, false, false).await;
        assert_eq!(t.total, bytes.len());
        assert_eq!(t.kept(), &bytes[bytes.len() - 1000..]);
        // `whole`: kept entire under the cap, nothing kept past it.
        let mut t = Tail::default();
        read_tail(Some(&bytes[..]), &mut t, 1000, false, true).await;
        assert_eq!(t.total, bytes.len());
        assert!(t.kept().is_empty(), "past the cap nothing is stored");
        let mut t = Tail::default();
        read_tail(Some(&bytes[..500]), &mut t, 1000, false, true).await;
        assert_eq!(t.kept(), &bytes[..500]);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn what_a_command_backgrounded_cannot_write_after_the_call_returns() {
        // The risk: a command that exited normally left its group running,
        // so `(sleep 1; touch marker) &` wrote after the call returned —
        // after a convergence judged the tree, or after a rollback restored
        // it (review round 8, finding 3).
        let d = tempfile::tempdir().unwrap();
        let argv = shell_argv("(sleep 1; touch marker) > /dev/null 2>&1 & exit 0");
        let c = run(spec(&argv, d.path(), Duration::from_secs(20), 1024))
            .await
            .unwrap();
        assert_eq!(c.code, Some(0));
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !d.path().join("marker").exists(),
            "the backgrounded write landed after the call returned"
        );
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
