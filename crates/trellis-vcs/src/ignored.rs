//! Files git ignores, watched around something that can write them.
//!
//! `git status` does not list ignored files, and the rollback's
//! `git clean -fd` does not remove them, so a writer that can reach any
//! path (a process agent's command; in a model run `bash`, a custom tool or
//! a hook) could leave `.venv/.../sitecustomize.py`, or an ignored path
//! under `.trellis/`, behind a rollback, where a later check that runs the
//! repository's Python would execute it (ADR 0026 §11, ADR 0023 §5). A
//! snapshot of every ignored file with its stat, taken before and compared
//! after, finds what was created, changed or deleted, and what was created
//! can be removed. Moved here from the process-agent code so model runs use
//! the same one.

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

/// What `stat` says about one file, enough to tell that it changed without
/// reading it: length and modification time, and on Unix the inode and the
/// change time, which a process cannot set back the way it can `mtime`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime: Option<std::time::SystemTime>,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: (i64, i64),
}

impl Stamp {
    fn of(p: &Path) -> Option<Stamp> {
        // Not followed: a link is recorded as the link, and removing one
        // the command created removes the link, not what it points at.
        let m = std::fs::symlink_metadata(p).ok()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Stamp {
            len: m.len(),
            mtime: m.modified().ok(),
            #[cfg(unix)]
            ino: m.ino(),
            #[cfg(unix)]
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
}

/// Every untracked file git ignores, with its [`Stamp`], taken just before
/// the command starts and again just after it ends.
///
/// `git status` does not list ignored files, so without this a command
/// could write `.venv/.../sitecustomize.py`, an ignored path under
/// `.trellis/`, or an ignored path `deny_write` names, and neither the
/// breach test nor governance nor the rollback (`git clean -fd` leaves
/// ignored files) would see it (ADR 0026 §11). Compared by stat rather than
/// by content, so a large `.venv` costs one `stat` per file, not a hash;
/// what that misses is stated in the ADR.
#[derive(Clone)]
pub struct IgnoredSnapshot {
    files: std::collections::BTreeMap<String, Stamp>,
}

/// What the command did to ignored files.
#[derive(Debug, Default)]
pub struct IgnoredChanges {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl IgnoredChanges {
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.created
            .iter()
            .chain(&self.modified)
            .chain(&self.deleted)
    }

    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (word, list) in [
            ("created", &self.created),
            ("modified", &self.modified),
            ("deleted", &self.deleted),
        ] {
            if !list.is_empty() {
                parts.push(format!("{word} {}", shown(list)));
            }
        }
        parts.join("; ")
    }
}

/// Up to ten names, then a count: a `pip install` into `.venv` is
/// thousands of files, and the report is read by a person.
pub fn shown(list: &[String]) -> String {
    const MAX: usize = 10;
    let mut s = list
        .iter()
        .take(MAX)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if list.len() > MAX {
        s.push_str(&format!(" and {} more", list.len() - MAX));
    }
    s
}

impl IgnoredSnapshot {
    pub fn take(root: &Path) -> Result<IgnoredSnapshot, String> {
        let mut git = crate::command();
        git.args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ])
        .current_dir(root)
        .stdin(Stdio::null());
        let o = git
            .output()
            .map_err(|e| format!("git ls-files could not run: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "git ls-files --ignored failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        let text = String::from_utf8_lossy(&o.stdout);
        let files = text
            .split('\0')
            .filter(|p| !p.is_empty())
            .filter_map(|p| Some((p.to_string(), Stamp::of(&root.join(p))?)))
            .collect();
        Ok(IgnoredSnapshot { files })
    }

    pub fn diff(&self, after: &IgnoredSnapshot) -> IgnoredChanges {
        let mut c = IgnoredChanges::default();
        for (p, s) in &after.files {
            match self.files.get(p) {
                None => c.created.push(p.clone()),
                Some(was) if was != s => c.modified.push(p.clone()),
                Some(_) => {}
            }
        }
        for p in self.files.keys() {
            if !after.files.contains_key(p) {
                c.deleted.push(p.clone());
            }
        }
        c
    }
}

/// Removes the ignored files the command created, then any directory that
/// removal left empty, up to (not including) the root. Returns what could
/// not be removed. A directory is removed only when empty, so one that held
/// anything else stays; one that was empty before the command and held only
/// what it created goes too, which git would not notice either way.
pub fn remove_created(root: &Path, created: &[String]) -> Vec<String> {
    let mut failed = Vec::new();
    for rel in created {
        let p = root.join(rel);
        // `remove_file` removes a file, or a link of any kind on Unix. A
        // Windows directory link takes `remove_dir`, which removes the link
        // itself; tried second, so a dangling one (which `is_dir` reads as
        // not a directory) goes too, and a real directory stays unless it
        // is empty.
        let r = std::fs::remove_file(&p).or_else(|e| match std::fs::remove_dir(&p) {
            Ok(()) => Ok(()),
            Err(_) => Err(e),
        });
        match r {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                failed.push(format!("{rel}: {e}"));
                continue;
            }
        }
        let mut dir = p.parent();
        while let Some(d) = dir {
            if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    failed
}

/// The ignored-file half of a rollback: removes what `changes` says was
/// created, and names what cannot be undone, since no copy of an ignored
/// file is kept: one that existed before and was changed or deleted. Run
/// before the snapshot's own rollback, which then removes the directories
/// git tracks that the removal emptied.
///
/// Returns whether the rollback may call the tree restored. False when a
/// created file could not be removed, when a pre-existing one was deleted
/// (a loss the user has to be told of, not a cache rewritten), and when a
/// pre-existing one was changed that `protected` names: an ignored path
/// under `.trellis/` or one `deny_write` covers, a write the run was
/// refused anyway.
///
/// Any other pre-existing file that was changed is named and left as it
/// is, and does not count (review round 9). The tracked tree is restored,
/// which is what ADR 0009 promises; and every writer that runs pytest or
/// mypy rewrites their caches, so counting it (round 8) ended nearly every
/// failed run as not restored and stopped `work`.
pub fn undo(
    root: &Path,
    changes: &IgnoredChanges,
    protected: &dyn Fn(&str) -> bool,
    out: &mut dyn Write,
) -> bool {
    let failed = remove_created(root, &changes.created);
    if !changes.created.is_empty() {
        let _ = writeln!(
            out,
            "ignored:  removed what the run created: {}",
            shown(&changes.created)
        );
    }
    for f in &failed {
        let _ = writeln!(out, "ignored:  could NOT be removed: {f}");
    }
    let (lost, left): (Vec<String>, Vec<String>) =
        changes.modified.iter().cloned().partition(|p| protected(p));
    let lost: Vec<String> = lost.into_iter().chain(changes.deleted.clone()).collect();
    if !left.is_empty() {
        let _ = writeln!(
            out,
            "ignored:  changed by the run and left as it is (trellis keeps no copy of \
             ignored files; the tracked tree is restored): {}",
            shown(&left)
        );
    }
    if !lost.is_empty() {
        let _ = writeln!(
            out,
            "ignored:  NOT put back (trellis keeps no copy of ignored files), so the \
             tree is NOT restored: {}",
            shown(&lost)
        );
    }
    failed.is_empty() && lost.is_empty()
}

/// A snapshot taken before a run, compared now: what the run did to
/// ignored files, or why that could not be read. A listing that fails is
/// not read as "every ignored file deleted", and nothing is removed on its
/// strength.
pub fn changes_since(root: &Path, before: &IgnoredSnapshot) -> Result<IgnoredChanges, String> {
    IgnoredSnapshot::take(root).map(|after| before.diff(&after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_created_ignored_file_is_found_and_removed_and_a_changed_one_is_named() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::write(
            root.join(".gitignore"),
            ".venv/
",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".venv/lib")).unwrap();
        std::fs::write(root.join(".venv/lib/old.py"), "x = 1\n").unwrap();
        let before = IgnoredSnapshot::take(root).unwrap();

        std::fs::create_dir_all(root.join(".venv/lib/site")).unwrap();
        std::fs::write(root.join(".venv/lib/site/sitecustomize.py"), "import os\n").unwrap();
        std::fs::write(root.join(".venv/lib/old.py"), "x = 22\n").unwrap();
        let c = changes_since(root, &before).unwrap();
        assert_eq!(c.created, [".venv/lib/site/sitecustomize.py"]);
        assert_eq!(c.modified, [".venv/lib/old.py"]);

        let mut out = Vec::new();
        assert!(
            undo(root, &c, &|_| false, &mut out),
            "an unprotected changed file is named, and the tree is restored"
        );
        assert!(
            !root.join(".venv/lib/site").exists(),
            "emptied directory removed"
        );
        assert!(
            root.join(".venv/lib/old.py").exists(),
            "pre-existing file stays"
        );
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("left as it is") && out.contains("old.py"),
            "{out}"
        );
    }

    #[test]
    fn only_created_files_are_undone_and_the_tree_is_restored() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        let before = IgnoredSnapshot::take(root).unwrap();
        std::fs::write(root.join("new.log"), "x").unwrap();
        let c = changes_since(root, &before).unwrap();
        let mut out = Vec::new();
        assert!(undo(root, &c, &|_| false, &mut out));
        assert!(!root.join("new.log").exists());
    }

    #[test]
    fn a_deleted_file_or_a_changed_protected_one_is_not_restored() {
        // Loss is still loss: a pre-existing ignored file deleted, or one
        // the rules protect changed, cannot be put back and says so.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "*.env\n").unwrap();
        std::fs::write(root.join("gone.env"), "a").unwrap();
        std::fs::write(root.join("kept.env"), "a").unwrap();
        let before = IgnoredSnapshot::take(root).unwrap();
        std::fs::remove_file(root.join("gone.env")).unwrap();
        std::fs::write(root.join("kept.env"), "bb").unwrap();
        let c = changes_since(root, &before).unwrap();

        let mut out = Vec::new();
        let deleted_only = IgnoredChanges {
            deleted: c.deleted.clone(),
            ..IgnoredChanges::default()
        };
        assert!(!undo(root, &deleted_only, &|_| false, &mut out));
        let changed_only = IgnoredChanges {
            modified: c.modified.clone(),
            ..IgnoredChanges::default()
        };
        assert!(undo(root, &changed_only, &|_| false, &mut out));
        assert!(!undo(root, &changed_only, &|p| p == "kept.env", &mut out));
    }
}
