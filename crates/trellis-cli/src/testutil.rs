//! Fixtures shared by this crate's tests.
//!
//! Five modules built the same temporary git repository five times, and by
//! the time the quality audit's Pass 6 surveyed it, **two of the seven copies
//! in the workspace had lost `core.autocrlf false`** — including one written
//! during that same audit. Duplication is cheaper than the wrong abstraction
//! right up until a copy drifts, and this one had.
//!
//! It unifies five of the seven. `tests/cli.rs` is a separate crate and
//! `trellis-vcs` another, so both keep their own; a helper that reached them
//! would need a shared dev-dependency crate, which is more structure than two
//! fixtures are worth.

use std::path::{Path, PathBuf};

/// A temporary directory unique to this process.
///
/// The process id is not decoration. These fixtures `remove_dir_all` then
/// `create_dir_all` a fixed location, and two test processes running at once
/// — which is exactly what `cargo mutants -j N` does — delete each other's
/// repository mid-test and fail for a reason that has nothing to do with the
/// code under test. That cost one mutation run before it was found.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trellis-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `git init` plus the configuration every fixture here needs.
///
/// `core.autocrlf false` is the one that matters and the one that went
/// missing. Under a global `autocrlf=true`, git rewrites LF to CRLF on
/// checkout and normalises on `add` — so a byte-for-byte assertion measures
/// git's line-ending policy rather than whether rollback restored the file,
/// and a test asserting a clean `git status` can see its own untouched files
/// reported as modified.
pub(crate) fn git_init(dir: &Path) {
    for args in [
        ["init", "-q"].as_slice(),
        &["config", "user.email", "t@example.invalid"],
        &["config", "user.name", "t"],
        &["config", "core.autocrlf", "false"],
    ] {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must be on PATH to run these tests");
    }
}

/// Stages everything and commits it.
pub(crate) fn git_commit_all(dir: &Path, message: &str) {
    for args in [["add", "-A"].as_slice(), &["commit", "-qm", message]] {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
    }
}
