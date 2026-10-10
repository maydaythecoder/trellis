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

/// A shell argv for `script`, as TOML, with literal strings so a Windows
/// path's backslashes are not escapes.
pub(crate) fn sh(script: &str) -> String {
    if cfg!(windows) {
        format!("['cmd', '/C', '{script}']")
    } else {
        format!("['sh', '-c', '{script}']")
    }
}

/// A link at `link` to `target`, or `false` with the reason printed
/// when this host cannot make one (Windows without Developer Mode).
pub(crate) fn link(target: &Path, link: &Path) -> bool {
    match trellis_core::paths::symlink_for_test(target, link) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("{why}");
            false
        }
    }
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
    git_stdout(dir, args);
}

/// [`git_ok`], returning what git printed: for a test that reads the
/// repository back (`log`, `status`, `rev-parse`). A read that failed would
/// otherwise come back as empty output, which reads as "clean" or "no
/// commit" and lets the assertion on it pass or fail for the wrong reason.
pub(crate) fn git_stdout(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Stages everything and commits it, through [`git_ok`].
///
/// It used to spawn bare `git` and drop both exit statuses (#240), so a
/// fixture whose commit failed (nothing staged, no identity) went on with
/// no commit and failed later at an unrelated assertion.
pub(crate) fn git_commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-qm", message]);
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

/// A driver whose every figure differs from the trait's default, so a
/// wrapper that answered the default instead of forwarding is seen (#248).
/// It is never driven.
pub(crate) struct Figures {
    compactions: Vec<trellis_loop::Compacted>,
    requested: Vec<trellis_loop::RequestedCompaction>,
}

/// [`Figures`]: 7 tokens, 4,321 prompt bytes, one compaction eliding 50 B,
/// one requested compaction eliding 60 B, and a last step of 13 output
/// tokens queued 250 ms.
pub(crate) fn figures() -> Figures {
    Figures {
        compactions: vec![trellis_loop::Compacted {
            turn: 2,
            elided_turns: 1,
            elided_bytes: 50,
            request_bytes: 400,
            budget: 500,
            fits: true,
        }],
        requested: vec![trellis_loop::RequestedCompaction {
            turn: 3,
            turns_folded: 1,
            elided_turns: 2,
            elided_bytes: 60,
            bytes_before: 900,
            bytes_after: 840,
        }],
    }
}

impl trellis_loop::Model for Figures {
    async fn next_step(
        &mut self,
        _: &trellis_loop::Transcript,
    ) -> Result<trellis_loop::Step, String> {
        Err("not driven".into())
    }
    fn tokens_used(&self) -> u64 {
        7
    }
    fn prompt_bytes(&self) -> Option<usize> {
        Some(4321)
    }
    fn compactions(&self) -> &[trellis_loop::Compacted] {
        &self.compactions
    }
    fn requested_compactions(&self) -> &[trellis_loop::RequestedCompaction] {
        &self.requested
    }
    fn step_usage(&self) -> trellis_loop::StepUsage {
        trellis_loop::StepUsage {
            completion_tokens: Some(13),
            queue_wait_ms: Some(250),
        }
    }
}
