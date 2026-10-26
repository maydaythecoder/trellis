//! Fixture repositories built once per test process and handed out as
//! independent copies.
//!
//! A test that needs a committed repository used to run `git init`, three
//! `git config`s, `add`, `commit` and `rev-parse` for itself: seven process
//! spawns before the code under test ran, in tests #227 found bound by
//! process spawns rather than by work (#232). A [`Template`] runs those once,
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

/// A repository with one module, `app/util.py` defining `helper`, and no
/// git: the smallest tree the loop can index and edit.
pub(crate) fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(d.path().join("app")).unwrap();
    std::fs::write(
        d.path().join("app/util.py"),
        "def helper():\n    return 1\n",
    )
    .unwrap();
    d
}

/// A ten-iteration budget with every other limit at its default.
pub(crate) fn budget() -> crate::budget::Budget {
    crate::budget::Budget {
        max_iterations: 10,
        ..crate::budget::Budget::default()
    }
}

/// A checker that really runs and exits `code`.
///
/// The point of the loop is that a `Verdict` can only come from an
/// executed process, so a convergence test that faked one would be
/// testing the opposite of the property. The platform shell is the one
/// executable guaranteed present wherever these tests run; trailing paths
/// are appended by `typecheck` and ignored by `exit`.
pub(crate) fn checker(code: u8) -> trellis_checks::CheckConfig {
    let argv = if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), format!("exit {code}")]
    } else {
        vec!["sh".into(), "-c".into(), format!("exit {code}")]
    };
    trellis_checks::CheckConfig {
        typecheck: argv,
        ..trellis_checks::CheckConfig::default()
    }
}

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

/// One `turn` event's fields, as `name -> value as text`. A field the event
/// did not record is absent, which is what tells an unreported figure from
/// a 0.
pub(crate) type TurnFields = std::collections::BTreeMap<String, String>;

/// The `turn`, `model call` and `plan` events emitted on this thread while
/// [`TurnEvents::install`]'s guard lives.
///
/// Hand-written over `tracing` alone rather than `tracing-subscriber`, which
/// this crate does not depend on. One process-wide subscriber hands each
/// event to the capture installed on the emitting thread, if any, so a
/// `#[tokio::test]` (current-thread) sees its own run's events and no other
/// test's. Not a scoped default per test: `tracing` caches whether a
/// callsite is wanted, and with scoped defaults coming and going on
/// parallel test threads a callsite could be cached as unwanted while a
/// capture was installed, and its events silently dropped.
#[derive(Clone, Default)]
pub(crate) struct TurnEvents(std::sync::Arc<std::sync::Mutex<Vec<TurnFields>>>);

thread_local! {
    static CAPTURE: std::cell::RefCell<Option<TurnEvents>> = const { std::cell::RefCell::new(None) };
}

/// Ends a capture when dropped.
pub(crate) struct Installed;

impl Drop for Installed {
    fn drop(&mut self) {
        CAPTURE.with(|c| c.borrow_mut().take());
    }
}

impl TurnEvents {
    pub(crate) fn install(&self) -> Installed {
        static GLOBAL: std::sync::Once = std::sync::Once::new();
        GLOBAL.call_once(|| {
            tracing::subscriber::set_global_default(Router)
                .expect("no other global subscriber in trellis-loop's tests");
        });
        CAPTURE.with(|c| *c.borrow_mut() = Some(self.clone()));
        Installed
    }

    /// The `turn` events only.
    pub(crate) fn taken(&self) -> Vec<TurnFields> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.get("message").map(String::as_str) == Some("turn"))
            .cloned()
            .collect()
    }

    /// The `plan` events only (ADR 0058 §5).
    pub(crate) fn plans(&self) -> Vec<TurnFields> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.get("message").map(String::as_str) == Some("plan"))
            .cloned()
            .collect()
    }

    /// Every kept event, `model call`, `turn` and `plan`, in the order
    /// emitted.
    pub(crate) fn taken_in_order(&self) -> Vec<TurnFields> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Fields(TurnFields);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), format!("{v:?}"));
    }
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        self.0.insert(f.name().into(), v.into());
    }
}

/// The process-wide subscriber behind [`TurnEvents`].
struct Router;

impl tracing::Subscriber for Router {
    /// A `turn` event is an info event. Everything else stays disabled, so
    /// the fields of the rest (a request body at trace) are not built in
    /// every other test for nothing.
    fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
        m.is_event() && *m.level() == tracing::Level::INFO
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let Some(capture) = CAPTURE.with(|c| c.borrow().clone()) else {
            return;
        };
        let mut f = Fields::default();
        event.record(&mut f);
        // The test `TurnCapture` in trellis-cli's rpc applies. A `model
        // call` is kept too, in stream order with the turns, so a test can
        // hold that one precedes each turn (#614).
        let message = f.0.get("message").map(String::as_str);
        let is_turn =
            message == Some("turn") && f.0.contains_key("turn") && f.0.contains_key("call");
        let is_model_call = message == Some("model call") && f.0.contains_key("turn");
        let is_plan = message == Some("plan") && f.0.contains_key("turn");
        if is_turn || is_model_call || is_plan {
            capture.0.lock().unwrap().push(f.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
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
