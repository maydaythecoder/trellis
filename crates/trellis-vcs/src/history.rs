//! Read-only views of a repository's history, for rpc's `get_codebase
//! {ref}`: a revision resolved to a dated commit, and a commit's files
//! written out somewhere else to be indexed (the desktop's Atlas, the
//! owner's request of 2026-10-11).
//!
//! Every git command here is a plumbing read (`rev-parse`, `log`,
//! `ls-tree`, `cat-file`) through [`command`]. None of them writes the
//! worktree, the index, a ref or `.git/worktrees`; a commit's files are
//! written only into a new directory under the system's temporary
//! directory, which is removed when its [`TreeCopy`] is dropped. That is
//! why the copy is not a `git checkout` into another work tree, which
//! writes the repository's index, nor a `git worktree add`, which writes
//! `.git/worktrees`, nor `git archive`, whose tar stream trellis would need
//! a tar reader or a `tar` child to unpack. One limit, stated rather than
//! worked around: in a partial clone, `cat-file` fetches a blob the clone
//! lacks into the object store, as any read of it would.

use super::{VcsError, command, git, git_opt};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A commit and its committer date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The full object id.
    pub commit: String,
    /// The committer date in strict ISO 8601, with the committer's own
    /// offset (`%cI`): what the commit says, not this machine's zone.
    pub date: String,
    /// The same date as Unix seconds, for measuring back from it.
    pub time: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// Refused before git runs: a revision is never passed as an option.
    #[error(
        "`{0}` starts with `-`, and a revision is never handed to git where it could be read as an option"
    )]
    OptionLike(String),
    /// Git resolved it to no commit.
    #[error("`{0}` names no commit in this repository")]
    Unknown(String),
    /// A tree entry whose path would be written outside the copy, or into
    /// a `.git` there. Git refuses to check such a tree out; so does this.
    #[error("the tree at {commit} holds a path that would be written outside its copy: {path:?}")]
    UnsafePath { commit: String, path: String },
    /// A blob the tree names that the object store does not hold.
    #[error("{path} at {commit} is blob {oid}, which the object store does not hold")]
    Missing {
        commit: String,
        path: String,
        oid: String,
    },
    #[error(transparent)]
    Vcs(#[from] VcsError),
}

impl From<std::io::Error> for HistoryError {
    fn from(e: std::io::Error) -> Self {
        HistoryError::Vcs(VcsError::Io(e))
    }
}

/// The commit `HEAD` resolves to, dated; `None` when it resolves to none
/// (an unborn branch: a repository with no commit yet).
pub fn head(root: &Path) -> Result<Option<Stamp>, VcsError> {
    let Some(out) = git_opt(root, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])? else {
        return Ok(None);
    };
    stamp(root, out.trim()).map(Some)
}

/// The commit `rev` names, peeled through any tag, dated.
///
/// `rev` comes from a client, so it is never read as an option: one that
/// starts with `-` is refused here, and the rest reach git after
/// `--end-of-options`, peeled with `^{commit}` so a tree or blob name is
/// not taken for a commit. Only the full id it resolves to is passed on.
pub fn resolve(root: &Path, rev: &str) -> Result<Stamp, HistoryError> {
    if rev.starts_with('-') {
        return Err(HistoryError::OptionLike(rev.to_string()));
    }
    let peeled = format!("{rev}^{{commit}}");
    // `-q --verify` exits 1, saying nothing, for a name that resolves to no
    // commit; any other failure is git's and is reported as one.
    let Some(out) = git_opt(
        root,
        &["rev-parse", "-q", "--verify", "--end-of-options", &peeled],
    )?
    else {
        return Err(HistoryError::Unknown(rev.to_string()));
    };
    Ok(stamp(root, out.trim())?)
}

/// `commit`'s committer date, from one `log -1` of its full id.
fn stamp(root: &Path, commit: &str) -> Result<Stamp, VcsError> {
    let out = git(
        root,
        &["log", "-1", "--format=%H%x09%ct%x09%cI", commit, "--"],
    )?;
    let line = out.trim_end();
    let mut f = line.splitn(3, '\t');
    match (f.next(), f.next().and_then(|t| t.parse().ok()), f.next()) {
        (Some(id), Some(time), Some(date)) => Ok(Stamp {
            commit: id.to_string(),
            date: date.to_string(),
            time,
        }),
        _ => Err(failed(
            &["log", "-1", "--format=%H%x09%ct%x09%cI", commit],
            &format!("unexpected output: {line:?}"),
        )),
    }
}

/// A commit's files, written into a directory of their own and removed
/// with it when this is dropped.
#[derive(Debug)]
pub struct TreeCopy {
    dir: PathBuf,
    /// How many files were written.
    pub files: usize,
}

impl TreeCopy {
    /// The directory the files are in, laid out as the commit lays them out.
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TreeCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes the regular files of `commit`'s tree whose repository-relative,
/// `/`-separated path `keep` accepts into a new directory under the
/// system's temporary directory, and nothing else.
///
/// `commit` is a full object id, as [`resolve`] gives. The tree is listed
/// with `ls-tree -r -z` and the blobs are read in one `cat-file --batch`,
/// so the bytes are the committed ones: no filter, smudge or end-of-line
/// conversion runs, since each would be the repository's code or config
/// acting on the copy. Symlinks and submodules are not written (an index
/// built over a work tree does not follow a link to a file either), nor is
/// a path that is not UTF-8. A path that would leave the directory (a `..`
/// or empty component, a `.git` one, or on Windows a `\` or `:` in a name)
/// refuses the whole copy, as git refuses to check such a tree out.
pub fn tree_copy(
    root: &Path,
    commit: &str,
    keep: impl Fn(&str) -> bool,
) -> Result<TreeCopy, HistoryError> {
    if !is_object_id(commit) {
        return Err(HistoryError::Unknown(commit.to_string()));
    }
    let listing = git_bytes(root, &["ls-tree", "-r", "-z", "--full-tree", commit])?;
    let mut wanted: Vec<(String, String)> = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let Some(tab) = entry.iter().position(|b| *b == b'\t') else {
            continue;
        };
        let meta = String::from_utf8_lossy(&entry[..tab]);
        let mut m = meta.split(' ');
        let (Some(mode), Some(kind), Some(oid)) = (m.next(), m.next(), m.next()) else {
            continue;
        };
        if kind != "blob" || !matches!(mode, "100644" | "100755") {
            continue;
        }
        let Ok(path) = std::str::from_utf8(&entry[tab + 1..]) else {
            continue;
        };
        if !keep(path) {
            continue;
        }
        if !safe_relative(path) {
            return Err(HistoryError::UnsafePath {
                commit: commit.to_string(),
                path: path.to_string(),
            });
        }
        wanted.push((path.to_string(), oid.to_string()));
    }
    let copy = TreeCopy {
        dir: fresh_dir()?,
        files: 0,
    };
    write_blobs(root, commit, copy, &wanted)
}

/// Reads each of `wanted`'s blobs from one `cat-file --batch` and writes
/// it under `copy`'s directory. On any failure `copy` is dropped, and its
/// directory with it.
fn write_blobs(
    root: &Path,
    commit: &str,
    mut copy: TreeCopy,
    wanted: &[(String, String)],
) -> Result<TreeCopy, HistoryError> {
    let args = ["cat-file", "--batch"];
    let mut child = command()
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Fed from its own thread, and stderr drained on another: git writes
    // each answer as it reads each request, so a request list written on
    // this thread while nothing reads the answers would fill both pipes.
    let mut stdin = child.stdin.take().ok_or_else(|| missing_pipe("stdin"))?;
    let requests: Vec<String> = wanted.iter().map(|(_, oid)| oid.clone()).collect();
    let feeder = std::thread::spawn(move || -> std::io::Result<()> {
        for oid in requests {
            writeln!(stdin, "{oid}")?;
        }
        Ok(())
    });
    let mut stderr = child.stderr.take().ok_or_else(|| missing_pipe("stderr"))?;
    let drain = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        s
    });
    let stdout = child.stdout.take().ok_or_else(|| missing_pipe("stdout"))?;
    let read = read_batch(
        &mut std::io::BufReader::new(stdout),
        commit,
        &mut copy,
        wanted,
    );
    // Whatever was read, the child is reaped and the threads joined before
    // the answer: a failed read leaves git blocked on a full pipe, so it is
    // killed rather than waited on.
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let fed = feeder.join();
    let err = drain.join().unwrap_or_default();
    read?;
    if !status.success() {
        return Err(failed(&args, &String::from_utf8_lossy(&err)).into());
    }
    if let Ok(Err(e)) = fed {
        return Err(e.into());
    }
    Ok(copy)
}

/// The answers to `wanted`'s requests, in order, each written to its path.
fn read_batch(
    out: &mut impl BufRead,
    commit: &str,
    copy: &mut TreeCopy,
    wanted: &[(String, String)],
) -> Result<(), HistoryError> {
    let unexpected = |what: String| failed(&["cat-file", "--batch"], &what);
    for (path, oid) in wanted {
        let mut header = String::new();
        out.read_line(&mut header)?;
        let mut h = header.trim_end().split(' ');
        match (h.next(), h.next(), h.next()) {
            (Some(id), Some("missing"), None) if id == oid => {
                return Err(HistoryError::Missing {
                    commit: commit.to_string(),
                    path: path.clone(),
                    oid: oid.clone(),
                });
            }
            (Some(id), Some("blob"), Some(size)) if id == oid => {
                let size: u64 = size
                    .parse()
                    .map_err(|_| unexpected(format!("a size that is not a number: {header:?}")))?;
                let mut bytes = Vec::new();
                out.by_ref().take(size).read_to_end(&mut bytes)?;
                let mut newline = [0u8; 1];
                out.read_exact(&mut newline)?;
                if bytes.len() as u64 != size || newline != *b"\n" {
                    return Err(unexpected(format!("{oid} ended early")).into());
                }
                let mut dest = copy.dir.clone();
                dest.extend(path.split('/'));
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                // `create_new`: a tree naming one path twice is malformed,
                // and the second write is refused rather than kept.
                std::fs::File::options()
                    .write(true)
                    .create_new(true)
                    .open(&dest)?
                    .write_all(&bytes)?;
                copy.files += 1;
            }
            _ => {
                return Err(
                    unexpected(format!("asked for {oid}, read the header {header:?}")).into(),
                );
            }
        }
    }
    Ok(())
}

fn missing_pipe(which: &str) -> VcsError {
    VcsError::Io(std::io::Error::other(format!(
        "cat-file --batch was started without its {which} pipe"
    )))
}

/// Whether `path`, `/`-separated and repository-relative, stays inside the
/// directory it is joined to: no empty, `.` or `..` component and no `.git`
/// one in any case, and on Windows no `\` (a separator there) or `:` (a
/// drive or a stream) in a name.
fn safe_relative(path: &str) -> bool {
    path.split('/').all(|c| {
        !c.is_empty()
            && c != "."
            && c != ".."
            && !c.eq_ignore_ascii_case(".git")
            && !(cfg!(windows) && (c.contains('\\') || c.contains(':')))
    })
}

/// A full object id: 40 hex digits (SHA-1) or 64 (SHA-256).
fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A new, empty directory under the system's temporary directory, which
/// only this user can enter (`0700` on Unix), since a commit's source is
/// about to be written into it. A new one each time, not `create_dir_all`:
/// one someone made there first, under the name picked, is refused.
fn fresh_dir() -> Result<PathBuf, VcsError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "trellis-tree-{}-{}-{nanos}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    #[cfg(unix)]
    let builder = {
        let mut b = std::fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b
    };
    #[cfg(not(unix))]
    let builder = std::fs::DirBuilder::new();
    builder.create(&dir)?;
    Ok(dir)
}

/// `git <args>`'s stdout as bytes, for output that may hold paths that
/// are not UTF-8.
fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, VcsError> {
    let out = command().args(args).current_dir(root).output()?;
    if !out.status.success() {
        return Err(failed(args, &String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}

/// The failure of `git <args>`, named as every `GitFailed` here names one.
fn failed(args: &[&str], stderr: &str) -> VcsError {
    VcsError::GitFailed {
        command: format!("git {}", args.join(" ")),
        stderr: stderr.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sh(root: &Path, args: &[&str]) -> String {
        git(root, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    /// A repository whose first commit holds Python files, a README, an
    /// executable and (on Unix) a symlink, and whose second commit changes
    /// one of them, so a copy of the first differs from the worktree.
    fn repo(name: &str) -> (PathBuf, String) {
        let dir =
            std::env::temp_dir().join(format!("trellis-vcs-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg/sub")).unwrap();
        sh(&dir, &["init", "-q"]);
        sh(&dir, &["config", "user.email", "t@example.invalid"]);
        sh(&dir, &["config", "user.name", "t"]);
        sh(&dir, &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(dir.join("pkg/__init__.py"), "").unwrap();
        std::fs::write(dir.join("pkg/sub/deep.py"), "X = 'é'\r\n").unwrap();
        std::fs::write(dir.join("README"), "not python\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.py", dir.join("link.py")).unwrap();
        sh(&dir, &["add", "-A"]);
        // Committed in a zone other than UTC, so a date given in this
        // machine's zone, or from the author line, would differ.
        let out = command()
            .args(["commit", "-qm", "first"])
            .env("GIT_COMMITTER_DATE", "2024-06-01T00:00:00+02:00")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+00:00")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        let first = sh(&dir, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(dir.join("a.py"), "def f():\n    return 2\n").unwrap();
        sh(&dir, &["commit", "-qam", "second"]);
        (dir, first)
    }

    /// Every file under `dir`, by `/`-separated relative path, with its
    /// bytes; symlinks by their target.
    fn files_under(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let rel = p
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let md = p.symlink_metadata().unwrap();
                if md.file_type().is_symlink() {
                    let target = std::fs::read_link(&p).unwrap();
                    out.insert(rel, target.to_string_lossy().as_bytes().to_vec());
                } else if md.is_dir() {
                    stack.push(p);
                } else {
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn tree_copy_writes_the_commit_s_kept_files_byte_for_byte_and_nothing_else() {
        // The risk: an index "at a ref" built over the worktree's bytes, a
        // filtered or line-ending-converted copy, or files the index would
        // never read; and a symlink followed out of the copy.
        let (dir, first) = repo("copy");
        let copy = tree_copy(&dir, &first, |p| p.ends_with(".py")).unwrap();
        let got = files_under(copy.path());
        let want: BTreeMap<String, Vec<u8>> = [
            ("a.py", &b"def f():\n    return 1\n"[..]),
            ("pkg/__init__.py", b""),
            ("pkg/sub/deep.py", "X = 'é'\r\n".as_bytes()),
        ]
        .into_iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect();
        assert_eq!(got, want);
        assert_eq!(copy.files, 3);
        assert!(
            !copy.path().starts_with(&dir),
            "written outside the repository"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_copy_leaves_the_worktree_the_index_and_the_git_dir_byte_identical() {
        // The risk: a copy taken by checking a commit out, which rewrites
        // the index (and, through `worktree add`, `.git/worktrees`) of the
        // repository the user is working in.
        let (dir, first) = repo("no-writes");
        std::fs::write(dir.join("a.py"), "def f():\n    return 3\n").unwrap();
        let before = files_under(&dir);
        let copy = tree_copy(&dir, &first, |p| p.ends_with(".py")).unwrap();
        let stamp = resolve(&dir, &first).unwrap();
        let _ = head(&dir).unwrap();
        assert_eq!(stamp.commit, first);
        assert_eq!(files_under(&dir), before, "the repository changed");
        let path = copy.path().to_path_buf();
        drop(copy);
        assert!(!path.exists(), "the copy outlived its TreeCopy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A commit, on no branch, whose tree is `top` named `name` and holding
    /// one Python file: the shape `ls-tree -r` lists as `<name>/x.py`.
    fn commit_with_entry(dir: &Path, name: &str) -> String {
        let blob = {
            let mut c = command()
                .args(["hash-object", "-w", "--stdin"])
                .current_dir(dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(b"Y = 1\n").unwrap();
            let o = c.wait_with_output().unwrap();
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        let mktree = |line: String| -> String {
            let mut c = command()
                .args(["mktree"])
                .current_dir(dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(line.as_bytes()).unwrap();
            let o = c.wait_with_output().unwrap();
            assert!(o.status.success(), "mktree {line:?}");
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        let inner = mktree(format!("100644 blob {blob}\tx.py\n"));
        let top = mktree(format!("040000 tree {inner}\t{name}\n"));
        sh(dir, &["commit-tree", &top, "-m", "crafted"])
            .trim()
            .to_string()
    }

    #[test]
    fn a_tree_path_that_would_leave_the_copy_refuses_the_copy() {
        // The risk: a crafted commit whose tree names `../x.py` or
        // `.git/x.py`, written through the copy into the temporary
        // directory's parent or into a git directory of its own. Git
        // refuses to check either out; `ls-tree` lists both.
        let (dir, _) = repo("unsafe");
        for name in ["..", ".git", ".GIT"] {
            let commit = commit_with_entry(&dir, name);
            match tree_copy(&dir, &commit, |p| p.ends_with(".py")) {
                Err(HistoryError::UnsafePath { path, .. }) => {
                    assert_eq!(path, format!("{name}/x.py"));
                }
                other => panic!("{name}: {other:?}"),
            }
        }
        assert!(!dir.parent().unwrap().join("x.py").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_refuses_an_option_and_names_a_revision_that_is_no_commit() {
        // The risk: a client's `ref` read by git as an option
        // (`--output=...` writes a file), or an unknown one answered as
        // some other commit.
        let (dir, _) = repo("resolve");
        let tree = sh(&dir, &["rev-parse", "HEAD^{tree}"]).trim().to_string();
        for (rev, want) in [
            ("--output=/tmp/x", "option"),
            ("-n", "option"),
            ("no-such-ref", "unknown"),
            (tree.as_str(), "unknown"),
        ] {
            let e = resolve(&dir, rev).unwrap_err();
            match (want, &e) {
                ("option", HistoryError::OptionLike(r)) | ("unknown", HistoryError::Unknown(r)) => {
                    assert_eq!(r, rev);
                    assert!(e.to_string().contains(rev), "{e}");
                }
                _ => panic!("{rev}: {e:?}"),
            }
        }
        let e = tree_copy(&dir, "HEAD", |_| true).unwrap_err();
        assert!(
            matches!(e, HistoryError::Unknown(_)),
            "a name, not an id: {e:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_peels_a_tag_and_dates_the_commit_by_its_committer() {
        // The risk: an annotated tag answered with the tag object's id, or
        // a date in this machine's zone rather than the commit's.
        let (dir, first) = repo("peel");
        sh(&dir, &["tag", "-a", "v1", "-m", "release", &first]);
        let s = resolve(&dir, "v1").unwrap();
        assert_eq!(s.commit, first);
        assert_eq!(s.date, "2024-06-01T00:00:00+02:00");
        assert_eq!(s.time, 1_717_192_800);
        let h = head(&dir).unwrap().unwrap();
        assert_eq!(h.commit, sh(&dir, &["rev-parse", "HEAD"]).trim());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_of_a_repository_with_no_commit_is_none_not_an_error() {
        let dir =
            std::env::temp_dir().join(format!("trellis-vcs-history-unborn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, &["init", "-q"]);
        assert_eq!(head(&dir).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
