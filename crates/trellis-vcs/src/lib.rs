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

    /// The first wall a user hits, so it says how to get past it.
    ///
    /// `04-v1.0-harness.md` calls this out by name. The message used to state
    /// the rule and stop, which leaves someone holding work they have not
    /// committed and a tool that will not start — and the obvious reading of
    /// "requires a clean worktree" is that TRELLIS wants to throw their
    /// changes away.
    #[error("{}", dirty_worktree_message(.0))]
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

/// Why a clean worktree is required, and the three ways past it.
///
/// Written out because the requirement reads as arbitrary until you know what
/// it buys: rollback is `reset --hard` to a recorded commit, which cannot
/// distinguish this run's edits from work that was already there. Requiring a
/// clean tree is what makes "nothing was accepted" mean exactly that.
fn dirty_worktree_message(status: &str) -> String {
    format!(
        "{DIRTY_HEAD}
{}
{DIRTY_TAIL}",
        status.trim_end()
    )
}

const DIRTY_HEAD: &str = "the worktree has uncommitted changes, and a run needs a clean one.
";

/// A raw string, so what a user reads is what is written here.
///
/// Continuation-escaped literals fold their source indentation into the
/// content, which this project has shipped eight times — and a message about
/// a wall the user has just hit is a poor place for the ninth.
const DIRTY_TAIL: &str = r#"
Rollback is `git reset --hard` to a commit recorded before the run, and that
cannot tell this run's edits from work that was already there. The requirement
is what makes "nothing was accepted" mean exactly that: your own changes are
never at risk from a run, because a run never starts on top of them.

Any of these clears it:

  git add -A && git commit -m '...'    keep the work
  git stash                            set it aside for now
  git checkout -- .                    discard it

If the changes are a previous run's, `trellis` opens a session where /diff
shows them and /accept or /revert decides."#;

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
    // Scrubbed: `git commit` runs the repository's own hooks, and they
    // inherit this environment.
    let out =
        trellis_core::secrets::scrub(Command::new("git").args(args).current_dir(root)).output()?;
    if !out.status.success() {
        return Err(VcsError::GitFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The absolute git directory for the worktree at `root`.
///
/// In a linked worktree `.git` is a file, and this is the per-worktree
/// directory it names (`<main>/.git/worktrees/<name>`), not the common one.
/// Shelled out for the same reason as everything else here; the local work
/// queue (ADR 0020) is what needs it.
pub fn git_dir(root: &Path) -> Result<PathBuf, VcsError> {
    let out = git(root, &["rev-parse", "--absolute-git-dir"])?;
    Ok(PathBuf::from(out.trim()))
}

/// A digest of what a commit of the whole worktree would hold right now:
/// the tree id `git add -A && git write-tree` would give, computed in a
/// throwaway copy of the index so the user's index is never touched.
///
/// Content-complete for everything git does not ignore — tracked files and
/// untracked ones alike, bytes included. `git status --porcelain` is not: a
/// file rewritten after it was first created keeps the same `??` line. Two
/// equal fingerprints mean nothing a converged change could carry differs.
/// Ignored files are outside it, as they are outside every change trellis
/// proposes (ADR 0023, "after a pass").
///
/// Costs one index copy, one `add -A` (stat-cached, so it rehashes only
/// what changed) and one `write-tree`. Writes blob objects into the object
/// store as a side effect, which `git gc` collects.
pub fn worktree_fingerprint(root: &Path) -> Result<String, VcsError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let index = PathBuf::from(git(root, &["rev-parse", "--git-path", "index"])?.trim());
    let index = if index.is_absolute() {
        index
    } else {
        root.join(index)
    };
    let tmp = index.with_file_name(format!(
        "trellis-fingerprint-{}-{}.index",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A repository with no commits and nothing staged has no index yet;
    // an empty one is what git would start from.
    if index.is_file() {
        std::fs::copy(&index, &tmp)?;
    }
    let run = |args: &[&str]| -> Result<String, VcsError> {
        let mut git = Command::new("git");
        git.args(args).current_dir(root).env("GIT_INDEX_FILE", &tmp);
        // `git add -A` runs the repository's filters (clean/smudge drivers),
        // which are repository-supplied commands (ADR 0024 §3).
        trellis_core::secrets::scrub(&mut git);
        let out = git.output()?;
        if !out.status.success() {
            return Err(VcsError::GitFailed {
                command: format!("git {}", args.join(" ")),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let tree = run(&["add", "-A"]).and_then(|_| run(&["write-tree"]));
    let _ = std::fs::remove_file(&tmp);
    Ok(tree?.trim().to_string())
}

/// What [`worktree_fingerprint`] detects, computed without writing a single
/// object: for the bracket around every hooked tool call, which runs up to
/// four times per call, where the tree form wrote a loose blob for every
/// untracked file and a tree each time. Compared for equality only; it is
/// not a tree id, so [`tree_diff`] cannot take it.
///
/// A manifest: `HEAD`'s commit id, then one line per path whose worktree
/// content differs from `HEAD` — worktree mode and blob id from
/// `git hash-object` **without `-w`** — or `deleted`. The paths come from
/// one `git status --porcelain=v2 --untracked-files=all`, run with
/// `--no-optional-locks` so the user's index is not refreshed behind their
/// back. The index columns are ignored and a path whose worktree bytes and
/// mode equal `HEAD`'s is dropped, so staging or unstaging alone changes
/// nothing, as with the tree form.
///
/// Differences from the tree form, stated:
/// - it moves with `HEAD`: a commit with an unchanged worktree changes it
///   (the tree form would not). That errs toward "changed".
/// - an untracked file's mode is read from the filesystem (executable bit
///   on Unix, a symlink as `120000`); on Windows every untracked file reads
///   `100644`, so a mode-only change to one is not seen.
/// - a symlink is hashed by its link text, as git stores one (on Windows
///   with its separators written `/`), never by what it points at.
/// - an untracked nested repository is recorded by the commit it has
///   checked out, the gitlink `git add -A` stages: an uncommitted edit
///   inside it is not seen, as it was not by the tree form.
/// - a submodule entry is recorded as its status line, not hashed.
/// - a path containing a newline cannot go through `--stdin-paths`; the
///   digest then falls back to the tree form, objects and all.
pub fn worktree_digest(root: &Path) -> Result<String, VcsError> {
    let status = git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )?;
    let mut head = String::from("(unknown)");
    // path -> (HEAD mode, HEAD blob), for tracked paths git names.
    let mut in_head: std::collections::BTreeMap<String, (String, String)> = Default::default();
    // path -> worktree mode, for every path to hash.
    let mut worktree: std::collections::BTreeMap<String, String> = Default::default();
    // Lines kept verbatim: submodules and conflicts.
    let mut verbatim: Vec<String> = Vec::new();
    // Untracked nested repositories, as gitlink lines.
    let mut nested: Vec<String> = Vec::new();
    let mut fields = status.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        let cols: Vec<&str> = f.splitn(9, ' ').collect();
        match cols.first().copied() {
            Some("#") => {
                if let Some(oid) = f.strip_prefix("# branch.oid ") {
                    head = oid.to_string();
                }
            }
            // `status` names an untracked nested repository `dir/`, and
            // nothing inside it: a directory, which `hash-object` cannot
            // hash. `git add -A` would stage it as a gitlink to its HEAD,
            // so that is what is recorded.
            Some("?") if f.ends_with('/') => {
                let path = f[2..].to_string();
                nested.push(format!("160000 {} {path}", nested_head(&root.join(&path))));
            }
            Some("?") => {
                let path = f[2..].to_string();
                let mode = untracked_mode(&root.join(&path));
                worktree.insert(path, mode.to_string());
            }
            Some("1") | Some("2") if cols.len() >= 8 && cols[2].starts_with('N') => {
                // 1 XY sub mH mI mW hH hI path
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let path = if cols[0] == "2" {
                    cols.get(8)
                        .and_then(|rest| rest.split_once(' '))
                        .map(|(_, p)| p.to_string())
                        .unwrap_or_default()
                } else {
                    cols.get(8).map(|p| (*p).to_string()).unwrap_or_default()
                };
                // For a rename, `mH`/`hH` describe the source in HEAD. The
                // source is gone from where it was unless the worktree still
                // has it, in which case a `?` line names it too.
                let head_path = if cols[0] == "2" {
                    let orig = fields.next().unwrap_or_default().to_string();
                    worktree
                        .entry(orig.clone())
                        .or_insert_with(|| "000000".into());
                    orig
                } else {
                    path.clone()
                };
                in_head.insert(head_path, (cols[3].to_string(), cols[6].to_string()));
                worktree.insert(path, cols[5].to_string());
            }
            _ => verbatim.push(f.to_string()),
        }
    }
    let present: Vec<(&String, &String, bool)> = worktree
        .iter()
        .map(|(p, mode)| {
            let exists = mode.as_str() != "000000" && root.join(p).symlink_metadata().is_ok();
            (p, mode, exists)
        })
        .collect();
    // A symlink is hashed as git stores it, by its link text: reading
    // through it fails on a directory or a dangling target, and hashes
    // bytes git would never commit for it.
    let is_link = |p: &String| {
        root.join(p)
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    };
    let to_hash: Vec<&String> = present
        .iter()
        .filter(|x| x.2 && !is_link(x.0))
        .map(|x| x.0)
        .collect();
    if to_hash.iter().any(|p| p.contains('\n')) {
        return worktree_fingerprint(root).map(|t| format!("tree {t}"));
    }
    let mut hashed = hash_without_writing(root, &to_hash)?.into_iter();
    let mut manifest = format!("head {head}\n");
    for (path, mode, exists) in present {
        let head_entry = in_head.get(path).filter(|(m, _)| m.as_str() != "000000");
        if exists {
            let blob = if is_link(path) {
                hash_link(root, path)?
            } else {
                hashed.next().unwrap_or_default()
            };
            // The same bytes and mode as HEAD: whatever the index says,
            // the worktree does not differ here.
            if head_entry.is_some_and(|(m, h)| m == mode && *h == blob) {
                continue;
            }
            manifest.push_str(&format!("{mode} {blob} {path}\n"));
        } else if head_entry.is_some() {
            manifest.push_str(&format!("deleted {path}\n"));
        }
        // Neither in HEAD nor in the worktree: added in the index and
        // removed from disk, which changes no worktree byte.
    }
    for v in nested.into_iter().chain(verbatim) {
        manifest.push_str(&v);
        manifest.push('\n');
    }
    Ok(manifest)
}

/// The commit an untracked nested repository has checked out, as the
/// gitlink `git add -A` would record; `(unborn)` before its first commit.
///
/// Asked only of a directory with its own `.git`: without one, `rev-parse`
/// would walk up and answer for the outer repository.
fn nested_head(dir: &Path) -> String {
    if !dir.join(".git").exists() {
        return "(not-a-repository)".into();
    }
    git(dir, &["rev-parse", "--verify", "-q", "HEAD"])
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "(unborn)".into())
}

/// The blob id of a symlink's link text, which is how git stores a
/// symlink: `git hash-object --stdin`, without `-w`, so the object format
/// is the repository's and nothing is written.
fn hash_link(root: &Path, path: &str) -> Result<String, VcsError> {
    use std::io::Write;
    let target = std::fs::read_link(root.join(path))?;
    // Git for Windows stores a link's separators as `/`.
    let text = target.to_string_lossy().replace('\\', "/");
    let mut cmd = Command::new("git");
    cmd.args(["hash-object", "--stdin"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    trellis_core::secrets::scrub(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin.write_all(text.as_bytes())?;
    drop(stdin);
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(VcsError::GitFailed {
            command: "git hash-object --stdin".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `100644`, `100755` or `120000` for a file git does not track, as it
/// would stage it. Executable bits exist on Unix only.
fn untracked_mode(p: &Path) -> &'static str {
    match p.symlink_metadata() {
        Ok(m) if m.file_type().is_symlink() => "120000",
        #[cfg(unix)]
        Ok(m) => {
            use std::os::unix::fs::PermissionsExt;
            if m.permissions().mode() & 0o111 != 0 {
                "100755"
            } else {
                "100644"
            }
        }
        #[cfg(not(unix))]
        Ok(_) => "100644",
        Err(_) => "000000",
    }
}

/// Blob ids for `paths`, as `git hash-object --stdin-paths` computes them
/// (the repository's clean filters applied), with no `-w`: nothing is
/// written to the object store.
fn hash_without_writing(root: &Path, paths: &[&String]) -> Result<Vec<String>, VcsError> {
    use std::io::Write;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("git");
    cmd.args(["hash-object", "--stdin-paths"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Clean filters are repository-supplied commands (ADR 0024 §3).
    trellis_core::secrets::scrub(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut input = String::new();
    for p in paths {
        input.push_str(p);
        input.push('\n');
    }
    let mut stdin = child.stdin.take().expect("piped");
    // Written on its own thread: git writes one id per path as it reads, and
    // a large list would otherwise fill both pipes and stall.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(VcsError::GitFailed {
            command: "git hash-object --stdin-paths".into(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let ids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    if ids.len() != paths.len() {
        return Err(VcsError::GitFailed {
            command: "git hash-object --stdin-paths".into(),
            stderr: format!("{} ids for {} paths", ids.len(), paths.len()),
        });
    }
    Ok(ids)
}

/// The paths that differ between two trees from [`worktree_fingerprint`].
///
/// Both trees' objects are in the store, because computing a fingerprint
/// writes them. For saying *what* changed when a guard fires, rather than
/// only that something did.
pub fn tree_diff(root: &Path, before: &str, after: &str) -> Result<Vec<String>, VcsError> {
    let out = git(root, &["diff-tree", "-r", "--name-only", before, after])?;
    Ok(out.lines().map(str::to_string).collect())
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
/// The same two steps as [`Snapshot::rollback`], and now literally the same
/// code: they had drifted, with rollback attempting both steps and reporting
/// a loud aggregated failure while this one stopped at the first error and
/// returned a bare git message. Unlike rollback this has no recorded commit
/// to return to — it drops whatever is pending against current `HEAD`, which
/// is what "I do not want what that run proposed" means.
pub fn discard_pending(root: &Path) -> Result<(), VcsError> {
    reset_and_clean(root, "HEAD")
}

/// `reset --hard <to>` followed by `clean -fd`, attempting both.
///
/// Both steps run even when the first fails. A `clean` that succeeds after a
/// failed `reset` removes the run's new files, which is strictly better than
/// leaving both kinds of residue; stopping at the first error gives up
/// earlier than necessary on the one path that exists to recover from
/// failure.
fn reset_and_clean(root: &Path, to: &str) -> Result<(), VcsError> {
    let reset = git(root, &["reset", "--hard", to]);
    let clean = git(root, &["clean", "-fd"]);
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
                root: root.display().to_string(),
                head: to.to_string(),
                detail,
            })
        }
    }
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
        reset_and_clean(&self.root, &self.head)
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
        // Process id in the path, because these fixtures `remove_dir_all` then
        // `create_dir_all` a fixed location. Two test processes running at once
        // — which is exactly what `cargo mutants -j N` does — delete each
        // other's repository mid-test and fail for a reason that has nothing
        // to do with the code under test.
        let dir =
            std::env::temp_dir().join(format!("trellis-vcs-test-{name}-{}", std::process::id()));
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
    fn the_fingerprint_sees_a_rewritten_untracked_file_and_leaves_the_index_alone() {
        // The risk: `status --porcelain` says `?? new.py` before and after a
        // rewrite of that file, so a guard built on it would miss a hook
        // changing a file the run created.
        let dir = temp_repo("fingerprint");
        let clean = worktree_fingerprint(&dir).unwrap();
        assert_eq!(clean, worktree_fingerprint(&dir).unwrap(), "stable");
        std::fs::write(dir.join("new.py"), "x = 1\n").unwrap();
        let created = worktree_fingerprint(&dir).unwrap();
        assert_ne!(clean, created);
        std::fs::write(dir.join("new.py"), "x = 2\n").unwrap();
        assert_ne!(created, worktree_fingerprint(&dir).unwrap());
        assert_eq!(
            git(&dir, &["status", "--porcelain"]).unwrap().trim(),
            "?? new.py",
            "the user's index was not written"
        );
        std::fs::write(dir.join(".gitignore"), "build/\n").unwrap();
        let ignored_before = worktree_fingerprint(&dir).unwrap();
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(dir.join("build/x"), "y").unwrap();
        assert_eq!(
            ignored_before,
            worktree_fingerprint(&dir).unwrap(),
            "ignored files are outside it, as documented"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn loose_objects(dir: &Path) -> u64 {
        git(dir, &["count-objects", "-v"])
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("count: "))
            .and_then(|n| n.trim().parse().ok())
            .expect("count-objects prints `count:`")
    }

    #[test]
    fn the_per_call_digest_writes_no_objects_and_sees_every_worktree_change() {
        // The risk: the digest taken around every hooked tool call wrote a
        // loose blob for every untracked file, and a tree, each time: up to
        // four times per call, for the whole run.
        let dir = temp_repo("digest");
        std::fs::write(dir.join("new.py"), "x = 1\n").unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        let objects = loose_objects(&dir);
        let first = worktree_digest(&dir).unwrap();
        assert_eq!(loose_objects(&dir), objects, "the digest wrote objects");
        assert_eq!(first, worktree_digest(&dir).unwrap(), "stable");

        // Content-complete, as the tree form is: an untracked file rewritten
        // keeps its `??` line in `status --porcelain`, and is still seen.
        std::fs::write(dir.join("new.py"), "x = 2\n").unwrap();
        let rewritten = worktree_digest(&dir).unwrap();
        assert_ne!(first, rewritten, "an untracked rewrite");
        std::fs::write(dir.join("a.py"), "def f():\n    return 3\n").unwrap();
        let tracked = worktree_digest(&dir).unwrap();
        assert_ne!(rewritten, tracked, "a tracked file rewritten again");
        std::fs::remove_file(dir.join("a.py")).unwrap();
        let deleted = worktree_digest(&dir).unwrap();
        assert_ne!(tracked, deleted, "a tracked file deleted");
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        assert_ne!(
            deleted,
            worktree_digest(&dir).unwrap(),
            "put back as committed"
        );

        // Independent of the index, as the tree form is: staging changes no
        // byte in the worktree, and neither does unstaging.
        let before = worktree_digest(&dir).unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        assert_eq!(before, worktree_digest(&dir).unwrap(), "staging alone");
        git(&dir, &["rm", "-q", "--cached", "a.py"]).unwrap();
        assert_eq!(before, worktree_digest(&dir).unwrap(), "unstaging alone");
        git(&dir, &["reset", "-q"]).unwrap();
        assert_eq!(before, worktree_digest(&dir).unwrap(), "reset alone");

        // Ignored files are outside it, as documented.
        std::fs::write(dir.join(".gitignore"), "build/\n").unwrap();
        let ignored_before = worktree_digest(&dir).unwrap();
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(dir.join("build/x"), "y").unwrap();
        assert_eq!(ignored_before, worktree_digest(&dir).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_untracked_nested_repository_is_digested_by_its_head_not_hashed_as_a_file() {
        // The risk: `status` names an untracked nested repository `nested/`,
        // `git hash-object` cannot hash a directory, the digest errored,
        // every hooked call read as "changed", and a run could never
        // converge. The old `git add -A` form recorded it as a gitlink: its
        // HEAD, and nothing inside.
        let dir = temp_repo("digest-nested");
        let clean = worktree_digest(&dir).unwrap();
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]).unwrap();
        // With no commit yet, as `git init` leaves it.
        let unborn = worktree_digest(&dir).expect("a nested repository with no commit");
        assert_ne!(clean, unborn, "the nested repository is an untracked entry");
        assert_eq!(unborn, worktree_digest(&dir).unwrap(), "stable");

        git(&nested, &["config", "user.email", "t@example.invalid"]).unwrap();
        git(&nested, &["config", "user.name", "t"]).unwrap();
        std::fs::write(nested.join("x.py"), "x = 1\n").unwrap();
        git(&nested, &["add", "-A"]).unwrap();
        git(&nested, &["commit", "-qm", "one"]).unwrap();
        let one = worktree_digest(&dir).expect("a nested repository with a commit");
        assert_ne!(unborn, one, "its HEAD is what is recorded");
        assert_eq!(one, worktree_digest(&dir).unwrap(), "stable");
        std::fs::write(nested.join("x.py"), "x = 2\n").unwrap();
        git(&nested, &["commit", "-qam", "two"]).unwrap();
        assert_ne!(one, worktree_digest(&dir).unwrap(), "a new nested commit");
        // Nothing written into the outer store either.
        let objects = loose_objects(&dir);
        worktree_digest(&dir).unwrap();
        assert_eq!(loose_objects(&dir), objects);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlink, or `None` where the platform refuses to make one (Windows
    /// without developer mode), in which case the test says so.
    fn symlink(target: &Path, link: &Path, dir: bool) -> Option<()> {
        #[cfg(windows)]
        let r = if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        #[cfg(unix)]
        let r = {
            let _ = dir;
            std::os::unix::fs::symlink(target, link)
        };
        match r {
            Ok(()) => Some(()),
            Err(e) => {
                eprintln!("skipped: cannot create a symlink here: {e}");
                None
            }
        }
    }

    fn remove_link(p: &Path) {
        // A directory link is a directory entry on Windows, a file on Unix.
        std::fs::remove_dir(p)
            .or_else(|_| std::fs::remove_file(p))
            .unwrap();
    }

    #[test]
    fn an_untracked_symlink_is_digested_by_its_link_text() {
        // The risk: `git hash-object` reads through a link, so one to a
        // directory made the digest error (every hooked pass voided), and
        // one that dangles did too. Git stores a symlink as its link text;
        // so does the digest.
        let dir = temp_repo("digest-symlink");
        std::fs::create_dir_all(dir.join("d")).unwrap();
        std::fs::write(dir.join("d/f.py"), "x = 1\n").unwrap();
        std::fs::create_dir_all(dir.join("e")).unwrap();
        std::fs::write(dir.join("e/f.py"), "x = 1\n").unwrap();
        let Some(()) = symlink(Path::new("d"), &dir.join("to-dir"), true) else {
            return;
        };
        let first = worktree_digest(&dir).expect("a link to a directory");
        assert_eq!(first, worktree_digest(&dir).unwrap(), "stable");
        assert!(first.contains("120000 "), "{first}");

        // Retargeted at identical content: the link text changed, and so
        // does the digest, as a commit of it would.
        remove_link(&dir.join("to-dir"));
        symlink(Path::new("e"), &dir.join("to-dir"), true).unwrap();
        assert_ne!(first, worktree_digest(&dir).unwrap(), "a retargeted link");

        // One that points nowhere is an entry, not an error.
        symlink(Path::new("missing"), &dir.join("dangling"), false).unwrap();
        worktree_digest(&dir).expect("a dangling link");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fired_guard_can_name_the_paths_that_changed() {
        // The risk: a guard that only says "something changed" leaves the
        // person to guess which file, and a flaky demotion cannot be told
        // from a real one.
        let dir = temp_repo("tree-diff");
        let before = worktree_fingerprint(&dir).unwrap();
        std::fs::write(dir.join("hook-wrote.txt"), "x").unwrap();
        let after = worktree_fingerprint(&dir).unwrap();
        assert_eq!(
            tree_diff(&dir, &before, &after).unwrap(),
            vec!["hook-wrote.txt".to_string()]
        );
        assert!(tree_diff(&dir, &before, &before).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
        let msg = err.to_string();
        assert!(
            msg.contains("reset --hard") && msg.contains("never at risk"),
            "the refusal must say why, or it reads as an arbitrary obstacle: {msg}"
        );
        assert!(
            msg.contains("git stash") && msg.contains("git commit"),
            "and it must give the ways out — this is the first wall users hit: {msg}"
        );
        assert!(
            !msg.contains(
                "  
"
            ),
            "no folded indentation in the message a stuck user reads: {msg:?}"
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

    /// C-002: `Worktree` is written ahead of need for brief 4.13's
    /// per-partition isolation and nothing calls it yet, so both its mutants
    /// survive every run — the mutation evidence for a finding first made by
    /// reading.
    ///
    /// Tested rather than deleted. It has no replacement, unlike
    /// `tools::args` which `ToolCallWire` superseded and
    /// `Trail::factual_is_intact` which duplicated a better check; and it
    /// sits in the per-partition rollback path, so when v1.1 wires it up it
    /// needs to already work. Dead-and-verified is the right state for code
    /// deliberately written early. Dead-and-unverified is not.
    #[test]
    fn a_partition_worktree_is_created_on_its_own_branch() {
        let dir = temp_repo("worktree-create");
        let base = git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let wt = Worktree::create(&dir, "20260921-run", 0, &base).expect("worktree add");

        assert!(wt.path.exists(), "the worktree directory must exist");
        assert!(
            wt.path.join("a.py").exists(),
            "and carry a checkout of the base commit"
        );
        assert_eq!(wt.branch, "trellis/20260921-run/p0");
        assert!(
            wt.path.ends_with("partition-0"),
            "the path names the partition: {}",
            wt.path.display()
        );

        // Isolation is the whole point: editing inside the worktree must not
        // reach the tree a run was started from, because that tree is what
        // ADR 0009's clean-worktree gate is checked against.
        //
        // Asserted against the parent's *tracked* file rather than against an
        // empty `git status`, and the difference is C-010. The worktree lives
        // at `.trellis/work/...` inside the repository, so git reports that
        // directory as untracked however well isolated the edit is. That is a
        // real problem — it dirties the tree and the next run refuses to
        // start — and it is fixed where it belongs, in the `.trellis/.gitignore`
        // the index cache scaffolds. It is not this property.
        std::fs::write(
            wt.path.join("a.py"),
            "def f():
    return 2
",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():
    return 1
",
            "an edit in the partition's worktree reached the parent"
        );
        let dirty = git(&dir, &["status", "--porcelain"]).unwrap();
        assert!(
            !dirty.contains("a.py"),
            "the parent's tracked files must be untouched: {dirty}"
        );

        wt.discard().expect("worktree remove");
    }

    #[test]
    fn discarding_a_partition_removes_its_directory_and_its_branch() {
        // "A failed partition's work never lands partially" — the branch has
        // to go too. Leaving it behind means a later run finds a branch named
        // for a partition that failed, holding commits nothing will integrate.
        let dir = temp_repo("worktree-discard");
        let base = git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let wt = Worktree::create(&dir, "20260921-run", 3, &base).expect("worktree add");
        std::fs::write(wt.path.join("a.py"), "def f():\n    return 3\n").unwrap();

        let branches_before = git(&dir, &["branch", "--list"]).unwrap();
        assert!(
            branches_before.contains("trellis/20260921-run/p3"),
            "{branches_before}"
        );

        wt.discard().expect("worktree remove");

        assert!(!wt.path.exists(), "the directory must be gone");
        let branches_after = git(&dir, &["branch", "--list"]).unwrap();
        assert!(
            !branches_after.contains("trellis/20260921-run/p3"),
            "the branch outlived the worktree: {branches_after}"
        );
        // And the parent is untouched by any of it.
        assert!(
            git(&dir, &["status", "--porcelain"])
                .unwrap()
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn two_partitions_get_separate_worktrees() {
        // The property v1.1 depends on. Two agents editing the same file in
        // different partitions must not see each other, which is what makes
        // "filesystem isolation for free" true rather than hopeful.
        let dir = temp_repo("worktree-two");
        let base = git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let a = Worktree::create(&dir, "run", 0, &base).expect("first");
        let b = Worktree::create(&dir, "run", 1, &base).expect("second");
        assert_ne!(a.path, b.path);
        assert_ne!(a.branch, b.branch);

        std::fs::write(a.path.join("a.py"), "def f():\n    return 10\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(b.path.join("a.py")).unwrap(),
            "def f():\n    return 1\n",
            "one partition's edit was visible in another's worktree"
        );

        a.discard().expect("discard first");
        assert!(b.path.exists(), "discarding one must not remove the other");
        b.discard().expect("discard second");
    }
}
