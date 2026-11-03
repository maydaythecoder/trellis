//! The repository walk: which files the index is built from.
//!
//! Kept apart from the resolver because the two change for different
//! reasons: this changes when a directory to skip is added, the resolver
//! when a binding rule does. Every consumer that needs the file set reads it
//! from here, so the graph and anything shown beside it see the same files.

use std::path::{Path, PathBuf};

/// Directories never walked. `.trellis/work` holds per-partition worktrees,
/// which are copies of the repository and would double every count.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".trellis",
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    "site-packages",
    ".eggs",
    "build",
    "dist",
];

/// Repository-relative path with forward slashes, whatever the platform.
pub(crate) fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every `.py` file under `root`, excluding vendored and generated trees.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || !e
                    .file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|x| x == "py"))
        .collect();
    // Deterministic ids: the same repository must produce the same index,
    // or `insta` snapshots and committed trails are meaningless.
    out.sort();
    out
}

/// Every first-party Python file, as repository-relative paths.
///
/// The same walk the graph is built from, deliberately: a second walker in
/// another crate would drift from this one, and the drift would be silent —
/// a prompt naming a file the index does not know, or omitting one it does.
///
/// Separators are `/` on every platform, because these paths are handed to a
/// model that must quote them inside JSON, and a Windows `\` is either an
/// invalid escape or a lie about the path.
pub fn discover_relative(root: &Path) -> Vec<String> {
    discover(root).iter().map(|p| rel_path(root, p)).collect()
}

/// Reads every indexable file under `root` as a `(relative path, source)` pair.
pub fn read_sources(root: &Path) -> Vec<(String, String)> {
    discover(root)
        .iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(p).ok()?;
            Some((rel_path(root, p), src))
        })
        .collect()
}
