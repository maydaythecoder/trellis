//! Executed checks, and the only type in the workspace that may assert
//! correctness.
//!
//! Every public entry point here spawns a real process and reports what it
//! did. Nothing in this crate consults a model.

pub mod coverage;
pub mod governance;
mod verdict;

pub use verdict::{AuditMode, CheckDetail, CheckKind, Verdict};

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

/// Raw result of a process that ran to completion (or failed to start).
///
/// `pub` so callers can see what happened, but note that constructing one
/// buys nothing on its own: the `Verdict` constructors that consume it are
/// `pub(crate)`.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub command: String,
    /// `None` when the process could not be spawned or was killed. Never
    /// reads as success.
    pub exit_code: Option<i32>,
    pub failure_count: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// How to invoke this repository's checks, from `.trellis/config.toml`.
///
/// Commands are configured rather than detected because a wrong guess here
/// produces a confident verdict about the wrong thing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckConfig {
    /// e.g. `["mypy", "--output", "json"]`
    pub typecheck: Vec<String>,
    /// e.g. `["pytest", "-q"]`
    pub tests: Vec<String>,
    /// Interpreter used for the import check.
    pub python: String,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            // `--no-incremental` is a correctness flag, not a preference, and
            // removing it for speed reintroduces S20.
            //
            // mypy's incremental cache keys a file on its size and mtime. A
            // `git checkout` that restores a previous version of the same byte
            // length therefore looks unchanged to it, and mypy reports the
            // cached result for code it never read. `Snapshot::rollback` does
            // exactly that checkout on every non-converging run, and
            // `-> int:` to `-> str:` is both length-preserving and the kind of
            // edit an agent makes constantly.
            //
            // The resulting `Verdict` is truthful about everything it asserts
            // structurally — a process ran, it exited zero — and wrong about
            // the only thing anyone cares about. The seal guarantees a verdict
            // came from an executed process; it cannot guarantee that process
            // read the file. This flag is what closes that gap for mypy.
            typecheck: vec!["mypy".into(), "--no-incremental".into()],
            tests: vec!["pytest".into(), "-q".into()],
            python: "python".into(),
        }
    }
}

/// Runs the configured type checker over `paths`.
#[tracing::instrument(skip(cfg), fields(n_paths = paths.len()))]
pub async fn typecheck(root: &Path, cfg: &CheckConfig, paths: &[PathBuf]) -> Verdict {
    let mut argv = cfg.typecheck.clone();
    argv.extend(paths.iter().map(|p| p.display().to_string()));
    Verdict::from_typecheck(run(root, &argv).await)
}

/// Runs the configured test runner over `paths`.
///
/// `paths` should already be narrowed to the tests covering the change; see
/// `docs/TECH_SPIKE.md` on deriving that coverage map.
#[tracing::instrument(skip(cfg), fields(n_paths = paths.len()))]
pub async fn run_tests(root: &Path, cfg: &CheckConfig, paths: &[PathBuf]) -> Verdict {
    let mut argv = cfg.tests.clone();
    argv.extend(paths.iter().map(|p| p.display().to_string()));
    Verdict::from_tests(run(root, &argv).await)
}

/// Imports `module` in a fresh subprocess.
///
/// Catches the breakage class that type checking misses and tests do not
/// reach: syntax errors in an untested branch, import cycles, a decorator
/// that raises at module scope.
#[tracing::instrument(skip(cfg))]
pub async fn import_check(root: &Path, cfg: &CheckConfig, module: &str) -> Verdict {
    let argv = vec![cfg.python.clone(), "-c".into(), format!("import {module}")];
    Verdict::from_import(run(root, &argv).await)
}

/// Audits the repository's own governance rules.
///
/// **Produces a `Verdict`, which is the whole point.** `last_check_passed`
/// reads verdicts, so a violation reported only into the model's context is
/// one a model can converge past — the rule would be a suggestion again.
///
/// The evaluation is a substring search and nothing more. A governance check
/// that exercised judgment would be the thing under audit auditing itself,
/// which is Constraint 2 by another route.
///
/// Rank decides the outcome and only the outcome: a `non-negotiable`
/// violation fails, a `nice-to-have` is reported and does not. `failure_count`
/// counts only the failing ones, so a run's own summary does not inflate.
#[tracing::instrument(skip(rules, files))]
pub fn governance_audit(root: &Path, rules: &[governance::Rule], files: &[String]) -> Verdict {
    let started = Instant::now();
    let scan = governance::violations(root, rules, files);
    let found = &scan.violations;
    let (failing, advisory): (Vec<_>, Vec<_>) =
        found.iter().partition(|v| v.rank.fails_the_audit());
    // Asked to audit files a rule actually applies to, and unable to read any
    // of them. Passing there would report compliance that was never
    // established, and `passed()` is what convergence reads.
    //
    // Keyed on `unreadable`, deliberately not on `files`: a run that edited
    // files no `forbid` rule scopes to has an empty `read` too, and failing it
    // would fail every run in a repository whose rules are all `deny-write` —
    // exactly the "a rule broken somewhere this run never went" failure the
    // orchestrator's scoping exists to prevent, arriving by the other door.
    let blind = scan.read.is_empty() && !scan.unreadable.is_empty();

    let mut out = String::new();
    for v in &failing {
        out.push_str(&format!(
            "VIOLATION [{}] {}:{}  {}
",
            v.rule, v.file, v.line, v.text
        ));
    }
    for v in &advisory {
        out.push_str(&format!(
            "advisory  [{}] {}:{}  {}
",
            v.rule, v.file, v.line, v.text
        ));
    }
    if !scan.unreadable.is_empty() {
        out.push_str(&format!(
            "NOT AUDITED: {} file(s) a rule applies to could not be read: {}
",
            scan.unreadable.len(),
            scan.unreadable.join(", ")
        ));
    }
    if blind {
        out.push_str(&format!(
            concat!(
                "this audit establishes nothing: {} file(s) a rule applies to ",
                "and none could be read
"
            ),
            scan.unreadable.len()
        ));
    } else if found.is_empty() {
        // Counted from what was read, not from what was requested.
        out.push_str(&format!(
            "{} rule(s) checked over {} file(s); none broken
",
            rules.len(),
            scan.read.len()
        ));
    }

    // A real ProcessOutput, because the seal takes nothing else. No
    // subprocess ran — the match is in-process — so the command records what
    // was actually evaluated rather than inventing a shell line that would
    // not reproduce.
    Verdict::from_governance(ProcessOutput {
        command: format!(
            "governance audit: {} rule(s), {} file(s) read of {} in scope",
            rules.len(),
            scan.read.len(),
            files.len()
        ),
        exit_code: Some(i32::from(!failing.is_empty() || blind)),
        failure_count: Some(failing.len() as u32),
        stdout: out,
        stderr: String::new(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Whether any `*.py` file sits under `<root>/.trellis/`, links followed
/// as the trust digest follows them, to a bounded depth so a link cycle
/// ends.
fn dot_trellis_holds_python(root: &Path) -> bool {
    fn any_py(dir: &Path, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        rd.filter_map(Result::ok).any(|e| {
            let p = e.path();
            match std::fs::metadata(&p) {
                Ok(m) if m.is_dir() => any_py(&p, depth + 1),
                Ok(_) => p.extension().is_some_and(|x| x.eq_ignore_ascii_case("py")),
                Err(_) => false,
            }
        })
    }
    any_py(&root.join(".trellis"), 0)
}

async fn run(root: &Path, argv: &[String]) -> ProcessOutput {
    let started = Instant::now();
    let command = argv.join(" ");
    let Some((program, args)) = argv.split_first() else {
        return ProcessOutput {
            command,
            exit_code: None,
            failure_count: None,
            stdout: String::new(),
            stderr: "empty command".into(),
            duration_ms: 0,
        };
    };

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A check's output becomes a verdict's detail, and from there a trail:
    // a checker that prints its environment must find no key in it.
    trellis_core::secrets::scrub(cmd.as_std_mut());
    // A suite that imports code under `.trellis/` would write
    // `__pycache__` there, which the trust digest covers, and untrust the
    // repository mid-run (ADR 0023 §4). Only then: with no Python under
    // `.trellis/` nothing a check imports can write there, and a large
    // suite keeps the bytecode cache that saves it recompiling every file
    // on every check.
    if dot_trellis_holds_python(root) {
        cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    }
    let result = cmd.output().await;

    match result {
        Ok(o) => ProcessOutput {
            command,
            exit_code: o.status.code(),
            failure_count: None, // TODO(v0.2): parse structured output per tool.
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
        // A check that could not run is not a check that passed.
        Err(e) => ProcessOutput {
            command,
            exit_code: None,
            failure_count: None,
            stdout: String::new(),
            stderr: format!("failed to spawn: {e}"),
            duration_ms: started.elapsed().as_millis() as u64,
        },
    }
}

#[cfg(test)]
mod default_config_tests {
    use super::*;

    /// S20: mypy's incremental cache made a `Verdict` assert a pass about code
    /// the checker never read.
    ///
    /// The flag looks like a performance preference, which is why this test
    /// exists: it states in one place that removing it is a correctness
    /// regression, and names what goes wrong. The reproduction is in
    /// `RISK_REGISTER.md` under S20 — check a broken file, fix it, restore it
    /// with `git checkout`, check again, and mypy reports the cached pass
    /// because the restored file has the same size and a fresh mtime.
    ///
    /// `Snapshot::rollback` performs exactly that restore on every
    /// non-converging run, so without this flag the hazard is not an edge
    /// case; it is the normal path.
    #[test]
    fn s20_the_default_type_checker_is_invoked_cache_free() {
        let argv = CheckConfig::default().typecheck;
        assert!(
            argv.iter().any(|a| a == "--no-incremental"),
            "the default mypy invocation must defeat the incremental cache; \
             without it a rollback-restored file can return a stale pass. \
             argv was {argv:?}"
        );
    }

    /// The flag protects a default, not a mandate.
    ///
    /// A repository that configures its own checker takes responsibility for
    /// its cache behaviour, and TRELLIS must not rewrite the command it was
    /// given — a harness that silently appends flags to a user's check command
    /// produces verdicts about an invocation the user never wrote.
    /// Q-17 asks whether a harness could ever verify a command is cache-free;
    /// it cannot today, so the honest position is to run what was configured.
    #[test]
    fn a_configured_checker_is_run_verbatim_flags_and_all() {
        let cfg = CheckConfig {
            typecheck: vec!["pyright".into()],
            ..CheckConfig::default()
        };
        assert_eq!(cfg.typecheck, ["pyright"]);
    }
}

#[cfg(test)]
mod governance_audit_tests {
    use super::*;

    fn repo(rule: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join(".trellis/governance");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("r.toml"), rule).unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        d
    }

    /// The gate item: "a governance rule can fail an audit, mechanically."
    ///
    /// Failing means producing a `Verdict` that does not pass, because that
    /// is the only thing convergence reads. A violation printed into the
    /// model's context and nowhere else is one the model converges past.
    #[test]
    fn a_non_negotiable_violation_fails_the_verdict() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "x = psycopg2.connect('s')\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(!v.passed(), "a broken non-negotiable rule must fail");
        assert_eq!(v.detail().kind, CheckKind::Governance);
        assert_eq!(v.detail().failure_count, Some(1));
        assert!(v.detail().stdout.contains("VIOLATION [No direct DB]"));
        assert!(v.detail().stdout.contains("app/h.py:1"));
    }

    #[test]
    fn an_audit_that_could_read_nothing_it_was_given_does_not_pass() {
        // The defect this test exists for: the orchestrator shadowed its
        // `paths` with `["."]` before the governance arm ran, so the audit was
        // handed a directory, `read_to_string` failed, the failure was
        // swallowed by a `continue`, and the verdict passed reporting
        // "1 rule(s) checked over 1 file(s); none broken". Every measured model
        // calls run_checks with no paths, so that was the live path, and a
        // non-negotiable rule could not fail in it.
        let d = repo(
            "name = \"No direct DB\"
rank = \"non-negotiable\"
             forbid = { pattern = \"psycopg2.connect\" }
",
        );
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &[".".to_string()]);
        assert!(
            !v.passed(),
            "claiming compliance over a path that could not be read is the              bare `complete` the brief forbids, wearing a rule's clothes"
        );
        assert!(
            v.detail().stdout.contains("establishes nothing"),
            "{:?}",
            v.detail().stdout
        );
        assert!(
            !v.detail().stdout.contains("establishes nothing:  "),
            "the fifth recurrence of the cargo fmt string-continuation defect              would land here: {:?}",
            v.detail().stdout
        );
        assert!(
            !v.detail().stdout.contains("none broken"),
            "it must not also claim a clean audit: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_run_outside_every_rules_scope_passes_rather_than_failing_blind() {
        // The regression the blind rule nearly introduced. A file no `forbid`
        // rule applies to is neither read nor unreadable, so keying "this
        // audit established nothing" on an empty `read` would fail every run
        // that edited a file outside every scope.
        let d = repo(
            "name = \"No direct DB\"
rank = \"non-negotiable\"
             forbid = { pattern = \"psycopg2.connect\" }
scope = [\"app/handlers/\"]
",
        );
        std::fs::create_dir_all(d.path().join("app/repo")).unwrap();
        std::fs::write(
            d.path().join("app/repo/db.py"),
            "x = psycopg2.connect('s')
",
        )
        .unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/repo/db.py".to_string()]);
        assert!(
            v.passed(),
            "the repository layer is supposed to reach the database; failing              here forbids the architecture the rule exists to protect: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_repository_whose_rules_are_all_deny_write_passes_the_audit() {
        // `deny-write` is enforced at the orchestrator's write path, not here.
        // Such a repository has no `forbid` rule to evaluate, so every audit
        // reads zero files — and must still pass, or governance breaks every
        // run in the repositories most likely to configure it. The wedge
        // fixture is exactly this shape.
        let d = repo(
            "name = \"Vendored code is not ours\"
rank = \"non-negotiable\"
             deny-write = { paths = [\"vendor/\"] }
",
        );
        std::fs::write(
            d.path().join("app/h.py"),
            "x = 1
",
        )
        .unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed(), "{:?}", v.detail().stdout);
    }

    #[test]
    fn an_unreadable_file_alongside_readable_ones_is_named_not_skipped() {
        let d = repo(
            "name = \"No direct DB\"
rank = \"non-negotiable\"
             forbid = { pattern = \"psycopg2.connect\" }
",
        );
        std::fs::write(
            d.path().join("app/ok.py"),
            "x = 1
",
        )
        .unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(
            d.path(),
            &rules,
            &["app/ok.py".to_string(), "app/gone.py".to_string()],
        );
        // One file was read and is clean, so the audit did establish
        // something and passes — but it must say what it could not see.
        assert!(v.passed());
        assert!(
            v.detail().stdout.contains("NOT AUDITED") && v.detail().stdout.contains("app/gone.py"),
            "{:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn the_file_count_reports_what_was_read_not_what_was_requested() {
        let d = repo(
            "name = \"No direct DB\"
rank = \"non-negotiable\"
             forbid = { pattern = \"psycopg2.connect\" }
",
        );
        std::fs::write(
            d.path().join("app/ok.py"),
            "x = 1
",
        )
        .unwrap();
        let rules = governance::load(d.path()).unwrap();
        let v = governance_audit(
            d.path(),
            &rules,
            &["app/ok.py".to_string(), "app/gone.py".to_string()],
        );
        assert!(
            v.detail().stdout.contains("over 1 file(s)"),
            "two requested, one readable: the coverage claim comes from the              read, or it is a count of intentions: {:?}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_nice_to_have_violation_is_reported_and_passes() {
        // If a nice-to-have failed the audit the ranking would mean nothing:
        // every rule would be non-negotiable in practice.
        let d = repo(
            "name = \"Prefer pathlib\"\nrank = \"nice-to-have\"\n\
             forbid = { pattern = \"os.path.join\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "p = os.path.join(a, b)\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed(), "a nice-to-have must not fail the audit");
        assert_eq!(v.detail().failure_count, Some(0));
        assert!(
            v.detail().stdout.contains("advisory  [Prefer pathlib]"),
            "and must still be reported, or ranking it was pointless: {}",
            v.detail().stdout
        );
    }

    #[test]
    fn a_clean_repository_passes_and_says_what_was_checked() {
        let d = repo(
            "name = \"No direct DB\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n",
        );
        std::fs::write(d.path().join("app/h.py"), "from app.repo import get\n").unwrap();
        let rules = governance::load(d.path()).unwrap();

        let v = governance_audit(d.path(), &rules, &["app/h.py".to_string()]);
        assert!(v.passed());
        // "Passed" with no indication of what was examined is the kind of
        // clean bill S2 warns about.
        assert!(
            v.detail()
                .stdout
                .contains("1 rule(s) checked over 1 file(s)")
        );
    }

    #[test]
    fn no_rules_passes_without_implying_anything_was_verified() {
        let d = tempfile::tempdir().unwrap();
        let v = governance_audit(d.path(), &[], &[]);
        assert!(v.passed());
        assert!(v.detail().stdout.contains("0 rule(s)"));
    }
}

#[cfg(test)]
mod bytecode_tests {
    use super::*;

    fn echo_flag() -> Vec<String> {
        if cfg!(windows) {
            ["cmd", "/C", "echo [%PYTHONDONTWRITEBYTECODE%]"]
        } else {
            ["sh", "-c", "echo [$PYTHONDONTWRITEBYTECODE]"]
        }
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// The risk: a test suite that imports code under `.trellis/` wrote
    /// `__pycache__/*.pyc` there. The trust digest covers bytecode (a
    /// sourceless pyc is importable), so a grounded check untrusted the
    /// repository, and with the per-spawn trust recheck (ADR 0023 §4) it
    /// would void the run at the next hook: trellis's own checker, not the
    /// model, breaking trust.
    #[tokio::test]
    async fn a_check_runs_without_bytecode_writes_when_dot_trellis_holds_python() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/hooks")).unwrap();
        std::fs::write(d.path().join(".trellis/hooks/gate.py"), "x = 1\n").unwrap();
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
    }

    /// The cost is paid only where it buys something: a repository with no
    /// Python under `.trellis/` keeps its test suite's bytecode cache.
    #[tokio::test]
    async fn a_check_keeps_bytecode_caching_when_dot_trellis_holds_no_python() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(d.path().join(".trellis/config.toml"), "").unwrap();
        let cfg = CheckConfig {
            tests: echo_flag(),
            ..CheckConfig::default()
        };
        let v = run_tests(d.path(), &cfg, &[]).await;
        assert!(!v.detail().stdout.contains("[1]"), "{}", v.detail().stdout);
    }
}
