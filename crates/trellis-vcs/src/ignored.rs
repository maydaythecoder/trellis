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
use std::process::{Command, Stdio};

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
        let mut git = Command::new("git");
        git.args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ])
        .current_dir(root)
        .stdin(Stdio::null());
        trellis_core::secrets::scrub(&mut git);
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
        let is_dir_link =
            std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink() && p.is_dir());
        // A Windows directory link is removed with `remove_dir`; on Unix a
        // link of any kind with `remove_file`.
        let r = if is_dir_link && cfg!(windows) {
            std::fs::remove_dir(&p)
        } else {
            std::fs::remove_file(&p)
        };
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
/// Returns whether the ignored files are as they were: false when a
/// created file could not be removed, and false when any pre-existing one
/// was changed or deleted. A rollback that leaves a changed file behind
/// has not restored the tree, whatever git says about the files it sees
/// (review round 8; before it, this returned true and the run exited as
/// restored).
pub fn undo(root: &Path, changes: &IgnoredChanges, out: &mut dyn Write) -> bool {
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
    let lost: Vec<String> = changes
        .modified
        .iter()
        .chain(&changes.deleted)
        .cloned()
        .collect();
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

/// A model run's ignored files, judged by who changed them.
///
/// The writers this guards (`bash`, custom tools, hooks) share the run with
/// the checks, which run the repository's own tools, and those rewrite
/// their caches (`.mypy_cache`, `__pycache__`, `.pytest_cache`) on every
/// run. One before-and-after over the whole run held the checks' rewrites
/// against the run, and since a changed pre-existing ignored file makes a
/// rollback not restored ([`undo`]), nearly every failed run that ran a
/// check would have exited as unrestored. So the orchestrator brackets each
/// check: [`before_unguarded`](Self::before_unguarded) holds what changed
/// since the last mark against the run, and
/// [`after_unguarded`](Self::after_unguarded) moves the mark past what the
/// check changed. Files created during the run are all removed on rollback,
/// whoever created them, as before.
///
/// What this gives up, stated: a check runs repository code, and a model
/// run can edit that code, so a test the run rewrote can change a
/// pre-existing ignored file and not be held against the run. A run with
/// none of the guarded writers keeps no ledger at all and was already
/// exposed to exactly that. Costs two more listings per check.
pub struct IgnoredLedger {
    original: IgnoredSnapshot,
    mark: IgnoredSnapshot,
    modified: std::collections::BTreeSet<String>,
    deleted: std::collections::BTreeSet<String>,
    /// A listing failed at a bracket: what happened across it is unknown,
    /// so the run's changes cannot be read.
    unread: Option<String>,
}

impl IgnoredLedger {
    pub fn take(root: &Path) -> Result<IgnoredLedger, String> {
        let s = IgnoredSnapshot::take(root)?;
        Ok(IgnoredLedger {
            original: s.clone(),
            mark: s,
            modified: Default::default(),
            deleted: Default::default(),
            unread: None,
        })
    }

    /// What changed between the mark and `now`, held against the run: only
    /// files that existed before it, since one it created is removed anyway.
    /// A pre-existing file that reappears since the mark (a check deleted
    /// it, a guarded writer wrote it back) is a change too: the rollback
    /// does not remove it, since it existed at the start.
    fn attribute(&mut self, now: &IgnoredSnapshot) {
        let d = self.mark.diff(now);
        for p in d.modified.into_iter().chain(d.created) {
            if self.original.files.contains_key(&p) {
                self.modified.insert(p);
            }
        }
        for p in d.deleted {
            if self.original.files.contains_key(&p) {
                self.deleted.insert(p);
            }
        }
    }

    /// Before something whose ignored writes are not the run's (a check).
    pub fn before_unguarded(&mut self, root: &Path) {
        match IgnoredSnapshot::take(root) {
            Ok(now) => {
                self.attribute(&now);
                self.mark = now;
            }
            Err(e) => self.unread = Some(e),
        }
    }

    /// After it. A listing that fails keeps the old mark, so the check's
    /// changes are held against the run: the conservative reading.
    pub fn after_unguarded(&mut self, root: &Path) {
        if let Ok(now) = IgnoredSnapshot::take(root) {
            self.mark = now;
        }
    }

    /// What the run did to ignored files: every file created since it
    /// started, and every pre-existing one a guarded writer changed or
    /// deleted.
    pub fn changes(&mut self, root: &Path) -> Result<IgnoredChanges, String> {
        if let Some(e) = &self.unread {
            return Err(e.clone());
        }
        let now = IgnoredSnapshot::take(root)?;
        self.attribute(&now);
        let created = self.original.diff(&now).created;
        self.mark = now;
        Ok(IgnoredChanges {
            created,
            modified: self
                .modified
                .iter()
                .filter(|p| !self.deleted.contains(*p))
                .cloned()
                .collect(),
            deleted: self.deleted.iter().cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            !undo(root, &c, &mut out),
            "a changed file left behind is not a restored tree"
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
            out.contains("NOT put back") && out.contains("old.py"),
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
        assert!(undo(root, &c, &mut out));
        assert!(!root.join("new.log").exists());
    }

    /// Sets a file's modification time well away from now, so a rewrite
    /// within the same second is still a different stamp.
    fn touch_back(p: &Path) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
        )
        .unwrap();
    }

    #[test]
    fn the_ledger_holds_a_guarded_writer_s_change_against_the_run_and_not_a_check_s() {
        // The risk: a check rewrites its own cache on every run, and one
        // before-and-after over the whole run read that as the run changing
        // a pre-existing ignored file, which is an unrestored rollback.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "*.cache\n*.env\n").unwrap();
        std::fs::write(root.join("mypy.cache"), "a").unwrap();
        std::fs::write(root.join("site.env"), "a").unwrap();
        std::fs::write(root.join("gone.env"), "a").unwrap();
        let mut l = IgnoredLedger::take(root).unwrap();

        // A guarded writer, before the check.
        std::fs::write(root.join("site.env"), "bb").unwrap();
        l.before_unguarded(root);
        // The check.
        std::fs::write(root.join("mypy.cache"), "bbb").unwrap();
        touch_back(&root.join("mypy.cache"));
        std::fs::write(root.join("made.cache"), "c").unwrap();
        l.after_unguarded(root);
        // A guarded writer, after it.
        std::fs::remove_file(root.join("gone.env")).unwrap();

        let c = l.changes(root).unwrap();
        assert_eq!(c.modified, ["site.env"]);
        assert_eq!(c.deleted, ["gone.env"]);
        assert_eq!(c.created, ["made.cache"], "whoever made it, it goes");
    }
}
