//! Writes into a repository that never go through a symbolic link.
//!
//! A repository can commit `.trellis`, a directory under it, or any file
//! trellis writes there as a link to somewhere outside itself. A write that
//! follows the link then lands outside the repository: `fs::write` follows
//! a live link, and `exists()` reads a dangling one as absent, so the write
//! creates the link's target (#721, #723, #725). Every writer of a path the
//! repository can plant checks it here first, so the rule and its error are
//! one thing rather than a copy per crate.
//!
//! `symlink_metadata` throughout: it examines the link itself, so a
//! dangling one is caught rather than followed to nothing. Its
//! `is_symlink` is also true for a Windows junction.

use std::path::{Component, Path, PathBuf};

/// A path a writer refused because it is a symbolic link (a junction on
/// Windows).
///
/// Carried inside the `io::Error` the checks return, so a caller that only
/// prints the error names the link, and one that needs to tell this refusal
/// from a disk error can downcast to it.
#[derive(Debug)]
pub struct LinkRefused {
    pub path: PathBuf,
    /// What was not written, for the message: "the index cache", "the trail".
    pub what: &'static str,
}

impl std::fmt::Display for LinkRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is a symbolic link; {} is not written through a link, so nothing was written",
            self.path.display(),
            self.what
        )
    }
}

impl std::error::Error for LinkRefused {}

/// `Err` carrying [`LinkRefused`] when `p` is a link, live or dangling.
///
/// An entry that cannot be looked at is not refused here: the write that
/// follows meets the same error and reports it.
pub fn refuse_link(p: &Path, what: &'static str) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(std::io::Error::other(LinkRefused {
            path: p.to_path_buf(),
            what,
        })),
        _ => Ok(()),
    }
}

/// [`refuse_link`] for every entry from `root` down to `root.join(rel)`:
/// `.trellis`, then `.trellis/agents`, then `.trellis/agents/x.toml`.
///
/// A link at any directory on the way moves the write as surely as one at
/// the file. `root` itself is not checked; it is the repository. `rel`
/// must be plain names: a `..`, a root or a drive prefix would make the
/// walk check entries the write never passes through, so it is refused as
/// `InvalidInput` rather than walked.
pub fn refuse_links_below(root: &Path, rel: &Path, what: &'static str) -> std::io::Result<()> {
    let mut p = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(name) => {
                p.push(name);
                refuse_link(&p, what)?;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "{} is not a plain path under {}; {what} was not written",
                        rel.display(),
                        root.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Creates `p`, which must not exist in any form, and writes `bytes` to it.
///
/// `create_new` is `O_EXCL`: it fails on any existing entry, a link
/// included, instead of following it, so a link planted after a check is
/// not followed either.
pub fn create_new(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)?;
    f.write_all(bytes)?;
    f.flush()
}

/// Writes `bytes` to `path` through a fresh temporary file beside it and a
/// rename over it, creating or replacing the file.
///
/// The temporary file is made with [`create_new`], and a rename replaces
/// the directory entry and never writes through one, so a link at `path`
/// is replaced rather than followed. Callers still [`refuse_link`] first:
/// replacing a committed link silently is a change to the repository the
/// user did not ask for. An interrupted write leaves the old file or none,
/// never half of one, and the temporary file is removed on failure.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path);
    if let Err(e) = create_new(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// `.<name>.<pid>.<n>.tmp` beside `path`. Unique per process and per call,
/// so a temporary file a crash left behind never makes `create_new` fail on
/// the next write, and two writes in one process never share one.
fn temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = N.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("trellis-core-links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn refused(e: &std::io::Error) -> &LinkRefused {
        e.get_ref()
            .and_then(|i| i.downcast_ref::<LinkRefused>())
            .unwrap_or_else(|| panic!("expected LinkRefused, got {e:?}"))
    }

    #[test]
    fn a_link_anywhere_on_the_way_down_is_refused_and_named() {
        // The risk: checking only the file, so a linked parent directory
        // still moves the write outside the repository.
        let d = scratch("walk");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let root = d.join("repo");
        std::fs::create_dir_all(root.join(".trellis")).unwrap();
        let link = root.join(".trellis/agents");
        if let Err(e) = crate::paths::symlink_for_test(&outside, &link) {
            eprintln!("skipped: {e}");
            return;
        }
        let e = refuse_links_below(&root, Path::new(".trellis/agents/x.toml"), "the import")
            .unwrap_err();
        assert_eq!(refused(&e).path, link);
        assert!(e.to_string().contains("symbolic link"), "{e}");
        assert!(e.to_string().contains("the import"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_dangling_link_is_refused_though_exists_reads_it_as_absent() {
        let d = scratch("dangling");
        let link = d.join("f");
        if let Err(e) = crate::paths::symlink_for_test(&d.join("nowhere"), &link) {
            eprintln!("skipped: {e}");
            return;
        }
        assert!(!link.exists(), "the premise: exists() follows the link");
        let e = refuse_link(&link, "x").unwrap_err();
        assert_eq!(refused(&e).path, link);
        assert!(create_new(&link, b"x").is_err(), "create_new never follows");
        assert!(!d.join("nowhere").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_parent_reference_is_refused_rather_than_walked() {
        // `..` would make the walk check entries the write never meets.
        let d = scratch("parent");
        let e = refuse_links_below(&d, Path::new("a/../../b"), "x").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn replace_file_replaces_and_leaves_no_temporary_file() {
        // A temporary file left beside the target is untracked and would
        // dirty the worktree and block the next run (ADR 0009).
        let d = scratch("replace");
        let p = d.join("f.toml");
        replace_file(&p, b"one").unwrap();
        replace_file(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
