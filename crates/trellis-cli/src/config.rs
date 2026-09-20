//! Reading `.trellis/config.toml`.
//!
//! The file existed and was documented before anything read it, which meant a
//! repository could configure its checks and be quietly ignored — the worst
//! version of this, because the run still produces confident verdicts, just
//! about the wrong commands.
//!
//! Parsed here in the binary rather than in a library: it is the one place
//! that already depends on every crate the file configures, and `anyhow`
//! belongs at this edge.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use trellis_checks::CheckConfig;
use trellis_loop::{Budget, CostRegime};

#[derive(Debug, Default, Deserialize)]
pub struct RepoConfig {
    #[serde(default)]
    pub checks: Option<ChecksSection>,
    #[serde(default)]
    pub budget: Option<BudgetSection>,
    #[serde(default)]
    #[allow(dead_code)]
    pub agents: Option<AgentsSection>,
}

#[derive(Debug, Deserialize)]
pub struct ChecksSection {
    pub typecheck: Option<Vec<String>>,
    pub tests: Option<Vec<String>>,
    pub python: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BudgetSection {
    pub max_iterations: Option<u32>,
    pub max_wall_clock_s: Option<u64>,
    pub max_tokens: Option<u64>,
    pub regime: Option<String>,
}

/// Parsed but not yet consumed: multi-agent execution is v1.1.
///
/// It is read now so that a repository carrying an `[agents]` section does not
/// fail to load on a version that cannot act on it — a config file that errors
/// because of a section meant for a later release is a worse failure than one
/// that is quietly inert.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AgentsSection {
    /// `1`, or the string `"auto"`. Never derived from CPU count (S18).
    pub count: Option<toml::Value>,
    pub failure_policy: Option<String>,
}

impl RepoConfig {
    /// Loads `<root>/.trellis/config.toml`, or the defaults when absent.
    ///
    /// A malformed file is an error, not a fallback to defaults. Silently
    /// running the default `mypy` because a user's TOML had a typo is exactly
    /// the confident-verdict-about-the-wrong-thing failure the file exists to
    /// prevent.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(".trellis").join("config.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        toml::from_str(&text).with_context(|| format!("{} is not valid TOML", path.display()))
    }

    pub fn checks(&self) -> CheckConfig {
        let mut c = CheckConfig::default();
        if let Some(s) = &self.checks {
            if let Some(v) = &s.typecheck {
                c.typecheck = v.clone();
            }
            if let Some(v) = &s.tests {
                c.tests = v.clone();
            }
            if let Some(v) = &s.python {
                c.python = v.clone();
            }
        }
        c
    }

    pub fn budget(&self) -> Budget {
        let mut b = Budget::default();
        if let Some(s) = &self.budget {
            if let Some(v) = s.max_iterations {
                b.max_iterations = v;
            }
            if let Some(v) = s.max_wall_clock_s {
                b.max_wall_clock = Duration::from_secs(v);
            }
            if let Some(v) = s.max_tokens {
                b.max_tokens = v;
            }
            // An unrecognised regime keeps the default rather than guessing.
            // The two differ in which resource is scarce, and guessing wrong
            // means budgeting the wrong one.
            if let Some(v) = &s.regime {
                b.regime = match v.as_str() {
                    "metered" => CostRegime::Metered,
                    "local" => CostRegime::Local,
                    _ => b.regime,
                };
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir.join(".trellis")).unwrap();
        std::fs::write(dir.join(".trellis/config.toml"), body).unwrap();
    }

    #[test]
    fn an_absent_file_yields_defaults() {
        let d = tempfile::tempdir().unwrap();
        let c = RepoConfig::load(d.path()).unwrap();
        assert_eq!(c.checks().typecheck, vec!["mypy".to_string()]);
    }

    #[test]
    fn configured_commands_are_used_verbatim() {
        // The whole point: a repository whose checks need flags must get them.
        // Running bare `mypy` against a project whose dependencies are absent
        // produces a failing verdict about missing stubs, not about the code.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "[checks]\ntypecheck = [\"mypy\", \"--ignore-missing-imports\"]\ntests = [\"pytest\", \"-x\"]\n",
        );
        let c = RepoConfig::load(d.path()).unwrap();
        assert_eq!(c.checks().typecheck, ["mypy", "--ignore-missing-imports"]);
        assert_eq!(c.checks().tests, ["pytest", "-x"]);
        // Unset keys keep their defaults rather than becoming empty.
        assert_eq!(c.checks().python, "python");
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_silent_default() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "[checks]\ntypecheck = \"not a list\"\n");
        let e = RepoConfig::load(d.path()).unwrap_err();
        assert!(format!("{e}").contains("config.toml"), "{e}");
    }

    #[test]
    fn budget_values_are_read_and_the_regime_falls_back_rather_than_guessing() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "[budget]\nmax_iterations = 7\nmax_wall_clock_s = 90\nregime = \"nonsense\"\n",
        );
        let c = RepoConfig::load(d.path()).unwrap();
        let b = c.budget();
        assert_eq!(b.max_iterations, 7);
        assert_eq!(b.max_wall_clock, Duration::from_secs(90));
        assert_eq!(b.max_tokens, Budget::default().max_tokens);
        assert_eq!(b.regime, Budget::default().regime);
    }
}
