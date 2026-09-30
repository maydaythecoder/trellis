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

pub mod git_guard;
pub mod ignored;

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

/// Why the git guard ([`git_guard::GitGuard::capture`]) or the ignored-file
/// listing ([`ignored::IgnoredSnapshot::take`]) could not be taken. The
/// messages are the ones these returned as strings before (review round
/// 10), since a caller prints them as they are.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// `what` is the command, e.g. `git rev-parse`.
    #[error("{what} could not run: {source}")]
    Spawn {
        what: &'static str,
        source: std::io::Error,
    },
    /// Trimmed stderr.
    #[error("{what} failed: {stderr}")]
    Failed { what: &'static str, stderr: String },
    #[error("{} could not be read: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} could not be resolved: {source}", path.display())]
    Unresolved {
        path: PathBuf,
        source: std::io::Error,
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
    /// The branch `HEAD` was on (`refs/heads/main`), `None` when detached.
    /// A rollback puts `HEAD` back on it before resetting: `reset --hard`
    /// moves whichever branch `HEAD` is on, and after a writer's `git
    /// checkout -b` that is the writer's, not this one (review round 10).
    pub branch: Option<String>,
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
    output(command().args(args).current_dir(root), args)
}

/// Settings every `git` trellis runs sets on its own command line, where no
/// config file (repository, worktree, include, global) can outrank them,
/// since a `bash` call, custom tool, hook or process agent can write
/// `.git/config` and the git guard puts it back only once the writer is
/// done (ADR 0023 §5):
/// - `core.fsmonitor=`: the command `git status` and `git add` would run
///   to ask what changed. Empty rather than `false`: git before 2.36 took
///   the setting as a hook path only, and would run a command named
///   `false`; an empty value is off in every version;
/// - `core.hooksPath` at a path that cannot hold a hook (`/dev/null`, or
///   `NUL` on Windows): a hook runs on any index write
///   (`post-index-change`), `git add` into a copied index included.
///
/// Clean filters are neutralised separately ([`filters_off`]) and only
/// where a digest is taken, since a filter is named per driver and a
/// rollback's checkout needs the user's own (Git LFS's smudge).
pub const OVERRIDES: [&str; 4] = ["-c", "core.fsmonitor=", "-c", NO_HOOKS];

#[cfg(windows)]
const NO_HOOKS: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const NO_HOOKS: &str = "core.hooksPath=/dev/null";

/// `git` with [`OVERRIDES`], its environment scrubbed of keys.
///
/// Every `git` the workspace starts outside tests comes from here,
/// [`command_with_hooks`] or [`git_argv`] (review round 10: `grep` and
/// `find` started plain `git` and ran a planted `core.fsmonitor`).
/// `every_git_spawn_goes_through_trellis_vcs` in `trellis-cli`'s
/// `tests/hygiene.rs` fails on one that does not. For a tokio command,
/// `tokio::process::Command::from(command())`.
pub fn command() -> Command {
    let mut c = Command::new("git");
    c.args(OVERRIDES);
    trellis_core::secrets::scrub(&mut c);
    c
}

/// `git`, [`OVERRIDES`], then `args`, as an argv for a runner that takes
/// one (`trellis-loop`'s `exec::run`, which scrubs keys itself).
pub fn git_argv(args: &[&str]) -> Vec<String> {
    std::iter::once("git")
        .chain(OVERRIDES)
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

/// `git` with `core.fsmonitor` off and the hooks left alone: for the
/// trail's commit, which runs the user's hooks as any commit does, and for
/// the git guard's `rev-parse --git-path hooks`, which must name the real
/// hooks directory.
pub(crate) fn command_with_hooks() -> Command {
    let mut c = Command::new("git");
    c.args(["-c", "core.fsmonitor="]);
    trellis_core::secrets::scrub(&mut c);
    c
}

/// `-c` pairs that turn off every filter driver the configuration defines,
/// for the commands a digest runs (`status`, `hash-object`, `add`), which
/// would otherwise run a driver's `clean` or `process` command on any file
/// whose stat changed. Git cannot be told "no filters" for `status` or
/// `add`, so the drivers are listed (`git config --get-regexp`, which reads
/// and runs nothing) and each is emptied: an empty command is no filter,
/// and `required=false` stops an empty one from failing the command. A
/// driver name that `-c` cannot carry (one with `=` in it) makes git refuse
/// the command, which reads as "changed": the safe direction.
///
/// Listed at every call, not once per run: a writer can define a driver
/// between two digests. The digest then compares raw bytes rather than
/// filtered ones, on both sides, so equality still means what it did.
fn filters_off(root: &Path) -> Result<Vec<String>, VcsError> {
    let args = [
        "config",
        "--name-only",
        "--get-regexp",
        r"^filter\..*\.(clean|smudge|process|required)$",
    ];
    let out = command().args(args).current_dir(root).output()?;
    // 1 is "no such key": no drivers.
    if !out.status.success() && out.status.code() != Some(1) {
        return Err(VcsError::GitFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let mut names: Vec<String> = Vec::new();
    for key in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(name) = key
            .strip_prefix("filter.")
            .and_then(|k| k.rsplit_once('.'))
            .map(|(n, _)| n.to_string())
        else {
            continue;
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let mut pairs = Vec::new();
    for n in names {
        for (k, v) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            pairs.push("-c".to_string());
            pairs.push(format!("filter.{n}.{k}={v}"));
        }
    }
    Ok(pairs)
}

/// Runs `cmd`, its stdout as text, or the failure named by `args`.
fn output(cmd: &mut Command, args: &[&str]) -> Result<String, VcsError> {
    let out = cmd.output()?;
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
        // The copy keeps the index's mtime. Git decides whether a stat entry
        // can be trusted by comparing it with the index file's own mtime (the
        // racy-clean check); a copy stamped "now" makes every entry look
        // settled, so a same-size edit made in the index's second is hashed
        // as its old content. Linux's `copy` stamps now and Windows'
        // `CopyFile` does not, which is why this showed up as a Linux-only
        // flake of the after_run guard.
        let mtime = std::fs::metadata(&index)?.modified()?;
        std::fs::File::options()
            .write(true)
            .open(&tmp)?
            .set_modified(mtime)?;
    }
    // `git add -A` would run the repository's clean filters, and its index
    // write a `post-index-change` hook: both off (`filters_off`, `command`).
    let filters = filters_off(root);
    let run = |args: &[&str]| -> Result<String, VcsError> {
        let filters = filters.as_ref().map_err(|e| VcsError::GitFailed {
            command: "git config --get-regexp filter".into(),
            stderr: e.to_string(),
        })?;
        output(
            command()
                .args(filters)
                .args(args)
                .current_dir(root)
                .env("GIT_INDEX_FILE", &tmp),
            args,
        )
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
/// - a submodule entry is recorded as its status line, not hashed (a
///   renamed one with its origPath, which every `2` record carries).
/// - a path containing a newline cannot go through `--stdin-paths`; the
///   digest then falls back to the tree form, objects and all.
pub fn worktree_digest(root: &Path) -> Result<String, VcsError> {
    // `status` and `hash-object` run clean filters on files whose stat
    // changed; a writer may have planted one a call ago (`filters_off`).
    let filters = filters_off(root)?;
    let args = [
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
    ];
    let status = output(command().args(&filters).args(args).current_dir(root), &args)?;
    let Parsed {
        head,
        in_head,
        mut worktree,
        verbatim,
        nested,
    } = parse_status_v2(&status);
    let nested: Vec<String> = nested
        .into_iter()
        .map(|path| format!("160000 {} {path}", nested_head(&root.join(&path))))
        .collect();
    for (path, mode) in worktree.iter_mut() {
        if mode.is_empty() {
            *mode = untracked_mode(&root.join(path)).to_string();
        }
    }
    // (path, mode, exists, is a symlink), each looked at once: a second
    // look could disagree with the first and misalign the hashed ids.
    let present: Vec<(&String, &String, bool, bool)> = worktree
        .iter()
        .map(|(p, mode)| {
            let md = root.join(p).symlink_metadata();
            let exists = mode.as_str() != "000000" && md.is_ok();
            let link = md.is_ok_and(|m| m.file_type().is_symlink());
            (p, mode, exists, link)
        })
        .collect();
    // A symlink is hashed as git stores it, by its link text: reading
    // through it fails on a directory or a dangling target, and hashes
    // bytes git would never commit for it.
    let to_hash: Vec<&String> = present
        .iter()
        .filter(|x| x.2 && !x.3)
        .map(|x| x.0)
        .collect();
    if to_hash.iter().any(|p| p.contains('\n')) {
        return worktree_fingerprint(root).map(|t| format!("tree {t}"));
    }
    let links: Vec<&String> = present.iter().filter(|x| x.2 && x.3).map(|x| x.0).collect();
    let mut hashed = hash_without_writing(root, &to_hash, &filters)?.into_iter();
    let mut linked = hash_links(root, &links)?.into_iter();
    let mut manifest = format!("head {head}\n");
    for (path, mode, exists, link) in present {
        let head_entry = in_head.get(path).filter(|(m, _)| m.as_str() != "000000");
        if exists {
            let blob = if link {
                linked.next().unwrap_or_default()
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

/// `git status --porcelain=v2 -z --branch` split into what
/// [`worktree_digest`] hashes. Pure, so the record grammar can be tested
/// with crafted bytes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    head: String,
    /// path -> (HEAD mode, HEAD blob), for tracked paths git names.
    in_head: std::collections::BTreeMap<String, (String, String)>,
    /// path -> worktree mode, for every path to hash; empty for an
    /// untracked file, whose mode is read from the filesystem by the caller.
    worktree: std::collections::BTreeMap<String, String>,
    /// Records kept verbatim: submodules and conflicts.
    verbatim: Vec<String>,
    /// Untracked nested repositories, by path.
    nested: Vec<String>,
}

fn parse_status_v2(status: &str) -> Parsed {
    let mut out = Parsed {
        head: String::from("(unknown)"),
        ..Parsed::default()
    };
    let mut fields = status.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        let cols: Vec<&str> = f.splitn(9, ' ').collect();
        match cols.first().copied() {
            Some("#") => {
                if let Some(oid) = f.strip_prefix("# branch.oid ") {
                    out.head = oid.to_string();
                }
            }
            // `status` names an untracked nested repository `dir/`, and
            // nothing inside it: a directory, which `hash-object` cannot
            // hash. `git add -A` would stage it as a gitlink to its HEAD,
            // so that is what is recorded.
            Some("?") if f.ends_with('/') => out.nested.push(f[2..].to_string()),
            Some("?") => {
                out.worktree.insert(f[2..].to_string(), String::new());
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
                    out.worktree
                        .entry(orig.clone())
                        .or_insert_with(|| "000000".into());
                    orig
                } else {
                    path.clone()
                };
                out.in_head
                    .insert(head_path, (cols[3].to_string(), cols[6].to_string()));
                out.worktree.insert(path, cols[5].to_string());
            }
            // Every `2` record carries its origPath as the next field,
            // whatever else it is: a renamed submodule (`S...`) lands here,
            // and leaving its origPath unread made the next record's parse
            // start on a path, so everything after it was misread.
            Some("2") => match fields.next() {
                Some(orig) => out.verbatim.push(format!("{f}\t{orig}")),
                None => out.verbatim.push(f.to_string()),
            },
            _ => out.verbatim.push(f.to_string()),
        }
    }
    out
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

/// The blob ids of symlinks' link texts, which is how git stores a symlink,
/// in one `git hash-object --no-filters --stdin-paths` for all of them: one
/// process per link had been a spawn per symlink per call. Each link text
/// is written to a file in a fresh directory under the system temp dir —
/// never in the worktree, whose digest this is — and hashed from there.
/// Without `-w`, so the object format is the repository's and nothing is
/// written to its store; `--no-filters`, since git applies none to a link.
fn hash_links(root: &Path, paths: &[&String]) -> Result<Vec<String>, VcsError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // A counter as well as the clock: two digests in one process within
    // one clock tick (Windows counts in 100 ns) would otherwise collide,
    // `create_dir` would fail, and a failed digest reads as a change.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "trellis-links-{}-{nanos}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir(&tmp)?;
    /// Removes the directory however the hashing ends.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(tmp.clone());
    let mut files = Vec::with_capacity(paths.len());
    for (i, p) in paths.iter().enumerate() {
        let target = std::fs::read_link(root.join(p))?;
        // Git for Windows stores a link's separators as `/`.
        let text = target.to_string_lossy().replace('\\', "/");
        let f = tmp.join(i.to_string());
        std::fs::write(&f, text.as_bytes())?;
        files.push(f.to_string_lossy().into_owned());
    }
    let refs: Vec<&String> = files.iter().collect();
    hash_paths(root, &refs, &[], &["--no-filters"])
}

/// One link's blob id through `git hash-object --stdin`, as the digest
/// computed it before links were batched: the reference the batch is
/// tested against.
#[cfg(test)]
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
/// with `config` (the digest's [`filters_off`]) in force, with no `-w`:
/// nothing is written to the object store.
fn hash_without_writing(
    root: &Path,
    paths: &[&String],
    config: &[String],
) -> Result<Vec<String>, VcsError> {
    hash_paths(root, paths, config, &[])
}

/// `git hash-object --stdin-paths` over `paths`, with `config` pairs
/// before the subcommand and `extra` flags after it, one id per path in
/// order.
fn hash_paths(
    root: &Path,
    paths: &[&String],
    config: &[String],
    extra: &[&str],
) -> Result<Vec<String>, VcsError> {
    use std::io::Write;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut cmd = command();
    cmd.args(config)
        .arg("hash-object")
        .args(extra)
        .arg("--stdin-paths")
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
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
    // The one git call that keeps the user's hooks: a commit in their
    // repository runs their `pre-commit`, as any commit does. It runs after
    // the git guard has put `.git/` back, so the hooks are theirs.
    let with_hooks =
        |args: &[&str]| output(command_with_hooks().args(args).current_dir(root), args);
    let mut add: Vec<&str> = vec!["add", "--"];
    add.extend_from_slice(paths);
    with_hooks(&add)?;

    // `--only` so staged-but-unrelated changes cannot ride along.
    let mut commit: Vec<&str> = vec!["commit", "--only", "-m", message, "--"];
    commit.extend_from_slice(paths);
    with_hooks(&commit)?;
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
    recovery_steps(
        root,
        to,
        String::new(),
        &[
            ("reset --hard", &["reset", "--hard", to]),
            ("clean -fd", &["clean", "-fd"]),
        ],
    )
}

/// Runs every step in order, each even when an earlier one failed, and
/// appends each failure to `detail` as `<label>: <error>`, one per line.
/// `Err(RollbackFailed)` when `detail` is non-empty afterwards, so a caller
/// that starts it with a failure of its own always gets the error back.
///
/// The one place recovery steps run: [`reset_and_clean`] and
/// [`Snapshot::rollback`]'s fallback each hand-built this before, on the
/// one path that must stay correct, and two copies are two chances to drift
/// in what they attempt or how they report it.
fn recovery_steps(
    root: &Path,
    head: &str,
    mut detail: String,
    steps: &[(&str, &[&str])],
) -> Result<(), VcsError> {
    for (label, args) in steps {
        if let Err(e) = git(root, args) {
            if !detail.is_empty() {
                detail.push_str("\n  ");
            }
            detail.push_str(&format!("{label}: {e}"));
        }
    }
    if detail.is_empty() {
        return Ok(());
    }
    Err(VcsError::RollbackFailed {
        root: root.display().to_string(),
        head: head.to_string(),
        detail,
    })
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
    let branch = branch_of(root)?;
    Ok(Snapshot {
        root: root.to_path_buf(),
        head,
        branch,
    })
}

/// The branch `HEAD` is on (`refs/heads/main`), or `None` when detached.
///
/// `symbolic-ref -q`, which reads what `HEAD` points at without resolving
/// it, so a branch with no commit yet (`git checkout --orphan x`) is still
/// named. `rev-parse --symbolic-full-name HEAD` failed there, and the
/// rollback returned before restoring anything (review round 11). Exit 1
/// with `-q` is "not a symbolic ref": detached.
fn branch_of(root: &Path) -> Result<Option<String>, VcsError> {
    Ok(git_opt(root, &["symbolic-ref", "-q", "HEAD"])?
        .map(|out| out.trim().to_string())
        .filter(|name| !name.is_empty()))
}

/// The commit `HEAD` resolves to, or `None` when it resolves to none: an
/// unborn branch, which a writer's `checkout --orphan` leaves. That is a
/// move to report, not an error that stops the report.
fn head_commit(root: &Path) -> Result<Option<String>, VcsError> {
    Ok(
        git_opt(root, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?
            .map(|out| out.trim().to_string()),
    )
}

/// `git <args>`'s stdout, `None` on exit 1, `GitFailed` on any other
/// failure. Only for a query run with `-q`, where exit 1 is git's answer
/// "there is none" (detached, unborn) rather than a failure: without `-q`
/// the same state is reported on stderr with another code, and reading
/// every exit 1 as absent would hide a real failure.
fn git_opt(root: &Path, args: &[&str]) -> Result<Option<String>, VcsError> {
    let out = command().args(args).current_dir(root).output()?;
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    if !out.status.success() {
        return Err(VcsError::GitFailed {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

fn describe_head(sha: &str, branch: &Option<String>) -> String {
    if sha.is_empty() {
        return match branch {
            Some(b) => format!("{} (no commit)", b.trim_start_matches("refs/heads/")),
            None => "no commit".to_string(),
        };
    }
    let short = sha.get(..12).unwrap_or(sha);
    match branch {
        Some(b) => format!("{short} on {}", b.trim_start_matches("refs/heads/")),
        None => format!("{short} (detached)"),
    }
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
    ///
    /// `HEAD` is put back on the recorded branch first (or detached, when
    /// it was): `reset --hard` moves whichever branch `HEAD` is on, so after
    /// a writer's `git checkout -b` it would reset the writer's branch and
    /// leave the user's holding the writer's commits. When `HEAD` cannot be
    /// put back, the tree is still restored, by `read-tree --reset -u` and
    /// `clean -fd`, which write the index and the worktree and move no ref:
    /// a reset would move a branch the snapshot never recorded, and
    /// returning without touching the tree (as before review round 11) left
    /// every edit in place. The failure is then reported with whatever else
    /// failed. A branch the writer created, or another branch it moved, is
    /// left as it is: only `HEAD` and its branch are recorded.
    #[tracing::instrument(skip(self), fields(head = %self.head))]
    pub fn rollback(&self) -> Result<(), VcsError> {
        let Err(head) = self.put_head_back() else {
            return reset_and_clean(&self.root, &self.head);
        };
        recovery_steps(
            &self.root,
            &self.head,
            format!("HEAD could not be put back where it was: {head}"),
            &[
                (
                    "read-tree --reset -u",
                    &["read-tree", "--reset", "-u", &self.head],
                ),
                ("clean -fd", &["clean", "-fd"]),
            ],
        )
    }

    /// Re-points `HEAD` at the recorded branch, or detaches it at the
    /// recorded commit, without touching the worktree; the reset that
    /// follows does that. Nothing is written when it is already there.
    fn put_head_back(&self) -> Result<(), VcsError> {
        if branch_of(&self.root)? == self.branch {
            return Ok(());
        }
        match &self.branch {
            Some(b) => git(&self.root, &["symbolic-ref", "HEAD", b]).map(|_| ()),
            None => git(
                &self.root,
                &["update-ref", "--no-deref", "HEAD", &self.head],
            )
            .map(|_| ()),
        }
    }

    /// Whether the worktree currently matches the snapshot exactly.
    ///
    /// Used by the rollback test to assert the gate condition rather than
    /// assume it.
    pub fn is_restored(&self) -> Result<bool, VcsError> {
        let head = head_commit(&self.root)?.unwrap_or_default();
        let status = git(&self.root, &["status", "--porcelain"])?;
        // The branch too: on another branch at the same commit, the next
        // commit lands somewhere the snapshot never was.
        Ok(head == self.head && branch_of(&self.root)? == self.branch && status.trim().is_empty())
    }

    /// `Some` describing the move when `HEAD` is no longer the recorded
    /// commit on the recorded branch: a commit, a reset, a checkout, a
    /// branch switch. A run may not move it. Committed work is invisible to
    /// `git status`, so every judgment made from the worktree's changes
    /// (the `.trellis/` breach, `deny_write`, governance) would miss it,
    /// and a converged run leaves no commit for review (review round 10).
    pub fn head_moved(&self) -> Result<Option<String>, VcsError> {
        // Empty on an unborn branch (`checkout --orphan`), described as
        // "no commit": a move, never a match.
        let head = head_commit(&self.root)?.unwrap_or_default();
        let branch = branch_of(&self.root)?;
        if head == self.head && branch == self.branch {
            return Ok(None);
        }
        Ok(Some(format!(
            "HEAD moved from {} to {}",
            describe_head(&self.head, &self.branch),
            describe_head(&head, &branch)
        )))
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

    #[test]
    fn a_renamed_submodule_record_consumes_its_orig_path() {
        // The risk: a `2` record for a submodule (`S...`) fell through to
        // the verbatim arm without reading its origPath, so the origPath
        // was parsed as the next record and every record after it was read
        // one field late. Crafted, since a renamed submodule needs a second
        // repository to set up and the grammar is what is under test.
        let status = concat!(
            "# branch.oid 0123abcd\0",
            "2 R. S... 160000 160000 160000 aaaa aaaa R100 libs/new\0libs/old\0",
            "? x.py\0",
            "1 .M N... 100644 100644 100644 bbbb bbbb app.py\0",
        );
        let p = parse_status_v2(status);
        assert_eq!(p.head, "0123abcd");
        assert_eq!(
            p.verbatim,
            ["2 R. S... 160000 160000 160000 aaaa aaaa R100 libs/new\tlibs/old"],
            "one record, both paths"
        );
        assert_eq!(p.worktree.get("x.py").map(String::as_str), Some(""));
        assert_eq!(p.worktree.get("app.py").map(String::as_str), Some("100644"));
        assert_eq!(
            p.in_head.get("app.py"),
            Some(&("100644".to_string(), "bbbb".to_string()))
        );
        assert!(!p.worktree.contains_key("libs/old"), "{p:?}");
    }

    #[test]
    fn a_renamed_file_record_still_parses_as_before() {
        let status = "2 R. N... 100644 100644 100644 cccc cccc R100 b.py\0a.py\0? z\0";
        let p = parse_status_v2(status);
        assert_eq!(p.worktree.get("b.py").map(String::as_str), Some("100644"));
        assert_eq!(p.worktree.get("a.py").map(String::as_str), Some("000000"));
        assert_eq!(
            p.in_head.get("a.py"),
            Some(&("100644".to_string(), "cccc".to_string()))
        );
        assert_eq!(p.worktree.get("z").map(String::as_str), Some(""));
        assert!(p.verbatim.is_empty());
    }

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

    /// A repository whose config runs a marker-writing command from
    /// `core.fsmonitor`, a `post-index-change` hook and a clean filter bound
    /// to every `*.py` (`required`, so a filter that is merely skipped
    /// fails loudly), with a tracked file edited and an untracked one
    /// added. What `bash` could plant a call before a digest runs.
    fn planted_repo(name: &str) -> (PathBuf, PathBuf) {
        let dir = temp_repo(name);
        let marks = dir.with_file_name(format!("{}-marks", dir.file_name().unwrap().display()));
        let _ = std::fs::remove_dir_all(&marks);
        std::fs::create_dir_all(&marks).unwrap();
        let m = marks.display().to_string().replace('\\', "/");
        let script = |what: &str, tail: &str| format!("#!/bin/sh\necho x >> '{m}/{what}'\n{tail}");
        let fsm = marks.join("fsmonitor.sh");
        std::fs::write(&fsm, script("fsmonitor", "exit 1\n")).unwrap();
        std::fs::write(marks.join("clean.sh"), script("filter", "cat\n")).unwrap();
        let hook = dir.join(".git/hooks/post-index-change");
        std::fs::write(&hook, script("hook", "")).unwrap();
        #[cfg(unix)]
        for p in [&fsm, &hook] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let raw = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        raw(&[
            "config",
            "core.fsmonitor",
            &fsm.display().to_string().replace('\\', "/"),
        ]);
        raw(&[
            "config",
            "filter.planted.clean",
            &format!("sh '{m}/clean.sh'"),
        ]);
        raw(&["config", "filter.planted.required", "true"]);
        std::fs::write(dir.join(".gitattributes"), "*.py filter=planted\n").unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        std::fs::write(dir.join("b.py"), "y = 1\n").unwrap();
        (dir, marks)
    }

    fn marked(marks: &Path) -> Vec<String> {
        ["fsmonitor", "filter", "hook"]
            .into_iter()
            .filter(|w| marks.join(w).exists())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn trellis_s_own_git_runs_no_planted_fsmonitor_hook_or_filter() {
        // The risk (review round 9, finding 4): the git guard puts `.git/`
        // back only after the loop, and the digest and fingerprint trellis
        // takes around every hooked call ran `git status`, `hash-object`
        // and `add -A` in between, which run `core.fsmonitor`, a
        // `post-index-change` hook and the repository's clean filters: a
        // `bash` call could plant any of them and have trellis run it.
        let (dir, marks) = planted_repo("planted");
        worktree_digest(&dir).unwrap();
        worktree_fingerprint(&dir).unwrap();
        assert_eq!(marked(&marks), Vec::<String>::new(), "trellis ran it");
        // Every other git call carries the fsmonitor and hook overrides;
        // filters stay on there (a rollback's checkout needs Git LFS's).
        pending(&dir).unwrap();
        assert!(!marks.join("fsmonitor").exists() && !marks.join("hook").exists());
        let _ = std::fs::remove_file(marks.join("filter"));

        // The control: the same repository, asked by plain git, runs all
        // three, so the assertion above is about the overrides.
        let idx = marks.join("control.index");
        std::fs::copy(dir.join(".git/index"), &idx).unwrap();
        for args in [&["status", "--porcelain"][..], &["add", "-A"][..]] {
            let _ = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_INDEX_FILE", &idx)
                .output()
                .unwrap();
        }
        assert_eq!(marked(&marks), ["fsmonitor", "filter", "hook"], "control");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&marks);
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
    fn links_hashed_in_one_batch_get_the_ids_one_process_each_gave() {
        // A guard, for the per-call cost cut: one `hash-object` for every
        // symlink, where there had been one per link. The ids must be what
        // they were, or every digest with a link in it would move once and
        // void a pass for nothing.
        let dir = temp_repo("digest-links-batch");
        std::fs::create_dir_all(dir.join("d")).unwrap();
        std::fs::write(dir.join("f.py"), "x = 1\n").unwrap();
        let Some(()) = symlink(Path::new("d"), &dir.join("to-dir"), true) else {
            return;
        };
        symlink(Path::new("f.py"), &dir.join("to-file"), false).unwrap();
        symlink(Path::new("missing/x"), &dir.join("dangling"), false).unwrap();
        let names: Vec<String> = ["to-dir", "to-file", "dangling"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let refs: Vec<&String> = names.iter().collect();
        let batch = hash_links(&dir, &refs).unwrap();
        let one_each: Vec<String> = names.iter().map(|n| hash_link(&dir, n).unwrap()).collect();
        assert_eq!(batch, one_each);
        assert_eq!(hash_links(&dir, &[]).unwrap(), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_same_size_edit_in_the_index_s_own_second_is_not_read_as_unchanged() {
        // The risk, seen on Linux CI: the fingerprint copies the index, and
        // the copy's fresh mtime defeated git's racy-clean check. An edit of
        // the same size landing in the index's own second (`return 1` to
        // `return 2`, as fast tests do) then matched its stat entry and was
        // hashed as the old content, so the guard saw a stale tree and
        // demoted converged runs at random.
        //
        // Set up exactly that: the entry's mtime equals the index's, the
        // size is unchanged, and ctime is not trusted (as when it too falls
        // in the same second). Git itself re-reads such a file.
        let dir = temp_repo("racy");
        git(&dir, &["config", "core.trustctime", "false"]).unwrap();
        let a = dir.join("a.py");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        let touch = |p: &Path, t: std::time::SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        touch(&a, old);
        // Record that mtime in the index, written now: not racy yet.
        git(&dir, &["update-index", "--refresh"]).unwrap();
        let before = worktree_fingerprint(&dir).unwrap();

        std::fs::write(&a, "def f():\n    return 2\n").unwrap();
        touch(&a, old);
        let index = PathBuf::from(
            git(&dir, &["rev-parse", "--git-path", "index"])
                .unwrap()
                .trim(),
        );
        touch(&dir.join(index), old);
        assert_ne!(
            before,
            worktree_fingerprint(&dir).unwrap(),
            "a same-size edit in the index's second must still change the fingerprint"
        );
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

    fn symbolic_head(dir: &Path) -> String {
        git(dir, &["rev-parse", "--symbolic-full-name", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }

    fn sha_of(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).unwrap().trim().to_string()
    }

    #[test]
    fn rollback_after_a_branch_switch_puts_the_original_branch_back_at_the_snapshot() {
        // The risk: a writer commits on the user's branch and then switches
        // to a new one. `reset --hard <snapshot>` alone moved the writer's
        // branch, left the user's holding the writer's commit, and
        // `is_restored` said yes because HEAD's sha matched (review round 10).
        let dir = temp_repo("branch-switch");
        let snap = snapshot(&dir).unwrap();
        let original = symbolic_head(&dir);
        assert!(original.starts_with("refs/heads/"), "{original}");

        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        git(&dir, &["commit", "-qam", "on the user's branch"]).unwrap();
        git(&dir, &["checkout", "-qb", "elsewhere"]).unwrap();
        std::fs::write(dir.join("b.py"), "x = 1\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "on the writer's branch"]).unwrap();
        assert!(snap.head_moved().unwrap().is_some());

        snap.rollback().unwrap();

        assert_eq!(symbolic_head(&dir), original, "HEAD is back on the branch");
        assert_eq!(
            sha_of(&dir, &original),
            snap.head,
            "the user's branch no longer holds the writer's commit"
        );
        assert!(snap.is_restored().unwrap());
        assert!(snap.head_moved().unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_restored_is_false_on_another_branch_at_the_same_commit() {
        // Same sha, different branch: the next commit would land on the
        // writer's branch, so it is not the state the snapshot recorded.
        let dir = temp_repo("branch-same-sha");
        let snap = snapshot(&dir).unwrap();
        git(&dir, &["checkout", "-qb", "elsewhere"]).unwrap();
        assert!(!snap.is_restored().unwrap());
        assert!(snap.head_moved().unwrap().is_some());
        snap.rollback().unwrap();
        assert!(snap.is_restored().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_detached_snapshot_is_rolled_back_detached_and_moves_no_branch() {
        // Detached at the snapshot; the writer checks out a branch and
        // commits. `reset --hard` alone would have moved that branch.
        let dir = temp_repo("detached");
        let first = sha_of(&dir, "HEAD");
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        git(&dir, &["commit", "-qam", "second"]).unwrap();
        let branch = symbolic_head(&dir);
        let tip = sha_of(&dir, "HEAD");
        git(&dir, &["checkout", "-q", "--detach", &first]).unwrap();
        let snap = snapshot(&dir).unwrap();

        git(&dir, &["checkout", "-q", &branch["refs/heads/".len()..]]).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 3\n").unwrap();
        git(&dir, &["commit", "-qam", "third"]).unwrap();
        let moved_tip = sha_of(&dir, "HEAD");

        snap.rollback().unwrap();
        assert_eq!(symbolic_head(&dir), "HEAD", "detached again");
        assert_eq!(sha_of(&dir, "HEAD"), first);
        assert_eq!(
            sha_of(&dir, &branch),
            moved_tip,
            "a branch the snapshot was not on is not reset by the rollback"
        );
        assert_ne!(moved_tip, tip);
        assert!(snap.is_restored().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_after_an_orphan_checkout_puts_the_branch_back_and_restores_the_tree() {
        // The risk (review round 11): a writer's `git checkout --orphan x`
        // leaves HEAD on a branch with no commit. `rev-parse
        // --symbolic-full-name HEAD` fails there, `put_head_back` returned
        // that error, and `rollback` returned before `reset --hard` and
        // `clean -fd`: nothing was restored, on the one path that has to
        // work.
        let dir = temp_repo("orphan");
        let snap = snapshot(&dir).unwrap();
        let original = symbolic_head(&dir);

        git(&dir, &["checkout", "-q", "--orphan", "elsewhere"]).unwrap();
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        std::fs::write(dir.join("planted.py"), "x = 1\n").unwrap();
        let moved = snap.head_moved().unwrap();
        assert!(
            moved.as_deref().is_some_and(|m| m.contains("no commit")),
            "an unborn HEAD is a move, not an error: {moved:?}"
        );
        assert!(!snap.is_restored().unwrap());

        snap.rollback().unwrap();

        assert_eq!(symbolic_head(&dir), original, "HEAD is back on the branch");
        assert!(snap.is_restored().unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n"
        );
        assert!(
            !dir.join("planted.py").exists(),
            "untracked file left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_head_that_cannot_be_put_back_still_restores_the_tree_and_reports_it() {
        // The method's doc says every step is attempted. A HEAD-restore
        // failure returned before the tree was touched (review round 11).
        // Here the recorded branch is a name `symbolic-ref` refuses, so
        // putting HEAD back fails for certain; the tree must still be put
        // back, without moving the branch HEAD is on now, and the failure
        // reported.
        let dir = temp_repo("head-unrestorable");
        let mut snap = snapshot(&dir).unwrap();
        let original = symbolic_head(&dir);
        snap.branch = Some("refs/heads/..bad".into());

        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        std::fs::write(dir.join("planted.py"), "x = 1\n").unwrap();

        let e = snap.rollback().expect_err("HEAD could not be put back");
        let msg = e.to_string();
        assert!(msg.contains("HEAD could not be put back"), "{msg}");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.py")).unwrap(),
            "def f():\n    return 1\n",
            "the tracked edit is undone anyway"
        );
        assert!(
            !dir.join("planted.py").exists(),
            "the new file is removed anyway"
        );
        assert_eq!(symbolic_head(&dir), original);
        assert_eq!(sha_of(&dir, &original), snap.head, "no branch moved");
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
