//! Executed checks, and the only type in the workspace that may assert
//! correctness.
//!
//! Every public entry point here spawns a real process and reports what it
//! did. Nothing in this crate consults a model.

pub mod verdict;

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
            typecheck: vec!["mypy".into()],
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
    let argv = vec![
        cfg.python.clone(),
        "-c".into(),
        format!("import {module}"),
    ];
    Verdict::from_import(run(root, &argv).await)
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
