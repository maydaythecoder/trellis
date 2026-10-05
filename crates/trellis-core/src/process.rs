//! Killing a child process and what it started.
//!
//! Here because two crates need it and both may depend on this one: the
//! loop's tool runner (`bash`, custom tools, hooks) and the grounded checks,
//! where a hung mypy or pytest held a run with no deadline. One copy, so a
//! fix to how a tree is reached lands for both.
//!
//! Synchronous std, no async runtime: this crate depends on nothing that
//! would give it one, and a kill is two short-lived commands. An async
//! caller runs it on a blocking thread (`tokio::task::spawn_blocking`).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one kill command may take before it is abandoned.
const KILL_COMMAND_BOUND: Duration = Duration::from_secs(2);

/// Kills process `pid` and what it started, as far as the platform allows
/// without `unsafe`: on Unix the child must lead its own process group
/// (`CommandExt::process_group(0)` at spawn), which `kill -KILL -- -<pgid>`
/// signals whole; on Windows `taskkill /T` walks the tree from the child.
/// A process that left the group (`setsid`), or whose parent already exited
/// on Windows (the tree link is the parent pid), is not reached.
///
/// Call it only while `pid` has not been reaped. Until then the id is still
/// the child's, alive or a zombie; after it, the id (and on Unix the group
/// id) may belong to a stranger, and the signal would reach them. The
/// caller still kills and reaps the child itself afterwards, whatever this
/// did.
pub fn kill_tree(pid: u32) {
    if cfg!(windows) {
        quietly(kill_command(
            "taskkill",
            &["/T", "/F", "/PID", &pid.to_string()],
        ));
    } else {
        kill_group(pid);
    }
}

/// Kills the process group `pgid` leads or led: on Unix the group outlives
/// its leader while any member lives, and `kill -KILL -- -<pgid>` reaches
/// every member that did not leave it. Call it only while the group
/// provably has a member (its leader not yet reaped, or a holder process
/// the caller keeps in it): a group with no members could have had its id
/// reused, and the signal would reach whoever took it.
///
/// Nothing on Windows: there the tree is held by the Job Object that exec
/// children and process agents are spawned into through `process-wrap`
/// (ADR 0035, amending ADR 0023 §3), so there is nothing for this function
/// to do.
pub fn kill_group(pgid: u32) {
    if cfg!(windows) {
        return;
    }
    quietly(kill_command("kill", &["-KILL", "--", &format!("-{pgid}")]));
}

/// A kill command with no input, output or key. The arguments are a
/// formatted number and fixed flags, never a shell string.
fn kill_command(program: &str, args: &[&str]) -> Command {
    let mut k = Command::new(program);
    // `taskkill` and `kill` need no key, and are given none.
    crate::secrets::scrub(&mut k);
    k.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    k
}

/// Runs a kill command for [`KILL_COMMAND_BOUND`] at most. std has no
/// timed wait, so it is polled; a kill command that does not finish in time
/// is killed and reaped rather than left to hold the caller.
fn quietly(mut k: Command) {
    let Ok(mut k) = k.spawn() else { return };
    let started = Instant::now();
    loop {
        match k.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if started.elapsed() >= KILL_COMMAND_BOUND => {
                let _ = k.kill();
                let _ = k.wait();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

// Unix only: the Windows path is `taskkill /T`, exercised by the checks'
// and the tool runner's timeout tests.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The risk this helper exists for: a timeout that killed only the
    /// direct child left what it started running (a shell's `sleep`, a
    /// test runner's workers).
    #[test]
    fn kill_tree_reaches_a_grandchild_in_the_group() {
        use std::os::unix::process::CommandExt;
        let d = std::env::temp_dir().join(format!("trellis-kill-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut c = Command::new("sh");
        c.args(["-c", "sleep 30 & echo $! > bg.pid; wait"])
            .current_dir(&d)
            .process_group(0);
        crate::secrets::scrub(&mut c);
        let mut child = c.spawn().unwrap();
        let pidfile = d.join("bg.pid");
        let bg = (0..100)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(20));
                let s = std::fs::read_to_string(&pidfile).ok()?;
                Some(s.trim().to_string()).filter(|s| !s.is_empty())
            })
            .expect("the shell wrote its background pid");
        kill_tree(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(gone(&bg), "background sleep {bg} survived kill_tree");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Whether `pid` is gone or a zombie, polled for two seconds. A killed
    /// process whose parent does not reap it stays a zombie, which `kill -0`
    /// still reports alive; `ps` shows its state.
    fn gone(pid: &str) -> bool {
        for _ in 0..20 {
            let mut ps = Command::new("ps");
            crate::secrets::scrub(&mut ps);
            let out = ps
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
