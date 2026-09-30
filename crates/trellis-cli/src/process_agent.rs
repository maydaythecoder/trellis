//! Process agents: Paperclip's `process` adapter, with trellis's checks
//! as the only judge (ADR 0026 §11).
//!
//! A profile with `command = [argv]` runs its tasks by running that command
//! in a throwaway clone of the repository (ADR 0032, "Process agents"),
//! carrying back what it changed there under the refusals every writer
//! meets, then running the same two grounded checks `trellis check` runs,
//! and the governance rules a model run obeys. The run converged if and
//! only if the command changed something, all of it came back, and both
//! checks passed; anything else is rolled back exactly as a failed model
//! run is (ADR 0009).
//!
//! **The external agent never produces a verdict (ADR 0004).** Its exit
//! code, its output and whatever it writes are data. The two `Verdict`s
//! that decide convergence are constructed by `trellis-checks` from
//! processes it ran itself; nothing here can make one.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use trellis_queue::RunEnd;
use trellis_queue::profile::Profile;
use trellis_vcs::git_guard::GitGuard;
use trellis_vcs::ignored::{IgnoredChanges, IgnoredSnapshot};

/// What the command did before the checks ran.
enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Interrupted,
}

/// The files the command touched, against the commit recorded before it
/// (`base`), not the current `HEAD`: a file the command committed is as
/// touched as one it left, and judged the same (review round 10). The
/// tracked side is `git diff <base>` (worktree against that commit, so
/// committed, staged and unstaged changes alike, `--no-renames` so a
/// rename's source is named too); the untracked side is every untracked
/// file rather than its directory, so each can be audited. `-z` on both,
/// so no path is quoted or cut.
fn changed(root: &Path, base: &str) -> Result<Vec<String>, String> {
    // Run after the git guard has put `.git/` back; `command` turns
    // `core.fsmonitor` and hooks off regardless.
    let list = |args: &[&str]| -> Result<Vec<String>, String> {
        let mut git = trellis_vcs::command();
        git.args(args).current_dir(root).stdin(Stdio::null());
        let o = git
            .output()
            .map_err(|e| format!("git {} could not run: {e}", args[0]))?;
        if !o.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&o.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect())
    };
    let mut out = list(&["diff", "--name-only", "-z", "--no-renames", base, "--"])?;
    for p in list(&["ls-files", "-z", "--others", "--exclude-standard"])? {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// What judges the run, read from the worktree **before** the command runs:
/// the `[checks]` commands, `[governance] deny_write`, and the rule
/// documents under `.trellis/governance/`. Read after, they were the
/// command's to choose: it could rewrite `typecheck` to `true` or drop a
/// `deny_write` line and be judged by its own rules (ADR 0026 §11).
struct Judge {
    checks: trellis_checks::CheckConfig,
    rules: Vec<trellis_checks::governance::Rule>,
    /// Built once: `denied` runs per modified ignored file in undo's
    /// protected closure, and parsing and folding every rule each time
    /// was repeated work (review round 12). Carries the profile's
    /// `allow_write` too, which only [`Self::governance`] consults.
    deny: trellis_loop::Governance,
}

impl Judge {
    /// `profile` was read before the command ran, as the rest is: its
    /// `allow_write` is one more thing the command may not choose.
    fn load(root: &Path, profile: &Profile) -> Result<Judge, String> {
        let rules = trellis_checks::governance::load(root).map_err(|e| e.to_string())?;
        let cfg = crate::config::RepoConfig::load(root).map_err(|e| format!("{e:#}"))?;
        let mut deny: Vec<String> = cfg
            .governance()
            .rules()
            .iter()
            .map(|r| r.as_written())
            .collect();
        deny.extend(trellis_checks::governance::deny_write_paths(&rules));
        let mut deny = trellis_loop::Governance::new(&deny);
        if let Some(allow) = &profile.file.allow_write {
            deny = deny.with_allow_write(&profile.name, allow);
        }
        Ok(Judge {
            // Once per run, not per check spawn (review round 11).
            checks: cfg.checks().for_run(root),
            rules,
            deny,
        })
    }

    /// Governance as a model run meets it: `deny_write` over every path the
    /// command changed, then the profile's `allow_write` over those the
    /// repository's rules let through, then the `forbid` audit over those
    /// files. The first two are a list of breaches; the last is a `Verdict` from
    /// `trellis-checks`, present only when the repository has rule
    /// documents, as the loop audits only then.
    ///
    /// Any path under `.trellis/` is a breach whatever the rules say: that
    /// directory holds the checks, the rules and the trust inputs that judge
    /// this run, and a change there is the command editing its own judge.
    fn governance(
        &self,
        root: &Path,
        touched: &[String],
    ) -> (Vec<String>, Option<trellis_checks::Verdict>) {
        let mut denied = self.denied(root, touched.iter());
        // Over what git shows changed, not over ignored files: the checks
        // and the tests write caches there on every run, and a scope that
        // counted them would refuse every run of a scoped agent. Ignored
        // files are not part of the change a run leaves for review.
        denied.extend(touched.iter().filter_map(|p| {
            if trellis_loop::names_trellis_dir(p) || self.deny.denies_write_at(root, p).is_some() {
                return None;
            }
            self.deny
                .outside_allow_write_at(root, p)
                .map(|why| format!("{p} ({why})"))
        }));
        let audit = (!self.rules.is_empty())
            .then(|| trellis_checks::governance_audit(root, &self.rules, touched));
        (denied, audit)
    }

    /// The `.trellis/` breach and `deny_write` over `paths`, whether git
    /// shows them or ignores them: an ignored path is still a write, and a
    /// rule that says "not here" does not ask git first.
    fn denied<'p>(&self, root: &Path, paths: impl Iterator<Item = &'p String>) -> Vec<String> {
        let g = &self.deny;
        paths
            .filter_map(|p| {
                if trellis_loop::names_trellis_dir(p) {
                    return Some(format!(
                        "{p} (under .trellis/, which holds what judges this run)"
                    ));
                }
                g.denies_write_at(root, p)
                    .map(|r| format!("{p} (deny_write `{}`)", r.as_written()))
            })
            .collect()
    }
}

pub(crate) struct ProcessRun<'a> {
    pub root: &'a Path,
    pub profile: &'a Profile,
    pub task_id: &'a str,
    /// The task text, with the profile's instructions ahead of it.
    pub text: &'a str,
    pub stop: &'a AtomicBool,
    /// The profile's services, started by the caller before this run and
    /// recorded first among the trail's actions (ADR 0026 §12).
    pub services: &'a [trellis_trail::Action],
}

pub(crate) fn run(p: ProcessRun<'_>, out: &mut dyn Write) -> RunEnd {
    let ProcessRun {
        root,
        profile,
        task_id,
        text,
        stop,
        services,
    } = p;
    let argv = profile.file.command.clone().unwrap_or_default();
    let timeout = Duration::from_secs(
        profile
            .file
            .timeout_s
            .unwrap_or(trellis_queue::profile::DEFAULT_TIMEOUT_S),
    );

    // Trusted first (ADR 0023 §4): `work` already waits on this, and the
    // runner checks again rather than assume every caller did.
    if let Err(why) = crate::tooling::require_trusted(root) {
        return RunEnd::Error {
            run_id: None,
            error: format!("agent `{}` runs a repository command; {why}", profile.name),
        };
    }
    // A clean tree first, exactly as a model run requires (ADR 0009).
    let snapshot = match trellis_vcs::snapshot(root) {
        Ok(s) => s,
        Err(e) => {
            return RunEnd::Error {
                run_id: None,
                error: format!("a clean git worktree is required before a run: {e}"),
            };
        }
    };
    let _ = writeln!(
        out,
        "agent:    {} (process: {})",
        profile.name,
        argv.join(" ")
    );
    let _ = writeln!(out, "snapshot at {}", snapshot.head);
    // Before the command, for the reason the git guard below is: what is
    // read after it is whatever it left there.
    let judge = match Judge::load(root, profile) {
        Ok(j) => j,
        Err(e) => {
            // Nothing ran, so nothing changed; the snapshot needs no rollback.
            return RunEnd::Error {
                run_id: None,
                error: format!("the checks and governance rules could not be read: {e}"),
            };
        }
    };
    // Before the command, since nothing can be put back that was not read.
    let git_guard = match GitGuard::capture(root) {
        Ok(g) => g,
        Err(e) => {
            return RunEnd::Error {
                run_id: None,
                error: format!("could not read .git/config and hooks before the command: {e}"),
            };
        }
    };
    // The command runs here, not in the repository (ADR 0032). A clone
    // that cannot be made is never a reason to run it in place.
    let clone = match trellis_loop::AgentClone::prepare(root) {
        Ok(c) => c,
        Err(e) => {
            return RunEnd::Error {
                run_id: None,
                error: format!("the agent's throwaway clone could not be prepared: {e}"),
            };
        }
    };
    // Last before the spawn, so what differs afterwards is the command's.
    // The guards on the repository stay for a command that writes there by
    // absolute path, which the clone does not stop (ADR 0032, "Limits").
    let ignored_before = match IgnoredSnapshot::take(root) {
        Ok(s) => s,
        Err(e) => {
            return RunEnd::Error {
                run_id: None,
                error: format!("could not list the ignored files before the command: {e}"),
            };
        }
    };

    let started = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::piped())
        .env("TRELLIS_TASK", text)
        .env("TRELLIS_TASK_ID", task_id)
        .env("TRELLIS_AGENT", &profile.name)
        // As for hooks and tools (`exec::run`): bytecode it wrote under
        // `.trellis/` would untrust the repository it was trusted in.
        .env("PYTHONDONTWRITEBYTECODE", "1");
    // Its working directory is the clone, and no `GIT_DIR` or the like
    // sends a `git` it runs back to the repository.
    clone.confine(&mut cmd);
    trellis_core::secrets::scrub(&mut cmd);
    // After the scrub, so exactly the names the profile lists come back. An
    // external agent may need a key of its own; whatever it then prints goes
    // to the terminal, so passing one is the profile owner's choice, made in
    // a committed file (ADR 0026 §11).
    for name in &profile.file.pass_env {
        if let Some(v) = std::env::var_os(name) {
            cmd.env(name, v);
        }
    }
    // In trellis's own process group, the terminal's foreground group: it
    // is the user's command on the user's terminal. In a group of its own
    // (round 8) it was a background job, stopped by SIGTTOU or SIGTTIN the
    // moment it touched the terminal (review round 9). What that costs is
    // in `Running::end`. On Windows it is spawned into a Job Object
    // (ADR 0035).
    let child = Running::spawn(cmd);
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            // Nothing ran, so nothing changed; the snapshot needs no rollback.
            return RunEnd::Error {
                run_id: None,
                error: format!("could not start `{}`: {e}", argv[0]),
            };
        }
    };
    if let Some(mut stdin) = child.take_stdin() {
        let body = text.to_string();
        // On its own thread: a command that never reads stdin must not
        // block the wait below once the pipe buffer is full.
        std::thread::spawn(move || {
            let _ = stdin.write_all(body.as_bytes());
        });
    }
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ended::Exited(status.code()),
            Ok(None) => {}
            Err(_) => break Ended::Exited(None),
        }
        if stop.load(Ordering::SeqCst) {
            child.end();
            break Ended::Interrupted;
        }
        if started.elapsed() >= timeout {
            child.end();
            break Ended::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // However it ended, before anything is restored, judged or rolled back:
    // on Windows what the command left running ends here, so a backgrounded
    // writer cannot land after the comparison (ADR 0035).
    child.finish();
    let secs = started.elapsed().as_secs();
    let exit_text = match &ended {
        Ended::Exited(Some(c)) => format!("exited {c} after {secs}s"),
        Ended::Exited(None) => format!("ended by a signal after {secs}s"),
        Ended::TimedOut => format!("killed at the {}s timeout", timeout.as_secs()),
        Ended::Interrupted => format!("interrupted after {secs}s"),
    };
    let _ = writeln!(out, "process:  {exit_text}");

    // Whatever else happens, `.git/config` and the hooks go back first: a
    // change there is never part of a converged run, and the worktree
    // rollback would not undo it.
    let (git_changed, git_unrestored) = git_guard.restore();
    if !git_changed.is_empty() {
        let _ = writeln!(
            out,
            "git:      the command changed {}; put back",
            git_changed.join(", ")
        );
    }
    for f in &git_unrestored {
        let _ = writeln!(out, "git:      could NOT be put back: {f}");
    }
    // After `.git/` is back, since the carry-back asks the repository's git
    // what it ignores. Only a command that ran to its end is carried back:
    // what a timed-out or interrupted one left is half a change.
    let back = if matches!(ended, Ended::Exited(_)) {
        let back = clone.carry_back(root, &judge.deny);
        report_carry_back(&back, out);
        back
    } else {
        let _ = writeln!(
            out,
            "clone:    nothing carried back from an unfinished command"
        );
        trellis_loop::CarriedBack::default()
    };
    // The clone, and whatever the command left in it.
    drop(clone);
    // After `.git/` is back, before anything is judged: a commit, reset or
    // checkout is a breach whatever else the command did. What it
    // committed is invisible to `git status`, so without this the
    // `.trellis/` breach, `deny_write` and the audit judged only what it
    // left uncommitted (review round 10). The rollback puts `HEAD` back on
    // the recorded branch at the recorded commit. A HEAD that cannot be
    // read counts as moved.
    let head_moved = match snapshot.head_moved() {
        Ok(m) => m,
        Err(e) => Some(format!("HEAD could not be read after the command: {e}")),
    };
    if let Some(m) = &head_moved {
        let _ = writeln!(out, "git:      {m}; a run may not move HEAD");
    }

    // Before the checks, which write caches of their own (`.mypy_cache`).
    // A listing that fails reads as every ignored file deleted: nothing is
    // removed on its strength, and the run cannot converge.
    let (ignored, ignored_unread) = match IgnoredSnapshot::take(root) {
        Ok(after) => (ignored_before.diff(&after), None),
        Err(e) => (IgnoredChanges::default(), Some(e)),
    };
    if let Some(e) = &ignored_unread {
        let _ = writeln!(
            out,
            "ignored:  could not list ignored files after the command: {e}"
        );
    }
    if !ignored.is_empty() {
        let _ = writeln!(
            out,
            "ignored:  the command changed files git ignores: {}",
            ignored.summary()
        );
    }
    let ignored_denied = judge.denied(root, ignored.all());

    let touched = match changed(root, &snapshot.head) {
        Ok(t) => t,
        Err(e) => {
            let restored =
                undo(root, &snapshot, &ignored, &judge, out) && git_unrestored.is_empty();
            return if restored {
                RunEnd::Error {
                    run_id: None,
                    error: format!("could not read what the command changed: {e}"),
                }
            } else {
                RunEnd::NotRestored { run_id: None }
            };
        }
    };

    // The checks run only on a completed command that changed something:
    // a timed-out or interrupted command left a tree nobody should verify,
    // and an unchanged tree has nothing to verify.
    let mut verdicts = Vec::new();
    // Ignored writes are judged whether or not anything git shows changed,
    // and so is what the carry-back refused on the judge's grounds.
    let mut denied = ignored_denied;
    denied.extend(
        back.breaches
            .iter()
            .map(|b| format!("{} ({})", b.path, b.why)),
    );
    let mut checked = false;
    let judge_touched = touched
        .iter()
        .chain(ignored.all())
        .chain(back.breaches.iter().map(|b| &b.path))
        .any(|p| trellis_loop::names_trellis_dir(p));
    if matches!(ended, Ended::Exited(_)) && !touched.is_empty() {
        let (d, audit) = judge.governance(root, &touched);
        denied.extend(d);
        verdicts.extend(audit);
    }
    for d in &denied {
        let _ = writeln!(out, "governance: refused: {d}");
    }
    // A moved HEAD is refused whatever the checks say, so they are not run.
    if matches!(ended, Ended::Exited(_)) && !touched.is_empty() && head_moved.is_none() {
        let checks = &judge.checks;
        let paths = vec![PathBuf::from(".")];
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                verdicts.extend(rt.block_on(async {
                    vec![
                        trellis_checks::typecheck(root, checks, &paths).await,
                        trellis_checks::run_tests(root, checks, &paths).await,
                    ]
                }));
                checked = true;
            }
            Err(e) => {
                let _ = writeln!(out, "checks:   could not start a runtime: {e}");
            }
        }
        for v in &verdicts {
            let d = v.detail();
            let _ = writeln!(
                out,
                "check:    {:?} {} ({})",
                d.kind,
                if v.passed() { "passed" } else { "FAILED" },
                d.command
            );
            // The audit's findings name the rule and the line; a bare
            // "FAILED" is not something anyone can act on.
            if d.kind == trellis_checks::CheckKind::Governance {
                for l in d.stdout.lines().take(20) {
                    let _ = writeln!(out, "          {l}");
                }
            }
        }
    }
    // Both checks ran and every verdict passed (the governance audit's too,
    // when there is one); nothing was written where a rule denies it; and
    // nothing under `.git/` changed; and HEAD did not move.
    let converged = checked
        && verdicts.iter().all(|v| v.passed())
        && denied.is_empty()
        && back.refused.is_empty()
        && git_changed.is_empty()
        && head_moved.is_none()
        && ignored_unread.is_none();
    let why = if converged {
        "converged: both checks passed"
    } else {
        match &ended {
            Ended::TimedOut => "not converged: timed out",
            Ended::Interrupted => "interrupted",
            _ if !git_changed.is_empty() => {
                "not converged: the command changed files under .git/, which a run may not"
            }
            _ if head_moved.is_some() => {
                "not converged: the command moved HEAD (a commit, reset or checkout), which \
                 a run may not: a run leaves its change uncommitted, for review"
            }
            _ if ignored_unread.is_some() => {
                "not converged: what the command did to ignored files could not be read"
            }
            // Before "changed nothing": a command whose every change was
            // refused left nothing in the repository, and that is not why.
            _ if judge_touched => {
                "not converged: the command changed .trellis/, which holds the checks and \
                 rules that judge it"
            }
            _ if !denied.is_empty() => {
                "not converged: the command wrote where [governance] deny_write refuses, \
                 or outside the agent's allow_write"
            }
            _ if !back.refused.is_empty() => {
                "not converged: part of what the command changed in its clone could not be \
                 carried back"
            }
            _ if touched.is_empty() => {
                "not converged: the command changed nothing, so there was nothing to verify"
            }
            _ => "not converged: a check failed",
        }
    };
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} ({why})",
        if converged {
            "CONVERGED"
        } else {
            "NOT CONVERGED"
        }
    );

    let restored = if converged {
        true
    } else {
        // A `.git/` file that could not be put back is a tree not restored.
        undo(root, &snapshot, &ignored, &judge, out) && git_unrestored.is_empty()
    };

    let run_id = write_trail(
        root,
        TrailInput {
            task: text,
            base: &snapshot.head,
            argv: &argv,
            profile: &profile.name,
            touched: &touched,
            exit_text: &exit_text,
            ignored: &ignored,
            verdicts,
            services,
        },
        out,
    );

    match ended {
        Ended::Interrupted => RunEnd::Interrupted { run_id, restored },
        _ if converged => RunEnd::Converged { run_id },
        _ if restored => RunEnd::RolledBack { run_id },
        _ => RunEnd::NotRestored { run_id },
    }
}

/// What the carry-back did, one line per kind. The judge's refusals are
/// printed with the rest of governance's, as `governance: refused:`.
fn report_carry_back(back: &trellis_loop::CarriedBack, out: &mut dyn Write) {
    use trellis_vcs::ignored::shown;
    for (what, list) in [
        ("edited", &back.written),
        ("created", &back.created),
        ("deleted", &back.deleted),
    ] {
        if !list.is_empty() {
            let _ = writeln!(out, "clone:    {what} from the clone: {}", shown(list));
        }
    }
    if !back.ignored.is_empty() {
        let _ = writeln!(
            out,
            "clone:    left in the clone, as the repository's git ignores it: {}",
            shown(&back.ignored)
        );
    }
    for r in &back.refused {
        let _ = writeln!(out, "clone:    not carried back: {}", r.why);
    }
}

/// The rollback of a run that did not converge: the ignored files the
/// command created are removed first (`git clean -fd` leaves ignored files,
/// and then removes the directories their removal emptied), then the
/// worktree goes back to the snapshot. An ignored file that existed before
/// and was changed or deleted cannot be put back, because trellis keeps no
/// copy of ignored files; it is named. `false` when a created file could not
/// be removed, a pre-existing one was deleted, or changed where `.trellis/`
/// or `deny_write` protects it, or the worktree is not restored. A changed
/// one nothing protects does not count: an agent that runs the tests
/// rewrites their caches every time (review round 9).
fn undo(
    root: &Path,
    snapshot: &trellis_vcs::Snapshot,
    ignored: &IgnoredChanges,
    judge: &Judge,
    out: &mut dyn Write,
) -> bool {
    let protected = |p: &str| {
        !judge
            .denied(root, std::iter::once(&p.to_string()))
            .is_empty()
    };
    let removed = trellis_vcs::ignored::undo(root, ignored, &protected, out);
    rollback(snapshot, out) && removed
}

/// The running command. On Unix a plain child in trellis's own process
/// group (ADR 0026 §11); on Windows a child in a Job Object of its own,
/// with kill-on-close and no breakaway (ADR 0035).
#[cfg(unix)]
struct Running(std::process::Child);

#[cfg(unix)]
impl Running {
    /// Spawns `cmd`, which the caller has already scrubbed.
    fn spawn(mut cmd: Command) -> std::io::Result<Running> {
        cmd.spawn().map(Running)
    }

    fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.0.stdin.take()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }

    /// Ends the command on a timeout or an interrupt, while it is still
    /// alive and unreaped. It shares trellis's process group, so there is no
    /// group of its own to signal, and signalling trellis's would end
    /// trellis: the command itself is killed, and what it started may
    /// outlive it and write after the comparison (ADR 0026 §11). Ctrl-C at
    /// the terminal reaches the whole foreground group, the command's
    /// children included, as it reaches trellis.
    fn end(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }

    /// After the command has ended: nothing is signalled on Unix, for the
    /// reason `end` gives.
    fn finish(&mut self) {}
}

#[cfg(windows)]
struct Running {
    child: Box<dyn process_wrap::tokio::ChildWrapper>,
    stdin: Option<std::fs::File>,
    /// The wrapper is tokio's; its pipes and its wait need a runtime. Last,
    /// so it outlives the child when both drop.
    rt: tokio::runtime::Runtime,
}

#[cfg(windows)]
impl Running {
    /// Spawns `cmd`, which the caller has already scrubbed, into a Job
    /// Object of its own: created suspended, assigned, then resumed, so
    /// nothing it starts runs outside the job. The job allows no breakaway,
    /// so a `CREATE_BREAKAWAY_FROM_JOB` child fails to start rather than
    /// escape, and it is kill-on-close, so the tree also ends if trellis
    /// does. A job that cannot be made or joined kills the child and is an
    /// error, as a command that cannot start is.
    fn spawn(cmd: Command) -> std::io::Result<Running> {
        use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let _entered = rt.enter();
        let mut wrapped = CommandWrap::from(tokio::process::Command::from(cmd));
        wrapped.wrap(KillOnDrop).wrap(JobObject);
        let mut child = wrapped.spawn()?;
        // A plain file handle, so the existing writer thread can write it
        // with no runtime.
        let stdin = match child.stdin().take().map(|s| s.into_owned_handle()) {
            Some(Ok(h)) => Some(std::fs::File::from(h)),
            Some(Err(e)) => {
                tracing::warn!("the command's stdin could not be taken; it gets none: {e}");
                None
            }
            None => None,
        };
        Ok(Running { child, stdin, rt })
    }

    fn take_stdin(&mut self) -> Option<std::fs::File> {
        self.stdin.take()
    }

    /// The command itself, not the job: the wrapper's own wait would wait
    /// for everything the command started.
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.inner_mut().try_wait()
    }

    /// Ends the command and everything in its job on a timeout or an
    /// interrupt, then reaps the command.
    fn end(&mut self) {
        self.finish();
        let rt = &self.rt;
        let wait = self.child.inner_mut().wait();
        let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), wait).await });
    }

    /// Ends whatever is still in the command's job (`TerminateJobObject`),
    /// however the command ended. The job handle names the tree, not a pid,
    /// so this is safe after the command has been reaped.
    fn finish(&mut self) {
        if let Err(e) = self.child.start_kill() {
            tracing::warn!("could not end the command's job: {e}");
        }
    }
}

fn rollback(snapshot: &trellis_vcs::Snapshot, out: &mut dyn Write) -> bool {
    if let Err(e) = snapshot.rollback() {
        let _ = writeln!(out, "ROLLBACK FAILED: {e}");
    }
    match snapshot.is_restored() {
        Ok(true) => {
            // The branch too: after a checkout, putting HEAD back on it is
            // part of what the rollback did.
            let _ = writeln!(
                out,
                "rolled back to {}{}",
                snapshot.head,
                snapshot
                    .branch
                    .as_deref()
                    .map(|b| format!(" on {}", b.trim_start_matches("refs/heads/")))
                    .unwrap_or_else(|| " (detached)".into())
            );
            true
        }
        _ => {
            let _ = writeln!(
                out,
                "the tree is NOT restored to {}; inspect it before running again",
                snapshot.head
            );
            false
        }
    }
}

struct TrailInput<'a> {
    task: &'a str,
    base: &'a str,
    argv: &'a [String],
    profile: &'a str,
    touched: &'a [String],
    exit_text: &'a str,
    ignored: &'a IgnoredChanges,
    verdicts: Vec<trellis_checks::Verdict>,
    services: &'a [trellis_trail::Action],
}

/// The trail for a process run, built from `trellis-trail` types: the
/// model run's `record_trail` takes a loop transcript this run does not
/// have. Written after any rollback, as there, so `git clean` cannot take
/// it, and committed alone.
fn write_trail(root: &Path, t: TrailInput<'_>, out: &mut dyn Write) -> Option<String> {
    use trellis_trail::{
        Action, Approval, CheckOutcome, Factual, Judgment, Provenance, RadiusCounts, SCHEMA, Trail,
    };
    let check_outcomes = t
        .verdicts
        .iter()
        .map(|v| CheckOutcome {
            kind: format!("{:?}", v.kind()),
            passed: v.passed(),
        })
        .collect();
    let factual = Factual {
        proposed: t.touched.to_vec(),
        radius: RadiusCounts::default(),
        // Real verdicts, which this path — unlike the loop's — holds: they
        // serialize, and read back as inert `RecordedCheck` data.
        checks: t.verdicts,
        check_outcomes,
        approved_by: Approval::None,
        mode_counts: crate::audit_modes(root, t.touched),
        context: None,
        actions: t
            .services
            .iter()
            .cloned()
            .chain(std::iter::once(Action {
                turn: 1,
                tool: "process".into(),
                target: t.argv.join(" "),
                outcome: t.exit_text.to_string(),
                wrote: !t.touched.is_empty(),
                changed: Vec::new(),
                changed_count: 0,
            }))
            // What it did to files git ignores: not `proposed`, which is what
            // a commit would carry, but recorded, since a converged run keeps
            // what it created there (ADR 0026 §11).
            .chain((!t.ignored.is_empty()).then(|| Action {
                turn: 1,
                tool: "process".into(),
                target: "files git ignores".into(),
                outcome: t.ignored.summary(),
                wrote: true,
                changed: Vec::new(),
                changed_count: 0,
            }))
            .collect(),
        control: Some("auto (process agent)".into()),
        // An external process's tokens are not visible to trellis.
        tokens: None,
        prompt_tokens: None,
        agent: Some(t.profile.to_string()),
        // A label is a session's (`/name`), and a process agent runs from
        // `work`, never from a session.
        label: None,
        // A process agent is given its task on stdin and nothing else.
        inputs: Vec::new(),
        // Nothing reaches a process agent after it starts, so nothing
        // typed mid-run can be recorded for it.
        steering: Vec::new(),
        // One process, one tree: no partitions to collide (ADR 0042).
        suspected_recall_gaps: Vec::new(),
    };
    let (timestamp, started_at) = trellis_trail::now();
    let slug = trellis_trail::slugify(t.task);
    let trail = Trail {
        schema: SCHEMA,
        run_id: format!("{timestamp}-{slug}"),
        task: t.task.to_string(),
        base_commit: t.base.to_string(),
        started_at,
        provenance: Provenance {
            trellis_version: env!("CARGO_PKG_VERSION").to_string(),
            model: Some(format!("process:{}", t.argv.join(" "))),
            tier: None,
        },
        factual_sha256: Trail::compute_factual_sha256(&factual),
        factual,
        judgment: Judgment::default(),
    };
    match trellis_trail::write(root, &trail, &timestamp, &slug) {
        Ok(p) => {
            let rel = Trail::path(&timestamp, &slug);
            if let Err(e) =
                trellis_vcs::commit_paths(root, &[&rel], &format!("trellis: trail for {}", t.task))
            {
                let _ = writeln!(out, "trail written but not committed: {e}");
            }
            let _ = writeln!(out, "trail:    {}", p.display());
            Some(trail.run_id)
        }
        Err(e) => {
            let _ = writeln!(out, "could not write the trail: {e}");
            None
        }
    }
}
