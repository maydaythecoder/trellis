//! Services an agent profile declares (ADR 0026 §12): long-running commands,
//! such as a dev server, that a task runs beside it.
//!
//! Started in the task's worktree before the task runs, and killed with
//! their whole process tree when [`Services`] is dropped, which is how every
//! way out of a task ends: a converged or failed run, an error, Ctrl-C (the
//! run returns `Interrupted`), and a panic unwinding through the caller.
//!
//! **What a service prints is a log, never a check (ADR 0004).** Its stdout
//! and stderr go to a file in the git directory; nothing here reads them, and
//! the trail records only that the service ran. Convergence still comes from
//! the `Verdict`s `trellis-checks` makes.
//!
//! Each started service is recorded at `<git-dir>/trellis/services/<pid>.json`
//! and the record is removed once its tree has been killed and the service
//! reaped. A record that is still there names a service trellis did not see
//! end yet: its task is still running, or trellis itself was killed, or the
//! reap timed out. `trellis doctor` lists the ones whose process is still
//! alive, and says which of them belong to a task still claimed.

use crate::liveness::SystemLiveness;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use trellis_queue::profile::{Profile, ServiceFile};
use trellis_queue::{Liveness, Probe};

/// The directory under `<git-dir>/trellis/` holding records and logs.
pub(crate) const DIR: &str = "services";

/// How long a killed service may take to be reaped before trellis stops
/// waiting and leaves its record for `doctor`.
const REAP_BOUND: Duration = Duration::from_secs(2);

/// Largest record `doctor` reads. A record is a few hundred bytes; the git
/// directory is writable by anything a run starts, so the read is bounded.
const RECORD_MAX_BYTES: u64 = 64 * 1024;

/// What is written for each started service, for `doctor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub pid: u32,
    /// The start time [`SystemLiveness`] reported just after the spawn,
    /// compared with a later probe so a reused pid is not reported as the
    /// service. `None` when the probe could not say.
    pub started: Option<String>,
    pub agent: String,
    pub service: String,
    pub task: String,
    pub argv: Vec<String>,
}

/// The services running for one task. Dropping it kills every one of them
/// with its process tree, in the reverse of the order they started.
pub(crate) struct Services {
    running: Vec<Service>,
}

struct Service {
    name: String,
    argv: Vec<String>,
    pid: u32,
    /// Removed once the service is known to have ended; left otherwise.
    record: Option<PathBuf>,
    proc: Proc,
}

impl Services {
    /// No services: what a task whose profile declares none holds.
    pub(crate) fn none() -> Self {
        Services {
            running: Vec::new(),
        }
    }

    /// Starts every service `profile` declares, in `root`, for task
    /// `task_id`. The caller has already decided the repository is trusted
    /// (ADR 0023 §4): a service is repository code.
    ///
    /// If one cannot start, the ones already started are killed before the
    /// error returns (the partial set is dropped), so a failed start leaves
    /// nothing running.
    pub(crate) fn start(
        root: &Path,
        trellis_dir: &Path,
        profile: &Profile,
        task_id: &str,
        out: &mut dyn Write,
    ) -> Result<Self> {
        let mut set = Services::none();
        if profile.file.services.is_empty() {
            return Ok(set);
        }
        let dir = trellis_dir.join(DIR);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        for svc in &profile.file.services {
            let s = start_one(root, &dir, &profile.name, svc, task_id)
                .with_context(|| format!("service `{}` did not start", svc.name))?;
            // The argv is the repository's: made plain, so an escape
            // sequence in it cannot repaint the terminal.
            let _ = writeln!(
                out,
                "service:  {} started (pid {}): {}; log {}",
                s.name,
                s.pid,
                crate::plain(&s.argv.join(" ")),
                crate::plain(
                    &dir.join(log_name(&profile.name, &svc.name))
                        .display()
                        .to_string()
                )
            );
            set.running.push(s);
        }
        Ok(set)
    }

    /// One trail action per service: that it ran, and what it ran. Never
    /// its output, and never an outcome a reader could take for a check:
    /// `Action` has no pass field (ADR 0004).
    pub(crate) fn trail_actions(&self) -> Vec<trellis_trail::Action> {
        self.running
            .iter()
            .map(|s| trellis_trail::Action {
                turn: 1,
                tool: "service".to_string(),
                target: format!("{}: {}", s.name, s.argv.join(" ")),
                outcome: "started before the task; its output is a log, not a check".to_string(),
                wrote: false,
                // A service writes nothing through trellis, so it has no
                // changed paths to record; what it touches is not a proposal.
                changed: Vec::new(),
                changed_count: 0,
                duration_ms: None,
                // Not an edit: no line counts (ADR 0007, #811).
                lines_added: None,
                lines_removed: None,
            })
            .collect()
    }

    #[cfg(test)]
    fn pids(&self) -> Vec<u32> {
        self.running.iter().map(|s| s.pid).collect()
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        while let Some(mut s) = self.running.pop() {
            if s.proc.stop() {
                if let Some(r) = s.record.take()
                    && let Err(e) = std::fs::remove_file(&r)
                {
                    tracing::warn!(
                        "service `{}` stopped, but its record {} was not removed: {e}",
                        s.name,
                        r.display()
                    );
                }
            } else {
                // Left for `doctor`: trellis did not see this one end.
                tracing::warn!(
                    "service `{}` (pid {}) was killed but not reaped in {REAP_BOUND:?}; \
                     `trellis doctor` lists it while it is alive",
                    s.name,
                    s.pid
                );
            }
        }
    }
}

fn log_name(agent: &str, service: &str) -> String {
    // Both names are `valid_name`'s alphabet, so neither can hold a
    // separator or `..`.
    format!("{agent}.{service}.log")
}

fn start_one(
    root: &Path,
    dir: &Path,
    agent: &str,
    svc: &ServiceFile,
    task_id: &str,
) -> Result<Service> {
    let log_path = dir.join(log_name(agent, &svc.name));
    // Replaced per start: the log is this task's, and a service that runs
    // for every task would otherwise grow it without end.
    let log = create_fresh(&log_path)?;
    let err_log = log
        .try_clone()
        .with_context(|| format!("cannot reopen {}", log_path.display()))?;
    let (program, args) = svc
        .command
        .split_first()
        .context("`command` names no program")?;
    let mut cmd = Command::new(program);
    // The scrubbed environment and nothing put back: a profile's
    // `pass_env` is for its process agent, not for its services.
    trellis_core::secrets::scrub(&mut cmd);
    cmd.args(args)
        .current_dir(root)
        .env("TRELLIS_TASK_ID", task_id)
        .env("TRELLIS_AGENT", agent)
        .env("TRELLIS_SERVICE", &svc.name)
        // No terminal input or output: under `work --json` stdout is JSONL
        // only, and on Unix a service in a group of its own writing to the
        // terminal would be stopped by SIGTTOU.
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err_log));
    let (proc, pid) = Proc::spawn(cmd)?;
    let started = match SystemLiveness.probe(pid) {
        Probe::Running { started } => started,
        Probe::Gone | Probe::Unknown(_) => None,
    };
    let record = Record {
        pid,
        started,
        agent: agent.to_string(),
        service: svc.name.clone(),
        task: task_id.to_string(),
        argv: svc.command.clone(),
    };
    let path = dir.join(format!("{pid}.json"));
    let mut s = Service {
        name: svc.name.clone(),
        argv: svc.command.clone(),
        pid,
        record: None,
        proc,
    };
    // A service that cannot be recorded could outlive trellis unseen by
    // `doctor`, so it is not left running.
    let body = serde_json::to_vec_pretty(&record).context("service record")?;
    if let Err(e) = create_fresh(&path).and_then(|mut f| {
        f.write_all(&body)
            .with_context(|| format!("cannot write {}", path.display()))
    }) {
        s.proc.stop();
        return Err(e);
    }
    s.record = Some(path);
    Ok(s)
}

/// A new file at `path`, whatever was there removed first, and opened with
/// `create_new`, which does not follow a link: a link planted at a log's or
/// a record's name (by anything else that writes the git directory) is
/// removed as a link, and its target is never truncated. A stale record
/// under a reused pid goes the same way.
fn create_fresh(path: &Path) -> Result<std::fs::File> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot replace {}", path.display())),
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// A started service, held so its whole tree can be killed.
///
/// Unix: it leads a process group of its own and is not reaped until
/// [`Proc::stop`], so the group id stays its own and `kill -KILL -- -<pgid>`
/// cannot reach a stranger (`trellis_core::process::kill_tree`).
#[cfg(unix)]
struct Proc(std::process::Child);

#[cfg(unix)]
impl Proc {
    fn spawn(mut cmd: Command) -> Result<(Proc, u32)> {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let child = cmd.spawn().context("could not start")?;
        let pid = child.id();
        Ok((Proc(child), pid))
    }

    /// Kills the group, then the service itself, then reaps it. `true` when
    /// it was reaped within [`REAP_BOUND`].
    fn stop(&mut self) -> bool {
        trellis_core::process::kill_tree(self.0.id());
        let _ = self.0.kill();
        reap(|| self.0.try_wait())
    }
}

/// A started service, held so its whole tree can be killed.
///
/// Windows: in a Job Object of its own (ADR 0035): created suspended,
/// assigned, then resumed, with no breakaway and kill-on-close, so what it
/// starts is in the job and the tree also ends if trellis does.
#[cfg(windows)]
struct Proc {
    child: Box<dyn process_wrap::tokio::ChildWrapper>,
    /// The wrapper is tokio's; its wait needs a runtime. Last, so it
    /// outlives the child when both drop.
    rt: tokio::runtime::Runtime,
}

#[cfg(windows)]
impl Proc {
    fn spawn(cmd: Command) -> Result<(Proc, u32)> {
        use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("could not build a runtime to hold the service")?;
        let entered = rt.enter();
        let mut wrapped = CommandWrap::from(tokio::process::Command::from(cmd));
        wrapped.wrap(KillOnDrop).wrap(JobObject);
        let mut child = wrapped.spawn().context("could not start")?;
        let Some(pid) = child.id() else {
            // Exited and reaped already, which the wrapper does not do on
            // its own; end the job either way.
            let _ = child.start_kill();
            anyhow::bail!("started, but has no process id");
        };
        drop(entered);
        Ok((Proc { child, rt }, pid))
    }

    /// Ends everything in the job (`TerminateJobObject`), then reaps the
    /// service itself. The job handle names the tree, not a pid, so this is
    /// safe whether or not the service already exited. `true` when it was
    /// reaped within [`REAP_BOUND`].
    fn stop(&mut self) -> bool {
        if let Err(e) = self.child.start_kill() {
            tracing::warn!("could not end the service's job: {e}");
        }
        let wait = self.child.inner_mut().wait();
        matches!(
            self.rt
                .block_on(async { tokio::time::timeout(REAP_BOUND, wait).await }),
            Ok(Ok(_))
        )
    }
}

/// Polls `try_wait` for [`REAP_BOUND`]: std has no timed wait, and a drop
/// must not hang on a process in uninterruptible sleep.
#[cfg(unix)]
fn reap(mut try_wait: impl FnMut() -> std::io::Result<Option<std::process::ExitStatus>>) -> bool {
    let until = std::time::Instant::now() + REAP_BOUND;
    loop {
        match try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) if std::time::Instant::now() >= until => return false,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Every record in `<git-dir>/trellis/services/` whose process is still
/// alive and is the one recorded (same start time where both are known).
/// Records that cannot be read are returned as the reason, so `doctor` can
/// say so rather than skip them.
pub(crate) fn outliving(trellis_dir: &Path, live: &dyn Liveness) -> (Vec<Record>, Vec<String>) {
    let dir = trellis_dir.join(DIR);
    let (mut alive, mut unreadable) = (Vec::new(), Vec::new());
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (alive, unreadable),
        Err(e) => {
            unreadable.push(format!("{}: {e}", dir.display()));
            return (alive, unreadable);
        }
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for p in paths {
        match read_record(&p) {
            Ok(r) => {
                let same = match live.probe(r.pid) {
                    Probe::Running { started } => match (&r.started, &started) {
                        (Some(a), Some(b)) => a == b,
                        // Without both times a reuse cannot be ruled out,
                        // and a live pid is reported rather than hidden.
                        _ => true,
                    },
                    Probe::Gone => false,
                    Probe::Unknown(_) => true,
                };
                if same {
                    alive.push(r);
                }
            }
            Err(e) => unreadable.push(e),
        }
    }
    (alive, unreadable)
}

fn read_record(p: &Path) -> std::result::Result<Record, String> {
    use std::io::Read;
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    let name = name.unwrap_or_default();
    let f = std::fs::File::open(p).map_err(|e| format!("{name}: {e}"))?;
    let mut text = String::new();
    f.take(RECORD_MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{name}: {e}"))?;
    if text.len() as u64 > RECORD_MAX_BYTES {
        return Err(format!("{name}: larger than {RECORD_MAX_BYTES} bytes"));
    }
    let r: Record = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
    // The file name is the pid trellis wrote; a record whose body names
    // another is not one trellis wrote as it stands.
    if name != format!("{}.json", r.pid) {
        return Err(format!("{name}: names pid {}", r.pid));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers from a fixed table, so the pid-reuse rule is tested without
    /// depending on what runs on this machine.
    struct Table(Vec<(u32, Probe)>);
    impl Liveness for Table {
        fn host(&self) -> Option<String> {
            None
        }
        fn probe(&self, pid: u32) -> Probe {
            self.0
                .iter()
                .find(|(p, _)| *p == pid)
                .map(|(_, r)| r.clone())
                .unwrap_or(Probe::Gone)
        }
    }

    fn record(dir: &Path, pid: u32, started: Option<&str>) {
        let r = Record {
            pid,
            started: started.map(str::to_string),
            agent: "a".into(),
            service: "web".into(),
            task: "t1".into(),
            argv: vec!["srv".into()],
        };
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::to_vec(&r).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn doctor_lists_only_a_recorded_service_that_is_still_the_same_process() {
        // The risk: a pid reused by an unrelated process read as a service
        // that outlived its task, or a record of a gone one reported.
        let t = crate::testutil::scratch("services-outliving");
        let dir = t.join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        record(&dir, 11, Some("100"));
        record(&dir, 12, Some("100"));
        record(&dir, 13, Some("100"));
        std::fs::write(dir.join("14.json"), "{not json").unwrap();
        record(&dir, 15, None);
        std::fs::rename(dir.join("15.json"), dir.join("16.json")).unwrap();
        let live = Table(vec![
            (
                11,
                Probe::Running {
                    started: Some("100".into()),
                },
            ),
            (
                12,
                Probe::Running {
                    started: Some("999".into()),
                },
            ),
            (16, Probe::Running { started: None }),
        ]);
        let (alive, bad) = outliving(&t, &live);
        assert_eq!(alive.iter().map(|r| r.pid).collect::<Vec<_>>(), [11]);
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad.iter().any(|b| b.starts_with("14.json")), "{bad:?}");
        assert!(bad.iter().any(|b| b.contains("names pid 15")), "{bad:?}");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_profile_without_services_starts_nothing_and_writes_nothing() {
        let t = crate::testutil::scratch("services-none");
        let p = trellis_queue::profile::parse("a", "model = \"x\"\n").unwrap();
        let s = Services::start(&t, &t, &p, "t1", &mut std::io::sink()).unwrap();
        assert!(s.pids().is_empty());
        assert!(s.trail_actions().is_empty());
        assert!(!t.join(DIR).exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_service_that_cannot_start_leaves_the_ones_before_it_stopped_and_unrecorded() {
        // The risk: the second service's failure returned early and the
        // first one, already running, was never killed.
        let t = crate::testutil::scratch("services-partial");
        let sleeper = if cfg!(windows) {
            "command = ['ping', '-n', '30', '127.0.0.1']"
        } else {
            "command = ['sleep', '30']"
        };
        let p = trellis_queue::profile::parse(
            "a",
            &format!(
                "[[services]]\nname = \"first\"\n{sleeper}\n\
                 [[services]]\nname = \"second\"\ncommand = ['trellis-no-such-program-7f3a']\n"
            ),
        )
        .unwrap();
        let e = Services::start(&t, &t, &p, "t1", &mut std::io::sink())
            .err()
            .expect("the second service cannot start");
        assert!(format!("{e:#}").contains("second"), "{e:#}");
        let left: Vec<_> = std::fs::read_dir(t.join(DIR))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        assert!(left.is_empty(), "a record outlived the failed start");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_record_is_read_up_to_its_bound_and_refused_by_size_past_it() {
        // #248. The risk: the bound off by one either way, or a record
        // past it cut short and refused as bad JSON rather than for its
        // size, which hides what is wrong with it.
        let t = crate::testutil::scratch("services-record-bound");
        let r = Record {
            pid: 21,
            started: None,
            agent: "a".into(),
            service: "web".into(),
            task: "t1".into(),
            argv: vec!["srv".into()],
        };
        let body = serde_json::to_string(&r).unwrap();
        let sized = |n: u64| format!("{body}{}", " ".repeat(n as usize - body.len()));
        let p = t.join("21.json");
        std::fs::write(&p, sized(RECORD_MAX_BYTES)).unwrap();
        assert_eq!(read_record(&p), Ok(r), "exactly the bound is read");
        for n in [RECORD_MAX_BYTES + 1, RECORD_MAX_BYTES + 100] {
            std::fs::write(&p, sized(n)).unwrap();
            let e = read_record(&p).unwrap_err();
            assert!(e.contains("larger than 65536 bytes"), "{n}: {e}");
        }
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn no_services_directory_is_nothing_and_one_that_cannot_be_read_is_said() {
        // #248. The risk: `doctor` calling a missing directory unreadable,
        // or keeping quiet about one it could not read.
        let t = crate::testutil::scratch("services-dir");
        let none = Table(Vec::new());
        assert_eq!(outliving(&t, &none), (Vec::new(), Vec::new()));
        std::fs::write(t.join(DIR), "a file, not a directory").unwrap();
        let (alive, bad) = outliving(&t, &none);
        assert!(alive.is_empty());
        assert_eq!(bad.len(), 1, "{bad:?}");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_name_that_cannot_be_replaced_is_said_to_be_that() {
        // #248. The risk: a log or record name held by a directory
        // reported as a failed create, which sends the reader to the wrong
        // cause.
        let t = crate::testutil::scratch("services-fresh");
        std::fs::create_dir_all(t.join("x.log")).unwrap();
        let e = format!("{:#}", create_fresh(&t.join("x.log")).unwrap_err());
        assert!(e.contains("cannot replace"), "{e}");
        let _ = std::fs::remove_dir_all(&t);
    }
}
