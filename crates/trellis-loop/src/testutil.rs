//! Fixture repositories built once per test process and handed out as
//! independent copies.
//!
//! A test that needs a committed repository used to run `git init`, three
//! `git config`s, `add`, `commit` and `rev-parse` for itself: seven process
//! spawns before the code under test ran, which on Windows under a loaded
//! gate cost seconds per test (#227, #232). A [`Template`] runs those once,
//! reads every byte of the result into memory and deletes it; each
//! [`Template::instance`] writes the bytes into a fresh temporary directory.
//! Nothing is hard-linked and no path is shared, so no test's writes, to
//! the worktree or to `.git/`, can reach the template or another test
//! (`an_instance_shares_no_bytes_with_the_template_or_another_instance`).
//!
//! What the code under test spawns is untouched: a writer still runs in its
//! own throwaway clone of the instance (ADR 0032).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// `git` through `trellis_vcs`, failing the test on a non-zero exit.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
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

/// A repository's directories and files, by path relative to its root.
pub(crate) struct Template {
    /// Every directory, parents before children. Recorded apart from the
    /// files because git needs some that are empty (`.git/refs/tags`).
    dirs: Vec<PathBuf>,
    files: Vec<(PathBuf, Vec<u8>)>,
    head: Option<String>,
}

impl Template {
    /// Runs `build` in an empty temporary directory, snapshots what it left
    /// there, and deletes the directory: a template left on disk would be
    /// a path two tests could both reach.
    pub(crate) fn build(build: impl FnOnce(&Path)) -> Template {
        let d = tempfile::tempdir().expect("tempdir");
        build(d.path());
        let head = trellis_vcs::command()
            .args(["rev-parse", "--verify", "-q", "HEAD"])
            .current_dir(d.path())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        walk(d.path(), d.path(), &mut dirs, &mut files);
        dirs.sort();
        files.sort();
        Template { dirs, files, head }
    }

    /// The template's HEAD commit; every instance has the same one.
    pub(crate) fn head(&self) -> &str {
        self.head
            .as_deref()
            .expect("the template has a commit; build one before asking for HEAD")
    }

    /// A fresh copy of the template in its own temporary directory.
    pub(crate) fn instance(&self) -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        for dir in &self.dirs {
            std::fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        // Dated in the past so that no entry is newer than the index the
        // refresh below writes: a racily clean entry would make git re-hash
        // it later, and a status that rewrites `.git/index` would read as a
        // writer's change to `.git/`.
        let past = SystemTime::now() - Duration::from_secs(10);
        for (path, bytes) in &self.files {
            let p = d.path().join(path);
            std::fs::write(&p, bytes).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&p)
                .and_then(|f| f.set_modified(past))
                .unwrap();
        }
        // The copied index holds the template's stat data (inode, ctime,
        // mtime), none of which matches the copy. Refreshed, the instance
        // is what `git commit` in this directory would have left.
        if self.files.iter().any(|(p, _)| p == Path::new(".git/index")) {
            git(d.path(), &["update-index", "-q", "--refresh"]);
        }
        d
    }
}

fn walk(dir: &Path, base: &Path, dirs: &mut Vec<PathBuf>, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let rel = p.strip_prefix(base).unwrap().to_path_buf();
        if p.is_dir() {
            dirs.push(rel);
            walk(&p, base, dirs, files);
        } else {
            files.push((rel, std::fs::read(&p).unwrap()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> Template {
        Template::build(|root| {
            for args in [
                ["init", "-q"].as_slice(),
                &["config", "core.autocrlf", "false"],
                &["config", "user.email", "t@example.invalid"],
                &["config", "user.name", "t"],
            ] {
                git(root, args);
            }
            std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
            git(root, &["add", "-A"]);
            git(root, &["commit", "-qm", "base"]);
        })
    }

    /// Every file but the index (whose stat data differs per copy by
    /// design), by path and content.
    fn bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        walk(root, root, &mut dirs, &mut files);
        files.retain(|(p, _)| p != Path::new(".git/index"));
        files.sort();
        files
    }

    #[test]
    fn an_instance_shares_no_bytes_with_the_template_or_another_instance() {
        // The risk a shared fixture brings: one test's writes reaching
        // another's repository, so a test passes or fails on what another
        // test did. Writes here go where a hostile writer's would, into the
        // worktree and into `.git/` (config, a ref, an object file).
        let t = committed();
        let a = t.instance();
        let b = t.instance();
        let pristine = bytes(b.path());

        std::fs::write(a.path().join("a.py"), "changed\n").unwrap();
        std::fs::write(a.path().join(".git/config"), "[core]\n").unwrap();
        std::fs::write(a.path().join(".git/refs/heads/planted"), t.head()).unwrap();
        let object = bytes(a.path())
            .into_iter()
            .map(|(p, _)| p)
            .find(|p| p.starts_with(".git/objects") && !p.starts_with(".git/objects/info"))
            .expect("a committed repository has an object");
        std::fs::write(a.path().join(&object), b"not an object").unwrap();

        let c = t.instance();
        assert_eq!(bytes(b.path()), pristine, "a write reached a live instance");
        assert_eq!(bytes(c.path()), pristine, "a write reached the template");
        assert_eq!(git(c.path(), &["status", "--porcelain"]), "");
        assert_eq!(git(c.path(), &["rev-parse", "HEAD"]).trim(), t.head());
    }
}
