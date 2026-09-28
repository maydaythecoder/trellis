//! Process agents: Paperclip's `process` adapter, with trellis's checks
//! as the only judge (ADR 0026 §11).
//!
//! A profile with `command = [argv]` runs its tasks by running that command
//! in the worktree, then running the same two grounded checks `trellis
//! check` runs, and the governance rules a model run obeys. The run
//! converged if and only if the command changed
//! something and both checks passed; anything else is rolled back exactly
//! as a failed model run is (ADR 0009).
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
use trellis_vcs::ignored::{IgnoredChanges, IgnoredSnapshot};

/// What the command did before the checks ran.
enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Interrupted,
}

/// The files the command touched, as `git status` names them: every
/// untracked file rather than its directory, so each can be audited, and
/// `-z` so no path is quoted or cut. (Read through `trellis_vcs::pending`
/// before, whose trimmed lines lost the first character of a modified
/// tracked file's path.)
fn changed(root: &Path) -> Result<Vec<String>, String> {
    let mut git = Command::new("git");
    git.args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(root)
        .stdin(Stdio::null());
    // `git status` can run the repository's filters and fsmonitor.
    trellis_core::secrets::scrub(&mut git);
    let o = git
        .output()
        .map_err(|e| format!("git status could not run: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let mut out = Vec::new();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        let (xy, path) = (f.get(..2).unwrap_or(""), f.get(3..).unwrap_or(""));
        if !path.is_empty() {
            out.push(path.to_string());
        }
        // A rename or copy carries its source as the next field; the
        // source is changed too (it is gone).
        if (xy.contains('R') || xy.contains('C'))
            && let Some(src) = fields.next()
        {
            out.push(src.to_string());
        }
    }
    Ok(out)
}

/// The files under `.git/` a command can use to run code later, as bytes:
/// `config` (`core.hooksPath`, aliases, `core.fsmonitor`), every file in
/// the hooks directory, and `info/exclude`, which decides what `git status`
/// and the rollback's `git clean` can see at all: a `*` written there would
/// hide every later run's new files. `git status` never shows these and
/// the rollback never touches them, so they are compared around the command
/// and put back. Nothing else in `.git/` is covered (ADR 0026 §11).
struct GitGuard {
    root: PathBuf,
    config: PathBuf,
    hooks: PathBuf,
    exclude: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl GitGuard {
    fn capture(root: &Path) -> Result<GitGuard, String> {
        let mut git = Command::new("git");
        git.args([
            "rev-parse",
            "--git-path",
            "config",
            "--git-path",
            "hooks",
            "--git-path",
            "info/exclude",
        ])
        .current_dir(root)
        .stdin(Stdio::null());
        trellis_core::secrets::scrub(&mut git);
        let o = git
            .output()
            .map_err(|e| format!("git rev-parse could not run: {e}"))?;
        let text = String::from_utf8_lossy(&o.stdout);
        let mut lines = text.lines().map(|l| root.join(l.trim()));
        let (Some(config), Some(hooks), Some(exclude)) = (lines.next(), lines.next(), lines.next())
        else {
            return Err(format!(
                "git rev-parse --git-path failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        };
        let mut g = GitGuard {
            root: root.to_path_buf(),
            config,
            hooks,
            exclude,
            files: Vec::new(),
        };
        g.files = g.read()?;
        Ok(g)
    }

    fn read(&self) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
        let mut paths = vec![self.config.clone(), self.exclude.clone()];
        let mut dirs = vec![self.hooks.clone()];
        while let Some(d) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else {
                    paths.push(p);
                }
            }
        }
        paths.sort();
        let mut out = Vec::new();
        for p in paths {
            match std::fs::read(&p) {
                Ok(b) => out.push((p, b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{} could not be read: {e}", p.display())),
            }
        }
        Ok(out)
    }

    fn label(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .display()
            .to_string()
            .replace('\\', "/")
    }

    /// What differs from the capture, each put back: a changed or deleted
    /// file gets its bytes again, a new one is removed. Returns the paths
    /// that differed, and any that could not be put back.
    fn restore(&self) -> (Vec<String>, Vec<String>) {
        let now = match self.read() {
            Ok(n) => n,
            Err(e) => return (vec![format!("the git directory ({e})")], vec![e]),
        };
        let (mut differed, mut failed) = (Vec::new(), Vec::new());
        for (p, bytes) in &self.files {
            if now.iter().find(|(q, _)| q == p).map(|(_, b)| b) != Some(bytes) {
                differed.push(self.label(p));
                if let Err(e) = std::fs::write(p, bytes) {
                    failed.push(format!("{}: {e}", self.label(p)));
                }
            }
        }
        for (p, _) in &now {
            if !self.files.iter().any(|(q, _)| q == p) {
                differed.push(self.label(p));
                if let Err(e) = std::fs::remove_file(p) {
                    failed.push(format!("{}: {e}", self.label(p)));
                }
            }
        }
        (differed, failed)
    }
}

/// What judges the run, read from the worktree **before** the command runs:
/// the `[checks]` commands, `[governance] deny_write`, and the rule
/// documents under `.trellis/governance/`. Read after, they were the
/// command's to choose: it could rewrite `typecheck` to `true` or drop a
/// `deny_write` line and be judged by its own rules (ADR 0026 §11).
struct Judge {
    checks: trellis_checks::CheckConfig,
    rules: Vec<trellis_checks::governance::Rule>,
    deny: Vec<String>,
}

impl Judge {
    fn load(root: &Path) -> Result<Judge, String> {
        let rules = trellis_checks::governance::load(root).map_err(|e| e.to_string())?;
        let cfg = crate::config::RepoConfig::load(root).map_err(|e| format!("{e:#}"))?;
        let mut deny: Vec<String> = cfg
            .governance()
            .rules()
            .iter()
            .map(|r| r.as_written())
            .collect();
        deny.extend(trellis_checks::governance::deny_write_paths(&rules));
        Ok(Judge {
            checks: cfg.checks(),
            rules,
            deny,
        })
    }

    /// Governance as a model run meets it: `deny_write` over every path the
    /// command changed, then the `forbid` audit over those files. The first
    /// is a list of breaches; the second is a `Verdict` from
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
        let denied = self.denied(root, touched.iter());
        let audit = (!self.rules.is_empty())
            .then(|| trellis_checks::governance_audit(root, &self.rules, touched));
        (denied, audit)
    }

    /// The `.trellis/` breach and `deny_write` over `paths`, whether git
    /// shows them or ignores them: an ignored path is still a write, and a
    /// rule that says "not here" does not ask git first.
    fn denied<'p>(&self, root: &Path, paths: impl Iterator<Item = &'p String>) -> Vec<String> {
        let g = trellis_loop::Governance::new(&self.deny);
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
}

pub(crate) fn run(p: ProcessRun<'_>, out: &mut dyn Write) -> RunEnd {
    let ProcessRun {
        root,
        profile,
        task_id,
        text,
        stop,
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
    let judge = match Judge::load(root) {
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
    // Last before the spawn, so what differs afterwards is the command's.
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
        .current_dir(root)
        .stdin(Stdio::piped())
        .env("TRELLIS_TASK", text)
        .env("TRELLIS_TASK_ID", task_id)
        .env("TRELLIS_AGENT", &profile.name)
        // As for hooks and tools (`exec::run`): bytecode it wrote under
        // `.trellis/` would untrust the repository it was trusted in.
        .env("PYTHONDONTWRITEBYTECODE", "1");
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
    let child = cmd.spawn();
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
    if let Some(mut stdin) = child.stdin.take() {
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
            // `kill` reaches the direct child only; a grandchild it started
            // can outlive it (ADR 0026 §11, not solved here).
            let _ = child.kill();
            let _ = child.wait();
            break Ended::Interrupted;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break Ended::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
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

    let touched = match changed(root) {
        Ok(t) => t,
        Err(e) => {
            let restored = undo(root, &snapshot, &ignored, out);
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
    // Ignored writes are judged whether or not anything git shows changed.
    let mut denied = ignored_denied;
    let mut checked = false;
    let judge_touched = touched
        .iter()
        .chain(ignored.all())
        .any(|p| trellis_loop::names_trellis_dir(p));
    if matches!(ended, Ended::Exited(_)) && !touched.is_empty() {
        let (d, audit) = judge.governance(root, &touched);
        denied.extend(d);
        verdicts.extend(audit);
    }
    for d in &denied {
        let _ = writeln!(out, "governance: refused: {d}");
    }
    if matches!(ended, Ended::Exited(_)) && !touched.is_empty() {
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
    // nothing under `.git/` changed.
    let converged = checked
        && verdicts.iter().all(|v| v.passed())
        && denied.is_empty()
        && git_changed.is_empty()
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
            _ if ignored_unread.is_some() => {
                "not converged: what the command did to ignored files could not be read"
            }
            _ if touched.is_empty() => {
                "not converged: the command changed nothing, so there was nothing to verify"
            }
            _ if judge_touched => {
                "not converged: the command changed .trellis/, which holds the checks and \
                 rules that judge it"
            }
            _ if !denied.is_empty() => {
                "not converged: the command wrote where [governance] deny_write refuses"
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
        undo(root, &snapshot, &ignored, out)
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

/// The rollback of a run that did not converge: the ignored files the
/// command created are removed first (`git clean -fd` leaves ignored files,
/// and then removes the directories their removal emptied), then the
/// worktree goes back to the snapshot. An ignored file that existed before
/// and was changed or deleted cannot be put back, because trellis keeps no
/// copy of ignored files; it is named. `false` when a created file could not
/// be removed or the worktree is not restored.
fn undo(
    root: &Path,
    snapshot: &trellis_vcs::Snapshot,
    ignored: &IgnoredChanges,
    out: &mut dyn Write,
) -> bool {
    let removed = trellis_vcs::ignored::undo_created(root, ignored, out);
    rollback(snapshot, out) && removed
}

fn rollback(snapshot: &trellis_vcs::Snapshot, out: &mut dyn Write) -> bool {
    if let Err(e) = snapshot.rollback() {
        let _ = writeln!(out, "ROLLBACK FAILED: {e}");
    }
    match snapshot.is_restored() {
        Ok(true) => {
            let _ = writeln!(out, "rolled back to {}", snapshot.head);
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
        actions: std::iter::once(Action {
            turn: 1,
            tool: "process".into(),
            target: t.argv.join(" "),
            outcome: t.exit_text.to_string(),
            wrote: !t.touched.is_empty(),
        })
        // What it did to files git ignores: not `proposed`, which is what
        // a commit would carry, but recorded, since a converged run keeps
        // what it created there (ADR 0026 §11).
        .chain((!t.ignored.is_empty()).then(|| Action {
            turn: 1,
            tool: "process".into(),
            target: "files git ignores".into(),
            outcome: t.ignored.summary(),
            wrote: true,
        }))
        .collect(),
        control: Some("auto (process agent)".into()),
        // An external process's tokens are not visible to trellis.
        tokens: None,
        agent: Some(t.profile.to_string()),
        // A process agent is given its task on stdin and nothing else.
        inputs: Vec::new(),
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
