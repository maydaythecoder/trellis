//! Running a writer trellis does not control in a throwaway clone
//! (ADR 0032): copy-in/diff-out.
//!
//! `bash` and custom tools used to run in the repository itself, with
//! write access to the whole worktree and to `.git/`, and eleven review
//! rounds found about fifteen ways such a writer could put a change where
//! the rollback's git checks do not look (ADR 0023 §5). Here the command
//! runs in a `git clone --no-hardlinks` of the repository in the system's
//! temporary directory instead. What trellis would show a check (the
//! tracked files and the untracked ones git does not ignore) is copied in
//! first, byte for byte, so the command sees the run's earlier edits. After
//! it exits, the files are compared with what was copied in, by reading
//! them, and a changed file goes back to the repository only through the
//! orchestrator's own write path: the same refusals as `edit`, the
//! repository's `deny_write`, recorded in `touched`, so the rollback
//! covers it exactly. Nothing else the command did leaves the clone: not
//! its `.git/` writes, not its index flags, not its `.gitignore`.
//!
//! This isolates a writer that edits where it stands. It does not isolate
//! one that looks for the repository by absolute path, reads the home
//! directory or opens a socket; the ADR lists what it does not do.

use crate::path_rules::{names_git_dir, real_rel, unsafe_repo_path};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Variables that point git at a repository. Inherited, they would send a
/// `git` the command runs in the clone, and trellis's own `git` calls in
/// this module, to the real repository, so both have them removed.
pub(crate) const GIT_LOCATION_VARS: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// The largest file carried back. A command can write gigabytes into a
/// tracked file, and carrying it back is reading it whole into memory and
/// writing it into the repository; a file larger than this is reported
/// and left in the clone.
pub(crate) const CARRY_CAP: u64 = 16 << 20;

/// File names git reads as instructions about other files: what it ignores
/// (and so what a rollback's `git clean` leaves), which filter a file goes
/// through, and where submodules are. A change to one is never carried
/// back, since it changes what the rollback reaches rather than what the
/// code does (ADR 0023 §5, "ignored-path tricks").
const GIT_INSTRUCTION_FILES: [&str; 3] = [".gitignore", ".gitattributes", ".gitmodules"];

/// Why a command could not be isolated. It is then not run: running it in
/// the repository instead would be the thing this module exists to stop.
#[derive(Debug, thiserror::Error)]
pub(crate) enum IsolateError {
    #[error("the {what} command of git could not run: {source}")]
    Spawn {
        what: String,
        source: std::io::Error,
    },
    #[error("the {what} command of git failed: {stderr}")]
    Git { what: String, stderr: String },
    #[error("the clone holds commit {clone} where the repository holds {host}")]
    Head { host: String, clone: String },
    #[error("{0}")]
    Io(String),
}

/// A clone prepared for one command, and what was copied into it.
///
/// Removed when dropped, whatever happened in between.
pub(crate) struct Sandbox {
    dir: PathBuf,
    /// Every regular file copied in, with the sha256 of the bytes it was
    /// given. The diff-out compares against these and nothing else.
    baseline: Vec<(String, [u8; 32])>,
}

/// What the command changed, as trellis will act on it.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    /// A file copied in whose bytes differ now, and the sha256 it was
    /// copied in with, which the repository's copy must still have for the
    /// change to be carried back. The bytes are read again, one file at a
    /// time, by [`Sandbox::read_changed`]: holding them all here was memory
    /// in proportion to what the command wrote.
    pub(crate) modified: Vec<(String, [u8; 32])>,
    /// Copied in, and gone from the clone. Not carried back.
    pub(crate) deleted: Vec<String>,
    /// Changed in a way that is not carried back, and why.
    pub(crate) refused: Vec<(String, String)>,
}

impl Sandbox {
    /// Clones `host`, strips what names it, and copies in its files.
    ///
    /// Blocking: git processes and a read of every tracked file.
    pub(crate) fn prepare(host: &Path) -> Result<Sandbox, IsolateError> {
        let host = std::path::absolute(host).map_err(|e| IsolateError::Io(e.to_string()))?;
        let files = listed(&host)?;
        let host_head = head_commit(&host)?;
        let dir = fresh_dir()?;
        // Owned from here, so an error below still removes it.
        let mut sb = Sandbox {
            dir,
            baseline: Vec::new(),
        };
        // `--no-hardlinks`: a hard-linked object is one file with two names,
        // and a write through the clone's would change the repository's.
        // Through `trellis_vcs::command()`, whose hooks path holds no hook,
        // so no template hook runs on the clone's checkout.
        let src = host.as_os_str().to_owned();
        git_in(
            &std::env::temp_dir(),
            &[
                "clone".into(),
                "--no-hardlinks".into(),
                "--quiet".into(),
                "--".into(),
                src,
                sb.dir.as_os_str().to_owned(),
            ],
        )?;
        let clone_head = head_commit(&sb.dir)?;
        if clone_head != host_head {
            return Err(IsolateError::Head {
                host: host_head.unwrap_or_else(|| "no commit".into()),
                clone: clone_head.unwrap_or_else(|| "no commit".into()),
            });
        }
        // The remote and the reflog both hold the repository's path, which
        // is all a command would need to write there instead. Removing them
        // hides the path from a casual `git remote -v`; it does not stop a
        // command that finds it another way (ADR 0032).
        git_in(
            &sb.dir,
            &["remote".into(), "remove".into(), "origin".into()],
        )?;
        let logs = sb.dir.join(".git").join("logs");
        if logs.exists() {
            std::fs::remove_dir_all(&logs)
                .map_err(|e| IsolateError::Io(format!("cannot remove the clone's reflog: {e}")))?;
        }
        sb.copy_in(&host, &files)?;
        Ok(sb)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }

    /// Brings every file copied in back to the repository's bytes as they
    /// are now, and takes those as the new baseline: what the hooks of one
    /// call share (ADR 0032, "Hooks"). Between two hooks the call's tool,
    /// and the carry-back of the hook before, wrote into the repository,
    /// and `post_tool` is there to look at what the tool did.
    ///
    /// Filesystem only in the clone, like `copy_in`: `ls-files` runs in the
    /// repository, and no `git` runs in a clone a command has had. A change
    /// the last hook made and the carry-back refused is overwritten, so no
    /// hook sees another's refused edit as the repository's.
    ///
    /// Blocking. `Err` leaves the clone in an unknown state; the caller
    /// drops it and prepares another.
    pub(crate) fn refresh(&mut self, host: &Path) -> Result<(), IsolateError> {
        let host = std::path::absolute(host).map_err(|e| IsolateError::Io(e.to_string()))?;
        let files = listed(&host)?;
        self.baseline.clear();
        self.copy_in(&host, &files)
    }

    /// Every file copied in, with the sha256 it was given: a process
    /// agent's carry-back (`agent_clone`) tells a new file from an edited
    /// one by it, and checks a deletion against it.
    pub(crate) fn copied_in(&self) -> &[(String, [u8; 32])] {
        &self.baseline
    }

    /// Writes each listed file's bytes from `host` into the clone, where
    /// the checkout left different ones or none, and records what it was
    /// given. A file that is not a regular file in `host` (a link, a
    /// submodule) is left as the checkout made it and is never carried
    /// back.
    fn copy_in(&mut self, host: &Path, files: &[String]) -> Result<(), IsolateError> {
        for rel in files {
            let from = host.join(rel);
            let to = self.dir.join(rel);
            // Written only where the path really lands inside the clone. A
            // tracked link to a directory outside it (`lib -> /etc`) would
            // otherwise take this write, or `create_dir_all`, out of it.
            if let Err(why) = inside(&self.dir, rel) {
                return Err(IsolateError::Io(why));
            }
            match std::fs::symlink_metadata(&from) {
                Ok(m) if m.is_file() => {}
                // Deleted in the worktree: deleted in the clone too, so the
                // command sees what a check would.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if std::fs::symlink_metadata(&to).is_ok() {
                        remove_any(&to).map_err(|e| io(rel, e))?;
                    }
                    continue;
                }
                // A link, a directory (a submodule): left as the checkout
                // made it, and never carried back.
                _ => continue,
            }
            let want = hash_file(&from).map_err(|e| io(rel, e))?;
            let have = match std::fs::symlink_metadata(&to) {
                Ok(m) if m.is_file() => Some(hash_file(&to).map_err(|e| io(rel, e))?),
                Ok(_) => {
                    // A link or directory where the repository has a file:
                    // replaced, so the write below cannot follow it.
                    remove_any(&to).map_err(|e| io(rel, e))?;
                    None
                }
                Err(_) => None,
            };
            if have != Some(want) {
                if let Some(dir) = to.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| io(rel, e))?;
                }
                std::fs::copy(&from, &to).map_err(|e| io(rel, e))?;
            }
            self.baseline.push((rel.clone(), want));
        }
        Ok(())
    }

    /// What the command changed among the files copied in, read from the
    /// filesystem. No `git` runs in the clone after the command: its
    /// `.git/config` is the command's by then, and a filter driver or
    /// `core.fsmonitor` planted there is a program git would run.
    pub(crate) fn changes(&self) -> Changes {
        let mut out = Changes::default();
        for (rel, before) in &self.baseline {
            let bytes = match self.read_regular(rel) {
                Ok(Some(b)) => b,
                Ok(None) => {
                    out.deleted.push(rel.clone());
                    continue;
                }
                Err(why) => {
                    out.refused.push((rel.clone(), why));
                    continue;
                }
            };
            let now: [u8; 32] = Sha256::digest(&bytes).into();
            if now == *before {
                continue;
            }
            if is_git_instruction_file(rel) {
                out.refused.push((
                    rel.clone(),
                    "tells git what to ignore or filter, which decides what a rollback \
                     reaches; change it with `edit`"
                        .into(),
                ));
                continue;
            }
            out.modified.push((rel.clone(), *before));
        }
        out
    }

    /// The bytes of a file [`changes`](Self::changes) listed as modified,
    /// read again under the same rules, or why they cannot be carried back.
    pub(crate) fn read_changed(&self, rel: &str) -> Result<Vec<u8>, String> {
        match self.read_regular(rel) {
            Ok(Some(b)) => Ok(b),
            Ok(None) => Err("was deleted in the clone after the command exited".into()),
            Err(why) => Err(why),
        }
    }

    /// `rel`'s bytes in the clone: `Ok(None)` when it is gone, `Err` when
    /// it may not be carried back. Never through a link, nor through a
    /// linked directory out of the clone: what a link points at is not the
    /// file git tracks, and `a.py -> ~/.ssh/id_rsa` read through would
    /// carry a key into a tracked file by accident. A command that copies
    /// such a file's bytes on purpose can; that is the "reads outside the
    /// clone" limit (ADR 0032), not something this refusal closes.
    fn read_regular(&self, rel: &str) -> Result<Option<Vec<u8>>, String> {
        inside(&self.dir, rel)?;
        let at = self.dir.join(rel);
        let meta = match std::fs::symlink_metadata(&at) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("could not be read: {e}")),
        };
        if !meta.is_file() {
            return Err("is no longer a regular file".into());
        }
        if meta.len() > CARRY_CAP {
            return Err(format!(
                "is {} bytes, over the {CARRY_CAP} carried back",
                meta.len()
            ));
        }
        read_capped(&at)
            .map(Some)
            .map_err(|e| format!("could not be read: {e}"))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best effort: a process the command left running may still hold a
        // file open (on Windows, that blocks the delete). Nothing in the
        // repository depends on this; a leftover clone is disk, not state.
        if let Err(e) = remove_tree(&self.dir) {
            tracing::warn!(
                dir = %self.dir.display(),
                "a throwaway clone could not be removed: {e}"
            );
        }
    }
}

/// [`Sandbox::prepare`] off the async runtime, with the failure as the
/// sentence a caller reports.
pub(crate) async fn prepare(host: &Path) -> Result<Sandbox, String> {
    let host = host.to_path_buf();
    tokio::task::spawn_blocking(move || Sandbox::prepare(&host))
        .await
        .map_err(|e| format!("preparing its throwaway clone failed: {e}"))?
        .map_err(|e| format!("its throwaway clone could not be prepared: {e}"))
}

/// Drops a clone on a blocking thread: removing it walks the whole tree,
/// which on the runtime's own thread stalls every other task.
pub(crate) async fn dispose(sandbox: Sandbox) {
    let _ = tokio::task::spawn_blocking(move || drop(sandbox)).await;
}

/// Whether `rel` is one of [`GIT_INSTRUCTION_FILES`], by any spelling
/// Windows would open as one.
pub(crate) fn is_git_instruction_file(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    GIT_INSTRUCTION_FILES.contains(&name.as_str())
}

/// Where `rel` really lands under `root` is inside it: `Ok` or why not.
///
/// The directory it is in is resolved, through any link; the last
/// component is not, because no caller follows it: a link there is read as
/// a link (`symlink_metadata`) and refused or removed, never opened.
fn inside(root: &Path, rel: &str) -> Result<(), String> {
    let parent = match rel.rfind(['/', '\\']) {
        Some(i) => &rel[..i],
        None => return Ok(()),
    };
    real_rel(root, parent)
        .map(|_| ())
        .map_err(|_| format!("`{rel}` leads out of the clone through a link"))
}

fn io(rel: &str, e: std::io::Error) -> IsolateError {
    IsolateError::Io(format!("`{rel}` could not be copied into the clone: {e}"))
}

/// A new, empty directory under the system's temporary directory, which
/// only this user can enter (`0700` on Unix; Windows gives each user a
/// temporary directory of their own), since the repository's source is
/// about to be copied into it. A new directory, not `create_dir_all`: one
/// someone made there first, under the name picked, is refused rather
/// than used.
fn fresh_dir() -> Result<PathBuf, IsolateError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut last = None;
    for _ in 0..8 {
        let name = format!(
            "trellis-clone-{}-{}-{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let dir = base.join(name);
        #[cfg(unix)]
        let builder = {
            let mut b = std::fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
            b
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = Some(e),
        }
    }
    Err(IsolateError::Io(format!(
        "no directory for a clone could be made under {}: {}",
        base.display(),
        last.map_or_else(String::new, |e| e.to_string())
    )))
}

/// The files trellis would carry into a change: what git tracks, and the
/// untracked files it does not ignore (a `write_file` earlier in the run).
/// `ls-files` reads the index and writes nothing, unlike `git status`,
/// which may refresh it.
fn listed(host: &Path) -> Result<Vec<String>, IsolateError> {
    let out = git_in(
        host,
        &[
            "ls-files".into(),
            "-z".into(),
            "--cached".into(),
            "--others".into(),
            "--exclude-standard".into(),
        ],
    )?;
    let mut files: Vec<String> = Vec::new();
    for raw in out.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let Ok(rel) = std::str::from_utf8(raw) else {
            // Not a path `edit` could name either.
            continue;
        };
        // Git refuses such paths in an index; a listing is still read as
        // data from the repository.
        if unsafe_repo_path(rel).is_some() || names_git_dir(rel) {
            continue;
        }
        files.push(rel.to_string());
    }
    // An unmerged path is listed once per stage.
    files.sort();
    files.dedup();
    Ok(files)
}

/// The commit `HEAD` names in `dir`, `None` on an unborn branch.
fn head_commit(dir: &Path) -> Result<Option<String>, IsolateError> {
    let args: [std::ffi::OsString; 4] = [
        "rev-parse".into(),
        "-q".into(),
        "--verify".into(),
        "HEAD^{commit}".into(),
    ];
    let mut c = git(dir, &args);
    let out = c.output().map_err(|source| IsolateError::Spawn {
        what: "rev-parse".into(),
        source,
    })?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(IsolateError::Git {
            what: "rev-parse".into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }),
    }
}

/// `git` from `trellis_vcs` (overrides, scrubbed keys), in `dir`, with no
/// variable that would point it at another repository.
fn git(dir: &Path, args: &[std::ffi::OsString]) -> std::process::Command {
    let mut c = trellis_vcs::command();
    c.args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    for v in GIT_LOCATION_VARS {
        c.env_remove(v);
    }
    c
}

/// Runs [`git`], its stdout, or the failure.
fn git_in(dir: &Path, args: &[std::ffi::OsString]) -> Result<Vec<u8>, IsolateError> {
    let what = args
        .first()
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = git(dir, args)
        .output()
        .map_err(|source| IsolateError::Spawn {
            what: what.clone(),
            source,
        })?;
    if !out.status.success() {
        return Err(IsolateError::Git {
            what,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(out.stdout)
}

pub(crate) fn hash_file(p: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

/// The file's bytes, refusing past [`CARRY_CAP`] even when it grew after
/// its size was read.
fn read_capped(p: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(p)?
        .take(CARRY_CAP + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > CARRY_CAP {
        return Err(std::io::Error::other(format!(
            "grew past the {CARRY_CAP} bytes carried back while it was read"
        )));
    }
    Ok(bytes)
}

/// Removes a file, link or directory without following a link.
fn remove_any(p: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if m.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        // A link to a directory on Windows is removed as a directory.
        std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p))
    }
}

/// `remove_dir_all`, tried twice: on Windows a process that has just
/// exited can hold a handle for a moment after.
fn remove_tree(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::remove_dir_all(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::ToolCall;
    use crate::exec::shell_argv;
    use crate::orchestrator::{Handle, Orchestrator};
    use crate::registry::{BashSettings, ToolSelection, ToolSet};
    use crate::tools::ToolResult;
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::time::Duration;
    use trellis_checks::CheckConfig;

    fn git(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be on PATH to run these tests");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A committed repository: a Python file, a `.gitignore`, and a
    /// tracked `.trellis/config.toml`. A copy of one built once per process
    /// (`testutil`); each writer still gets its own clone of the copy.
    fn committed() -> tempfile::TempDir {
        static T: std::sync::OnceLock<crate::testutil::Template> = std::sync::OnceLock::new();
        T.get_or_init(|| {
            crate::testutil::Template::build(|root| {
                std::fs::create_dir_all(root.join("app")).unwrap();
                std::fs::create_dir_all(root.join(".trellis")).unwrap();
                std::fs::write(root.join("app/util.py"), "def helper():\n    return 1\n").unwrap();
                std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
                std::fs::write(root.join(".trellis/config.toml"), "[checks]\n").unwrap();
                for args in [
                    ["init", "-q"].as_slice(),
                    &["config", "user.email", "t@example.invalid"],
                    &["config", "user.name", "t"],
                    &["config", "core.autocrlf", "false"],
                    &["add", "-A"],
                    &["commit", "-qm", "fixture"],
                ] {
                    crate::testutil::git(root, args);
                }
            })
        })
        .instance()
    }

    fn with_bash(root: &Path) -> Handle {
        with_bash_under(root, crate::os_sandbox::OsSandbox::detected().clone())
    }

    async fn bash(orch: &Handle, command: &str) -> (String, bool) {
        let d = orch
            .call(&ToolCall::Bash {
                command: command.into(),
            })
            .await;
        match d.result {
            ToolResult::Output(s) => (s, d.may_have_changed_tree),
            other => panic!("bash did not run: {other:?}"),
        }
    }

    /// Every file under `.git/`, by path and content.
    fn git_dir_digest(root: &Path) -> String {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                    out.push((rel, std::fs::read(&p).unwrap()));
                }
            }
        }
        let base = root.join(".git");
        let mut all = Vec::new();
        walk(&base, &base, &mut all);
        all.sort();
        let mut h = Sha256::new();
        for (rel, bytes) in &all {
            h.update(rel.as_bytes());
            h.update([0]);
            h.update(Sha256::digest(bytes));
        }
        format!("{} files, {:x}", all.len(), h.finalize())
    }

    #[tokio::test]
    async fn isolated_clone_keeps_gitignore_skip_worktree_and_git_dir_writes_out_of_the_repository()
    {
        // The risk (ADR 0023 §5, rounds 5-11): a command run in the
        // repository could hide a change from every git-based check by
        // editing `.gitignore`, flagging a file `skip-worktree`, or writing
        // into `.git/` (a hook path, a planted file). In the clone each of
        // those lands in the clone, and none of them is carried back.
        let d = committed();
        let root = d.path();
        // Whatever `git status` refreshes in the index, refreshed now, so
        // the bytes compared below are the ones the command could change.
        git(root, &["status", "--porcelain"]);
        let gitignore = std::fs::read(root.join(".gitignore")).unwrap();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let git_dir = git_dir_digest(root);

        let orch = with_bash(root);
        let command = if cfg!(windows) {
            "echo *.py>> .gitignore& git update-index --skip-worktree app/util.py& \
             echo planted> .git\\planted& git config core.hooksPath evil& \
             git ls-files -v app/util.py"
        } else {
            "echo '*.py' >> .gitignore; git update-index --skip-worktree app/util.py; \
             echo planted > .git/planted; git config core.hooksPath evil; \
             git ls-files -v app/util.py"
        };
        let (said, _) = bash(&orch, command).await;
        // The command did all of it, where it ran: `S` is skip-worktree.
        assert!(said.contains("S app/util.py"), "{said}");
        assert!(
            said.contains("not carried back: `.gitignore`"),
            "the `.gitignore` edit is refused, and said to be: {said}"
        );

        assert_eq!(std::fs::read(root.join(".gitignore")).unwrap(), gitignore);
        assert_eq!(
            std::fs::read(root.join(".git/index")).unwrap(),
            index,
            "the index (skip-worktree lives there) is unchanged"
        );
        assert_eq!(git_dir_digest(root), git_dir, "nothing under .git/ changed");
        assert!(!root.join(".git/planted").exists());
        assert_eq!(git(root, &["status", "--porcelain"]), "");
        assert_eq!(
            git(root, &["ls-files", "-v", "app/util.py"]).trim(),
            "H app/util.py"
        );
        assert!(orch.touched().await.is_empty(), "nothing was written");
    }

    #[tokio::test]
    async fn isolated_clone_carries_a_tracked_file_edit_back_through_the_edit_path() {
        // The risk: an isolation that dropped the command's work, or that
        // carried it back around trellis's own write path, where `touched`
        // (the rollback and the trail) and the `.trellis/` refusal live.
        let d = committed();
        let root = d.path();
        let orch = with_bash(root);
        let command = if cfg!(windows) {
            "echo def helper(): return 3 > app\\util.py& echo x> made.txt& \
             echo [hooks]> .trellis\\config.toml"
        } else {
            "echo 'def helper(): return 3' > app/util.py; echo x > made.txt; \
             echo '[hooks]' > .trellis/config.toml"
        };
        let (said, may_have_changed) = bash(&orch, command).await;
        let now = std::fs::read_to_string(root.join("app/util.py")).unwrap();
        assert!(now.contains("return 3"), "{now}\n{said}");
        assert!(
            said.contains("carried back from the clone: app/util.py"),
            "{said}"
        );
        assert_eq!(orch.touched().await, ["app/util.py"]);
        assert!(may_have_changed, "a pass before this call stops counting");
        // Only what trellis tracks comes back, and never `.trellis/`.
        assert!(!root.join("made.txt").exists(), "{said}");
        assert_eq!(
            std::fs::read_to_string(root.join(".trellis/config.toml")).unwrap(),
            "[checks]\n"
        );
        assert!(
            said.contains("`.trellis/config.toml` is under .trellis/"),
            "{said}"
        );
        // And the graph followed the file.
        let r = orch.blast_radius("helper").await;
        assert!(matches!(r, ToolResult::Radius(_)), "{r:?}");
    }

    #[tokio::test]
    async fn allow_write_is_enforced_where_a_command_s_clone_is_carried_back() {
        // The risk (#182): `allow_write` checked at the `edit` gate only, so
        // a `bash` command, whose writes reach the repository through the
        // carry-back and not through an `edit` call, wrote outside it.
        let d = committed();
        let root = d.path();
        let mut shell = shell_argv("");
        shell.pop();
        let tools = ToolSet::build(ToolSelection {
            bash: Some(BashSettings {
                shell,
                timeout: Duration::from_secs(60),
                allow: Vec::new(),
                unattended: true,
            }),
            ..Default::default()
        })
        .unwrap();
        let orch = Orchestrator::new(root, CheckConfig::default())
            .with_governance(
                crate::governance::Governance::new(&[]).with_allow_write("docs", &["docs/".into()]),
            )
            .with_tools(tools)
            .spawn()
            .0;
        let command = if cfg!(windows) {
            "echo def helper(): return 3 > app\\util.py"
        } else {
            "echo 'def helper(): return 3' > app/util.py"
        };
        let (said, _) = bash(&orch, command).await;
        assert!(
            said.contains(
                "`app/util.py` is outside the allow_write [`docs/`] of agent `docs` \
                 in .trellis/agents/docs.toml"
            ),
            "the refusal is named in the output: {said}"
        );
        let now = std::fs::read_to_string(root.join("app/util.py")).unwrap();
        assert!(!now.contains("return 3"), "not carried back: {now}");
        assert!(orch.touched().await.is_empty());
    }

    #[tokio::test]
    async fn vault_notes_root_is_enforced_where_a_command_s_clone_is_carried_back() {
        // The risk (ADR 0049 §2-3): the vault refusal checked at the `edit`
        // gate only, so a `bash` command, whose writes reach the repository
        // through the carry-back, rewrote a note: the model writing its own
        // future context (ADR 0022 §3).
        let d = committed();
        let root = d.path();
        std::fs::create_dir_all(root.join("vault")).unwrap();
        std::fs::write(root.join("vault/a.md"), "# a\n\nwritten by a person\n").unwrap();
        git(root, &["add", "vault/a.md"]);
        git(root, &["commit", "-qm", "a note"]);
        let committed_note = std::fs::read(root.join("vault/a.md")).unwrap();
        let mut shell = shell_argv("");
        shell.pop();
        let tools = ToolSet::build(ToolSelection {
            bash: Some(BashSettings {
                shell,
                timeout: Duration::from_secs(60),
                allow: Vec::new(),
                unattended: true,
            }),
            ..Default::default()
        })
        .unwrap();
        let orch = Orchestrator::new(root, CheckConfig::default())
            .with_governance(crate::governance::Governance::new(&[]).with_vault("vault", "vault/"))
            .with_tools(tools)
            .spawn()
            .0;
        let command = if cfg!(windows) {
            "echo written by the model> vault\\a.md"
        } else {
            "echo 'written by the model' > vault/a.md"
        };
        let (said, _) = bash(&orch, command).await;
        assert!(
            said.contains("[context] vault = \"vault\""),
            "the refusal names the setting: {said}"
        );
        assert_eq!(
            std::fs::read(root.join("vault/a.md")).unwrap(),
            committed_note,
            "not carried back: {said}"
        );
        assert!(orch.touched().await.is_empty());
    }

    #[tokio::test]
    async fn isolated_clone_edit_carried_back_is_undone_by_rollback() {
        // The point of carrying back through the edit path: the change is
        // an ordinary edit to a tracked file, which ADR 0009's rollback
        // restores exactly.
        let d = committed();
        let root = d.path();
        let snap = trellis_vcs::snapshot(root).unwrap();
        let orch = with_bash(root);
        let command = if cfg!(windows) {
            "echo changed> app\\util.py"
        } else {
            "echo changed > app/util.py"
        };
        bash(&orch, command).await;
        assert!(
            std::fs::read_to_string(root.join("app/util.py"))
                .unwrap()
                .starts_with("changed")
        );
        snap.rollback().unwrap();
        assert!(snap.is_restored().unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("app/util.py")).unwrap(),
            "def helper():\n    return 1\n"
        );
    }

    #[tokio::test]
    async fn a_command_in_the_clone_sees_the_run_s_earlier_edits() {
        // The risk: a clone of the commit alone, where a command (a test
        // run) checks the code as it was before this run changed it.
        let d = committed();
        let root = d.path();
        let orch = with_bash(root);
        let e = orch.edit("app/util.py", "return 1", "return 2").await;
        assert!(matches!(e, ToolResult::Edit(_)), "{e:?}");
        let (said, _) = bash(
            &orch,
            if cfg!(windows) {
                "type app\\util.py"
            } else {
                "cat app/util.py"
            },
        )
        .await;
        assert!(said.contains("return 2"), "{said}");
        // Nothing to carry back: the clone's copy is the repository's.
        assert!(!said.contains("carried back from"), "{said}");
    }

    #[tokio::test]
    async fn a_clone_is_removed_after_the_call_and_holds_no_path_to_the_repository() {
        // The remote and the reflog name the repository, which is all a
        // command needs to write there instead of in the clone.
        let d = committed();
        let root = d.path();
        let orch = with_bash(root);
        let command = if cfg!(windows) {
            "cd& git remote -v& dir /b .git"
        } else {
            "pwd; git remote -v; ls .git"
        };
        let (said, _) = bash(&orch, command).await;
        let first = said.lines().nth(1).unwrap_or_default().trim().to_string();
        assert!(first.contains("trellis-clone-"), "{said}");
        assert!(!said.contains("origin"), "{said}");
        assert!(!said.lines().any(|l| l.trim() == "logs"), "{said}");
        assert!(
            !Path::new(&first).exists(),
            "the clone at {first} outlived the call"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tracked_file_replaced_by_a_link_is_not_read_through() {
        // The risk: `ln -sf ~/.ssh/id_rsa app/util.py` in the clone (a
        // link a tool made, not one it meant to be read through), read as a
        // changed file, would carry the key into a tracked file and from
        // there into the trail.
        let d = committed();
        let root = d.path();
        let secret = tempfile::tempdir().unwrap();
        let key = secret.path().join("id");
        std::fs::write(&key, "SECRET-KEY-MATERIAL\n").unwrap();
        let orch = with_bash(root);
        let (said, _) = bash(&orch, &format!("ln -sf '{}' app/util.py", key.display())).await;
        assert!(said.contains("is no longer a regular file"), "{said}");
        let now = std::fs::read_to_string(root.join("app/util.py")).unwrap();
        assert!(!now.contains("SECRET"), "{now}");
        assert!(orch.touched().await.is_empty());
    }

    // --- Hooks (ADR 0032, "Hooks") ---------------------------------------

    fn hooked(root: &Path, cfg: crate::hooks::HookConfig) -> crate::hooks::Hooks {
        crate::hooks::Hooks::new(root, cfg, vec![("TRELLIS_RUN_ID".into(), "run-7".into())])
    }

    /// Wired as `trellis run` wires them: the `pre_tool` gate in the gate
    /// list, and the same hooks for `post_tool` and `turn_end`.
    fn with_hooks(root: &Path, h: crate::hooks::Hooks, g: crate::governance::Governance) -> Handle {
        let mut o = Orchestrator::new(root, CheckConfig::default()).with_governance(g);
        if let Some(pre) = h.pre_tool_gate() {
            o = o.with_gate(Box::new(pre));
        }
        o.with_hooks(h).spawn().0
    }

    fn read_util() -> ToolCall {
        ToolCall::ReadFile {
            path: "app/util.py".into(),
        }
    }

    #[tokio::test]
    async fn isolated_hook_writes_reach_the_repository_only_as_carried_back_tracked_edits() {
        // The risk (ADR 0023 §5, #174): a hook ran in the repository, so a
        // `pre_tool` script could edit `.gitignore`, set `core.hooksPath`,
        // plant a file under `.git/`, leave an untracked file, or rewrite
        // `.trellis/`, where the rollback's git checks do not look. In the
        // call's clone only its edit to a tracked file comes back, through
        // the carry-back `bash` uses, and it is in `touched`.
        let d = committed();
        let root = d.path();
        git(root, &["status", "--porcelain"]);
        let gitignore = std::fs::read(root.join(".gitignore")).unwrap();
        let write = if cfg!(windows) {
            "echo *.py>> .gitignore& echo planted> .git\\planted& \
             git config core.hooksPath evil& echo x> made.txt& \
             echo [hooks]> .trellis\\config.toml& echo def helper(): return 3 > app\\util.py"
        } else {
            "echo '*.py' >> .gitignore; echo planted > .git/planted; \
             git config core.hooksPath evil; echo x > made.txt; \
             echo '[hooks]' > .trellis/config.toml; echo 'def helper(): return 3' > app/util.py"
        };
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                pre_tool: Some(shell_argv(write)),
                timeout_s: Some(30),
                ..Default::default()
            },
        );
        let orch = with_hooks(root, h, crate::governance::Governance::default());
        let d1 = orch.call(&read_util()).await;
        // Carried back after the gate and before the tool: the read sees it.
        match &d1.result {
            ToolResult::FileContent(s) => assert!(s.contains("return 3"), "{s}"),
            other => panic!("the hook allowed the call: {other:?}"),
        }
        assert!(
            d1.may_have_changed_tree,
            "a carried-back hook write voids a pass"
        );
        assert_eq!(orch.touched().await, ["app/util.py"], "the rollback has it");
        assert_eq!(std::fs::read(root.join(".gitignore")).unwrap(), gitignore);
        assert_eq!(
            std::fs::read_to_string(root.join(".trellis/config.toml")).unwrap(),
            "[checks]\n"
        );
        assert!(
            !root.join("made.txt").exists(),
            "a new file stays in the clone"
        );
        assert!(!root.join(".git/planted").exists());
        let hooks_path = std::process::Command::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            !hooks_path.status.success(),
            "core.hooksPath was set in the repository"
        );
        assert_eq!(
            git(root, &["status", "--porcelain"]).trim(),
            "M app/util.py"
        );
    }

    #[tokio::test]
    async fn isolated_hook_post_tool_sees_the_call_s_edit_in_the_clone_pre_tool_shared() {
        // The risk: a clone taken before the call shows `post_tool` the
        // tree as it was before the tool ran, and `post_tool` is there to
        // inspect what the call did. The clone is refreshed before each
        // hook; the result still arrives on stdin; and it is one clone per
        // call, so a file `pre_tool` left is there for `post_tool`.
        let d = committed();
        let root = d.path();
        let logs = tempfile::tempdir().unwrap();
        let log = logs.path().join("post.log");
        let (pre, post) = if cfg!(windows) {
            (
                "echo seen> seen.txt".to_string(),
                format!(
                    "cd >> {log}& type app\\util.py >> {log}& \
                     if exist seen.txt echo SHARED-CLONE>> {log}& more >> {log}",
                    log = log.display()
                ),
            )
        } else {
            (
                "echo seen > seen.txt".to_string(),
                format!(
                    "pwd >> '{log}'; cat app/util.py >> '{log}'; \
                     [ -f seen.txt ] && echo SHARED-CLONE >> '{log}'; cat >> '{log}'",
                    log = log.display()
                ),
            )
        };
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                pre_tool: Some(shell_argv(&pre)),
                post_tool: Some(shell_argv(&post)),
                timeout_s: Some(30),
                ..Default::default()
            },
        );
        let orch = with_hooks(root, h, crate::governance::Governance::default());
        let e = orch
            .call(&ToolCall::Edit {
                path: "app/util.py".into(),
                old: "return 1".into(),
                new: "return 2".into(),
            })
            .await;
        assert!(matches!(e.result, ToolResult::Edit(_)), "{:?}", e.result);
        let seen = std::fs::read_to_string(&log).unwrap();
        assert!(
            seen.contains("trellis-clone-"),
            "not in the repository: {seen}"
        );
        assert!(seen.contains("return 2"), "the tree after the edit: {seen}");
        assert!(seen.contains("SHARED-CLONE"), "pre_tool's clone: {seen}");
        assert!(seen.contains(r#""result""#), "the result on stdin: {seen}");
        assert!(!root.join("seen.txt").exists());
        assert_eq!(orch.touched().await, ["app/util.py"]);
    }

    #[tokio::test]
    async fn isolated_hook_write_is_refused_by_deny_write_then_allow_write() {
        // The risk (#182's, for hooks): a hook's write judged by fewer rules
        // than `bash`'s. The carry-back is the one `bash` uses, so
        // `deny_write` is asked first and a profile's `allow_write` after.
        let d = committed();
        let root = d.path();
        std::fs::write(root.join("top.py"), "x = 1\n").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/a.md"), "old\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "top"]);
        let write = if cfg!(windows) {
            "echo changed> app\\util.py& echo changed> top.py& echo new> docs\\a.md"
        } else {
            "echo changed > app/util.py; echo changed > top.py; echo new > docs/a.md"
        };
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                turn_end: Some(shell_argv(write)),
                timeout_s: Some(30),
                ..Default::default()
            },
        );
        let g = crate::governance::Governance::new(&["app/".into()])
            .with_allow_write("docs", &["app/".into(), "docs/".into()]);
        let orch = with_hooks(root, h, g);
        let d1 = orch.call(&read_util()).await;
        assert!(
            matches!(d1.result, ToolResult::FileContent(_)),
            "{:?}",
            d1.result
        );
        assert_eq!(
            std::fs::read_to_string(root.join("app/util.py")).unwrap(),
            "def helper():\n    return 1\n",
            "denied by deny_write, which allow_write cannot widen"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("top.py")).unwrap(),
            "x = 1\n",
            "outside allow_write"
        );
        // The control: the hook ran, and a write both rules admit came back.
        assert!(
            std::fs::read_to_string(root.join("docs/a.md"))
                .unwrap()
                .starts_with("new")
        );
        assert_eq!(orch.touched().await, ["docs/a.md"]);
        assert!(d1.may_have_changed_tree);
    }

    #[tokio::test]
    async fn isolated_hook_pre_tool_denies_when_no_clone_can_be_made() {
        // The risk: a hook that could not be isolated run in the repository
        // instead, or read as an allow. `pre_tool` is a gate in front of a
        // write, so it fails closed.
        let d = tempfile::tempdir().unwrap(); // not a git repository
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(d.path().join("app/util.py"), "x = 1\n").unwrap();
        let write = if cfg!(windows) {
            "echo hooked> app\\util.py"
        } else {
            "echo hooked > app/util.py"
        };
        let h = hooked(
            d.path(),
            crate::hooks::HookConfig {
                pre_tool: Some(shell_argv(write)),
                ..Default::default()
            },
        );
        let orch = with_hooks(d.path(), h, crate::governance::Governance::default());
        match orch.call(&read_util()).await.result {
            ToolResult::Error(e) => assert!(e.contains("throwaway clone"), "{e}"),
            other => panic!("a pre_tool that could not run must deny: {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(d.path().join("app/util.py")).unwrap(),
            "x = 1\n"
        );
    }

    #[tokio::test]
    async fn isolated_hook_input_runs_in_a_clone_and_nothing_it_writes_comes_back() {
        // The risk: `input` runs before the snapshot, and the ignored-file
        // listing and the `.git/` digest are taken after it, so a write it
        // made to an ignored file or under `.git/` became the baseline no
        // guard compares against. It runs in a clone of its own, and
        // nothing comes back: before the snapshot there is no write path.
        let d = committed();
        let root = d.path();
        let write = if cfg!(windows) {
            "echo planted> .git\\planted& echo x> debug.log& echo changed> app\\util.py"
        } else {
            "echo planted > .git/planted; echo x > debug.log; echo changed > app/util.py"
        };
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                input: Some(shell_argv(write)),
                timeout_s: Some(30),
                ..Default::default()
            },
        );
        assert!(matches!(
            h.input("t").await,
            Some(crate::hooks::InputDecision::Proceed)
        ));
        assert!(!root.join(".git/planted").exists());
        assert!(!root.join("debug.log").exists(), "an ignored file");
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }

    // --- The four writers and `.git/` (#54) -------------------------------
    //
    // One test per writer trellis does not control, each running the same
    // `.git/` writes from where it stands: a planted file, `core.hooksPath`,
    // a branch, a tag and a `skip-worktree` flag in the index. ADR 0023 §5
    // and ADR 0026 §11 listed each of these as a way past the rollback for a
    // writer that ran in the repository. Each test first shows the writes
    // happened (in the writer's clone), so a writer that failed to start
    // cannot pass it, then that every byte under the repository's `.git/`
    // is as it was.

    /// The `.git/` writes, then `tail`, as one shell script.
    fn git_dir_writes(tail: &str) -> String {
        if cfg!(windows) {
            format!(
                "echo planted> .git\\planted& git config core.hooksPath planted-hooks& \
                 git branch planted-branch& git tag planted-tag& \
                 git update-index --skip-worktree app/util.py& {tail}"
            )
        } else {
            format!(
                "echo planted > .git/planted; git config core.hooksPath planted-hooks; \
                 git branch planted-branch; git tag planted-tag; \
                 git update-index --skip-worktree app/util.py; {tail}"
            )
        }
    }

    /// What the writer reports from its own `.git/`, proof the writes ran.
    const GIT_DIR_REPORT: &str =
        "git config --get core.hooksPath && git branch --list planted-branch && git tag --list";

    /// Asserts `report` shows every write of [`git_dir_writes`] landed.
    fn wrote_its_own_git_dir(report: &str) {
        for mark in ["planted-hooks", "planted-branch", "planted-tag"] {
            assert!(report.contains(mark), "{mark} missing: {report}");
        }
    }

    /// The repository's `.git/`, after refreshing what `git status` may
    /// rewrite in the index, so a later comparison sees only other writers.
    fn settled_git_dir(root: &Path) -> String {
        git(root, &["status", "--porcelain"]);
        git_dir_digest(root)
    }

    /// Nothing the writer did reached the repository's `.git/`. Compared
    /// before any further `git` runs there.
    fn git_dir_untouched(root: &Path, before: &str) {
        assert_eq!(git_dir_digest(root), before, "a byte under .git/ changed");
        assert!(!root.join(".git/planted").exists());
        let config = std::fs::read_to_string(root.join(".git/config")).unwrap();
        assert!(!config.contains("planted-hooks"), "{config}");
        assert_eq!(git(root, &["branch", "--list", "planted-branch"]), "");
        assert_eq!(git(root, &["tag", "--list"]), "");
        assert_eq!(
            git(root, &["ls-files", "-v", "app/util.py"]).trim(),
            "H app/util.py",
            "skip-worktree was set in the repository's index"
        );
    }

    #[tokio::test]
    async fn isolated_writer_bash_git_dir_writes_do_not_reach_the_repository() {
        let d = committed();
        let root = d.path();
        let before = settled_git_dir(root);
        let orch = with_bash(root);
        let (said, _) = bash(&orch, &git_dir_writes(GIT_DIR_REPORT)).await;
        wrote_its_own_git_dir(&said);
        git_dir_untouched(root, &before);
        assert!(orch.touched().await.is_empty(), "{said}");
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn isolated_writer_custom_tool_git_dir_writes_do_not_reach_the_repository() {
        let d = committed();
        let root = d.path();
        let before = settled_git_dir(root);
        let tool = crate::registry::CustomTool {
            name: "plant".into(),
            description: "Write under .git/.".into(),
            command: shell_argv(&git_dir_writes(GIT_DIR_REPORT)),
            schema: serde_json::json!({ "type": "object", "properties": {} }),
            timeout_s: Some(60),
        };
        let tools = ToolSet::build(ToolSelection {
            custom: vec![tool],
            ..Default::default()
        })
        .unwrap();
        let orch = Orchestrator::new(root, CheckConfig::default())
            .with_tools(tools)
            .spawn()
            .0;
        let d1 = orch
            .call(&ToolCall::Custom {
                name: "plant".into(),
                args: serde_json::json!({}),
            })
            .await;
        let said = match d1.result {
            ToolResult::Output(s) => s,
            other => panic!("the custom tool did not run: {other:?}"),
        };
        wrote_its_own_git_dir(&said);
        git_dir_untouched(root, &before);
        assert!(orch.touched().await.is_empty(), "{said}");
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn isolated_writer_hook_git_dir_writes_do_not_reach_the_repository() {
        // A hook has no observation, so what it saw of its own `.git/` goes
        // to a log outside the repository.
        let d = committed();
        let root = d.path();
        let logs = tempfile::tempdir().unwrap();
        let log = logs.path().join("hook.log");
        // Unquoted for `cmd`, which is handed the script as one argument and
        // reads an escaped `"` inside it literally.
        let report = if cfg!(windows) {
            format!("({GIT_DIR_REPORT}) > {}", log.display())
        } else {
            format!("({GIT_DIR_REPORT}) > '{}'", log.display())
        };
        let before = settled_git_dir(root);
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                pre_tool: Some(shell_argv(&git_dir_writes(&report))),
                timeout_s: Some(60),
                ..Default::default()
            },
        );
        let orch = with_hooks(root, h, crate::governance::Governance::default());
        let d1 = orch.call(&read_util()).await;
        assert!(
            matches!(d1.result, ToolResult::FileContent(_)),
            "the hook allowed the call: {:?}",
            d1.result
        );
        wrote_its_own_git_dir(&std::fs::read_to_string(&log).unwrap_or_default());
        git_dir_untouched(root, &before);
        assert!(orch.touched().await.is_empty());
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn isolated_writer_process_agent_git_dir_writes_do_not_reach_the_repository() {
        // Spawned as `trellis-cli`'s `process_agent` spawns it: confined to
        // the clone, then scrubbed, then carried back.
        let d = committed();
        let root = d.path();
        let before = settled_git_dir(root);
        let clone = crate::AgentClone::prepare(root).unwrap();
        let argv = shell_argv(&git_dir_writes(GIT_DIR_REPORT));
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(std::process::Stdio::null());
        clone.confine(&mut cmd);
        trellis_core::secrets::scrub(&mut cmd);
        let out = cmd.output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{said}");
        wrote_its_own_git_dir(&said);
        assert!(clone.path().join(".git/planted").exists(), "{said}");
        let back = clone.carry_back(root, &crate::governance::Governance::new(&[]));
        assert!(back.complete(), "{back:?}");
        assert!(
            back.written.is_empty() && back.created.is_empty() && back.deleted.is_empty(),
            "{back:?}"
        );
        drop(clone);
        git_dir_untouched(root, &before);
        assert_eq!(git(root, &["status", "--porcelain"]), "");
    }

    // --- The OS sandbox around the clone's command (ADR 0055, #621) ------
    //
    // Each test runs its probe twice: once with no sandbox, which must
    // succeed (so a probe that could never have worked cannot pass), then
    // under this platform's sandbox, given a fake home directory to hide.
    // On a machine where the sandbox is missing or fails its trial run the
    // second run must go ahead unconfined and say why, which is the
    // owner's warn-not-refuse decision; the confinement half is then not
    // exercised, and the test says so on stderr.

    /// This platform's sandbox, probed with `home` as the directory to hide.
    /// Only the unix tests below use these four helpers; on Windows they
    /// would be dead code under `-D warnings`.
    #[cfg(unix)]
    fn platform_sandbox(home: &Path) -> crate::os_sandbox::OsSandbox {
        use crate::os_sandbox::{OsSandbox, OsSandboxKind};
        let temp = std::env::temp_dir();
        if cfg!(target_os = "linux") {
            OsSandbox::probe(OsSandboxKind::Bwrap, Some(home), &temp)
        } else if cfg!(target_os = "macos") {
            OsSandbox::probe(OsSandboxKind::SandboxExec, Some(home), &temp)
        } else {
            OsSandbox::none("no OS sandbox on this platform")
        }
    }

    fn with_bash_under(root: &Path, os: crate::os_sandbox::OsSandbox) -> Handle {
        let mut shell = shell_argv("");
        shell.pop();
        let tools = ToolSet::build(ToolSelection {
            bash: Some(BashSettings {
                shell,
                timeout: Duration::from_secs(60),
                allow: Vec::new(),
                unattended: true,
            }),
            ..Default::default()
        })
        .unwrap();
        Orchestrator::new(root, CheckConfig::default())
            .with_tools(tools)
            .with_os_sandbox(os)
            .spawn()
            .0
    }

    /// A shell line that prints `CONNECTED` if a TCP connection to
    /// 127.0.0.1:`port` opens, and `NO-CONNECTION` otherwise.
    #[cfg(unix)]
    fn connect_line(port: u16) -> String {
        format!(
            "bash -c 'echo x > /dev/tcp/127.0.0.1/{port}' 2>/dev/null && echo CONNECTED \
             || echo NO-CONNECTION"
        )
    }

    /// A fake home directory holding a canary built at run time.
    #[cfg(unix)]
    fn fake_home() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let home = tempfile::tempdir().unwrap();
        let canary = format!("canary-621-{}", std::process::id());
        let file = home.path().join("credentials");
        std::fs::write(&file, &canary).unwrap();
        (home, file, canary)
    }

    #[cfg(unix)]
    fn unconfined_said_why(os: &crate::os_sandbox::OsSandbox) {
        let label = os.label();
        assert!(label.starts_with("none ("), "{label}");
        eprintln!("no working OS sandbox here ({label}); the confinement half was not exercised");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_writer_bash_cannot_open_a_tcp_connection_to_loopback() {
        // The risk (ADR 0032, "Limits"): the clone's command could open any
        // connection the user can and send anything it read. Loopback is
        // the hardest case for a namespace that keeps a `lo` of its own: a
        // port trellis's machine listens on must not be reachable from it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let d = committed();
        let root = d.path();
        let (home, _, _) = fake_home();

        let open = with_bash_under(root, crate::os_sandbox::OsSandbox::none("control"));
        let (said, _) = bash(&open, &connect_line(port)).await;
        assert!(
            said.contains("CONNECTED") && !said.contains("NO-CONNECTION"),
            "{said}"
        );

        let os = platform_sandbox(home.path());
        let confined = with_bash_under(root, os.clone());
        let (said, _) = bash(&confined, &connect_line(port)).await;
        if os.is_active() {
            assert!(said.contains("NO-CONNECTION"), "{}: {said}", os.label());
        } else {
            unconfined_said_why(&os);
            assert!(said.contains("CONNECTED"), "{said}");
        }
        drop(listener);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_writer_bash_cannot_read_a_file_under_a_fake_home() {
        // The risk (ADR 0032, "Limits"): the home directory, credentials
        // files and other repositories were readable to the command, which
        // could copy a key's bytes into a tracked file or its output.
        let d = committed();
        let root = d.path();
        let (home, file, canary) = fake_home();
        let read = format!("cat '{}' 2>/dev/null || echo UNREADABLE", file.display());

        let open = with_bash_under(root, crate::os_sandbox::OsSandbox::none("control"));
        let (said, _) = bash(&open, &read).await;
        assert!(said.contains(&canary), "{said}");

        let os = platform_sandbox(home.path());
        let confined = with_bash_under(root, os.clone());
        let (said, _) = bash(&confined, &read).await;
        if os.is_active() {
            assert!(!said.contains(&canary), "{}: {said}", os.label());
            assert!(said.contains("UNREADABLE"), "{said}");
            // The clone itself is still the command's to read and write.
            let (said, _) = bash(
                &confined,
                "cat app/util.py && echo w > scratch.txt && echo WROTE",
            )
            .await;
            assert!(
                said.contains("def helper") && said.contains("WROTE"),
                "{said}"
            );
        } else {
            unconfined_said_why(&os);
            assert!(said.contains(&canary), "{said}");
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn sandboxed_writer_bash_cannot_see_trellis_s_process() {
        // The risk: `/proc/<trellis pid>/environ` holds the provider key
        // trellis reads, and `/proc/<trellis pid>/cwd` leads to the
        // repository through the host's mounts, around a read-only root.
        // In its own pid namespace the command sees neither.
        let d = committed();
        let root = d.path();
        let (home, _, _) = fake_home();
        let probe = format!(
            "test -e /proc/{}/environ && echo VISIBLE || echo HIDDEN",
            std::process::id()
        );
        let open = with_bash_under(root, crate::os_sandbox::OsSandbox::none("control"));
        let (said, _) = bash(&open, &probe).await;
        assert!(said.contains("VISIBLE"), "{said}");

        let os = platform_sandbox(home.path());
        let (said, _) = bash(&with_bash_under(root, os.clone()), &probe).await;
        if os.is_active() {
            assert!(said.contains("HIDDEN"), "{said}");
        } else {
            unconfined_said_why(&os);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_writer_hook_cannot_open_a_tcp_connection_or_read_a_fake_home() {
        // Hooks run through the same `exec::run` as `bash`, from their own
        // clone; the sandbox must reach them as well, not only the tool.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let d = committed();
        let root = d.path();
        let (home, file, canary) = fake_home();
        let script = format!(
            "{}; cat '{}' 2>/dev/null || echo UNREADABLE",
            connect_line(port),
            file.display()
        );
        let os = platform_sandbox(home.path());
        let h = hooked(
            root,
            crate::hooks::HookConfig {
                before_run: Some(shell_argv(&script)),
                timeout_s: Some(30),
                ..Default::default()
            },
        )
        .with_os_sandbox(os.clone());
        let said = h.before_run().await.unwrap().unwrap();
        if os.is_active() {
            assert!(said.contains("NO-CONNECTION"), "{said}");
            assert!(
                !said.contains(&canary) && said.contains("UNREADABLE"),
                "{said}"
            );
        } else {
            unconfined_said_why(&os);
            assert!(
                said.contains("CONNECTED") && said.contains(&canary),
                "{said}"
            );
        }
        drop(listener);
    }

    #[test]
    fn git_instruction_files_are_recognised_by_any_spelling() {
        for p in [
            ".gitignore",
            "sub/.gitignore",
            "a\\.GitAttributes",
            ".gitmodules.",
        ] {
            assert!(super::is_git_instruction_file(p), "{p}");
        }
        for p in [".github/workflows/ci.yml", "gitignore", "x.gitignore.bak"] {
            assert!(!super::is_git_instruction_file(p), "{p}");
        }
    }
}
