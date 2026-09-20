//! The orchestrator: single owner of the index, serializer of writes.
//!
//! It owns the `Index` outright — no `Arc<RwLock<Index>>`, no shared mutable
//! graph. Everything else sends a message and awaits a `oneshot` reply, which
//! buys write/check serialization for free: the owner cannot process two
//! messages at once, so there is no lock anyone can forget to take.
//!
//! The cost is that a wide change serializes. That is a real ceiling and S5
//! says to measure it rather than assume it away.
//!
//! **Why an actor rather than `&mut Index` threaded through.** Agents are
//! concurrent tasks (v1.1) and checks are subprocesses, so the index has to be
//! reachable from several tasks at once. An actor makes "one writer" a
//! property of the topology; a borrow makes it a property of whoever wrote the
//! call chain.

use crate::tools::{EditOutcome, ToolResult};
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};
use trellis_checks::{CheckConfig, CheckKind};
use trellis_index::Session;

/// A request to the orchestrator. Every variant carries its own reply channel.
pub enum Request {
    BlastRadius {
        symbol: String,
        reply: oneshot::Sender<ToolResult>,
    },
    ReadFile {
        path: String,
        reply: oneshot::Sender<ToolResult>,
    },
    ApplyEdit {
        path: String,
        old: String,
        new: String,
        reply: oneshot::Sender<ToolResult>,
    },
    RunChecks {
        paths: Vec<String>,
        kinds: Vec<CheckKind>,
        reply: oneshot::Sender<ToolResult>,
    },
    /// Files written during this run, for the rollback path and the trail.
    Touched { reply: oneshot::Sender<Vec<String>> },
}

/// Handle to a running orchestrator. Cloneable; every clone talks to the one
/// owner.
#[derive(Clone)]
pub struct Handle(mpsc::Sender<Request>);

impl Handle {
    async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Request,
        on_dead: impl FnOnce() -> T,
    ) -> T {
        let (tx, rx) = oneshot::channel();
        if self.0.send(make(tx)).await.is_err() {
            return on_dead();
        }
        rx.await.unwrap_or_else(|_| on_dead())
    }

    pub async fn blast_radius(&self, symbol: &str) -> ToolResult {
        let symbol = symbol.to_string();
        self.ask(
            |reply| Request::BlastRadius { symbol, reply },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn read_file(&self, path: &str) -> ToolResult {
        let path = path.to_string();
        self.ask(
            |reply| Request::ReadFile { path, reply },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn edit(&self, path: &str, old: &str, new: &str) -> ToolResult {
        let (path, old, new) = (path.to_string(), old.to_string(), new.to_string());
        self.ask(
            |reply| Request::ApplyEdit {
                path,
                old,
                new,
                reply,
            },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn run_checks(&self, paths: Vec<String>, kinds: Vec<CheckKind>) -> ToolResult {
        self.ask(
            |reply| Request::RunChecks {
                paths,
                kinds,
                reply,
            },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn touched(&self) -> Vec<String> {
        self.ask(|reply| Request::Touched { reply }, Vec::new).await
    }
}

/// The owner. Holds the index and the only write path to the working tree.
pub struct Orchestrator {
    root: PathBuf,
    session: Session,
    checks: CheckConfig,
    touched: Vec<String>,
    /// Project rules, enforced here because this is the only write path to
    /// the working tree. A rule enforced anywhere else is a rule a tool call
    /// can route around.
    governance: crate::governance::Governance,
    /// The repository's rule documents, for the governance audit.
    rules: Vec<trellis_checks::governance::Rule>,
}

impl Orchestrator {
    pub fn new(root: &Path, checks: CheckConfig) -> Self {
        Self {
            root: root.to_path_buf(),
            session: Session::open(root),
            checks,
            touched: Vec::new(),
            governance: crate::governance::Governance::default(),
            rules: Vec::new(),
        }
    }

    /// Supplies the rule documents a governance audit evaluates.
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<trellis_checks::governance::Rule>) -> Self {
        self.rules = rules;
        self
    }

    /// Applies the repository's rules to this run.
    #[must_use]
    pub fn with_governance(mut self, g: crate::governance::Governance) -> Self {
        self.governance = g;
        self
    }

    /// Starts the actor on the current runtime and returns a handle.
    ///
    /// The channel is bounded: an agent that outruns the orchestrator should
    /// wait rather than queue unboundedly, because the queue would otherwise
    /// become an invisible second budget.
    pub fn spawn(self) -> (Handle, tokio::task::JoinHandle<Self>) {
        let (tx, rx) = mpsc::channel(32);
        let join = tokio::spawn(self.serve(rx));
        (Handle(tx), join)
    }

    #[tracing::instrument(skip_all)]
    async fn serve(mut self, mut rx: mpsc::Receiver<Request>) -> Self {
        while let Some(req) = rx.recv().await {
            match req {
                Request::BlastRadius { symbol, reply } => {
                    let _ = reply.send(self.blast_radius(&symbol));
                }
                Request::ReadFile { path, reply } => {
                    let _ = reply.send(self.read_file(&path));
                }
                Request::ApplyEdit {
                    path,
                    old,
                    new,
                    reply,
                } => {
                    let _ = reply.send(self.apply_edit(&path, &old, &new));
                }
                Request::RunChecks {
                    paths,
                    kinds,
                    reply,
                } => {
                    let _ = reply.send(self.run_checks(&paths, &kinds).await);
                }
                Request::Touched { reply } => {
                    let _ = reply.send(self.touched.clone());
                }
            }
        }
        self
    }

    pub fn index(&self) -> &trellis_index::Index {
        self.session.index()
    }

    fn blast_radius(&self, symbol: &str) -> ToolResult {
        let index = self.session.index();
        let qualified = |s: &trellis_index::SymbolEntry| {
            format!(
                "{}.{}",
                index.files[s.file.index()].module,
                index.interner.resolve(s.name)
            )
        };
        let matches: Vec<_> = index
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let q = qualified(s);
                q == symbol
                    || q.ends_with(&format!(".{symbol}"))
                    || index.interner.resolve(s.name) == symbol
            })
            .map(|(i, _)| trellis_core::SymbolId::from_index(i))
            .collect();

        match matches.len() {
            0 => ToolResult::Error(format!(
                "no first-party definition matches `{symbol}`; only definitions \
                 inside the repository root are indexed"
            )),
            1 => ToolResult::Radius(trellis_index::radius::blast_radius(index, matches[0])),
            n => ToolResult::Error(format!(
                "`{symbol}` matches {n} definitions; qualify it further"
            )),
        }
    }

    fn read_file(&self, path: &str) -> ToolResult {
        match self.resolve_in_repo(path) {
            Err(e) => ToolResult::Error(e),
            Ok(full) => match std::fs::read_to_string(&full) {
                Ok(s) => ToolResult::FileContent(s),
                Err(e) => ToolResult::Error(format!("cannot read {path}: {e}")),
            },
        }
    }

    /// Applies a literal replacement, then reindexes the edited file.
    ///
    /// `old` must appear exactly once. A unique anchor is the difference
    /// between an edit the trail can describe and one that silently hit the
    /// wrong occurrence — and the model cannot check this for itself, because
    /// checking it is reading the file it is about to change.
    fn apply_edit(&mut self, path: &str, old: &str, new: &str) -> ToolResult {
        // Project rules are checked before the file is read, so a denied path
        // is refused identically whether or not it exists — otherwise the
        // refusal message leaks which denied files are present.
        if let Some(rule) = self.governance.denies_write(path) {
            return ToolResult::Error(format!(
                concat!(
                    "edit refused: `{}` is denied by the rule `{}` in ",
                    ".trellis/config.toml. This is the repository's own rule, ",
                    "not a guess — change the rule if the file really should be ",
                    "editable."
                ),
                path,
                rule.as_written()
            ));
        }

        let full = match self.resolve_in_repo(path) {
            Ok(f) => f,
            Err(e) => return ToolResult::Error(e),
        };
        let src = match std::fs::read_to_string(&full) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Creating files is not supported, and saying so beats a
                // confusing read error. `Session::update` only knows paths
                // that were present when the session opened, so a new file
                // would be written to disk and never enter the graph — the
                // exact tree/graph divergence the reindex-on-edit test exists
                // to prevent.
                return ToolResult::Error(format!(
                    concat!(
                        "{} does not exist. This tool edits existing files ",
                        "only; creating files would leave the index unaware of ",
                        "them."
                    ),
                    path
                ));
            }
            Err(e) => return ToolResult::Error(format!("cannot read {path}: {e}")),
        };

        // An edit that replaces text with itself is refused, not applied.
        //
        // It would otherwise "succeed": the anchor occurs once, the identical
        // bytes are written back, and the model is told `applied: true` about
        // a change that did not happen. Three consequences, none of them
        // visible at the time. The model believes it acted and reasons from
        // that. The file joins `touched`, so it is recorded in the trail as
        // proposed and is restored by a rollback that has nothing to restore.
        // And an iteration of the budget is spent on a no-op.
        //
        // Found in the S7 floor run, where a weak model emitted
        // `edit{old: "156", new: "156"}` repeatedly — the failure this refusal
        // makes legible rather than letting it read as progress.
        if old == new {
            return ToolResult::Error(format!(
                concat!(
                    "edit refused: `old` and `new` are identical in {}, so this ",
                    "would change nothing. If the file is already correct, run ",
                    "the checks instead of editing."
                ),
                path
            ));
        }

        let hits = src.matches(old).count();
        if hits != 1 {
            return ToolResult::Error(format!(
                "edit anchor occurs {hits} times in {path}; it must occur exactly once \
                 so the edit is unambiguous"
            ));
        }
        let at = src.find(old).expect("occurrence counted");
        let updated = format!("{}{}{}", &src[..at], new, &src[at + old.len()..]);

        if let Err(e) = std::fs::write(&full, &updated) {
            return ToolResult::Error(format!("cannot write {path}: {e}"));
        }
        if !self.touched.iter().any(|p| p == path) {
            self.touched.push(path.to_string());
        }
        // Keep the graph in step with the tree. A stale index would let the
        // next blast radius describe code that no longer exists, so a refusal
        // here is reported rather than dropped — the write already happened.
        if !self.session.update(path, &updated) {
            return ToolResult::Error(format!(
                concat!(
                    "{} was written but is not part of the index session, so ",
                    "the graph is now stale. Re-open the repository before ",
                    "trusting any blast radius."
                ),
                path
            ));
        }

        ToolResult::Edit(EditOutcome {
            path: path.to_string(),
            applied: true,
            span: (at, at + new.len()),
        })
    }

    async fn run_checks(&self, paths: &[String], kinds: &[CheckKind]) -> ToolResult {
        // `requested` is kept distinct from the defaulted list below. Shadowing
        // the two together made the governance arm's "no paths given" branch
        // unreachable — `paths` was already `["."]` by the time it ran — so the
        // audit scoped to a directory it could not read and passed having
        // checked nothing. Every measured model calls this with no paths.
        let requested = paths;
        let paths: Vec<PathBuf> = if requested.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            requested.iter().map(PathBuf::from).collect()
        };
        // One verdict per call. Asking for several kinds runs the first that
        // fails, because a type error makes the test result uninformative
        // rather than additionally informative.
        for kind in kinds {
            let v = match kind {
                CheckKind::Typecheck => {
                    trellis_checks::typecheck(&self.root, &self.checks, &paths).await
                }
                CheckKind::Tests => {
                    trellis_checks::run_tests(&self.root, &self.checks, &paths).await
                }
                CheckKind::Import => {
                    let module = paths.first().and_then(|p| p.to_str()).unwrap_or(".");
                    trellis_checks::import_check(&self.root, &self.checks, module).await
                }
                CheckKind::Governance => {
                    // Audited over the files the run touched, not the whole
                    // tree: a rule broken somewhere this run never went is a
                    // pre-existing condition, and failing a run for it would
                    // make every run in a non-compliant repository fail.
                    let scope: Vec<String> = if requested.is_empty() {
                        self.touched.clone()
                    } else {
                        requested.to_vec()
                    };
                    trellis_checks::governance_audit(&self.root, &self.rules, &scope)
                }
            };
            if !v.passed() {
                return ToolResult::Checks(v);
            }
            if kind == kinds.last().expect("non-empty") {
                return ToolResult::Checks(v);
            }
        }
        ToolResult::Error("no check kinds requested".into())
    }

    /// Rejects anything that escapes the repository root.
    ///
    /// The model proposes these paths and the grammar guarantees only that
    /// they are well-formed strings (S7 metric 3). `..` is the cheap way a
    /// well-formed path becomes a write outside the tree the rollback covers.
    fn resolve_in_repo(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if p.is_absolute() || p.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "path `{rel}` must be relative to the repository root and must not \
                 contain `..`"
            ));
        }
        Ok(self.root.join(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("app/main.py"),
            "from app.util import helper\n\ndef go():\n    return helper()\n",
        )
        .unwrap();
        d
    }

    #[tokio::test]
    async fn a_radius_comes_back_with_its_coverage() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        match h.blast_radius("helper").await {
            ToolResult::Radius(r) => {
                assert_eq!(r.certain.len(), 1, "go calls helper");
                // S2: there is no API that hands back reach without limits.
                // The reach and the doubt are asserted separately because
                // there is no total to assert — see ADR 0014.
                assert_eq!(r.coverage.reached(), 1);
                assert!(
                    r.coverage.touched_files > 0,
                    "the doubt figure must arrive with the scale it ranges over"
                );
            }
            other => panic!("expected a radius, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_edit_updates_the_graph_the_next_radius_sees() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let before = match h.blast_radius("helper").await {
            ToolResult::Radius(r) => r.certain.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(before, 1);

        let out = h
            .edit(
                "app/main.py",
                "    return helper()",
                "    helper()\n    return helper()",
            )
            .await;
        assert!(matches!(out, ToolResult::Edit(e) if e.applied));

        // A stale index would still say 1 here, and every later claim about
        // blast radius would be describing code that no longer exists.
        match h.blast_radius("helper").await {
            ToolResult::Radius(r) => assert_eq!(r.certain.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn an_ambiguous_edit_anchor_is_refused() {
        let d = repo();
        std::fs::write(
            d.path().join("app/main.py"),
            "def a():\n    return 1\n\ndef b():\n    return 1\n",
        )
        .unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h.edit("app/main.py", "    return 1", "    return 2").await;
        match out {
            ToolResult::Error(e) => assert!(e.contains("occurs 2 times"), "{e}"),
            other => panic!("an ambiguous anchor must not be applied: {other:?}"),
        }
        // And nothing was written.
        let after = std::fs::read_to_string(d.path().join("app/main.py")).unwrap();
        assert!(after.contains("def a():\n    return 1"));
    }

    #[tokio::test]
    async fn paths_escaping_the_repository_are_refused() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        for bad in ["../outside.py", "app/../../outside.py"] {
            match h.read_file(bad).await {
                ToolResult::Error(e) => assert!(e.contains(".."), "{e}"),
                other => panic!("`{bad}` must be refused, got {other:?}"),
            }
            match h.edit(bad, "a", "b").await {
                ToolResult::Error(_) => {}
                other => panic!("`{bad}` must be refused for writes too, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn creating_a_file_is_refused_in_terms_that_say_why() {
        // The failure this prevents is silent: a new file would be written to
        // disk and never enter the index, so every later blast radius would
        // be computed against a tree it does not describe.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        match h
            .edit(
                "app/brand_new.py",
                "",
                "def x():
    pass
",
            )
            .await
        {
            ToolResult::Error(e) => {
                assert!(e.contains("does not exist"), "{e}");
                assert!(
                    e.contains("index"),
                    "the message must say why, not just that it failed: {e}"
                );
            }
            other => panic!("creating a file must be refused, got {other:?}"),
        }
        assert!(!d.path().join("app/brand_new.py").exists());
    }

    #[tokio::test]
    async fn a_denied_path_is_refused_by_the_orchestrator_not_asked_about() {
        // The rule has to bind here, at the only write path to the tree. A
        // rule that lives in the prompt is a suggestion, and "do not edit
        // generated files" is worthless as a suggestion.
        let d = repo();
        std::fs::create_dir_all(d.path().join("vendor")).unwrap();
        std::fs::write(
            d.path().join("vendor/lib.py"),
            "def x():
    return 1
",
        )
        .unwrap();

        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_governance(crate::governance::Governance::new(&["vendor/".into()]))
            .spawn();

        match h.edit("vendor/lib.py", "return 1", "return 2").await {
            ToolResult::Error(e) => {
                assert!(e.contains("vendor/"), "the refusal names the rule: {e}");
                assert!(!e.contains("  "), "{e:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(
            std::fs::read_to_string(d.path().join("vendor/lib.py"))
                .unwrap()
                .contains("return 1"),
            "the file must be untouched"
        );
        assert!(h.touched().await.is_empty());
        // And an allowed path still works, or the rule has eaten the tool.
        assert!(matches!(
            h.edit("app/util.py", "return 1", "return 2").await,
            ToolResult::Edit(_)
        ));
    }

    fn forbid_rule(root: &std::path::Path, pattern: &str, scope: &str) {
        let g = root.join(".trellis").join("governance");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(
            g.join("r.toml"),
            format!(
                "name = \"No direct DB access\"
rank = \"non-negotiable\"
                 forbid = {{ pattern = \"{pattern}\" }}
scope = [\"{scope}\"]
"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn a_broken_non_negotiable_fails_the_verdict_through_the_check_dispatch() {
        // The audit function has its own unit test. This one covers the wiring,
        // which is where S20 actually happened: a patch described in three
        // documents, tested at the layer below, and never reached by a run.
        // `last_check_passed` is what convergence reads, so a governance
        // violation that does not arrive as a failing Verdict is a violation a
        // model converges straight past.
        let d = repo();
        forbid_rule(d.path(), "psycopg2.connect", "app/");
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():
    return psycopg2.connect('x')
",
        )
        .unwrap();

        let rules = trellis_checks::governance::load(d.path()).expect("rules load");
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_rules(rules)
            .spawn();

        match h
            .run_checks(vec!["app/util.py".into()], vec![CheckKind::Governance])
            .await
        {
            ToolResult::Checks(v) => {
                assert!(
                    !v.passed(),
                    "a non-negotiable violation must fail the audit"
                );
                assert_eq!(v.detail().kind, CheckKind::Governance);
                assert!(
                    v.detail().stdout.contains("psycopg2.connect"),
                    "the failure names what was broken: {:?}",
                    v.detail().stdout
                );
            }
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_audit_scopes_to_what_the_run_touched_not_the_whole_tree() {
        // A rule broken somewhere this run never went is a pre-existing
        // condition. Failing the run for it would make every run in a
        // non-compliant repository fail, which trains a user to ignore
        // governance entirely — the opposite of the point.
        let d = repo();
        forbid_rule(d.path(), "psycopg2.connect", "app/");
        std::fs::write(
            d.path().join("app/main.py"),
            "def go():
    return psycopg2.connect('x')
",
        )
        .unwrap();

        let rules = trellis_checks::governance::load(d.path()).expect("rules load");
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_rules(rules)
            .spawn();

        // The run touches util.py, which is clean. main.py is dirty and
        // untouched.
        assert!(matches!(
            h.edit("app/util.py", "return 1", "return 2").await,
            ToolResult::Edit(_)
        ));
        match h.run_checks(vec![], vec![CheckKind::Governance]).await {
            ToolResult::Checks(v) => assert!(
                v.passed(),
                "a violation in a file the run never touched is not this run's: {:?}",
                v.detail().stdout
            ),
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_denied_path_is_refused_identically_whether_or_not_it_exists() {
        // The check runs before the file is read, so the message cannot be
        // used to learn which denied files are present. A small thing, but the
        // alternative is a refusal that differs by whether the file is there,
        // which is an oracle nobody intended to build.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_governance(crate::governance::Governance::new(&["secrets/".into()]))
            .spawn();

        let missing = h.edit("secrets/nope.py", "a", "b").await;
        match missing {
            ToolResult::Error(e) => {
                assert!(e.contains("secrets/"), "{e}");
                assert!(
                    !e.contains("does not exist"),
                    "a denied path must not report whether it exists: {e}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_edit_that_changes_nothing_is_refused_not_applied() {
        // S7 floor run: a weak model emitted edit{old: "156", new: "156"} over
        // and over. Without this the anchor occurs once, the identical bytes
        // are written back, and the model is told the edit applied — so it
        // reasons from a change that never happened, the file is recorded in
        // the trail as proposed, and a rollback restores something untouched.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let before = std::fs::read_to_string(d.path().join("app/util.py")).unwrap();
        match h.edit("app/util.py", "return 1", "return 1").await {
            ToolResult::Error(e) => {
                assert!(e.contains("change nothing"), "{e}");
                assert!(
                    !e.contains("  "),
                    "a message the model reads must not carry formatter debris: {e:?}"
                );
            }
            other => panic!("a no-op edit must be refused, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(d.path().join("app/util.py")).unwrap(),
            before
        );
        assert!(
            h.touched().await.is_empty(),
            "a refused edit must not enter the rollback set or the trail's              proposed list"
        );
    }

    #[tokio::test]
    async fn a_missing_file_is_refused_in_one_readable_sentence() {
        // The message is assembled from several source lines, which is how it
        // acquired a run of twenty spaces the last time it was written. It
        // goes to the model and to the user, so it is asserted, not assumed.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        match h.edit("app/nope.py", "a", "b").await {
            ToolResult::Error(e) => {
                assert!(e.contains("does not exist"), "{e}");
                assert!(!e.contains("  "), "{e:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edited_files_are_recorded_for_rollback() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        assert!(h.touched().await.is_empty());

        h.edit("app/util.py", "return 1", "return 2").await;
        h.edit("app/util.py", "return 2", "return 3").await;
        assert_eq!(
            h.touched().await,
            vec!["app/util.py"],
            "a file edited twice is one file to restore, not two"
        );
    }
}
