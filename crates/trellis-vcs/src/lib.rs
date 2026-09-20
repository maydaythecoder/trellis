//! Git snapshot and rollback. Deliberately boring.
//!
//! Before a run: require a clean worktree, record `HEAD`. During: the
//! orchestrator is the only writer. On any failure or budget exhaustion:
//! reset hard to the recorded commit and remove untracked files the run
//! created.
//!
//! Requiring a clean worktree is a real constraint on the user and the
//! right trade. The alternative — snapshotting a dirty tree and restoring
//! it — is a source of data-loss bugs, and this is the one path that has to
//! be reliable. See `docs/adr/0009-clean-worktree-requirement.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    #[error(
        "worktree is not clean. TRELLIS requires a clean worktree so rollback is exact.\n\
         Uncommitted changes:\n{0}"
    )]
    DirtyWorktree(String),

    #[error("git command failed: {command}\n{stderr}")]
    GitFailed { command: String, stderr: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The recovery path itself failed.
    ///
    /// Disk full, a permission denied, an `index.lock` held by another
    /// process. This is the one failure that must never be quiet: every other
    /// error leaves the repository as the user last saw it, and this one
    /// leaves it mid-run. The message carries the commands to finish the job
    /// by hand, because a user who has just been told their worktree is in an
    /// unknown state should not also have to work out the SHA.
    //
    // The text is built in `rollback_failure_message` rather than written
    // into this attribute. A multi-line literal here picks up the source
    // indentation of every continuation line, which is the `cargo fmt`
    // string-folding defect this repository has now hit six times — and the
    // last place it should appear is the one message a user reads while
    // being told their worktree is in an unknown state.
    #[error("{}", rollback_failure_message(.root, .head, .detail))]
    RollbackFailed {
        root: String,
        head: String,
        detail: String,
    },
}

/// What to print when the recovery path itself fails.
///
/// Every other error in TRELLIS leaves the repository as the user last saw
/// it. This one leaves it mid-run, so it names the worktree, gives the exact
/// commands rather than describing them, and says what to suspect when those
/// also fail.
fn rollback_failure_message(root: &str, head: &str, detail: &str) -> String {
    format!(
        concat!(
            "ROLLBACK FAILED — the worktree is NOT restored and still holds ",
            "this run's edits.\n\n  {detail}\n\nRecover by hand:\n\n",
            "  git -C {root} reset --hard {head}\n",
            "  git -C {root} clean -fd\n\n",
            "If that also fails something outside TRELLIS is holding the ",
            "repository: a running process, a lock file at .git/index.lock, ",
            "or a read-only mount.\n",
            "`git -C {root} status` will say what is still there."
        ),
        detail = detail,
        root = root,
        head = head
    )
}

/// A recorded point to return to.
///
/// Holding one is the precondition for any write. The orchestrator takes a
/// `Snapshot` before it will process an `ApplyEdit`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub root: PathBuf,
    /// Commit SHA recorded before the run.
    pub head: String,
}

/// Shelling out to `git` rather than using `git2` for v0.1.
///
/// The brief blesses this as an acceptable v0.2 shortcut. It is chosen here
/// deliberately: `reset --hard` and `clean -fd` have exact, well-understood
/// semantics as commands, and the one path that must never lose a user's
/// work is a poor place to be learning a binding's edge cases. `git2` stays
/// in the workspace manifest for the worktree management that multi-agent
/// mode needs (4.13), where the library genuinely helps.
fn git(root: &Path, args: &[&str]) -> Result<String, VcsError> {
    let out = Command::new("git").args(args).current_dir(root).output()?;
    if !out.status.success() {
        return Err(VcsError::GitFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Commits exactly the given paths, and nothing else.
///
/// Used for the decision trail. The brief calls for the trail to be
/// *committed* — durable, in the repository, readable by the next session —
/// and leaving it untracked would fail that twice over: the worktree would
/// never be clean again, so no later run could start, and the next rollback's
/// `git clean` would delete every accumulated trail.
///
/// Deliberately narrow. It stages the named paths only, so a run never
/// commits the code changes it proposed: acceptance of those is the user's,
/// and a tool that commits work nobody approved has taken that decision away.
pub fn commit_paths(root: &Path, paths: &[&str], message: &str) -> Result<(), VcsError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut add: Vec<&str> = vec!["add", "--"];
    add.extend_from_slice(paths);
    git(root, &add)?;

    // `--only` so staged-but-unrelated changes cannot ride along.
    let mut commit: Vec<&str> = vec!["commit", "--only", "-m", message, "--"];
    commit.extend_from_slice(paths);
    git(root, &commit)?;
    Ok(())
}

/// Records `HEAD`, refusing if the worktree is dirty.
/// Uncommitted changes, as `git status --porcelain` lines.
///
/// Empty means clean. Exposed because the interactive session has to show a
/// user what a converged run left them, and answer the question that follows
/// — keep it or drop it — without making them leave for a second terminal.
pub fn pending(root: &Path) -> Result<Vec<String>, VcsError> {
    if !root.join(".git").exists() {
        return Err(VcsError::NotARepo(root.to_path_buf()));
    }
    Ok(git(root, &["status", "--porcelain"])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// The uncommitted diff, as git renders it.
///
/// `pending` answers *which files*; this answers *what changed*. A converged
/// run leaves its edits for the user to accept or reject, and a decision made
/// on a list of filenames is not a review — it is a guess with extra steps.
///
/// Includes staged changes (`HEAD` rather than the index) because a run's
/// edits may have been staged by a previous `/accept` that only took some of
/// them, and a diff that silently omitted those would understate what
/// accepting now would commit.
///
/// Untracked files do not appear: git cannot diff what it has never seen.
/// `pending` lists them, which is why the caller shows both.
pub fn diff(root: &Path) -> Result<String, VcsError> {
    if !root.join(".git").exists() {
        return Err(VcsError::NotARepo(root.to_path_buf()));
    }
    git(root, &["--no-pager", "diff", "HEAD"])
}

/// Discards every uncommitted change, tracked and untracked alike.
///
/// The same two steps as `Snapshot::rollback` and for the same reason: either
/// alone leaves residue. Unlike rollback this has no recorded commit to
/// return to — it drops whatever is pending against current `HEAD`, which is
/// what "I do not want what that run proposed" means.
pub fn discard_pending(root: &Path) -> Result<(), VcsError> {
    git(root, &["reset", "--hard"])?;
    git(root, &["clean", "-fd"])?;
    Ok(())
}

pub fn snapshot(root: &Path) -> Result<Snapshot, VcsError> {
    if !root.join(".git").exists() {
        return Err(VcsError::NotARepo(root.to_path_buf()));
    }

    // `--porcelain` covers staged, unstaged, and untracked in one call, and
    // its format is stable across git versions by contract.
    let status = git(root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(VcsError::DirtyWorktree(status));
    }

    let head = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(Snapshot {
        root: root.to_path_buf(),
        head,
    })
}

impl Snapshot {
    /// Returns the repository to its exact pre-run state.
    ///
    /// Two steps, both needed: `reset --hard` restores tracked files,
    /// `clean -fd` removes files the run created. Either alone leaves
    /// residue, and residue is what turns "failure is boring" into "failure
    /// is a mystery".
    ///
    /// v1.0 gate condition 4 verifies this by inducing a mid-run panic.
    /// Both steps are attempted even when the first fails. A `clean` that
    /// succeeds after a failed `reset` removes this run's new files, which is
    /// strictly better than leaving both kinds of residue; stopping at the
    /// first error would make the recovery path give up earlier than it
    /// needs to.
    #[tracing::instrument(skip(self), fields(head = %self.head))]
    pub fn rollback(&self) -> Result<(), VcsError> {
        let reset = git(&self.root, &["reset", "--hard", &self.head]);
        let clean = git(&self.root, &["clean", "-fd"]);
        match (reset, clean) {
            (Ok(_), Ok(_)) => Ok(()),
            (r, c) => {
                let mut detail = String::new();
                if let Err(e) = r {
                    detail.push_str(&format!("reset --hard: {e}"));
                }
                if let Err(e) = c {
                    if !detail.is_empty() {
                        detail.push_str("\n  ");
                    }
                    detail.push_str(&format!("clean -fd: {e}"));
                }
                Err(VcsError::RollbackFailed {
                    root: self.root.display().to_string(),
                    head: self.head.clone(),
                    detail,
                })
            }
        }
    }

    /// Whether the worktree currently matches the snapshot exactly.
    ///
    /// Used by the rollback test to assert the gate condition rather than
    /// assume it.
    pub fn is_restored(&self) -> Result<bool, VcsError> {
        let head = git(&self.root, &["rev-parse", "HEAD"])?.trim().to_string();
        let status = git(&self.root, &["status", "--porcelain"])?;
        Ok(head == self.head && status.trim().is_empty())
    }
}

/// A git worktree for one partition (4.13).
///
/// Filesystem isolation for free, and per-partition rollback becomes
/// deleting a directory.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    parent: PathBuf,
}

impl Worktree {
    /// Creates `.trellis/work/<run>/partition-<n>/` branched from `base`.
    pub fn create(root: &Path, run: &str, partition: u32, base: &str) -> Result<Self, VcsError> {
        let path = root
            .join(".trellis")
            .join("work")
            .join(run)
            .join(format!("partition-{partition}"));
        let branch = format!("trellis/{run}/p{partition}");

        git(
            root,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &path.display().to_string(),
                base,
            ],
        )?;

        Ok(Self {
            path,
            branch,
            parent: root.to_path_buf(),
        })
    }

    /// Removes the worktree and its branch.
    ///
    /// Called for a failed partition: its work never lands partially.
    pub fn discard(&self) -> Result<(), VcsError> {
        git(
            &self.parent,
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.display().to_string(),
            ],
        )?;
        git(&self.parent, &["branch", "-D", &self.branch])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trellis-vcs-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.invalid"]).unwrap();
        git(&dir, &["config", "user.name", "t"]).unwrap();
        // Windows git defaults to autocrlf=true, which rewrites line
        // endings on checkout and makes byte-exact assertions platform
        // dependent. Pin it so the fixture behaves identically everywhere.
        git(&dir, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "init"]).unwrap();
        dir
    }

    #[test]
    fn committing_the_trail_leaves_other_changes_alone() {
        // The property that matters: a run must never commit the code it
        // proposed. Acceptance of that is the user's.
        let dir = temp_repo("commit-narrow");
        std::fs::write(
            dir.join("a.py"),
            "def f():
    return 99
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".trellis/runs")).unwrap();
        std::fs::write(dir.join(".trellis/runs/t.json"), "{}").unwrap();

        commit_paths(&dir, &[".trellis/runs/t.json"], "trellis: trail").unwrap();

        let status = git(&dir, &["status", "--porcelain"]).unwrap();
        assert!(
            status.contains("a.py"),
            "the proposed change must still be uncommitted: {status}"
        );
        assert!(
            !status.contains("t.json"),
            "the trail must have been committed: {status}"
        );
    }

    #[test]
    fn committing_nothing_is_not_an_error() {
        let dir = temp_repo("commit-empty");
        assert!(commit_paths(&dir, &[], "empty").is_ok());
    }

    #[test]
    fn snapshot_refuses_a_dirty_worktree() {
        let dir = temp_repo("dirty");
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();

        let err = snapshot(&dir).unwrap_err();
        assert!(matches!(err, VcsError::DirtyWorktree(_)));
        assert!(
            err.to_string().contains("so rollback is exact"),
            "the refusal must say why, or it reads as an arbitrary obstacle"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_restores_tracked_edits_and_removes_new_files() {
        let dir = temp_repo("rollback");
        let snap = snapshot(&dir).unwrap();

        // Simulate a run: edit a tracked file and create an untracked one.
        std::fs::write(dir.join("a.py"), "def f():\n    return 999\n").unwrap();
        std::fs::write(dir.join("new_module.py"), "import sys\n").unwrap();
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("pkg").join("b.py"), "x = 1\n").unwrap();
        assert!(!snap.is_restored().unwrap());

        snap.rollback().unwrap();

        assert!(
            snap.is_restored().unwrap(),
            "v1.0 gate 4: the repo must return to its exact pre-run state"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        assert!(
            !dir.join("new_module.py").exists(),
            "untracked file left behind"
        );
        assert!(!dir.join("pkg").exists(), "untracked directory left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_is_idempotent() {
        let dir = temp_repo("idempotent");
        let snap = snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "broken(\n").unwrap();

        snap.rollback().unwrap();
        // A second rollback — as happens when an error path fires twice —
        // must not fail or change anything.
        snap.rollback().unwrap();
        assert!(snap.is_restored().unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_after_an_induced_panic_returns_the_repo_to_its_prior_state() {
        // v1.0 gate condition 4, in miniature: the panic unwinds, the
        // snapshot outlives it, and rollback still restores. CI runs the
        // full version against the benchmark repo.
        let dir = temp_repo("panic");
        let snap = snapshot(&dir).unwrap();

        let result = std::panic::catch_unwind({
            let dir = dir.clone();
            move || {
                std::fs::write(dir.join("a.py"), "half-written").unwrap();
                std::fs::write(dir.join("stray.py"), "x").unwrap();
                panic!("induced mid-run failure");
            }
        });
        assert!(result.is_err(), "the panic must actually have fired");

        snap.rollback().unwrap();
        assert!(snap.is_restored().unwrap());
        assert!(!dir.join("stray.py").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- v1.0 gate condition 4: six injection points -------------------
    //
    // `04-v1.0-harness.md` asks for failure injected at each stage of a run,
    // with clean restoration proven at every one. A single induced panic
    // establishes that rollback works when the stack unwinds tidily; it says
    // nothing about a run killed between two writes, or one whose check
    // subprocess died holding a file open.
    //
    // The two the doc calls out as usually missed — orchestrator panic and
    // SIGINT — live in `trellis-loop` and `trellis-cli`, where the thing
    // being interrupted actually is.

    /// Injection point 1: between two file writes.
    #[test]
    fn rollback_mid_edit_between_two_writes() {
        // A run that edited one file and died before the second leaves a tree
        // that is internally inconsistent in a way no single-file test
        // reaches: `a.py` now calls a helper that `b.py` was going to define.
        let dir = temp_repo("inject-mid-edit");
        std::fs::write(dir.join("b.py"), "def helper():\n    return 1\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "two files"]).unwrap();
        let snap = snapshot(&dir).unwrap();

        std::fs::write(
            dir.join("a.py"),
            "from b import helper\n\ndef f():\n    return helper(2)\n",
        )
        .unwrap();
        // ... and the process dies here, before b.py gains the parameter.

        snap.rollback().unwrap();
        assert!(snap.is_restored().unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Injection point 2: a check subprocess killed while running.
    #[test]
    fn rollback_after_a_killed_check_subprocess() {
        // The case that worries: a killed child can leave the repository with
        // an open handle or a stale lock, and on Windows an open handle is
        // enough to make a delete fail. Rollback has to work anyway.
        let dir = temp_repo("inject-killed-check");
        let snap = snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        std::fs::write(dir.join("scratch.txt"), "partial check output").unwrap();

        // A real child process, killed mid-flight, with the repo as its cwd.
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sleep" })
            .args(if cfg!(windows) {
                vec!["/C", "ping -n 30 127.0.0.1 > nul"]
            } else {
                vec!["30"]
            })
            .current_dir(&dir)
            .spawn()
            .expect("spawn a check-like child");
        child.kill().expect("kill it mid-run");
        let _ = child.wait();

        snap.rollback().unwrap();
        assert!(
            snap.is_restored().unwrap(),
            "a killed check subprocess must not block restoration"
        );
        assert!(!dir.join("scratch.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Injection point 3: after the checks pass, before the user approves.
    #[test]
    fn rollback_after_checks_pass_but_before_approval() {
        // The state a converged run actually leaves: edits in the tree,
        // nothing committed, approval outstanding. A user who aborts here —
        // or a machine that loses power — must get their repository back,
        // and the trail must survive, because a converged-but-unapproved run
        // is exactly the one worth having a record of.
        let dir = temp_repo("inject-before-approval");
        let snap = snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 42\n").unwrap();

        // Writing the trail here — committed, before the rollback — is the
        // obvious thing and it is wrong. `reset --hard` returns HEAD to the
        // snapshot, which is *behind* the trail commit, so the record of the
        // run goes with it.
        std::fs::create_dir_all(dir.join(".trellis/runs")).unwrap();
        std::fs::write(dir.join(".trellis/runs/early.json"), "{\"task\":\"x\"}").unwrap();
        commit_paths(&dir, &[".trellis/runs/early.json"], "trellis: trail").unwrap();

        snap.rollback().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n",
            "the unapproved edit is gone"
        );
        assert!(
            !dir.join(".trellis/runs/early.json").exists(),
            "a trail committed BEFORE the rollback is destroyed by it — this is \
             why cmd_run writes the trail after rolling back, and the comment \
             there now has a test under it"
        );

        // Written after the rollback, as the run path actually does it: the
        // trail survives, because nothing resets past it again.
        std::fs::create_dir_all(dir.join(".trellis/runs")).unwrap();
        std::fs::write(dir.join(".trellis/runs/late.json"), "{\"task\":\"x\"}").unwrap();
        commit_paths(&dir, &[".trellis/runs/late.json"], "trellis: trail").unwrap();
        assert!(dir.join(".trellis/runs/late.json").exists());
        assert!(
            snapshot(&dir).is_ok(),
            "and the worktree is clean again, so the next run can start"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Injection point 4: the trail write itself fails.
    #[test]
    fn rollback_still_restores_when_the_trail_write_fails() {
        // A failure writing the record must not become a failure to recover.
        // The trail is the thing TRELLIS is for, but the worktree is the
        // thing the user has to keep working in.
        let dir = temp_repo("inject-trail-write");
        let snap = snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 7\n").unwrap();

        // A directory where the trail file wants to be: every write to that
        // path fails, on every platform.
        std::fs::create_dir_all(dir.join(".trellis/runs/r.json")).unwrap();
        let wrote = std::fs::write(dir.join(".trellis/runs/r.json"), "{}");
        assert!(wrote.is_err(), "the trail write must actually have failed");

        snap.rollback().unwrap();
        assert!(
            snap.is_restored().unwrap(),
            "a failed trail write must not leave the worktree unrestored"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The recovery path's own failure (`04-v1.0-harness.md`, task item 3).
    #[test]
    fn a_rollback_that_fails_is_loud_and_leaves_a_manual_path() {
        // Every other error in TRELLIS leaves the repository as the user last
        // saw it. This one leaves it mid-run, so it is the one that must
        // never be quiet — and the user should not have to work out the SHA
        // themselves while being told their worktree is in an unknown state.
        let dir = temp_repo("inject-recovery-fails");
        let mut snap = snapshot(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 3\n").unwrap();
        std::fs::write(dir.join("stray.py"), "x = 1\n").unwrap();

        // A commit that is not in this repository: `reset --hard` cannot
        // succeed, which is the shape of a disk-full or locked-index failure
        // without needing either.
        snap.head = "0".repeat(40);

        let err = snap.rollback().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ROLLBACK FAILED"), "{msg}");
        assert!(
            msg.contains("NOT restored"),
            "it must say the worktree still holds the edits: {msg}"
        );
        assert!(
            msg.contains(&dir.display().to_string()),
            "it must name the worktree, or the user cannot act: {msg}"
        );
        assert!(
            msg.contains("git -C") && msg.contains("reset --hard") && msg.contains("clean -fd"),
            "it must give the commands, not describe them: {msg}"
        );
        assert!(
            msg.contains("index.lock"),
            "it must name the likely cause when the manual path also fails: {msg}"
        );
        assert!(
            !msg.contains("  \n"),
            "no folded indentation in text a user reads under pressure: {msg:?}"
        );

        // The half that could still succeed did: `clean` runs even though
        // `reset` failed, so this run's new files are gone.
        assert!(
            !dir.join("stray.py").exists(),
            "stopping at the first error gives up earlier than necessary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
