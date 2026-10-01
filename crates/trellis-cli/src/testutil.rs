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
        git_ok(dir, args);
    }
}

/// `git <args>` in `dir` through `trellis_vcs::command()`, panicking with
/// git's stderr unless it exits 0.
///
/// A fixture step that failed used to be discarded with its exit status
/// (#238), so a repository missing its commit failed later, at whatever
/// the test asserted, and read as a fault in the code under test. Through
/// `trellis_vcs` so a fixture runs git as trellis does, with no hook or
/// fsmonitor from the environment's configuration.
pub(crate) fn git_ok(dir: &Path, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "fixture step `git {}` in {} failed ({}): {}",
        args.join(" "),
        dir.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
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

/// The trust store every test in this process uses, never the user's.
///
/// One path for the whole process, because the override is set once and
/// the first call wins: a second module choosing its own path would find
/// its entries written to the first's. Entries are keyed by repository
/// root, and each test has its own.
///
/// The store is emptied once, when this process first opens it (#199).
/// It is named by process id and was never removed, and every fixture
/// repository is named by the same id with the same bytes in it, so its
/// fingerprint is the same in every run. Windows reuses process ids, so
/// a test process that inherited an earlier one's id found a store that
/// already trusted its own repository at the same path with the same
/// fingerprint, and `hook_input_refusal_...`'s untrusted first run ran
/// the hook. `scratch` deletes its directory first for the same reason.
/// Once per process, not per call: tests trust in parallel, and a wipe
/// on every call would erase another test's entry mid-test.
pub(crate) fn trust_store() -> trellis_loop::TrustStore {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("trellis-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        trellis_loop::TrustStore::use_for_this_process(dir.join("trust.json"));
    });
    trellis_loop::TrustStore::user().unwrap()
}

/// Trusts `dir` as its configuration now stands, as `trellis trust` does.
pub(crate) fn trust(dir: &Path) {
    let cfg = crate::config::RepoConfig::load(dir).unwrap();
    trust_store()
        .trust(dir, &crate::tooling::fingerprint(dir, &cfg))
        .unwrap();
}
