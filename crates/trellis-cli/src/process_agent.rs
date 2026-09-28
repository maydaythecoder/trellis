//! Process agents: Paperclip's `process` adapter, with trellis's checks
//! as the only judge (ADR 0026 §11).
//!
//! A profile with `command = [argv]` runs its tasks by running that command
//! in the worktree, then running the same two grounded checks `trellis
//! check` runs. The run converged if and only if the command changed
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

/// What the command did before the checks ran.
enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Interrupted,
}

/// Paths `git status --porcelain` names: the files the command touched.
fn changed(root: &Path) -> Result<Vec<String>, String> {
    trellis_vcs::pending(root)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|l| l.get(3..))
                .map(|p| p.trim().trim_matches('"').to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .map_err(|e| e.to_string())
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

    let started = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(root)
        .stdin(Stdio::piped())
        .env("TRELLIS_TASK", text)
        .env("TRELLIS_TASK_ID", task_id)
        .env("TRELLIS_AGENT", &profile.name);
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

    let touched = match changed(root) {
        Ok(t) => t,
        Err(e) => {
            let restored = rollback(&snapshot, out);
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
    if matches!(ended, Ended::Exited(_)) && !touched.is_empty() {
        let checks = match crate::config::RepoConfig::load(root) {
            Ok(c) => c.checks(),
            Err(e) => {
                let restored = rollback(&snapshot, out);
                return if restored {
                    RunEnd::Error {
                        run_id: None,
                        error: format!("{e:#}"),
                    }
                } else {
                    RunEnd::NotRestored { run_id: None }
                };
            }
        };
        let paths = vec![PathBuf::from(".")];
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                verdicts = rt.block_on(async {
                    vec![
                        trellis_checks::typecheck(root, &checks, &paths).await,
                        trellis_checks::run_tests(root, &checks, &paths).await,
                    ]
                });
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
        }
    }
    let converged = verdicts.len() == 2 && verdicts.iter().all(|v| v.passed());
    let why = if converged {
        "converged: both checks passed"
    } else {
        match &ended {
            Ended::TimedOut => "not converged: timed out",
            Ended::Interrupted => "interrupted",
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
        rollback(&snapshot, out)
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
        actions: vec![Action {
            turn: 1,
            tool: "process".into(),
            target: t.argv.join(" "),
            outcome: t.exit_text.to_string(),
            wrote: !t.touched.is_empty(),
        }],
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
