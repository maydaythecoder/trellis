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
//! be reliable. See `docs/ADRs/0009-clean-worktree-requirement.md`.

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

/// Records `HEAD`, refusing if the worktree is dirty.
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
    #[tracing::instrument(skip(self), fields(head = %self.head))]
    pub fn rollback(&self) -> Result<(), VcsError> {
        git(&self.root, &["reset", "--hard", &self.head])?;
        git(&self.root, &["clean", "-fd"])?;
        Ok(())
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
    pub fn create(
        root: &Path,
        run: &str,
        partition: u32,
        base: &str,
    ) -> Result<Self, VcsError> {
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
            &["worktree", "remove", "--force", &self.path.display().to_string()],
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
        assert!(!dir.join("new_module.py").exists(), "untracked file left behind");
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
}
