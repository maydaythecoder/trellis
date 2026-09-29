//! Running a writer trellis does not control in a throwaway clone
//! (ADR 0034): copy-in/diff-out.
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

use crate::orchestrator::{names_git_dir, real_rel, unsafe_repo_path};
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
        let host_head = head(&host)?;
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
        let clone_head = head(&sb.dir)?;
        if clone_head != host_head {
            return Err(IsolateError::Head {
                host: host_head.unwrap_or_else(|| "no commit".into()),
                clone: clone_head.unwrap_or_else(|| "no commit".into()),
            });
        }
        // The remote and the reflog both hold the repository's path, which
        // is all a command would need to write there instead. Removing them
        // hides the path from a casual `git remote -v`; it does not stop a
        // command that finds it another way (ADR 0034).
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
    /// clone" limit (ADR 0034), not something this refusal closes.
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
fn head(dir: &Path) -> Result<Option<String>, IsolateError> {
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
    /// tracked `.trellis/config.toml`.
    fn committed() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
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
            git(root, args);
        }
        d
    }

    fn with_bash(root: &Path) -> Handle {
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
            .spawn()
            .0
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
