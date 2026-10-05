//! Whether a pid is running on this machine, and what this machine is
//! called: the probe `[work] stale_claims = "release"` needs (ADR 0020 §5).
//!
//! No `unsafe` (the workspace forbids it), so no `kill(pid, 0)` and no
//! `OpenProcess`. What std offers is used where it answers: `/proc` on
//! Linux, `COMPUTERNAME` on Windows. Elsewhere a platform command is run,
//! with the key variables removed like every other child (ADR 0024 §3) and
//! a time bound, and any answer it does not clearly give is
//! [`Probe::Unknown`], which keeps the claim.
//!
//! The start time each probe reports is an opaque string compared only with
//! one the same probe produced at claim time: Linux's is clock ticks since
//! boot, Windows' a FILETIME, other Unix `ps`'s `lstart` in the C locale
//! and UTC.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use trellis_queue::{Liveness, Probe};

/// PowerShell's cold start is the slow case; on a loaded machine it can take
/// seconds. Past this the answer is "unknown" and the claim stays.
const PROBE_BOUND: Duration = Duration::from_secs(20);

pub(crate) struct SystemLiveness;

impl Liveness for SystemLiveness {
    fn host(&self) -> Option<String> {
        let name = if cfg!(windows) {
            std::env::var("COMPUTERNAME").ok()
        } else if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/sys/kernel/hostname").ok()
        } else {
            let mut c = Command::new("hostname");
            trellis_core::secrets::scrub(&mut c);
            bounded(c).ok().filter(|(ok, _)| *ok).map(|(_, out)| out)
        }?;
        let name = name.trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    fn probe(&self, pid: u32) -> Probe {
        if cfg!(windows) {
            probe_windows(pid)
        } else if cfg!(target_os = "linux") {
            probe_proc(pid)
        } else {
            probe_ps(pid)
        }
    }
}

/// `/proc/<pid>/stat`: absent means no such process. Field 22 is the start
/// time; fields are counted after the command name's closing `)`, since the
/// name itself may hold spaces and parentheses.
fn probe_proc(pid: u32) -> Probe {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Probe::Gone,
        Err(e) => return Probe::Unknown(format!("/proc/{pid}/stat: {e}")),
    };
    parse_proc_stat(&stat)
}

fn parse_proc_stat(stat: &str) -> Probe {
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
        return Probe::Unknown("unreadable /proc stat".into());
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Field 3 (state) is fields[0]; field 22 (starttime) is fields[19].
    match fields.first() {
        // A zombie has exited; only its parent's reap is outstanding.
        Some(&("Z" | "X")) => Probe::Gone,
        Some(_) => Probe::Running {
            started: fields.get(19).map(|s| s.to_string()),
        },
        None => Probe::Unknown("unreadable /proc stat".into()),
    }
}

/// One PowerShell line: `GONE`, or `RUNNING` and the start time as a
/// FILETIME. A process whose start time is denied to this user (another
/// user's, an elevated one) is still `RUNNING`, without a time.
fn probe_windows(pid: u32) -> Probe {
    let script = format!(
        "$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; \
         if ($null -eq $p) {{ 'GONE' }} else {{ \
         try {{ 'RUNNING ' + $p.StartTime.ToFileTimeUtc() }} catch {{ 'RUNNING' }} }}"
    );
    let mut c = Command::new("powershell");
    trellis_core::secrets::scrub(&mut c);
    c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    match bounded(c) {
        Ok((true, out)) => parse_tagged(&out),
        Ok((false, out)) => Probe::Unknown(format!("powershell failed: {}", out.trim())),
        Err(e) => Probe::Unknown(e),
    }
}

fn parse_tagged(out: &str) -> Probe {
    let line = out.trim();
    if line == "GONE" {
        Probe::Gone
    } else if let Some(rest) = line.strip_prefix("RUNNING") {
        let t = rest.trim();
        Probe::Running {
            started: (!t.is_empty()).then(|| t.to_string()),
        }
    } else {
        Probe::Unknown(format!("unexpected probe output: {line}"))
    }
}

/// `ps -p <pid> -o lstart=`: prints the start time, or nothing and exits 1
/// when there is no such process.
fn probe_ps(pid: u32) -> Probe {
    let mut c = Command::new("ps");
    trellis_core::secrets::scrub(&mut c);
    c.args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .env("TZ", "UTC");
    match bounded(c) {
        Ok((true, out)) if !out.trim().is_empty() => Probe::Running {
            started: Some(out.trim().to_string()),
        },
        Ok((false, out)) if out.trim().is_empty() => Probe::Gone,
        Ok((_, out)) => Probe::Unknown(format!("ps said: {}", out.trim())),
        Err(e) => Probe::Unknown(e),
    }
}

/// Runs `c` for [`PROBE_BOUND`] at most and returns whether it succeeded and
/// its stdout. std has no timed wait, so it is polled; one that runs over is
/// killed and reaped. The output is one short line, so reading it after the
/// exit cannot deadlock on a full pipe.
fn bounded(mut c: Command) -> Result<(bool, String), String> {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let program = c.get_program().to_string_lossy().into_owned();
    let mut child = c.spawn().map_err(|e| format!("{program}: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= PROBE_BOUND => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} did not answer in {PROBE_BOUND:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out)
            .map_err(|e| format!("{program}: {e}"))?;
    }
    Ok((status.success(), out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proc_stat_is_read_after_the_last_paren_and_a_zombie_is_gone() {
        // A command name may hold `) ` itself; counting from the first paren
        // would read the wrong field as the start time and call a live
        // holder a stranger.
        let running = "42 (a) b) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(
            parse_proc_stat(running),
            Probe::Running {
                started: Some("987654".into())
            }
        );
        let zombie = "42 (x) Z 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 0 0";
        assert_eq!(parse_proc_stat(zombie), Probe::Gone);
        assert!(matches!(parse_proc_stat("garbage"), Probe::Unknown(_)));
    }

    #[test]
    fn only_a_clear_gone_is_gone() {
        assert_eq!(parse_tagged("GONE\r\n"), Probe::Gone);
        assert_eq!(
            parse_tagged("RUNNING 133712345\r\n"),
            Probe::Running {
                started: Some("133712345".into())
            }
        );
        assert_eq!(parse_tagged("RUNNING"), Probe::Running { started: None });
        assert!(matches!(parse_tagged(""), Probe::Unknown(_)));
        assert!(matches!(
            parse_tagged("Get-Process: denied"),
            Probe::Unknown(_)
        ));
    }

    /// A Windows probe that missed [`PROBE_BOUND`]: PowerShell had not
    /// started in time on a loaded host. "Could not answer" is the honest
    /// result then, not a wrong one (#221, #432). The suffix is built from
    /// PROBE_BOUND so a changed message or bound is not tolerated silently.
    fn answered_too_late(p: &Probe) -> bool {
        matches!(p, Probe::Unknown(why)
            if cfg!(windows) && why.ends_with(&format!("did not answer in {PROBE_BOUND:?}")))
    }

    #[test]
    fn a_late_windows_probe_is_tolerated_and_no_other_unknown_is() {
        let late = Probe::Unknown(format!("powershell did not answer in {PROBE_BOUND:?}"));
        assert_eq!(answered_too_late(&late), cfg!(windows));
        assert!(!answered_too_late(&Probe::Unknown(
            "Get-Process: denied".into()
        )));
        assert!(!answered_too_late(&Probe::Unknown(
            "powershell did not answer in 1s".into()
        )));
        assert!(!answered_too_late(&Probe::Gone));
    }

    #[test]
    fn this_process_is_running_and_answers_the_same_start_time_twice() {
        // What the pid-reuse check rests on: the probe's start time for one
        // process is stable, so a mismatch means a different process. Two
        // different Running answers still fail, as does any Unknown but a
        // Windows probe that ran out of time.
        let live = SystemLiveness;
        assert!(live.host().is_some(), "this machine's name is readable");
        let me = std::process::id();
        let a = live.probe(me);
        if answered_too_late(&a) {
            eprintln!("probe could not answer in time, not a wrong answer: {a:?}");
            return;
        }
        assert!(matches!(a, Probe::Running { .. }), "{a:?}");
        let b = live.probe(me);
        if answered_too_late(&b) {
            eprintln!("probe could not answer in time, not a wrong answer: {b:?}");
            return;
        }
        assert_eq!(a, b);
    }

    #[test]
    fn a_child_that_has_exited_and_been_reaped_is_gone() {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 0"]);
            c
        } else {
            Command::new("true")
        };
        trellis_core::secrets::scrub(&mut c);
        let mut child = c.spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // The risk this guards: a probe that calls an exited child Running,
        // which would let the queue keep a dead claim forever. That still
        // fails here, as does any Unknown but a Windows probe that ran out
        // of time (`answered_too_late`; #221, the owner's option C).
        match SystemLiveness.probe(pid) {
            Probe::Gone => {}
            late if answered_too_late(&late) => {
                eprintln!("probe could not answer in time, not a wrong answer: {late:?}");
            }
            other => panic!("an exited, reaped child must read Gone: {other:?}"),
        }
    }
}
