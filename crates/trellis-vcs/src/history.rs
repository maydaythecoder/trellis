//! Read-only views of a repository's history, for rpc's `get_codebase
//! {ref}` and `get_history`: a revision resolved to a dated commit, a
//! commit's files written out somewhere else to be indexed, and the
//! first-parent log and the tags in `HEAD`'s history (the desktop's Atlas,
//! the owner's request of 2026-10-11).
//!
//! Every git command here is a read (`rev-parse`, `log`, `rev-list`,
//! `for-each-ref`, `ls-tree`, `cat-file`) through [`command`]. None of them writes the
//! worktree, the index, a ref or `.git/worktrees`; a commit's files are
//! written only into a new directory under the system's temporary
//! directory, which is removed when its [`TreeCopy`] is dropped. That is
//! why the copy is not a `git checkout` into another work tree, which
//! writes the repository's index, nor a `git worktree add`, which writes
//! `.git/worktrees`, nor `git archive`, whose tar stream trellis would need
//! a tar reader or a `tar` child to unpack. In a partial clone, a blob the
//! clone lacks is not fetched, since the fetch runs whatever transport
//! `.git/config` names: [`command`] sets [`crate::ENV`], and the copy is
//! refused as [`HistoryError::Missing`] (security finding F1). On a git
//! too old to honour that variable, the fetch still happens.

use super::{VcsError, command, run_git, run_git_optional};
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
    let Some(out) = run_git_optional(root, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?
    else {
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
    let Some(out) = run_git_optional(
        root,
        &["rev-parse", "-q", "--verify", "--end-of-options", &peeled],
    )?
    else {
        return Err(HistoryError::Unknown(rev.to_string()));
    };
    Ok(stamp(root, out.trim())?)
}

/// A strict ISO 8601 date with UTC written as `+00:00`, never `Z`.
///
/// Git's `%cI` and `iso-strict` spell a zero offset differently by version:
/// the Windows runner's git wrote `2025-02-01T00:00:00Z` where Linux git
/// wrote `+00:00`. Both are valid, but a client comparing or displaying the
/// dates rpc answers should get one spelling whichever git produced them.
fn offset_form(date: &str) -> String {
    match date.strip_suffix('Z') {
        Some(rest) => format!("{rest}+00:00"),
        None => date.to_string(),
    }
}

/// `commit`'s committer date, from one `log -1` of its full id.
fn stamp(root: &Path, commit: &str) -> Result<Stamp, VcsError> {
    let args = [
        "log",
        "-1",
        // `log.showSignature` would run the configured `gpg.program` and
        // print its verdict into the output this parses.
        "--no-show-signature",
        "--format=%H%x09%ct%x09%cI",
        commit,
        "--",
    ];
    let out = run_git(root, &args)?;
    let line = out.trim_end();
    let mut f = line.splitn(3, '\t');
    match (f.next(), f.next().and_then(|t| t.parse().ok()), f.next()) {
        (Some(id), Some(time), Some(date)) => Ok(Stamp {
            commit: id.to_string(),
            date: offset_form(date),
            time,
        }),
        _ => Err(failed(&args, &format!("unexpected output: {line:?}"))),
    }
}

/// A commit on `HEAD`'s first-parent line and the paths it changed against
/// its first parent: for a merge, everything it brought in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub stamp: Stamp,
    /// The subject line as the commit holds it: data its author wrote.
    pub subject: String,
    /// Repository-relative and `/`-separated, sorted. A rename is a
    /// deletion and an addition, so both paths are here, whatever the
    /// repository's `diff.renames` says.
    pub paths: Vec<String>,
}

/// The newest `n` commits on `HEAD`'s first-parent line, newest first.
pub fn first_parent_changes(root: &Path, n: usize) -> Result<Vec<Change>, VcsError> {
    let limit = format!("--max-count={n}");
    let args = [
        "log",
        "--first-parent",
        &limit,
        "--no-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-show-signature",
        "--name-only",
        "-z",
        // A NUL before each record: paths and subjects hold no NUL, so an
        // empty field always means a record starts next.
        "--format=%x00%H%x09%ct%x09%cI%x09%s",
        "HEAD",
        "--",
    ];
    let out = git_bytes(root, &args)?;
    parse_log(&out).ok_or_else(|| failed(&args, "output that is not a log of commits"))
}

/// `first_parent_changes`' output, read: `None` when a record is not one.
fn parse_log(out: &[u8]) -> Option<Vec<Change>> {
    let mut changes: Vec<Change> = Vec::new();
    let (mut header_next, mut first_path) = (false, false);
    for field in out.split(|b| *b == 0) {
        if field.is_empty() {
            header_next = true;
            continue;
        }
        if header_next {
            let text = String::from_utf8_lossy(field);
            let mut f = text.splitn(4, '\t');
            let (commit, time, date, subject) = (f.next()?, f.next()?, f.next()?, f.next()?);
            changes.push(Change {
                stamp: Stamp {
                    commit: commit.to_string(),
                    date: offset_form(date),
                    time: time.parse().ok()?,
                },
                subject: subject.to_string(),
                paths: Vec::new(),
            });
            (header_next, first_path) = (false, true);
            continue;
        }
        // Git puts one newline between a record and its first path.
        let path = if first_path {
            field.strip_prefix(b"\n").unwrap_or(field)
        } else {
            field
        };
        first_path = false;
        changes
            .last_mut()?
            .paths
            .push(String::from_utf8_lossy(path).into_owned());
    }
    for c in &mut changes {
        c.paths.sort();
    }
    Some(changes)
}

/// How many commits on `HEAD`'s first-parent line come before the first
/// one committed before `time` (Unix seconds): `--since`, which stops the
/// walk there, so the count is of an unbroken run from `HEAD`.
pub fn first_parent_count_since(root: &Path, time: i64) -> Result<usize, VcsError> {
    let since = format!("--since=@{time}");
    count(
        root,
        &["rev-list", "--first-parent", "--count", &since, "HEAD"],
    )
}

/// How many commits on `HEAD`'s first-parent line `commit` (a full object
/// id) cannot reach: those after it, it excluded. Its ancestors are not on
/// the line past the first of them, so this too is an unbroken run.
pub fn first_parent_count_after(root: &Path, commit: &str) -> Result<usize, VcsError> {
    if !is_object_id(commit) {
        return Err(failed(
            &["rev-list", "--first-parent", "--count"],
            &format!("{commit:?} is not a full object id"),
        ));
    }
    let not = format!("^{commit}");
    count(
        root,
        &["rev-list", "--first-parent", "--count", &not, "HEAD"],
    )
}

fn count(root: &Path, args: &[&str]) -> Result<usize, VcsError> {
    let out = run_git(root, args)?;
    out.trim()
        .parse()
        .map_err(|_| failed(args, &format!("not a count: {out:?}")))
}

/// A tag and the commit it names, peeled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The name under `refs/tags/`.
    pub name: String,
    pub stamp: Stamp,
}

/// Every tag whose commit `HEAD` can reach, newest commit first, then by
/// name. An annotated tag is peeled to its commit and dated by it, not by
/// the tagger. A tag of a tree or a blob is not one, nor a tag of a tag:
/// `for-each-ref` peels one level, and a chain is rare enough to leave out
/// rather than resolve a name at a time.
pub fn tags_in_head(root: &Path) -> Result<Vec<Tag>, VcsError> {
    let args = [
        "for-each-ref",
        "--merged=HEAD",
        concat!(
            "--format=%(refname)%00%(objecttype)%00%(objectname)",
            "%00%(committerdate:unix)%00%(committerdate:iso-strict)",
            "%00%(*objecttype)%00%(*objectname)",
            "%00%(*committerdate:unix)%00%(*committerdate:iso-strict)",
        ),
        "refs/tags",
    ];
    let out = git_bytes(root, &args)?;
    let mut tags = Vec::new();
    for line in out.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let text = String::from_utf8_lossy(line);
        let f: Vec<&str> = text.split('\0').collect();
        let [
            refname,
            kind,
            id,
            time,
            date,
            peeled_kind,
            peeled_id,
            peeled_time,
            peeled_date,
        ] = f.as_slice()
        else {
            return Err(failed(&args, &format!("unexpected record: {text:?}")));
        };
        let (id, time, date) = match (*kind, *peeled_kind) {
            ("commit", _) => (id, time, date),
            ("tag", "commit") => (peeled_id, peeled_time, peeled_date),
            _ => continue,
        };
        let Ok(time) = time.parse() else {
            return Err(failed(&args, &format!("a date that is not one: {text:?}")));
        };
        tags.push(Tag {
            name: refname
                .strip_prefix("refs/tags/")
                .unwrap_or(refname)
                .to_string(),
            stamp: Stamp {
                commit: id.to_string(),
                date: offset_form(date),
                time,
            },
        });
    }
    tags.sort_by(|a, b| {
        b.stamp
            .time
            .cmp(&a.stamp.time)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(tags)
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
        run_git(root, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
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

    #[cfg(unix)]
    #[test]
    fn the_copy_directory_is_entered_by_its_owner_only() {
        // The risk (security finding F2, AUDIT-2026-10-11): a commit's
        // source written under the shared temporary directory where another
        // user on the machine can list and read it. ADR 0025 §5 states
        // `0700`; nothing held it.
        use std::os::unix::fs::PermissionsExt;
        let (dir, first) = repo("mode");
        let copy = tree_copy(&dir, &first, |p| p.ends_with(".py")).unwrap();
        let mode = std::fs::metadata(copy.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "the copy is {:o}", mode & 0o777);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_fails_partway_leaves_no_directory_behind() {
        // The risk (F2): a copy that fails after writing some files leaves
        // part of a commit's source in the temporary directory, once for
        // every snapshot the Atlas could not read. `write_blobs` is driven
        // with a directory whose path the test holds, since a failed
        // `tree_copy` returns none. The three failures it handles: a blob
        // the store lacks, an answer that is not the blob asked for, and
        // git exiting non-zero.
        let (dir, first) = repo("fails");
        let id = |rev: String| sh(&dir, &["rev-parse", &rev]).trim().to_string();
        let a = id(format!("{first}:a.py"));
        let tree = id(format!("{first}^{{tree}}"));
        let missing = {
            let mut c = command()
                .args(["hash-object", "--stdin"])
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin
                .take()
                .unwrap()
                .write_all(b"never written\n")
                .unwrap();
            let o = c.wait_with_output().unwrap();
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        // A `.git` file naming no directory: git exits 128 before it
        // answers anything.
        let broken = dir.with_file_name(format!("{}-broken", dir.file_name().unwrap().display()));
        let _ = std::fs::remove_dir_all(&broken);
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(
            broken.join(".git"),
            format!("gitdir: {}\n", broken.join("nowhere").display()),
        )
        .unwrap();
        let after_a = |p: &str, oid: &str| {
            vec![
                ("a.py".to_string(), a.clone()),
                (p.to_string(), oid.to_string()),
            ]
        };
        let cases = [
            ("missing", &dir, after_a("b.py", &missing)),
            ("header", &dir, after_a("t.py", &tree)),
            ("exit status", &broken, Vec::new()),
        ];
        for (what, root, wanted) in cases {
            let d = fresh_dir().unwrap();
            let copy = TreeCopy {
                dir: d.clone(),
                files: 0,
            };
            let got = write_blobs(root, &first, copy, &wanted);
            match (what, &got) {
                ("missing", Err(HistoryError::Missing { path, .. })) => assert_eq!(path, "b.py"),
                ("header", Err(HistoryError::Vcs(VcsError::GitFailed { stderr, .. }))) => {
                    assert!(stderr.contains(&tree), "{stderr}");
                }
                ("exit status", Err(HistoryError::Vcs(VcsError::GitFailed { stderr, .. }))) => {
                    // The wording is git's and has two forms: "not a git
                    // repository" and, from 2.56 for a dangling gitfile,
                    // "gitfile does not point to a valid repository".
                    assert!(
                        stderr.contains("not a git repository")
                            || stderr.contains("does not point to a valid repository"),
                        "{stderr}"
                    );
                }
                _ => panic!("{what}: {got:?}"),
            }
            assert!(
                !d.exists(),
                "{what}: the failed copy's directory outlived it"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&broken);
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
    fn utc_is_written_as_an_offset_whatever_git_wrote() {
        // The risk: one repository's dates answered in two spellings because
        // the machines that ran git differ (Windows runner, CI run 38162600641).
        assert_eq!(
            offset_form("2025-02-01T00:00:00Z"),
            "2025-02-01T00:00:00+00:00"
        );
        assert_eq!(
            offset_form("2025-02-01T00:00:00+00:00"),
            "2025-02-01T00:00:00+00:00"
        );
        assert_eq!(
            offset_form("2024-06-01T00:00:00+02:00"),
            "2024-06-01T00:00:00+02:00"
        );
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
    fn parse_log_reads_an_empty_commit_a_tabbed_subject_and_the_first_path_s_newline() {
        // The risk: a commit that changed nothing taking the next commit's
        // record for a path, or a subject with a tab cut at it.
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "\0{a}\t100\t1970-01-01T00:01:40Z\tx\ty\0\0{b}\t50\t1970-01-01T00:00:50+01:00\t\0\nz.py\0a b.py\0"
        );
        let got = parse_log(out.as_bytes()).unwrap();
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].stamp.commit, a);
        assert_eq!(got[0].subject, "x\ty");
        assert!(got[0].paths.is_empty(), "{got:?}");
        assert_eq!(got[1].stamp.time, 50);
        assert_eq!(got[1].stamp.date, "1970-01-01T00:00:50+01:00");
        assert_eq!(got[1].subject, "");
        assert_eq!(got[1].paths, ["a b.py", "z.py"], "sorted");
        assert_eq!(parse_log(b"\0not a record\0"), None);
        assert_eq!(parse_log(b""), Some(Vec::new()));
    }

    #[test]
    fn the_first_parent_walk_carries_a_merge_s_changes_and_not_its_side_commits() {
        // The risk: a merge listed with no paths, so everything a branch
        // brought in looks unchanged; or the branch's own commits counted
        // as the line's.
        let (dir, first) = repo("walk");
        let main = sh(&dir, &["symbolic-ref", "--short", "HEAD"]);
        sh(&dir, &["checkout", "-qb", "side"]);
        std::fs::write(dir.join("side.py"), "S = 1\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-qm", "on the side"]);
        sh(&dir, &["checkout", "-q", main.trim()]);
        sh(
            &dir,
            &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
        );
        let log = first_parent_changes(&dir, 10).unwrap();
        let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["merge side", "second", "first"]);
        assert_eq!(log[0].paths, ["side.py"]);
        assert_eq!(log[1].paths, ["a.py"]);
        assert_eq!(log[2].stamp.commit, first);
        assert_eq!(first_parent_changes(&dir, 1).unwrap().len(), 1);
        assert_eq!(first_parent_count_after(&dir, &first).unwrap(), 2);
        // `first` was committed at 2024-06-01T00:00:00+02:00; the others now.
        assert_eq!(first_parent_count_since(&dir, 1_717_192_800).unwrap(), 3);
        assert_eq!(first_parent_count_since(&dir, 1_717_192_801).unwrap(), 2);
        assert!(
            first_parent_count_after(&dir, "HEAD").is_err(),
            "an id only"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tags_in_head_peel_to_commits_newest_first_and_leave_out_what_head_cannot_reach() {
        // The risks: an annotated tag answered with the tag object's id or
        // the tagger's date; a tag on an unmerged branch offered as a
        // snapshot of this history; a tag of a tree read as a commit.
        let (dir, first) = repo("tags");
        let second = sh(&dir, &["rev-parse", "HEAD"]).trim().to_string();
        sh(&dir, &["tag", "light", &first]);
        let out = command()
            .args(["tag", "-a", "heavy", "-m", "release", &first])
            .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00+00:00")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        sh(&dir, &["tag", "latest"]);
        sh(&dir, &["tag", "a-tree", "HEAD^{tree}"]);
        let main = sh(&dir, &["symbolic-ref", "--short", "HEAD"]);
        sh(&dir, &["checkout", "-qb", "other", &first]);
        sh(&dir, &["commit", "--allow-empty", "-qm", "elsewhere"]);
        sh(&dir, &["tag", "elsewhere"]);
        sh(&dir, &["checkout", "-q", main.trim()]);
        let tags = tags_in_head(&dir).unwrap();
        let got: Vec<(&str, &str, &str)> = tags
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.stamp.commit.as_str(),
                    t.stamp.date.as_str(),
                )
            })
            .collect();
        let first_date = "2024-06-01T00:00:00+02:00";
        assert_eq!(got[0].0, "latest");
        assert_eq!(got[0].1, second);
        assert_eq!(
            &got[1..],
            [
                ("heavy", first.as_str(), first_date),
                ("light", first.as_str(), first_date)
            ],
            "{got:?}"
        );
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
