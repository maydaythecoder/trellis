//! The integrated check: what decides whether a plan's merged union
//! converged (ADR 0005, ADR 0042).
//!
//! Optimistic partitioning, pessimistic integration. Each partition's own
//! checks are a filter; they never decide the run (rule 6, S19 patch 2).
//! After the partitions merge, the root is type-checked and tested in full,
//! through `trellis-checks`' own entry points, the same run a single agent's
//! convergence reads. Only those verdicts can converge the run; a partition's
//! outcome can only stop it. Nothing here reads a partition's output, so no
//! agent's work is judged by another's (rule 5).
//!
//! When the union does not converge, the root goes back to the snapshot taken
//! before the plan ran: every partition's work together, never some of it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Display;
use std::future::Future;
use std::path::{Path, PathBuf};

use trellis_checks::{CheckConfig, CheckKind, Verdict};
use trellis_core::PartitionId;

use crate::partition::{FailurePolicy, Plan};
use crate::supervise::{PartitionReport, PartitionTask, SuperviseError, Supervised, Supervisor};

/// A failing integrated check whose own output named merged files from two
/// or more partitions: a collision the plan said could not happen, so an
/// edge the graph's recall missed (ADR 0005).
///
/// Suspected, not established. It says which merged files the failure's
/// output named, not that their pairing caused it. Built only from a
/// `Verdict`'s recorded stdout and stderr, never from model text; the trail
/// records it as `Factual::suspected_recall_gaps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspectedRecallGap {
    /// Ascending.
    pub partitions: Vec<PartitionId>,
    /// Root-relative, in the order they were merged.
    pub files: Vec<String>,
    pub check: CheckKind,
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    /// Every integrated check passed on the merged union. The root holds the
    /// union, uncommitted, for review.
    Converged,
    /// It did not converge, for the reason given, and the root was rolled
    /// back to where it was before the plan ran.
    RolledBack(String),
}

/// A plan's run and the integrated check over what it merged.
#[must_use = "a run that did not converge has already been rolled back; read `integration`"]
#[derive(Debug)]
pub struct Integrated<T> {
    pub reports: Vec<PartitionReport<T>>,
    /// Root-relative paths the merge wrote or removed. After a rollback,
    /// what was merged and then undone.
    pub merged: Vec<String>,
    /// The integrated type check, then the integrated tests, run on the root.
    pub checks: Vec<Verdict>,
    pub suspected_recall_gaps: Vec<SuspectedRecallGap>,
    pub integration: Integration,
}

impl<T> Integrated<T> {
    pub fn converged(&self) -> bool {
        self.integration == Integration::Converged
    }
}

impl Supervisor<'_> {
    /// Runs `plan` (see `run_plan`), then the integrated type check and tests
    /// over the whole root, whatever the partitions reported.
    ///
    /// The root must be clean (rule 7): it is snapshotted first, and a run
    /// that does not converge is rolled back to that snapshot, which takes
    /// every partition's merged work out together. A merge that fails part-way
    /// is rolled back the same way before its error is returned.
    ///
    /// Converges only when both integrated verdicts passed, `HEAD` did not
    /// move, and the merge landed what the policy requires: every partition
    /// under [`FailurePolicy::AllOrNothing`], at least one under
    /// [`FailurePolicy::LandClean`]. The last is a condition, never a
    /// substitute: an all-or-nothing merge that failed wrote nothing, and the
    /// untouched root it leaves would pass its checks.
    pub async fn run_integrated<T, E, F, Fut>(
        &self,
        plan: &Plan,
        checks: &CheckConfig,
        work: F,
    ) -> Result<Integrated<T>, SuperviseError>
    where
        F: Fn(PartitionTask) -> Fut,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Display + Send + 'static,
    {
        let armed = RollbackOnDrop(Some(trellis_vcs::snapshot(self.root)?));
        let Supervised {
            reports,
            merged,
            owner,
        } = match self.run_plan(plan, work).await {
            Ok(s) => s,
            Err(e) => {
                armed.rollback()?;
                return Err(e);
            }
        };

        // Both, always, and the tests even after a failed type check: the
        // second verdict is more output to attribute a collision from, and
        // the run is already not converging, so it cannot mislead.
        let paths = [PathBuf::from(".")];
        let verdicts = vec![
            trellis_checks::typecheck(self.root, checks, &paths).await,
            trellis_checks::run_tests(self.root, checks, &paths).await,
        ];
        let gaps: Vec<SuspectedRecallGap> = verdicts
            .iter()
            .filter(|v| !v.passed())
            .filter_map(|v| suspected_gap(self.root, v, &merged, &owner))
            .collect();
        for g in &gaps {
            tracing::warn!(
                check = ?g.check,
                partitions = ?g.partitions,
                files = ?g.files,
                "an integrated check failure named files from more than one partition"
            );
        }

        let why = not_converged(self.policy, &reports, &verdicts).or_else(|| {
            // Fail closed: a HEAD that could not be read is not one known
            // to be where it was.
            match armed.0.as_ref().map(trellis_vcs::Snapshot::head_moved) {
                Some(Ok(moved)) => moved,
                Some(Err(e)) => Some(format!("HEAD could not be read: {e}")),
                None => Some("the run's snapshot is gone".to_string()),
            }
        });
        let integration = match why {
            None => {
                armed.disarm();
                Integration::Converged
            }
            Some(why) => {
                armed.rollback()?;
                Integration::RolledBack(why)
            }
        };

        Ok(Integrated {
            reports,
            merged,
            checks: verdicts,
            suspected_recall_gaps: gaps,
            integration,
        })
    }
}

/// The run's snapshot, rolled back when dropped still armed.
///
/// Between the merge and the verdict the root holds a union nobody has
/// checked. A panic there, or the caller dropping the future (an interrupt,
/// a timeout), would otherwise leave it in place, where it reads as the
/// run's result. `Drop` logs a failed rollback rather than panicking: it may
/// run during an unwind, and a second panic aborts.
struct RollbackOnDrop(Option<trellis_vcs::Snapshot>);

impl RollbackOnDrop {
    /// The run converged: the root keeps the union.
    fn disarm(mut self) {
        self.0 = None;
    }

    /// Rolls back now, reporting a failure to the caller.
    fn rollback(mut self) -> Result<(), trellis_vcs::VcsError> {
        match self.0.take() {
            Some(s) => s.rollback(),
            None => Ok(()),
        }
    }
}

impl Drop for RollbackOnDrop {
    fn drop(&mut self) {
        if let Some(s) = self.0.take()
            && let Err(e) = s.rollback()
        {
            tracing::error!(error = %e, "the root was not rolled back after an interrupted plan");
        }
    }
}

/// Why the run may not converge, or `None` when nothing stops it.
///
/// A partition's outcome appears here only as a reason not to converge.
/// There is no branch in which a partition's pass stands in for a verdict.
fn not_converged<T>(
    policy: FailurePolicy,
    reports: &[PartitionReport<T>],
    verdicts: &[Verdict],
) -> Option<String> {
    match policy {
        FailurePolicy::AllOrNothing => {
            if let Some((id, e)) = reports
                .iter()
                .find_map(|r| r.outcome.as_ref().err().map(|e| (r.id, e)))
            {
                return Some(format!("partition {} {e}", id.0));
            }
        }
        FailurePolicy::LandClean => {
            if !reports.iter().any(|r| r.outcome.is_ok()) {
                return Some("no partition landed".to_string());
            }
        }
    }
    if verdicts.is_empty() {
        return Some("no integrated check ran".to_string());
    }
    verdicts
        .iter()
        .find(|v| !v.passed())
        .map(|v| format!("the integrated {:?} check failed", v.kind()))
}

/// The gap `v`'s failure suggests, when its output names merged files from
/// two or more partitions.
fn suspected_gap(
    root: &Path,
    v: &Verdict,
    merged: &[String],
    owner: &HashMap<String, PartitionId>,
) -> Option<SuspectedRecallGap> {
    let d = v.detail();
    let mut named = named_paths(&d.stdout, root, owner);
    named.extend(named_paths(&d.stderr, root, owner));
    let files: Vec<String> = merged
        .iter()
        .filter(|p| named.contains(p.as_str()))
        .cloned()
        .collect();
    let partitions: BTreeSet<PartitionId> =
        files.iter().filter_map(|p| owner.get(p).copied()).collect();
    (partitions.len() >= 2).then(|| SuspectedRecallGap {
        partitions: partitions.into_iter().collect(),
        files,
        check: v.kind(),
    })
}

/// The merged paths `text` names as whole paths: as written, with `\`
/// separators (a checker on Windows), after `./`, or absolute under `root`.
/// `a.py` is not named by `data.py`, `pkg/a.py`, `../a.py` or `a.py.bak`.
///
/// One pass over the text, split into path-shaped tokens and looked up,
/// rather than a search per merged path: the output is the repository's
/// checker's, up to `CHECK_OUTPUT_CAP` a stream, and the merged set is as
/// large as the partitions made it. A path holding a separator character
/// (a space, a colon) is not found; the record is a suspicion, and missing
/// one is the direction it may err in.
fn named_paths<'a>(
    text: &str,
    root: &Path,
    merged: &'a HashMap<String, PartitionId>,
) -> HashSet<&'a str> {
    // An absolute path's root may hold spaces, so it goes before the split.
    let root = root.display().to_string();
    let mut text = text.to_string();
    for form in [root.replace('\\', "/"), root.replace('/', "\\")] {
        if !form.is_empty() {
            for sep in ["/", "\\"] {
                text = text.replace(&format!("{form}{sep}"), " ");
            }
        }
    }
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '`'
                    | ':'
                    | ','
                    | ';'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '='
                    | '|'
            )
    })
    .filter_map(|token| {
        // Sentence punctuation after a path is not part of it.
        let token = token.trim_end_matches(['.', '-']);
        let token = token
            .strip_prefix("./")
            .or_else(|| token.strip_prefix(".\\"))
            .unwrap_or(token);
        let key = token.replace('\\', "/");
        merged.get_key_value(key.as_str()).map(|(k, _)| k.as_str())
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::partition::Partition;
    use crate::supervise::PartitionFailure;
    use std::collections::HashSet;
    use std::num::NonZeroUsize;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = trellis_vcs::command()
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must be on PATH to run these tests");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    const A: &str = "def a():\n    return 1\n";
    const B: &str = "def b():\n    return 1\n";

    /// A committed repository with `a.py` and `b.py`, and its HEAD.
    fn repo() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            &["config", "core.autocrlf", "false"],
            &["config", "user.email", "t@example.invalid"],
            &["config", "user.name", "t"],
        ] {
            git(d.path(), args);
        }
        std::fs::write(d.path().join("a.py"), A).unwrap();
        std::fs::write(d.path().join("b.py"), B).unwrap();
        git(d.path(), &["add", "-A"]);
        git(d.path(), &["commit", "-qm", "base"]);
        let base = git(d.path(), &["rev-parse", "HEAD"]).trim().to_string();
        (d, base)
    }

    /// Two parallel partitions: 0 owns `a.py`, 1 owns `b.py`, as the graph
    /// said, which is what makes a failure naming both a surprise.
    fn plan() -> Plan {
        let p = |id| Partition {
            id: PartitionId(id),
            units: Vec::new(),
            radius: HashSet::new(),
            files: HashSet::new(),
            conservative: false,
        };
        Plan {
            shared: None,
            parallel: vec![p(0), p(1)],
        }
    }

    fn budget() -> Budget {
        Budget {
            max_iterations: 40,
            max_tokens: 1_000,
            ..Budget::default()
        }
    }

    fn supervisor<'a>(
        root: &'a Path,
        base: &'a str,
        budget: &'a Budget,
        policy: FailurePolicy,
    ) -> Supervisor<'a> {
        Supervisor {
            root,
            run: "run",
            base,
            budget,
            width: NonZeroUsize::new(2).unwrap(),
            policy,
        }
    }

    /// The platform shell running `script`. The checks append `.`, which
    /// `sh -c` takes as `$0` and `cmd /C` hands to the last command.
    fn shell(sh: &str, cmd: &[&str]) -> Vec<String> {
        if cfg!(windows) {
            ["cmd", "/C"]
                .iter()
                .chain(cmd)
                .map(|s| s.to_string())
                .collect()
        } else {
            vec!["sh".into(), "-c".into(), sh.into()]
        }
    }

    /// A type check that fails only when `a.py` holds `A_NEW` and `b.py`
    /// holds `B_NEW`, printing an error line for each file in `blame`. Each
    /// edit alone passes it, so each partition's own run of it passes: the
    /// collision exists only in the union. A real executed check, as the
    /// grounding contract requires of anything a convergence test reads.
    fn collision_checker(blame: &[&str]) -> CheckConfig {
        let sh_echo: String = blame
            .iter()
            .map(|f| format!("echo '{f}:1: error: incompatible'; "))
            .collect();
        let sh =
            format!("grep -q A_NEW a.py && grep -q B_NEW b.py && {{ {sh_echo}exit 1; }}; exit 0");
        let mut cmd: Vec<String> = [
            "findstr", "/C:A_NEW", "a.py", ">NUL", "&&", "findstr", "/C:B_NEW", "b.py", ">NUL",
            "&&", "(",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for f in blame {
            cmd.extend(["echo", &format!("{f}:1:"), "error", "&"].map(String::from));
        }
        cmd.extend(["exit", "1", ")", "&", "exit", "0"].map(String::from));
        let cmd: Vec<&str> = cmd.iter().map(String::as_str).collect();
        CheckConfig {
            typecheck: shell(&sh, &cmd),
            tests: shell("exit 0", &["exit", "0"]),
            ..CheckConfig::default()
        }
    }

    /// Runs the plan with each partition making its edit and then running
    /// `checks` in its own worktree: `Ok` only when that local check passed.
    async fn run(
        dir: &Path,
        base: &str,
        policy: FailurePolicy,
        checks: &CheckConfig,
    ) -> Integrated<()> {
        let b = budget();
        let local = checks.clone();
        supervisor(dir, base, &b, policy)
            .run_integrated(&plan(), checks, move |task| {
                let local = local.clone();
                async move {
                    let (file, body) = match task.id.0 {
                        0 => ("a.py", "def a():\n    return 'A_NEW'\n"),
                        _ => ("b.py", "def b():\n    return 'B_NEW'\n"),
                    };
                    std::fs::write(task.root.join(file), body).map_err(|e| e.to_string())?;
                    let v =
                        trellis_checks::typecheck(&task.root, &local, &[PathBuf::from(".")]).await;
                    if v.passed() {
                        Ok(())
                    } else {
                        Err(format!("local check failed: {}", v.detail().stdout))
                    }
                }
            })
            .await
            .unwrap()
    }

    fn at_base(dir: &Path) -> bool {
        std::fs::read_to_string(dir.join("a.py")).unwrap() == A
            && std::fs::read_to_string(dir.join("b.py")).unwrap() == B
            && git(dir, &["status", "--porcelain"]).trim().is_empty()
    }

    #[tokio::test]
    async fn integrated_check_partition_local_passes_do_not_converge_the_run() {
        // Rule 6, S19 patch 2. The risk: every partition passed its own
        // check, and a supervisor that reads that as the run passing lands a
        // union nobody checked. Here each partition's local verdict is a real
        // pass, and the union fails; the run must not converge.
        let (dir, base) = repo();
        let out = run(
            dir.path(),
            &base,
            FailurePolicy::AllOrNothing,
            &collision_checker(&["a.py", "b.py"]),
        )
        .await;

        assert!(
            out.reports.iter().all(|r| r.outcome.is_ok()),
            "the premise: every partition passed locally: {:?}",
            out.reports
        );
        assert_eq!(out.merged, ["a.py", "b.py"], "both landed in the root");
        assert!(!out.converged(), "{:?}", out.integration);
        assert!(
            matches!(&out.integration, Integration::RolledBack(why) if why.contains("Typecheck")),
            "{:?}",
            out.integration
        );
        assert_eq!(out.checks.len(), 2, "the type check and the tests both ran");
        assert!(!out.checks[0].passed());
        assert_eq!(out.checks[0].kind(), CheckKind::Typecheck);
        assert_eq!(out.checks[1].kind(), CheckKind::Tests);
    }

    #[tokio::test]
    async fn rule5_a_partitions_own_passing_verdict_is_not_the_runs_verdict() {
        // Rule 5 (ADR 0005, ADR 0042 §3). The risk: a partition hands back a
        // real, sealed, passing `Verdict` from its own worktree, and the
        // supervisor accepts it as the run's pass, so one agent's check
        // stands in for a judgment on work it never saw. `run_integrated`
        // takes the partition's result as an opaque `T`, so it cannot read
        // that verdict by signature; this defends the behavioural half: a
        // plan whose every partition carries a pass still converges only on
        // the integrated verdicts, and here those fail.
        let (dir, base) = repo();
        let checks = collision_checker(&["a.py", "b.py"]);
        let local = checks.clone();
        let b = budget();
        let out = supervisor(dir.path(), &base, &b, FailurePolicy::AllOrNothing)
            .run_integrated(&plan(), &checks, move |task| {
                let local = local.clone();
                async move {
                    let (file, body) = match task.id.0 {
                        0 => ("a.py", "def a():\n    return 'A_NEW'\n"),
                        _ => ("b.py", "def b():\n    return 'B_NEW'\n"),
                    };
                    std::fs::write(task.root.join(file), body).map_err(|e| e.to_string())?;
                    // The only way to hold a `Verdict` outside
                    // `trellis-checks` is to run a check (rule 1).
                    let v =
                        trellis_checks::typecheck(&task.root, &local, &[PathBuf::from(".")]).await;
                    Ok::<_, String>((task.root.clone(), v))
                }
            })
            .await
            .unwrap();

        // The premise: each partition carries its own genuine pass.
        let carried: Vec<&(PathBuf, Verdict)> = out
            .reports
            .iter()
            .map(|r| r.outcome.as_ref().expect("every partition landed"))
            .collect();
        assert_eq!(carried.len(), 2);
        for (_, v) in &carried {
            assert!(v.passed(), "a partition's local check passed: {v:?}");
            assert_eq!(v.kind(), CheckKind::Typecheck);
        }
        // No partition could judge another's output: each checked a
        // worktree of its own, neither of them the root the union lands in.
        assert_ne!(carried[0].0, carried[1].0);
        for (root, _) in &carried {
            assert_ne!(root.as_path(), dir.path());
        }

        // The run is decided by the integrated verdicts alone.
        assert_eq!(out.merged, ["a.py", "b.py"]);
        assert!(!out.converged(), "{:?}", out.integration);
        assert!(
            matches!(&out.integration, Integration::RolledBack(why) if why.contains("Typecheck")),
            "{:?}",
            out.integration
        );
        assert_eq!(out.checks.len(), 2);
        assert_eq!(out.checks[0].kind(), CheckKind::Typecheck);
        assert!(
            !out.checks[0].passed(),
            "the integrated type check ran on the union and failed"
        );
        assert!(at_base(dir.path()));
    }

    #[tokio::test]
    async fn integrated_check_cross_partition_failure_records_a_suspected_recall_gap() {
        // A failure naming files that two partitions own separately is a
        // collision the plan said could not happen: the graph missed an
        // edge. It is recorded from the check's own output, with the
        // partitions and files it named.
        let (dir, base) = repo();
        let out = run(
            dir.path(),
            &base,
            FailurePolicy::AllOrNothing,
            &collision_checker(&["a.py", "b.py"]),
        )
        .await;

        assert!(!out.converged());
        assert_eq!(
            out.suspected_recall_gaps,
            [SuspectedRecallGap {
                partitions: vec![PartitionId(0), PartitionId(1)],
                files: vec!["a.py".into(), "b.py".into()],
                check: CheckKind::Typecheck,
            }]
        );

        // A failure that names one partition's file only is not evidence of
        // a collision, and is not recorded as one.
        let (dir, base) = repo();
        let out = run(
            dir.path(),
            &base,
            FailurePolicy::AllOrNothing,
            &collision_checker(&["a.py"]),
        )
        .await;
        assert!(!out.converged());
        assert!(
            out.suspected_recall_gaps.is_empty(),
            "{:?}",
            out.suspected_recall_gaps
        );
    }

    #[tokio::test]
    async fn integrated_check_failure_rolls_back_every_partition_together() {
        // ADR 0005: a failed union never lands partially. Both partitions'
        // edits were merged into the root before the check ran; both go.
        let (dir, base) = repo();
        let out = run(
            dir.path(),
            &base,
            FailurePolicy::AllOrNothing,
            &collision_checker(&["a.py", "b.py"]),
        )
        .await;

        assert!(!out.converged());
        assert_eq!(out.merged, ["a.py", "b.py"]);
        assert!(
            at_base(dir.path()),
            "the root still holds partition work: {}",
            git(dir.path(), &["status", "--porcelain"])
        );
        assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), base);
        assert!(!dir.path().join(".trellis/work/run").exists());
    }

    #[tokio::test]
    async fn integrated_check_passing_on_the_union_converges_and_keeps_it() {
        // The contrast the three above need: the same plan, a union that
        // passes, and the root keeps both partitions' work, uncommitted.
        let (dir, base) = repo();
        let pass = CheckConfig {
            typecheck: shell("exit 0", &["exit", "0"]),
            tests: shell("exit 0", &["exit", "0"]),
            ..CheckConfig::default()
        };
        let out = run(dir.path(), &base, FailurePolicy::AllOrNothing, &pass).await;

        assert!(out.converged(), "{:?}", out.integration);
        assert!(out.checks.iter().all(Verdict::passed));
        assert!(out.suspected_recall_gaps.is_empty());
        assert!(
            std::fs::read_to_string(dir.path().join("a.py"))
                .unwrap()
                .contains("A_NEW")
        );
        assert!(
            std::fs::read_to_string(dir.path().join("b.py"))
                .unwrap()
                .contains("B_NEW")
        );
        assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), base);
    }

    #[tokio::test]
    async fn integrated_check_passing_on_an_untouched_root_does_not_converge_a_failed_plan() {
        // The trap in "converged when the checks pass": an all-or-nothing
        // merge with a failed partition writes nothing, and the untouched
        // root passes. That pass says nothing about the work, which did not
        // land.
        let (dir, base) = repo();
        let b = budget();
        let pass = CheckConfig {
            typecheck: shell("exit 0", &["exit", "0"]),
            tests: shell("exit 0", &["exit", "0"]),
            ..CheckConfig::default()
        };
        let out = supervisor(dir.path(), &base, &b, FailurePolicy::AllOrNothing)
            .run_integrated(&plan(), &pass, |task| async move {
                if task.id.0 == 1 {
                    Err("ran out of iterations")
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap();

        assert!(out.checks.iter().all(Verdict::passed), "the premise");
        assert!(out.merged.is_empty());
        assert!(matches!(
            out.reports[1].outcome,
            Err(PartitionFailure::NotConverged(_))
        ));
        assert!(
            matches!(&out.integration, Integration::RolledBack(why) if why.contains("partition 1")),
            "{:?}",
            out.integration
        );
    }

    #[tokio::test]
    async fn integrated_check_interrupted_after_the_merge_rolls_every_partition_back() {
        // The risk: the run is dropped (an interrupt, a timeout) while the
        // integrated check is still running, and the unchecked union stays
        // in the root, where it reads as the run's result.
        let (dir, base) = repo();
        let b = budget();
        let slow = CheckConfig {
            typecheck: shell(
                "sleep 20",
                &["ping", "-n", "21", "127.0.0.1", ">NUL", "&", "exit", "1"],
            ),
            tests: shell("exit 0", &["exit", "0"]),
            ..CheckConfig::default()
        };
        let sup = supervisor(dir.path(), &base, &b, FailurePolicy::AllOrNothing);
        let p = plan();
        let fut = sup.run_integrated(&p, &slow, |task| async move {
            let file = if task.id.0 == 0 { "a.py" } else { "b.py" };
            std::fs::write(task.root.join(file), "A_NEW B_NEW\n").map_err(|e| e.to_string())
        });
        // Boxed, not `pin!`ed: dropping a `Pin<&mut>` would leave the future,
        // and its armed snapshot, alive to the end of the scope.
        let mut fut = Box::pin(fut);
        let merged = |f: &str| {
            std::fs::read_to_string(dir.path().join(f)).is_ok_and(|s| s.contains("A_NEW"))
        };
        // No wall-clock deadline: the future is the deadline. `run_plan` has
        // no await between writing the merge and the type check's first poll,
        // so once both files show the merge, the future is parked inside the
        // 20 s check, and it cannot finish before the merge is visible. A
        // fixed wait here measured host load (#225): under parallel test
        // load the partitions alone outlasted 15 s.
        loop {
            tokio::select! {
                out = &mut fut => panic!("the run ended before it was interrupted: {out:?}"),
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
            }
            if merged("a.py") && merged("b.py") {
                break;
            }
        }
        drop(fut);

        assert!(
            at_base(dir.path()),
            "the root still holds partition work: {}",
            git(dir.path(), &["status", "--porcelain"])
        );
    }

    #[tokio::test]
    async fn integrated_check_refuses_a_dirty_root() {
        // Rule 7: the rollback restores the snapshot, and a snapshot of a
        // dirty tree would restore the user's uncommitted work away.
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a.py"), "uncommitted\n").unwrap();
        let b = budget();
        let err = supervisor(dir.path(), &base, &b, FailurePolicy::AllOrNothing)
            .run_integrated(&plan(), &CheckConfig::default(), |_| async {
                Ok::<(), String>(())
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SuperviseError::Vcs(_)), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "uncommitted\n"
        );
    }

    /// Whether `text` names `rel`, merged alone.
    fn names(text: &str, root: &Path, rel: &str) -> bool {
        let owner = HashMap::from([(rel.to_string(), PartitionId(0))]);
        named_paths(text, root, &owner).contains(rel)
    }

    #[test]
    fn a_path_is_named_only_as_a_whole_path() {
        // A false match would record a recall gap between partitions whose
        // files the failure never named.
        let root = Path::new("/r");
        assert!(names("a.py:3: error: x", root, "a.py"));
        assert!(names("./a.py:3: error", root, "a.py"));
        assert!(names("FAILED pkg\\new.py::test_x", root, "pkg/new.py"));
        assert!(names("see a.py.", root, "a.py"));
        assert!(names("File \"/r/a.py\", line 3", root, "a.py"));
        assert!(names("E   /r/pkg/new.py:3", root, "pkg/new.py"));
        assert!(!names("data.py:3: error", root, "a.py"));
        assert!(!names("pkg/a.py:3: error", root, "a.py"));
        assert!(!names("../a.py:3: error", root, "a.py"));
        assert!(!names("a.py.bak", root, "a.py"));
        assert!(!names("a.pyi:1: error", root, "a.py"));
    }
}
