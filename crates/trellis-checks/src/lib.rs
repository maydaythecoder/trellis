//! Executed checks, and the only type in the workspace that may assert
//! correctness.
//!
//! Every check entry point here either spawns a real process or runs a
//! mechanical in-process scan (the governance audit), and reports what it
//! did. Nothing in this crate consults a model.

pub mod coverage;
pub mod governance;
mod tree_state;
mod verdict;

pub use verdict::{AuditMode, CheckDetail, CheckKind, Verdict, check_kind_name};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The default for [`CheckConfig::timeout_s`]: thirty minutes, well past a
/// large suite's normal run and short of leaving a hung one to hold a run
/// for good (ADR 0004).
pub const DEFAULT_CHECK_TIMEOUT_S: u64 = 1_800;

/// How many bytes of each of a check's output streams are held: 8 MiB, the
/// last ones written. The checker is the repository's own command, and one
/// that prints without end would otherwise be read into memory until the
/// host ran out (#171). The tail, because a type checker's summary and a
/// test runner's failures come last. The exit code is read apart from the
/// output, so a cut never changes a verdict.
pub const CHECK_OUTPUT_CAP: usize = 8 * 1024 * 1024;

/// Raw result of a process that ran to completion (or failed to start).
///
/// `pub` so callers can see what happened, but note that constructing one
/// buys nothing on its own: the `Verdict` constructors that consume it are
/// `pub(crate)`.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub command: String,
    /// `None` when the process could not be spawned or was killed. Never
    /// reads as success.
    pub exit_code: Option<i32>,
    pub failure_count: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    /// `Some(n)` when stdout wrote `n` bytes, more than
    /// [`CHECK_OUTPUT_CAP`]: only its last bytes were kept, and `stdout`
    /// opens with a line saying how many were dropped. `None` when it was
    /// kept whole.
    pub stdout_truncated: Option<u64>,
    /// As `stdout_truncated`, for stderr.
    pub stderr_truncated: Option<u64>,
    pub duration_ms: u64,
}

/// How to invoke this repository's checks, from `.trellis/config.toml`.
///
/// Commands are configured rather than detected because a wrong guess here
/// produces a confident verdict about the wrong thing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckConfig {
    /// e.g. `["mypy", "--output", "json"]`
    pub typecheck: Vec<String>,
    /// e.g. `["pytest", "-q"]`
    pub tests: Vec<String>,
    /// Interpreter used for the import check.
    pub python: String,
    /// Whether `.trellis/` holds Python, decided once for a run by
    /// [`for_run`](Self::for_run). `None` decides it at every spawn, as
    /// before review round 11, when the walk ran once per check spawn and a
    /// check runs many times a run. Never read from or written to a file:
    /// it describes the tree at run start, not the repository's settings.
    #[serde(skip)]
    pub dot_trellis_python: Option<bool>,
    /// Seconds a check may run before its process tree is killed and the
    /// check fails. `[checks] timeout_s`; without it a hung mypy or pytest
    /// held a run with no end (ADR 0004).
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    /// When the run's wall-clock budget runs out, if a run set one: a check
    /// is bounded by the earlier of this and `timeout_s`, so a check cannot
    /// carry a run past its budget. `None` for `trellis check`, which has no
    /// run budget. Set per call by the loop; never read from a file.
    #[serde(skip)]
    pub deadline: Option<Instant>,
}

fn default_timeout_s() -> u64 {
    DEFAULT_CHECK_TIMEOUT_S
}

impl CheckConfig {
    /// This configuration with [`dot_trellis_python`](Self::dot_trellis_python)
    /// decided now, once, for a run in `root`.
    ///
    /// Deciding it at run start is sound for the runs that can change the
    /// answer: `edit` and `write_file` refuse `.trellis/`, a trusted run
    /// ends at the first change under `.trellis/` (the trust guard), and a
    /// process agent that writes there does not converge. A caller that
    /// does not call this is slower, never wrong.
    pub fn for_run(mut self, root: &Path) -> Self {
        self.dot_trellis_python = Some(dot_trellis_holds_python(root));
        self
    }
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            // `--no-incremental` is a correctness flag, not a preference, and
            // removing it for speed reintroduces S20.
            //
            // mypy's incremental cache keys a file on its size and mtime. A
            // `git checkout` that restores a previous version of the same byte
            // length therefore looks unchanged to it, and mypy reports the
            // cached result for code it never read. `Snapshot::rollback` does
            // exactly that checkout on every non-converging run, and
            // `-> int:` to `-> str:` is both length-preserving and the kind of
            // edit an agent makes constantly.
            //
            // The resulting `Verdict` is truthful about everything it asserts
            // structurally — a process ran, it exited zero — and wrong about
            // the only thing anyone cares about. The seal guarantees a verdict
            // came from an executed process; it cannot guarantee that process
            // read the file. This flag is what closes that gap for mypy.
            typecheck: vec!["mypy".into(), "--no-incremental".into()],
            tests: vec!["pytest".into(), "-q".into()],
            python: "python".into(),
            dot_trellis_python: None,
            timeout_s: DEFAULT_CHECK_TIMEOUT_S,
            deadline: None,
        }
    }
}

/// Runs the configured type checker over `paths`.
#[tracing::instrument(skip(cfg), fields(n_paths = paths.len()))]
pub async fn typecheck(root: &Path, cfg: &CheckConfig, paths: &[PathBuf]) -> Verdict {
    let mut argv = cfg.typecheck.clone();
    argv.extend(paths.iter().map(|p| p.display().to_string()));
    Verdict::from_typecheck(Box::pin(run(root, cfg, &argv)).await)
}

/// Runs the configured test runner over `paths`.
///
/// `paths` should already be narrowed to the tests covering the change; see
/// `docs/TECH_SPIKE.md` on deriving that coverage map.
#[tracing::instrument(skip(cfg), fields(n_paths = paths.len()))]
pub async fn run_tests(root: &Path, cfg: &CheckConfig, paths: &[PathBuf]) -> Verdict {
    let mut argv = cfg.tests.clone();
    argv.extend(paths.iter().map(|p| p.display().to_string()));
    Verdict::from_tests(Box::pin(run(root, cfg, &argv)).await)
}

/// Imports `module` in a fresh subprocess.
///
/// Catches the breakage class that type checking misses and tests do not
/// reach: syntax errors in an untested branch, import cycles, a decorator
/// that raises at module scope.
#[tracing::instrument(skip(cfg))]
pub async fn import_check(root: &Path, cfg: &CheckConfig, module: &str) -> Verdict {
    let argv = vec![cfg.python.clone(), "-c".into(), format!("import {module}")];
    Verdict::from_import(Box::pin(run(root, cfg, &argv)).await)
}

/// Audits the repository's own governance rules.
///
/// **Produces a `Verdict`, which is the whole point.** `last_check_passed`
/// reads verdicts, so a violation reported only into the model's context is
/// one a model can converge past — the rule would be a suggestion again.
///
/// The evaluation is a substring search and nothing more. A governance check
/// that exercised judgment would be the thing under audit auditing itself,
/// which is Constraint 2 by another route.
///
/// Rank decides the outcome and only the outcome: a `non-negotiable`
/// violation fails, a `nice-to-have` is reported and does not. `failure_count`
/// counts only the failing ones, so a run's own summary does not inflate.
#[tracing::instrument(skip(rules, files))]
pub fn governance_audit(root: &Path, rules: &[governance::Rule], files: &[String]) -> Verdict {
    let started = Instant::now();
    let scan = governance::violations(root, rules, files);
    let found = &scan.violations;
    let (failing, advisory): (Vec<_>, Vec<_>) =
        found.iter().partition(|v| v.rank.fails_the_audit());
    // Asked to audit files a rule actually applies to, and unable to read any
    // of them. Passing there would report compliance that was never
    // established, and `passed()` is what convergence reads.
    //
    // Keyed on `unreadable`, deliberately not on `files`: a run that edited
    // files no `forbid` rule scopes to has an empty `read` too, and failing it
    // would fail every run in a repository whose rules are all `deny-write` —
    // exactly the "a rule broken somewhere this run never went" failure the
    // orchestrator's scoping exists to prevent, arriving by the other door.
    let blind = scan.read.is_empty() && !scan.unreadable.is_empty();

    let mut out = String::new();
    for v in &failing {
        out.push_str(&format!(
            "VIOLATION [{}] {}:{}  {}\n",
            v.rule, v.file, v.line, v.text
        ));
    }
    for v in &advisory {
        out.push_str(&format!(
            "advisory  [{}] {}:{}  {}\n",
            v.rule, v.file, v.line, v.text
        ));
    }
    if !scan.unreadable.is_empty() {
        out.push_str(&format!(
            "NOT AUDITED: {} file(s) a rule applies to could not be read: {}\n",
            scan.unreadable.len(),
            scan.unreadable.join(", ")
        ));
    }
    if blind {
        out.push_str(&format!(
            concat!(
                "this audit establishes nothing: {} file(s) a rule applies to ",
                "and none could be read\n"
            ),
            scan.unreadable.len()
        ));
    } else if found.is_empty() {
        // Counted from what was read, not from what was requested.
        out.push_str(&format!(
            "{} rule(s) checked over {} file(s); none broken\n",
            rules.len(),
            scan.read.len()
        ));
    }

    // A real ProcessOutput, because the seal takes nothing else. No
    // subprocess ran — the match is in-process — so the command records what
    // was actually evaluated rather than inventing a shell line that would
    // not reproduce.
    Verdict::from_governance(ProcessOutput {
        command: format!(
            "governance audit: {} rule(s), {} file(s) read of {} in scope",
            rules.len(),
            scan.read.len(),
            files.len()
        ),
        exit_code: Some(i32::from(!failing.is_empty() || blind)),
        failure_count: Some(failing.len() as u32),
        stdout: out,
        stderr: String::new(),
        stdout_truncated: None,
        stderr_truncated: None,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Whether any `*.py` file sits under `<root>/.trellis/`, links followed
/// as the trust digest follows them.
///
/// Each real directory is walked once, by its canonical path, as the trust
/// walker does: a depth bound alone let two links back to `.trellis/`
/// branch at every level, 2^32 reads before it gave up. A directory whose
/// real path is outside the repository is not walked at all (`.trellis/x
/// -> /` would read the disk) and answers `true`: the cautious answer costs
/// a suite its bytecode cache, the other one could cost the repository its
/// trust (ADR 0023 §4). The trust digest refuses such a link anyway.
///
/// `.trellis/runs/` is not walked (review round 11): it holds one trail per
/// run and grows without bound, and trellis writes no Python there. The
/// trust digest skips only the trails in it and hashes anything else, so a
/// `.py` placed there is missed here and not there: a check that imports
/// it writes `__pycache__` the digest sees, and the run ends
/// `TrustChanged`. That errs toward refusing, never toward running
/// uncovered code.
fn dot_trellis_holds_python(root: &Path) -> bool {
    fn any_py(
        dir: &Path,
        real_root: &Path,
        runs: Option<&Path>,
        seen: &mut std::collections::HashSet<PathBuf>,
    ) -> bool {
        let Ok(real) = std::fs::canonicalize(dir) else {
            return false;
        };
        if runs == Some(real.as_path()) {
            return false;
        }
        // Outside, or the root itself (`.trellis/up -> ..`), which would
        // walk the whole repository, `.git/` and `.venv/` included.
        if !real.starts_with(real_root) || real_root.starts_with(&real) {
            return true;
        }
        if !seen.insert(real) {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let found = match std::fs::metadata(&p) {
                Ok(m) if m.is_dir() => any_py(&p, real_root, runs, seen),
                Ok(_) => p.extension().is_some_and(|x| x.eq_ignore_ascii_case("py")),
                Err(_) => false,
            };
            if found {
                return true;
            }
        }
        false
    }
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return true;
    };
    let dot = root.join(".trellis");
    if !dot.exists() {
        return false;
    }
    let runs = std::fs::canonicalize(dot.join("runs")).ok();
    any_py(&dot, &real_root, runs.as_deref(), &mut Default::default())
}

/// Awaited boxed by its callers: its state (two pipe readers, the wait and
/// the timer) is large, and inline it made the orchestrator's future deep
/// enough that a debug build overflowed the main thread's 1 MiB stack on
/// Windows (`child_env.rs`), as `trellis-loop`'s `exec::run` once did.
async fn run(root: &Path, cfg: &CheckConfig, argv: &[String]) -> ProcessOutput {
    let started = Instant::now();
    let command = argv.join(" ");
    let Some((program, args)) = argv.split_first() else {
        return ProcessOutput {
            command,
            exit_code: None,
            failure_count: None,
            stdout: String::new(),
            stderr: "empty command".into(),
            stdout_truncated: None,
            stderr_truncated: None,
            duration_ms: 0,
        };
    };
    let limit = bound(cfg, started);

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A run that is dropped (interrupted, or its task cancelled) takes
        // its check with it rather than leaving a mypy running unowned.
        .kill_on_drop(true);
    // Its own process group, so a timeout reaches what the checker started
    // (pytest workers, a shell's children) and not only the checker.
    #[cfg(unix)]
    cmd.process_group(0);
    // A check's output becomes a verdict's detail, and from there a trail:
    // a checker that prints its environment must find no key in it.
    trellis_core::secrets::scrub(cmd.as_std_mut());
    // A suite that imports code under `.trellis/` would write
    // `__pycache__` there, which the trust digest covers, and untrust the
    // repository mid-run (ADR 0023 §4). Only then: with no Python under
    // `.trellis/` nothing a check imports can write there, and a large
    // suite keeps the bytecode cache that saves it recompiling every file
    // on every check.
    if cfg
        .dot_trellis_python
        .unwrap_or_else(|| dot_trellis_holds_python(root))
    {
        cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    }
    let mut child = match spawn(cmd) {
        Ok(c) => c,
        // A check that could not run is not a check that passed. On
        // Windows that includes a Job Object that could not be made or
        // joined: running the checker unheld is what #172 replaced.
        Err(e) => {
            return ProcessOutput {
                command,
                exit_code: None,
                failure_count: None,
                stdout: String::new(),
                stderr: format!("failed to spawn: {e}"),
                stdout_truncated: None,
                stderr_truncated: None,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    // Taken now: tokio's `id()` is `None` once the child has been reaped,
    // and the group is killed after that (`kill_group`).
    #[cfg(unix)]
    let pid = child.id();
    // A member of the checker's group that is not reaped until the group
    // has been signalled: alive or a zombie, it keeps the group id from
    // being reused, so the kill after a normal exit (observable only by
    // reaping the checker) cannot reach a stranger's group that took the
    // id. As `exec::run` in trellis-loop (ADR 0023 §3, review round 9).
    // `None` when it could not be started; the post-exit kill is then
    // skipped and said so. Unix only: on Windows the job handle names the
    // tree (ADR 0035).
    #[cfg(unix)]
    let mut holder = pid.and_then(hold_group);

    let (mut out, mut err) = (Captured::default(), Captured::default());
    let (so, se) = pipes(&mut child);
    let keys = tree_state::Keys::new();
    let (exited, drained, changes) = {
        // Read while waiting: a checker that fills a pipe nobody reads
        // blocks, and would then look hung when it is not.
        let mut reading = std::pin::pin!(async {
            tokio::join!(read_all(so, &mut out), read_all(se, &mut err));
        });
        let mut read = false;
        let exited = tokio::time::timeout(limit, async {
            loop {
                tokio::select! {
                    s = wait_command(&mut child) => break s,
                    _ = &mut reading, if !read => read = true,
                }
            }
        })
        .await;
        let mut changes = None;
        if let Ok(waited) = &exited {
            // The tree as the checker left it. Only for an exit that is a
            // pass (`Verdict` passes on exit code 0 and nothing else): a
            // failing check has nothing a later write could void, and the
            // walk costs a large tree real time.
            let before = match waited {
                Ok(s) if s.code() == Some(0) => Some(walk(root, &keys).await),
                _ => None,
            };
            // What is left in the pipes, for a short grace within the
            // bound. A process the checker left running that inherited
            // them keeps them open, and whatever it writes meanwhile is
            // caught by the second walk.
            if !read {
                let grace = LEFTOVER_GRACE.min(limit.saturating_sub(started.elapsed()));
                read = tokio::time::timeout(grace, &mut reading).await.is_ok();
            }
            // Whatever the exit code: what the checker left running is
            // killed before this call returns, so it cannot write after a
            // convergence judged the tree or a rollback restored it.
            #[cfg(unix)]
            if holder.is_some() {
                Box::pin(kill_group(pid)).await;
            } else {
                tracing::warn!(
                    "the group holder did not start; what the check left running is not killed"
                );
            }
            #[cfg(windows)]
            end_job(&mut child);
            // The kill closed the pipes it held, unless a process outside
            // the group or job holds them too.
            if !read {
                read = tokio::time::timeout(AFTER_KILL_DRAIN, &mut reading)
                    .await
                    .is_ok();
            }
            if let Some(before) = before {
                let after = walk(root, &keys).await;
                changes = Some(before.changes(&after, root, &keys));
            }
        } else {
            // Unreaped, so its pid and its group are still its own when
            // the tree is signalled.
            Box::pin(kill_tree(&mut child)).await;
        }
        (exited, read, changes)
    };
    // Reaped last, after every group signal: until now it held the id.
    #[cfg(unix)]
    if let Some(h) = &mut holder {
        let _ = h.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), h.wait()).await;
    }
    let (out, stdout_truncated) = out.into_text();
    let (err, stderr_truncated) = err.into_text();
    let failed = |stderr: String| ProcessOutput {
        command: command.clone(),
        exit_code: None,
        failure_count: None,
        stdout: out.clone(),
        stderr,
        stdout_truncated,
        stderr_truncated,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    match exited {
        // Exited zero, but the tree it was judged on changed before its
        // leftovers were killed: the pass would describe a tree that no
        // longer exists (a pass stops counting once the tree changes).
        // `None`, which never reads as success, rather than a new way to
        // fail a `Verdict`.
        Ok(Ok(status))
            if changes
                .as_ref()
                .is_some_and(|c| c.incomplete || c.count > 0) =>
        {
            let c = changes.expect("checked above");
            failed(format!("{}\n{err}", tree_changed(status, &c)))
        }
        Ok(Ok(status)) if drained => ProcessOutput {
            command,
            exit_code: status.code(),
            // Unparsed: a count read from a tool's text is a guess about
            // its format, and the exit code is what the verdict rests on.
            failure_count: None,
            stdout: out,
            stderr: err,
            stdout_truncated,
            stderr_truncated,
            duration_ms: started.elapsed().as_millis() as u64,
        },
        // Exited, and something still held its output open after its
        // processes were killed. Not a pass, whatever the exit code: the
        // verdict would rest on output that was cut.
        Ok(Ok(status)) => failed(format!(
            "exited ({status}), but its output was still held open after what it left running \
             was killed, by a process the kill did not reach; the output is incomplete\n{err}"
        )),
        Ok(Err(e)) => failed(format!("could not wait for the check: {e}\n{err}")),
        Err(_) => failed(format!(
            "timed out after {} s; the check's process tree was killed\n{err}",
            seconds(limit),
        )),
    }
}

/// How long a check's pipes may stay open after it exits before what it
/// left running is killed, within the check's bound. Long enough that a
/// leftover holding them is seen writing (the second walk catches it), and
/// short: before #172 the wait was the whole bound. A check that left
/// nothing holding its pipes has them at end of file already and pays
/// nothing.
const LEFTOVER_GRACE: Duration = Duration::from_secs(2);

/// How long the pipes may take to close once the group or job is killed.
const AFTER_KILL_DRAIN: Duration = Duration::from_secs(2);

/// The tree under `root`, walked off the async runtime. A walk that
/// panicked is an unknown state, which compares as changed: "could not
/// tell" is not "unchanged".
async fn walk(root: &Path, keys: &tree_state::Keys) -> tree_state::TreeState {
    let (root, keys) = (root.to_path_buf(), keys.clone());
    tokio::task::spawn_blocking(move || tree_state::TreeState::take(&root, &keys))
        .await
        .unwrap_or_else(|_| tree_state::TreeState::unknown())
}

/// The failure a changed tree gives a check that exited zero.
fn tree_changed(status: std::process::ExitStatus, c: &tree_state::Changes) -> String {
    if c.incomplete {
        return format!(
            "exited ({status}), but whether the working tree changed after its exit was \
             observed could not be established (the tree could not be walked whole), so the \
             pass does not count"
        );
    }
    let mut named = c.named.join(", ");
    if c.count > c.named.len() {
        named.push_str(&format!(
            "{}{} more",
            if named.is_empty() { "" } else { ", " },
            c.count - c.named.len()
        ));
    }
    format!(
        "exited ({status}), but the working tree changed after its exit was observed, before \
         what it left running was killed ({} path(s): {named}); a pass stops counting once the \
         tree changes",
        c.count
    )
}

/// What `run` spawned. On Windows the checker is wrapped in a Job Object
/// (ADR 0035), and the wrapper owns the process.
#[cfg(unix)]
type Child = tokio::process::Child;
#[cfg(windows)]
type Child = Box<dyn process_wrap::tokio::ChildWrapper>;

/// Spawns `cmd`, which the caller has already scrubbed.
#[cfg(unix)]
fn spawn(mut cmd: tokio::process::Command) -> std::io::Result<Child> {
    cmd.spawn()
}

/// Spawns `cmd`, which the caller has already scrubbed, into a Job Object
/// of its own, as `exec::run` in trellis-loop does (ADR 0035): created
/// suspended, assigned, then resumed, so nothing it starts runs outside
/// the job; no breakaway; kill-on-close, so the tree also ends when
/// trellis does. #184 saw a check's `ping` grandchild outlive the
/// pid-walking `taskkill /T` by about 120 s; the job reaches it by handle.
#[cfg(windows)]
fn spawn(cmd: tokio::process::Command) -> std::io::Result<Child> {
    use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
    let mut wrapped = CommandWrap::from(cmd);
    wrapped.wrap(KillOnDrop).wrap(JobObject);
    wrapped.spawn()
}

type Pipes = (
    Option<tokio::process::ChildStdout>,
    Option<tokio::process::ChildStderr>,
);

#[cfg(unix)]
fn pipes(child: &mut Child) -> Pipes {
    (child.stdout.take(), child.stderr.take())
}

#[cfg(windows)]
fn pipes(child: &mut Child) -> Pipes {
    (child.stdout().take(), child.stderr().take())
}

/// Waits for the checker itself, not for what it started.
#[cfg(unix)]
fn wait_command(
    child: &mut Child,
) -> impl std::future::Future<Output = std::io::Result<std::process::ExitStatus>> + '_ {
    child.wait()
}

/// Waits for the checker itself, not for what it started: the job
/// wrapper's own `wait` goes on to wait for every process in the job, and
/// a backgrounded grandchild would hold the call for its whole life.
#[cfg(windows)]
fn wait_command(
    child: &mut Child,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = std::io::Result<std::process::ExitStatus>> + Send + '_>,
> {
    child.inner_mut().wait()
}

/// Ends every process still in the checker's job (`TerminateJobObject`).
/// Safe after the checker has been reaped: the job handle, not a pid,
/// names the tree, so no reused id can make it reach a stranger.
#[cfg(windows)]
fn end_job(child: &mut Child) {
    if let Err(e) = child.start_kill() {
        tracing::warn!("could not end the check's job: {e}");
    }
}

/// Starts `cat` in process group `pgid`, reading a pipe trellis holds so it
/// ends with trellis. Joining needs the group to exist, and it does: the
/// checker leads it and has not been reaped yet (a zombie still counts).
/// A copy of trellis-loop's `exec::hold_group`, which is private there.
#[cfg(unix)]
fn hold_group(pgid: u32) -> Option<tokio::process::Child> {
    let mut c = tokio::process::Command::new("cat");
    c.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(i32::try_from(pgid).ok()?);
    trellis_core::secrets::scrub(c.as_std_mut());
    c.spawn().ok()
}

/// Kills the process group the checker led, after it has exited
/// ([`trellis_core::process::kill_group`]). Called only while the holder
/// `run` keeps unreaped is a member, so the group id is still ours.
#[cfg(unix)]
async fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let _ = tokio::task::spawn_blocking(move || trellis_core::process::kill_group(pid)).await;
}

/// Kills the checker and what it started on a timeout
/// ([`trellis_core::process::kill_tree`]), then the checker itself,
/// directly, whatever that did. Only while it is unreaped: its pid is still
/// its own until then.
#[cfg(unix)]
async fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        // The kill commands are std and blocking; off the runtime's threads.
        let _ = tokio::task::spawn_blocking(move || trellis_core::process::kill_tree(pid)).await;
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}

/// Kills every process in the checker's job, the checker included, then
/// reaps the checker.
#[cfg(windows)]
async fn kill_tree(child: &mut Child) {
    end_job(child);
    let _ = tokio::time::timeout(Duration::from_secs(2), wait_command(child)).await;
}

/// How long a check may run: `timeout_s`, or less when the run's budget
/// ends sooner. A `Duration`, never an `Instant` sum: `timeout_s` comes from
/// a repository's file, and `now + u64::MAX s` overflows.
fn bound(cfg: &CheckConfig, now: Instant) -> Duration {
    let own = Duration::from_secs(cfg.timeout_s);
    match cfg.deadline {
        Some(d) => own.min(d.saturating_duration_since(now)),
        None => own,
    }
}

/// `d` as a reader would write it: whole seconds when it is whole.
fn seconds(d: Duration) -> String {
    if d.subsec_nanos() == 0 {
        d.as_secs().to_string()
    } else {
        format!("{:.1}", d.as_secs_f64())
    }
}

/// One output stream as read: at most [`CHECK_OUTPUT_CAP`] bytes, the last
/// ones, and how many were written in all.
///
/// A `VecDeque` so dropping the head is cheap: trimming a `Vec` from the
/// front moves the whole 8 MiB for every 8 KiB read once the cap is
/// reached, and a checker printing gigabytes would pay that many times over.
#[derive(Default)]
struct Captured {
    tail: std::collections::VecDeque<u8>,
    total: u64,
}

impl Captured {
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        // Only the last `CHECK_OUTPUT_CAP` of one read can survive it.
        let bytes = &bytes[bytes.len().saturating_sub(CHECK_OUTPUT_CAP)..];
        // Dropped before the new bytes go in, so the buffer never holds (or
        // grows its allocation for) more than the cap.
        let over = (self.tail.len() + bytes.len()).saturating_sub(CHECK_OUTPUT_CAP);
        self.tail.drain(..over);
        self.tail.extend(bytes);
    }

    /// The kept bytes as text, and the stream's true length when some were
    /// dropped. A cut stream opens with a line that says so, because what
    /// reads this downstream (the model, the trail) sees only the text, and
    /// a log that begins mid-run would otherwise read as the whole of it.
    fn into_text(self) -> (String, Option<u64>) {
        let mut tail = Vec::from(self.tail);
        if self.total == tail.len() as u64 {
            return (String::from_utf8_lossy(&tail).into_owned(), None);
        }
        // The cut can land inside a UTF-8 sequence: its continuation bytes
        // (at most three) are dropped too, rather than shown as U+FFFD.
        let partial = tail
            .iter()
            .take(3)
            .take_while(|b| (**b & 0xC0) == 0x80)
            .count();
        tail.drain(..partial);
        let text = format!(
            "[trellis: output cut to its last {CHECK_OUTPUT_CAP} bytes: {} written, the first {} \
             dropped, {} kept]\n{}",
            self.total,
            self.total - tail.len() as u64,
            tail.len(),
            String::from_utf8_lossy(&tail)
        );
        (text, Some(self.total))
    }
}

/// Reads `stream` to its end into `into`, keeping what was read if the read
/// is abandoned part way: a timed-out check's partial output is still worth
/// showing.
async fn read_all<R: AsyncRead + Unpin>(stream: Option<R>, into: &mut Captured) {
    let Some(mut s) = stream else { return };
    let mut buf = [0u8; 8192];
    loop {
        match s.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => into.push(&buf[..n]),
        }
    }
}

#[cfg(test)]
mod default_config_tests {
    use super::*;

    /// S20: mypy's incremental cache made a `Verdict` assert a pass about code
    /// the checker never read.
    ///
    /// The flag looks like a performance preference, which is why this test
    /// exists: it states in one place that removing it is a correctness
    /// regression, and names what goes wrong. The reproduction is in
    /// `RISK_REGISTER.md` under S20 — check a broken file, fix it, restore it
    /// with `git checkout`, check again, and mypy reports the cached pass
    /// because the restored file has the same size and a fresh mtime.
    ///
    /// `Snapshot::rollback` performs exactly that restore on every
    /// non-converging run, so without this flag the hazard is not an edge
    /// case; it is the normal path.
    #[test]
    fn s20_the_default_type_checker_is_invoked_cache_free() {
        let argv = CheckConfig::default().typecheck;
        assert!(
            argv.iter().any(|a| a == "--no-incremental"),
            "the default mypy invocation must defeat the incremental cache; \
             without it a rollback-restored file can return a stale pass. \
             argv was {argv:?}"
        );
    }

    /// The flag protects a default, not a mandate.
    ///
    /// A repository that configures its own checker takes responsibility for
    /// its cache behaviour, and TRELLIS must not rewrite the command it was
    /// given — a harness that silently appends flags to a user's check command
    /// produces verdicts about an invocation the user never wrote.
    /// Q-17 asks whether a harness could ever verify a command is cache-free;
    /// it cannot today, so the honest position is to run what was configured.
    ///
    /// The configured checker is a shell that echoes every argument it was
    /// given, so a flag appended to the configured argv shows in its output
    /// as well as in the recorded command.
    #[tokio::test]
    async fn a_configured_checker_is_run_verbatim_flags_and_all() {
        let d = tempfile::tempdir().unwrap();
        let configured: Vec<String> = if cfg!(windows) {
            vec!["cmd", "/C", "echo", "pyright", "--strict"]
        } else {
            vec!["sh", "-c", "echo \"$0\" \"$@\"", "pyright", "--strict"]
        }
        .into_iter()
        .map(String::from)
        .collect();
        let cfg = CheckConfig {
            typecheck: configured.clone(),
            ..CheckConfig::default()
        };
        let v = typecheck(d.path(), &cfg, &[]).await;
        assert_eq!(v.detail().command, configured.join(" "));
        assert_eq!(
            v.detail().stdout.trim(),
            "pyright --strict",
            "the process must receive exactly the configured argv, with no \
             `--no-incremental` or other flag appended"
        );
        assert!(v.passed(), "{:?}", v.detail());
    }
}

#[cfg(test)]
mod governance_audit_tests {
    use super::*;

    fn repo(rule: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join(".trellis/governance");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("r.toml"), rule).unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        d
    }

    /// The gate item: "a governance rule can fail an audit, mechanically."
    ///
    /// Failing means producing a `Verdict` that does not pass, because that
    /// is the only thing convergence reads. A violation printed into the
    /// model's context and nowhere else is one the model converges past.
    #[test]
    fn a_non_negotiable_violation_fails_the_verdict() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "x = psycopg2.connect('s')\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(!v.passed(), "a broken non-negotiable rule must fail");
        assert_eq!(v.detail().kind, CheckKind::Governance);
        assert_eq!(v.detail().failure_count, Some(1));
        assert!(v.detail().stdout.contains("VIOLATION [No direct DB]"));
        assert!(v.detail().stdout.contains("app/h.py:1"));
    }

    #[test]
    fn an_audit_that_could_read_nothing_it_was_given_does_not_pass() {
        // The defect this test exists for: the orchestrator shadowed its
        // `paths` with `["."]` before the governance arm ran, so the audit was
        // handed a directory, `read_to_string` failed, the failure was
        // swallowed by a `continue`, and the verdict passed reporting
        // "1 rule(s) checked over 1 file(s); none broken". Every measured model
        // calls run_checks with no paths, so that was the live path, and a
        // non-negotiable rule could not fail in it.
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &[".".to_string()]);
        assert!(
            !v.passed(),
            "claiming compliance over a path that could not be read is the \
             bare `complete` the brief forbids, wearing a rule's clothes"
        );
        assert!(
            v.detail().stdout.contains("establishes nothing"),
            "{:?}",
            v.detail().stdout
        );
        assert!(
            !v.detail().stdout.contains("establishes nothing:  "),
            "the fifth recurrence of the cargo fmt string-continuation defect \
             would land here: {:?}",
            v.detail().stdout
        );
        assert!(
            !v.detail().stdout.contains("none broken"),
            "it must not also claim a clean audit: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_run_outside_every_rules_scope_passes_rather_than_failing_blind() {
        // The regression the blind rule nearly introduced. A file no `forbid`
        // rule applies to is neither read nor unreadable, so keying "this
        // audit established nothing" on an empty `read` would fail every run
        // that edited a file outside every scope.
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\nscope = [\"app/handlers/\"]\n",
        );
        std::fs::create_dir_all(d.path().join("app/repo")).unwrap();
        std::fs::write(
            d.path().join("app/repo/db.py"),
            "x = psycopg2.connect('s')\n",
        )
        .unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/repo/db.py".to_string()]);
        assert!(
            v.passed(),
            "the repository layer is supposed to reach the database; failing \
             here forbids the architecture the rule exists to protect: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_repository_whose_rules_are_all_deny_write_passes_the_audit() {
        // `deny-write` is enforced at the orchestrator's write path, not here.
        // Such a repository has no `forbid` rule to evaluate, so every audit
        // reads zero files — and must still pass, or governance breaks every
        // run in the repositories most likely to configure it. The wedge
        // fixture is exactly this shape.
        let d = repo(
            "name = \"Vendored code is not ours\"\nrank = \"non-negotiable\"\n\
             deny-write = { paths = [\"vendor/\"] }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "x = 1\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed(), "{:?}", v.detail().stdout);
    }

    #[test]
    fn an_unreadable_file_alongside_readable_ones_is_named_not_skipped() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/ok.py"), "x = 1\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(
            d.path(),
            &rules,
            &["app/ok.py".to_string(), "app/gone.py".to_string()],
        );
        // One file was read and is clean, so the audit did establish
        // something and passes — but it must say what it could not see.
        assert!(v.passed());
        assert!(
            v.detail().stdout.contains("NOT AUDITED") && v.detail().stdout.contains("app/gone.py"),
            "{:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn the_file_count_reports_what_was_read_not_what_was_requested() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/ok.py"), "x = 1\n").unwrap();
        let rules = governance::load(d.path()).unwrap();
        let v = governance_audit(
            d.path(),
            &rules,
            &["app/ok.py".to_string(), "app/gone.py".to_string()],
        );
        assert!(
            v.detail().stdout.contains("over 1 file(s)"),
            "two requested, one readable: the coverage claim comes from the \
             read, or it is a count of intentions: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_nice_to_have_violation_is_reported_and_passes() {
        // If a nice-to-have failed the audit the ranking would mean nothing:
        // every rule would be non-negotiable in practice.
        let d = repo(
            "name = \"Prefer pathlib\"\nrank = \"nice-to-have\"\n\
             forbid = { pattern = \"os.path.join\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "p = os.path.join(a, b)\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed(), "a nice-to-have must not fail the audit");
        assert_eq!(v.detail().failure_count, Some(0));
        assert!(
            v.detail().stdout.contains("advisory  [Prefer pathlib]"),
            "and must still be reported, or ranking it was pointless: {}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_clean_repository_passes_and_says_what_was_checked() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "from app.repo import get\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed());
        // "Passed" with no indication of what was examined is the kind of
        // clean bill S2 warns about.
        assert!(
            v.detail()
                .stdout
                .contains("1 rule(s) checked over 1 file(s)")
        );
    }

    #[test]
    fn no_rules_passes_without_implying_anything_was_verified() {
        let d = tempfile::tempdir().unwrap();
        let v = governance_audit(d.path(), &[], &[]);
        assert!(v.passed());
        assert!(v.detail().stdout.contains("0 rule(s)"));
    }
}

#[cfg(test)]
mod bytecode_tests {
    use super::*;

    fn echo_flag() -> Vec<String> {
        if cfg!(windows) {
            ["cmd", "/C", "echo [%PYTHONDONTWRITEBYTECODE%]"]
        } else {
            ["sh", "-c", "echo [$PYTHONDONTWRITEBYTECODE]"]
        }
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// The risk: a test suite that imports code under `.trellis/` wrote
    /// `__pycache__/*.pyc` there. The trust digest covers bytecode (a
    /// sourceless pyc is importable), so a grounded check untrusted the
    /// repository, and with the per-spawn trust recheck (ADR 0023 §4) it
    /// would void the run at the next hook: trellis's own checker, not the
    /// model, breaking trust.
    #[tokio::test]
    async fn a_check_runs_without_bytecode_writes_when_dot_trellis_holds_python() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/hooks")).unwrap();
        std::fs::write(d.path().join(".trellis/hooks/gate.py"), "x = 1\n").unwrap();
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
    }

    /// `dot_trellis_holds_python(root)` on another thread, or `None` if it
    /// has not answered within `secs`: a walk that never ends must fail the
    /// test, not hang the suite.
    fn within(root: &Path, secs: u64) -> Option<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(dot_trellis_holds_python(&root));
        });
        rx.recv_timeout(std::time::Duration::from_secs(secs)).ok()
    }

    fn link(target: &Path, at: &Path) -> bool {
        match trellis_core::paths::symlink_for_test(target, at) {
            Ok(()) => true,
            Err(why) => {
                eprintln!("{why}");
                false
            }
        }
    }

    /// The risk: two links back to `.trellis/` itself made the walk branch
    /// twice at every level to the depth bound, 2^32 directory reads, and
    /// every check spawn stalled on it. Each real directory is walked once.
    #[test]
    fn self_referencing_links_under_dot_trellis_end_the_walk_quickly() {
        let d = tempfile::tempdir().unwrap();
        let dot = d.path().join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join("config.toml"), "").unwrap();
        if !link(Path::new("."), &dot.join("a")) || !link(Path::new("."), &dot.join("b")) {
            return;
        }
        assert_eq!(within(d.path(), 10), Some(false), "no Python, and it ends");
        std::fs::write(dot.join("gate.py"), "").unwrap();
        assert_eq!(within(d.path(), 10), Some(true));
    }

    /// A link out of the repository is not walked (`.trellis/x -> /` would
    /// walk the disk once, cycle or not), and the answer is the cautious
    /// one: bytecode writes off costs a slower suite, never a broken trust.
    #[test]
    fn a_link_out_of_the_repository_is_not_walked_and_answers_yes() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let dot = d.path().join(".trellis");
        std::fs::create_dir_all(&dot).unwrap();
        if !link(outside.path(), &dot.join("lib")) {
            return;
        }
        assert_eq!(within(d.path(), 10), Some(true));

        // Nor a link to the repository root, which would walk all of it.
        std::fs::remove_dir_all(&dot).unwrap();
        std::fs::create_dir_all(&dot).unwrap();
        assert!(link(Path::new(".."), &dot.join("up")));
        assert_eq!(within(d.path(), 10), Some(true));
    }

    /// Decided once per run (review round 11): the walk ran at every check
    /// spawn. A guard: a configuration prepared for a run with no Python
    /// under `.trellis/` keeps that answer when Python appears later, which
    /// is what shows the walk is not repeated; and trails under `runs/` are
    /// not walked.
    #[tokio::test]
    async fn the_python_walk_is_decided_once_per_run_and_skips_the_trails() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/runs")).unwrap();
        std::fs::write(d.path().join(".trellis/runs/gate.py"), "").unwrap();
        assert!(!dot_trellis_holds_python(d.path()), "runs/ is not walked");
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        }
        .for_run(d.path());
        assert_eq!(cfg.dot_trellis_python, Some(false));
        std::fs::write(d.path().join(".trellis/late.py"), "").unwrap();
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(!v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
        // Without the run's answer it is decided at the spawn, as before.
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
    }

    /// The cost is paid only where it buys something: a repository with no
    /// Python under `.trellis/` keeps its test suite's bytecode cache.
    #[tokio::test]
    async fn a_check_keeps_bytecode_caching_when_dot_trellis_holds_no_python() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(d.path().join(".trellis/config.toml"), "").unwrap();
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(!v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    /// A checker that sleeps 30 s. On Unix a shell that starts the sleep in
    /// the background and records its pid, so the test can ask whether the
    /// grandchild survived; on Windows `ping` under `cmd`, a grandchild too.
    fn sleeper() -> Vec<String> {
        if cfg!(windows) {
            ["cmd", "/C", "ping -n 31 127.0.0.1 >NUL"]
        } else {
            ["sh", "-c", "sleep 30 & echo $! > bg.pid; wait"]
        }
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn quick() -> Vec<String> {
        if cfg!(windows) {
            ["cmd", "/C", "echo ok"]
        } else {
            ["sh", "-c", "echo ok"]
        }
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// The risk: a hung mypy or pytest had no deadline, and held the run
    /// (and `trellis check`) for as long as it hung. Under a 2 s bound a
    /// 30 s checker must fail, say why, and return well inside 10 s.
    #[tokio::test]
    async fn check_timeout_a_sleeping_checker_fails_naming_the_timeout() {
        let d = tempfile::tempdir().unwrap();
        let cfg = CheckConfig {
            tests: sleeper(),
            timeout_s: 2,
            ..CheckConfig::default()
        };
        let started = Instant::now();
        let v = run_tests(d.path(), &cfg, &[]).await;
        let took = started.elapsed();
        assert!(!v.passed(), "a check that never finished must not pass");
        assert_eq!(v.detail().exit_code, None);
        assert!(
            v.detail()
                .stderr
                .starts_with("timed out after 2 s; the check's process tree was killed"),
            "{:?}",
            v.detail().stderr
        );
        assert!(took < Duration::from_secs(10), "took {took:?}");
        #[cfg(unix)]
        {
            let pid = std::fs::read_to_string(d.path().join("bg.pid")).unwrap();
            assert!(
                gone(pid.trim()),
                "the checker's background sleep {} survived the timeout",
                pid.trim()
            );
        }
    }

    /// `min(timeout_s, remaining budget)`: a run whose budget ends in 2 s
    /// does not wait out a 30 s `timeout_s`.
    #[tokio::test]
    async fn check_timeout_the_run_budget_bounds_a_check_below_timeout_s() {
        let d = tempfile::tempdir().unwrap();
        let cfg = CheckConfig {
            tests: sleeper(),
            timeout_s: 30,
            deadline: Some(Instant::now() + Duration::from_secs(2)),
            ..CheckConfig::default()
        };
        let started = Instant::now();
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(!v.passed());
        assert!(
            v.detail().stderr.starts_with("timed out after "),
            "{:?}",
            v.detail().stderr
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// A check that finishes is untouched by the bound, and a repository's
    /// `timeout_s = u64::MAX` is a long bound, not an `Instant` overflow
    /// that panics the run.
    #[tokio::test]
    async fn check_timeout_a_quick_check_passes_under_any_bound() {
        let d = tempfile::tempdir().unwrap();
        for timeout_s in [30, u64::MAX] {
            let cfg = CheckConfig {
                tests: quick(),
                timeout_s,
                ..CheckConfig::default()
            };
            let v = run_tests(d.path(), &cfg, &[]).await;
            assert!(v.passed(), "{:?}", v.detail());
            assert!(v.detail().stdout.contains("ok"), "{:?}", v.detail().stdout);
        }
    }

    /// `[checks] timeout_s` is optional and defaults to thirty minutes.
    #[test]
    fn check_timeout_defaults_to_1800_s() {
        assert_eq!(CheckConfig::default().timeout_s, 1_800);
        let cfg: CheckConfig =
            toml::from_str("typecheck = [\"mypy\"]\ntests = [\"pytest\"]\npython = \"python\"\n")
                .unwrap();
        assert_eq!(cfg.timeout_s, DEFAULT_CHECK_TIMEOUT_S);
    }

    /// Whether `pid` is gone or a zombie, polled for two seconds: a killed
    /// process nobody reaps stays a zombie, which `kill -0` reports alive.
    #[cfg(unix)]
    fn gone(pid: &str) -> bool {
        for _ in 0..20 {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .stderr(Stdio::null())
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&out.stdout);
            if stat.trim().is_empty() || stat.trim_start().starts_with('Z') {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

#[cfg(test)]
mod output_cap_tests {
    use super::*;

    const WRITTEN: usize = 20 * 1024 * 1024;

    /// A checker that prints `big.txt` (20 MiB, no newline, every byte
    /// placed by its offset) to stdout, one line to stderr, and exits `code`.
    /// Returns its argv and the bytes it prints.
    fn printer(dir: &Path, code: i32) -> (Vec<String>, Vec<u8>) {
        let big: Vec<u8> = (0..WRITTEN)
            .map(|i| b"abcdefghijklmnopqrstuvwxyz0123456789"[i % 36])
            .collect();
        std::fs::write(dir.join("big.txt"), &big).unwrap();
        let argv = if cfg!(windows) {
            vec![
                "cmd".to_string(),
                "/C".into(),
                format!("type big.txt & echo small 1>&2 & exit {code}"),
            ]
        } else {
            vec![
                "sh".to_string(),
                "-c".into(),
                format!("cat big.txt; echo small >&2; exit {code}"),
            ]
        };
        (argv, big)
    }

    /// What `stdout` holds after the truncation line.
    fn body(stdout: &str) -> &str {
        let (note, body) = stdout.split_once('\n').expect("a truncation line");
        assert!(note.starts_with("[trellis: output cut"), "{note:?}");
        body
    }

    /// The risk (#171): a checker printing without end was read whole into
    /// memory. 20 MiB in leaves no more than the cap held, and what is held
    /// is exactly the last bytes written, where a summary sits.
    #[tokio::test]
    async fn check_output_cap_a_20_mib_checker_leaves_at_most_the_cap_with_the_tail_kept() {
        let d = tempfile::tempdir().unwrap();
        let (argv, big) = printer(d.path(), 0);
        let cfg = CheckConfig::default();
        let o = run(d.path(), &cfg, &argv).await;
        let kept = body(&o.stdout);
        assert_eq!(kept.len(), CHECK_OUTPUT_CAP);
        assert!(
            kept.as_bytes() == &big[WRITTEN - CHECK_OUTPUT_CAP..],
            "the kept bytes are not the last {CHECK_OUTPUT_CAP} written"
        );
        assert_eq!(o.stderr.trim(), "small", "the other stream is untouched");
    }

    /// A cut that is not recorded reads as the whole log. The true length
    /// is on `ProcessOutput`, the note reaches the verdict's detail (what
    /// the model and the trail see), and a stream under the cap records
    /// nothing. A cut inside a UTF-8 sequence drops its stray continuation
    /// bytes rather than showing U+FFFD.
    #[tokio::test]
    async fn check_output_cap_the_truncation_is_recorded() {
        let d = tempfile::tempdir().unwrap();
        let (argv, _) = printer(d.path(), 0);
        let cfg = CheckConfig {
            tests: argv.clone(),
            ..CheckConfig::default()
        };
        let o = run(d.path(), &cfg, &argv).await;
        assert_eq!(o.stdout_truncated, Some(WRITTEN as u64));
        assert_eq!(o.stderr_truncated, None);
        let note = format!(
            "[trellis: output cut to its last {CHECK_OUTPUT_CAP} bytes: {WRITTEN} written, \
             the first {} dropped, {CHECK_OUTPUT_CAP} kept]\n",
            WRITTEN - CHECK_OUTPUT_CAP
        );
        assert!(o.stdout.starts_with(&note), "{:?}", &o.stdout[..200]);
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(v.detail().stdout.starts_with(&note));

        let mut c = Captured::default();
        c.push("é".as_bytes());
        c.push(&vec![b'x'; CHECK_OUTPUT_CAP - 1]);
        let (text, total) = c.into_text();
        assert_eq!(total, Some(CHECK_OUTPUT_CAP as u64 + 1));
        assert!(
            !text.contains('\u{FFFD}'),
            "a split sequence shown as U+FFFD"
        );
        assert_eq!(body(&text).len(), CHECK_OUTPUT_CAP - 1);
    }

    /// The verdict rests on the exit code, never on the output: a cut
    /// passing check still passes and a cut failing one still fails, with
    /// its own code.
    #[tokio::test]
    async fn check_output_cap_the_exit_code_is_unchanged() {
        let d = tempfile::tempdir().unwrap();
        for (code, passes) in [(0, true), (3, false)] {
            let (argv, _) = printer(d.path(), code);
            let cfg = CheckConfig {
                tests: argv,
                ..CheckConfig::default()
            };
            let v = run_tests(d.path(), &cfg, &[]).await;
            assert_eq!(v.detail().exit_code, Some(code));
            assert_eq!(v.passed(), passes, "exit {code}");
            assert!(v.detail().stdout.starts_with("[trellis: output cut"));
        }
    }
}

#[cfg(test)]
mod leftover_tests {
    use super::*;

    /// A checker that starts a heartbeat in the background, waits until it
    /// has beaten once, and exits 0.
    ///
    /// With `holds_pipes` the heartbeat inherits the checker's stdout and
    /// stderr, and appends to `hb` without pause: it is writing through the
    /// whole grace, however loaded the machine, so a write after the exit
    /// is certain. Without, its output goes to the null device and it beats
    /// about once a second, 30 times. Bounded either way, so a regression
    /// fails the test rather than leaving a process that holds the run.
    fn heartbeat(dir: &Path, holds_pipes: bool) -> Vec<String> {
        if cfg!(windows) {
            // Script files, because std quotes a `cmd /C` argument with
            // `\"`, which cmd does not read as an escaped quote. Called as
            // `.\`, because `NoDefaultCurrentDirectoryInExePath` may be set.
            let (beat, quiet) = if holds_pipes {
                ("@for /L %%i in (1,1,3000) do @echo x>>hb\r\n", "")
            } else {
                (
                    "@for /L %%i in (1,1,30) do @(echo x>>hb& ping -n 2 127.0.0.1 >NUL)\r\n",
                    " >NUL 2>&1",
                )
            };
            std::fs::write(dir.join("beat.cmd"), beat).unwrap();
            std::fs::write(
                dir.join("check.cmd"),
                format!(
                    "@start /B cmd /C .\\beat.cmd{quiet}\r\n:wait\r\n\
                     @if not exist hb (ping -n 1 127.0.0.1 >NUL& goto wait)\r\n@exit /b 0\r\n"
                ),
            )
            .unwrap();
            vec!["cmd".into(), "/C".into(), r".\check.cmd".into()]
        } else {
            let (pause, beats, quiet) = if holds_pipes {
                ("", 50_000, "")
            } else {
                (" sleep 1;", 30, " >/dev/null 2>&1")
            };
            vec![
                "sh".into(),
                "-c".into(),
                format!(
                    "(i=0; while [ $i -lt {beats} ]; do echo x >> hb; i=$((i+1));{pause} \
                     done){quiet} & while [ ! -s hb ]; do sleep 0.05; done; exit 0"
                ),
            ]
        }
    }

    fn size(p: &Path) -> u64 {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }

    /// The risk (#172): a check that exited normally left what it started
    /// running, and that could write to the tree after the pass was
    /// judged, or after a rollback restored it. On Windows #184 saw a
    /// check's `ping` grandchild outlive the pid-walking `taskkill /T` by
    /// about 120 s. After the call returns the heartbeat must have beaten
    /// (else a silent heartbeat proves nothing) and must not beat again.
    #[tokio::test]
    async fn check_leftover_a_backgrounded_heartbeat_stops_when_the_check_returns() {
        let d = tempfile::tempdir().unwrap();
        let cfg = CheckConfig {
            tests: heartbeat(d.path(), false),
            timeout_s: 60,
            ..CheckConfig::default()
        };
        run_tests(d.path(), &cfg, &[]).await;
        let hb = d.path().join("hb");
        let before = size(&hb);
        assert!(before > 0, "the heartbeat never beat");
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(
            size(&hb),
            before,
            "what the check left running kept writing after the call returned"
        );
    }

    /// A process the checker left running that writes after the exit was
    /// observed voids the pass: the verdict would describe a tree that no
    /// longer exists. Held pipes keep it alive through the grace, so the
    /// write is certain to land after the exit, and the check must fail
    /// saying so, naming the file, whatever its exit code.
    #[tokio::test]
    async fn check_leftover_a_write_after_exit_fails_the_check() {
        let d = tempfile::tempdir().unwrap();
        let cfg = CheckConfig {
            tests: heartbeat(d.path(), true),
            timeout_s: 60,
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(
            !v.passed(),
            "a pass survived a later write: {:?}",
            v.detail()
        );
        assert_eq!(v.detail().exit_code, None);
        let why = &v.detail().stderr;
        assert!(
            why.contains("the working tree changed after its exit was observed"),
            "{why:?}"
        );
        assert!(why.contains("hb"), "the changed file is not named: {why:?}");
        let hb = d.path().join("hb");
        let before = size(&hb);
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(size(&hb), before, "the heartbeat outlived the check");
    }

    /// The other direction: a checker's own writes before it exits (a
    /// `.pytest_cache`, a `.coverage`) are not a change after its exit.
    #[tokio::test]
    async fn check_leftover_a_normal_check_still_passes() {
        let d = tempfile::tempdir().unwrap();
        let argv: Vec<String> = if cfg!(windows) {
            ["cmd", "/C", "echo x>out.txt& echo ok"]
        } else {
            ["sh", "-c", "echo x > out.txt; echo ok"]
        }
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cfg = CheckConfig {
            tests: argv,
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(v.passed(), "{:?}", v.detail());
        assert!(v.detail().stdout.contains("ok"), "{:?}", v.detail().stdout);
        assert!(d.path().join("out.txt").exists());
    }
}
