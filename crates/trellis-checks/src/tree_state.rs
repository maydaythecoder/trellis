//! What a check's working tree looked like, by metadata, so a write made
//! after the checker exited can be told apart from the checker's own (#172).
//!
//! A passing check is a claim about the tree as it stood when the checker
//! exited. A process the checker left running can still write after that,
//! and the pass would then describe a tree that no longer exists. The
//! leftover is killed (the job or the group, see `run_checker`); what it wrote
//! between the exit and the kill is found by comparing two of these, one
//! taken when the exit is observed and one after the kill.
//!
//! Metadata, not content: hashing every file twice per check would cost a
//! large repository seconds a check. A file or link is its relative path, its
//! kind, length and modification time, and on Unix its inode, mode and
//! change time (`ctime`, which a process cannot set back the way it can
//! `mtime`). On Windows there is no change time in std: a leftover that
//! rewrites a file to the same length and restores its modification time
//! is not seen there.
//!
//! A directory is recorded by its kind and its entries, not by its own
//! times, which carry nothing its entries do not. Its times were seen to
//! differ between the two walks on `.git/logs/refs` in `child_env.rs`,
//! where nothing this crate could see had written under it; the likely
//! cause is the parent's cached copy of a directory's times in a Windows
//! listing, not established.
//!
//! `.git/` at the root is not walked. The working tree is what a check
//! judges, as the loop's worktree digest defines it; trellis's own git
//! calls around a run write under `.git/`, and a change there during a run
//! is the trust guard's to catch (ADR 0023), not a check's.
//!
//! Each entry is kept as two 64-bit hashes, not as its path: a repository
//! with a `.venv` or `node_modules` has hundreds of thousands of entries.
//! The keys are random per process, so no repository can choose paths that
//! collide.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::path::{Path, PathBuf};

/// Past this many entries the walk stops and the state is incomplete,
/// which compares as changed. A tree a leftover keeps growing while it is
/// walked would otherwise hold the check for as long as it grew; no real
/// repository is near it.
const MAX_ENTRIES: usize = 5_000_000;

/// How many changed paths a report names; the count covers the rest.
const NAMED: usize = 5;

/// One walk of the tree under a root.
pub(crate) struct TreeState {
    /// Path hash -> entry hash.
    entries: HashMap<u64, u64>,
    /// The walk stopped at [`MAX_ENTRIES`].
    incomplete: bool,
}

/// What changed between two walks.
pub(crate) struct Changes {
    /// Paths added or changed, first [`NAMED`] of them.
    pub(crate) named: Vec<String>,
    /// Paths added, changed or removed.
    pub(crate) count: usize,
    /// Either walk stopped at [`MAX_ENTRIES`], so nothing can be said.
    pub(crate) incomplete: bool,
}

/// The hashing both walks of one check share.
#[derive(Clone)]
pub(crate) struct Keys(RandomState);

impl Keys {
    pub(crate) fn new() -> Self {
        Self(RandomState::new())
    }

    fn hash(&self, h: impl Hash) -> u64 {
        self.0.hash_one(h)
    }
}

impl TreeState {
    /// Walks everything under `root` but its `.git/`, `.trellis/` included,
    /// without following a link (a link's own metadata is recorded). An
    /// entry that cannot be read is recorded as the error it gave, so one
    /// that becomes readable or unreadable between walks is a change.
    pub(crate) fn take(root: &Path, keys: &Keys) -> Self {
        let mut state = Self {
            entries: HashMap::new(),
            incomplete: false,
        };
        let root_stamp = stamp(root.symlink_metadata());
        state.entries.insert(keys.hash(""), keys.hash(root_stamp));
        let mut dirs = vec![PathBuf::new()];
        while let Some(rel) = dirs.pop() {
            let rd = match std::fs::read_dir(root.join(&rel)) {
                Ok(rd) => rd,
                Err(e) => {
                    // Keyed apart from the directory's own entry, which a
                    // `\0` in a real name cannot collide with.
                    let key = keys.hash((&rel, "\0read_dir"));
                    state.entries.insert(key, keys.hash(e.kind()));
                    continue;
                }
            };
            for entry in rd {
                if state.entries.len() >= MAX_ENTRIES {
                    state.incomplete = true;
                    return state;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        let key = keys.hash((&rel, "\0entry"));
                        state.entries.insert(key, keys.hash(e.kind()));
                        continue;
                    }
                };
                if rel.as_os_str().is_empty() && entry.file_name() == ".git" {
                    continue;
                }
                let path = rel.join(entry.file_name());
                // `DirEntry::metadata` does not follow a link, on either
                // platform, and on Windows costs no extra call.
                let md = entry.metadata();
                if md.as_ref().is_ok_and(|m| m.file_type().is_dir()) {
                    dirs.push(path.clone());
                }
                state.entries.insert(keys.hash(&path), keys.hash(stamp(md)));
            }
        }
        state
    }

    /// A state that could not be taken. It compares as changed.
    pub(crate) fn unknown() -> Self {
        Self {
            entries: HashMap::new(),
            incomplete: true,
        }
    }

    /// What differs in `after`, a second walk of the same root with the
    /// same keys. Only hashes were kept, so the changed paths are named by
    /// walking once more; a removed path is counted and not named.
    pub(crate) fn changes(&self, after: &Self, root: &Path, keys: &Keys) -> Changes {
        let incomplete = self.incomplete || after.incomplete;
        let mut count = 0;
        let mut differs = Vec::new();
        for (path, entry) in &after.entries {
            if self.entries.get(path) != Some(entry) {
                count += 1;
                differs.push(*path);
            }
        }
        count += self
            .entries
            .keys()
            .filter(|p| !after.entries.contains_key(p))
            .count();
        Changes {
            named: if differs.is_empty() {
                Vec::new()
            } else {
                name(root, keys, &differs)
            },
            count,
            incomplete,
        }
    }
}

/// The relative paths whose hashes are in `wanted`, at most [`NAMED`],
/// found by walking `root` once more. Only reached when something changed.
fn name(root: &Path, keys: &Keys, wanted: &[u64]) -> Vec<String> {
    let wanted: std::collections::HashSet<u64> = wanted.iter().copied().collect();
    let mut found = Vec::new();
    if wanted.contains(&keys.hash("")) {
        found.push(".".to_string());
    }
    let mut seen = 0usize;
    let mut dirs = vec![PathBuf::new()];
    while let Some(rel) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            seen += 1;
            if found.len() >= NAMED || seen >= MAX_ENTRIES {
                return found;
            }
            if rel.as_os_str().is_empty() && entry.file_name() == ".git" {
                continue;
            }
            let path = rel.join(entry.file_name());
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path.clone());
            }
            if wanted.contains(&keys.hash(&path)) {
                found.push(path.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found
}

/// What is compared for one entry. The error kind stands in for an entry
/// whose metadata could not be read.
#[derive(Hash)]
enum Stamp {
    Dir,
    Read {
        kind: u8,
        len: u64,
        modified: Option<std::time::SystemTime>,
        #[cfg(unix)]
        unix: (u64, u32, i64, i64),
        #[cfg(windows)]
        attributes: u32,
    },
    Unreadable(std::io::ErrorKind),
}

fn stamp(md: std::io::Result<std::fs::Metadata>) -> Stamp {
    let md = match md {
        Ok(m) => m,
        Err(e) => return Stamp::Unreadable(e.kind()),
    };
    let t = md.file_type();
    // `is_dir` is false for a link to one, which is stamped as a link.
    if t.is_dir() {
        return Stamp::Dir;
    }
    let kind = if t.is_symlink() {
        b'l'
    } else if t.is_file() {
        b'f'
    } else {
        b'o'
    };
    #[cfg(unix)]
    let unix = {
        use std::os::unix::fs::MetadataExt;
        (md.ino(), md.mode(), md.ctime(), md.ctime_nsec())
    };
    #[cfg(windows)]
    let attributes = {
        use std::os::windows::fs::MetadataExt;
        md.file_attributes()
    };
    Stamp::Read {
        kind,
        len: md.len(),
        modified: md.modified().ok(),
        #[cfg(unix)]
        unix,
        #[cfg(windows)]
        attributes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparison `run_checker` rests on: an untouched tree compares
    /// equal, and an appended file, a new file and a removed file each count.
    #[test]
    fn two_walks_differ_exactly_when_the_tree_changed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("pkg")).unwrap();
        std::fs::write(d.path().join("pkg/a.py"), "x = 1\n").unwrap();
        std::fs::write(d.path().join("gone.txt"), "bye").unwrap();
        std::fs::create_dir_all(d.path().join(".git/logs")).unwrap();
        let keys = Keys::new();
        let before = TreeState::take(d.path(), &keys);
        let same = TreeState::take(d.path(), &keys);
        let c = before.changes(&same, d.path(), &keys);
        assert_eq!((c.count, c.incomplete), (0, false));

        std::fs::write(d.path().join("pkg/a.py"), "x = 1\ny = 2\n").unwrap();
        std::fs::write(d.path().join("pkg/new.py"), "").unwrap();
        std::fs::remove_file(d.path().join("gone.txt")).unwrap();
        // Not the working tree: trellis's own git calls write here.
        std::fs::write(d.path().join(".git/logs/HEAD"), "moved").unwrap();
        let after = TreeState::take(d.path(), &keys);
        let c = before.changes(&after, d.path(), &keys);
        assert!(c.named.iter().any(|p| p == "pkg/a.py"), "{:?}", c.named);
        assert!(c.named.iter().any(|p| p == "pkg/new.py"), "{:?}", c.named);
        // a.py, new.py and gone.txt; nothing under `.git/`, and no
        // directory for its own times.
        assert_eq!(c.count, 3, "{:?}", c.named);
    }
}
