//! Executed checks, and the only type in the workspace that may assert
//! correctness.
//!
//! Every public entry point here spawns a real process and reports what it
//! did. Nothing in this crate consults a model.

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
    let found = governance::violations(root, rules, files);
    let (failing, advisory): (Vec<_>, Vec<_>) =
        found.iter().partition(|v| v.rank.fails_the_audit());

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
    if found.is_empty() {
        out.push_str(&format!(
            "{} rule(s) checked over {} file(s); none broken
",
            rules.len(),
            files.len()
        ));
    }

    // A real ProcessOutput, because the seal takes nothing else. No
    // subprocess ran — the match is in-process — so the command records what
    // was actually evaluated rather than inventing a shell line that would
    // not reproduce.
    Verdict::from_governance(ProcessOutput {
        command: format!(
            "governance audit: {} rule(s), {} file(s)",
            rules.len(),
            files.len()
        ),
        exit_code: Some(i32::from(!failing.is_empty())),
        failure_count: Some(failing.len() as u32),
        stdout: out,
        stderr: String::new(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
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

    let result = tokio::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

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
