//! Project rules as documents the agent reads *and* conditions an audit can
//! fail against.
//!
//! **The audit produces a `Verdict` or it is advisory.** `last_check_passed`
//! is what convergence reads, and it reads verdicts. A violation printed into
//! the model's context but never entering a verdict is one a model can
//! converge straight past — the rule is a suggestion again. So a governance
//! audit runs a real process, over real files, and its result is sealed like
//! any other check.
//!
//! **The enforcement mechanism stays dumb on purpose.** A governance check
//! that is itself a model judgment reintroduces Constraint 2: the thing
//! deciding whether a rule was broken would be the thing being checked. These
//! are pattern matches over file contents. A model may *explain* a violation;
//! it never *detects* one.
//!
//! Two mechanisms, deliberately not one abstraction:
//!
//! - `deny_write` refuses an edit at the orchestrator's write path, before
//!   anything reaches disk.
//! - `forbid` fails an audit after the fact, over the files as they stand.
//!
//! They have different consequences — one prevents, one reports — and a
//! single "rule" type covering both would hide which you were getting.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How seriously a rule is meant.
///
/// The ranking has exactly one hard behaviour and it is the failure: a
/// `NonNegotiable` violation fails the audit, a `NiceToHave` violation is
/// reported and does not. Anything subtler would be the harness arbitrating
/// between rules the user ranked, which is the user's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rank {
    NonNegotiable,
    NiceToHave,
}

impl Rank {
    pub fn fails_the_audit(self) -> bool {
        matches!(self, Rank::NonNegotiable)
    }
}

/// How compliance is evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    /// A substring that must not appear in the scoped files.
    Forbid { pattern: String },
    /// Paths no run may edit. Enforced at the write path rather than here;
    /// the audit reports it so `trellis check` lists every rule in force.
    DenyWrite { paths: Vec<String> },
}

/// One rule, as `.trellis/governance/<name>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rule {
    pub name: String,
    pub rank: Rank,
    /// Prose the agent reads. The rule exists to be understood, not only
    /// enforced — a refusal a contributor cannot act on is a wall.
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub check: Mechanism,
    /// Files the rule applies to, as path prefixes. Empty means everywhere.
    #[serde(default)]
    pub scope: Vec<String>,
    /// File name the rule was read from, for naming it in a failure.
    #[serde(skip)]
    pub source: String,
}

/// One rule broken, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: String,
    pub rank: Rank,
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// Reads `<root>/.trellis/governance/*.toml`.
///
/// A malformed rule is an error, never a skip. A governance file that silently
/// did not load would leave a user believing a rule was in force while every
/// run ignored it — the same failure as a config typo, with worse
/// consequences.
pub fn load(root: &Path) -> Result<Vec<Rule>, GovernanceError> {
    let dir = root.join(".trellis").join("governance");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();

    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p)
            .map_err(|e| GovernanceError::Unreadable(p.display().to_string(), e.to_string()))?;
        let mut rule: Rule = toml::from_str(&text)
            .map_err(|e| GovernanceError::Malformed(p.display().to_string(), e.to_string()))?;
        rule.source = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        out.push(rule);
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    #[error("{0} could not be read: {1}")]
    Unreadable(String, String),
    #[error("{0} is not a valid governance rule: {1}")]
    Malformed(String, String),
}

/// Every violation of every `forbid` rule, over the files as they stand.
///
/// Mechanical: a substring search. No judgment is exercised and none is
/// available to exercise.
pub fn violations(root: &Path, rules: &[Rule], files: &[String]) -> Vec<Violation> {
    let mut out = Vec::new();
    for rule in rules {
        let Mechanism::Forbid { pattern } = &rule.check else {
            continue;
        };
        for file in files {
            if !in_scope(rule, file) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(root.join(file)) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if line.contains(pattern.as_str()) {
                    out.push(Violation {
                        rule: rule.name.clone(),
                        rank: rule.rank,
                        file: file.clone(),
                        line: i + 1,
                        text: line.trim().chars().take(100).collect(),
                    });
                }
            }
        }
    }
    out
}

fn in_scope(rule: &Rule, file: &str) -> bool {
    if rule.scope.is_empty() {
        return true;
    }
    let norm = file.replace('\\', "/");
    rule.scope.iter().any(|s| norm.starts_with(s.as_str()))
}

/// Paths every `deny-write` rule forbids, for the orchestrator's write path.
pub fn deny_write_paths(rules: &[Rule]) -> Vec<String> {
    rules
        .iter()
        .filter_map(|r| match &r.check {
            Mechanism::DenyWrite { paths } => Some(paths.clone()),
            Mechanism::Forbid { .. } => None,
        })
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rule(dir: &Path, name: &str, body: &str) {
        let g = dir.join(".trellis").join("governance");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join(name), body).unwrap();
    }

    #[test]
    fn a_forbid_rule_loads_and_matches() {
        let d = tempfile::tempdir().unwrap();
        write_rule(
            d.path(),
            "no-direct-db.toml",
            "name = \"No direct DB access from handlers\"\n\
             rank = \"non-negotiable\"\n\
             description = \"Handlers go through the repository layer.\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\n\
             scope = [\"app/handlers/\"]\n",
        );
        std::fs::create_dir_all(d.path().join("app/handlers")).unwrap();
        std::fs::write(
            d.path().join("app/handlers/users.py"),
            "import psycopg2\n\ndef get():\n    c = psycopg2.connect('x')\n",
        )
        .unwrap();

        let rules = load(d.path()).expect("loads");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rank, Rank::NonNegotiable);

        let v = violations(d.path(), &rules, &["app/handlers/users.py".into()]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].line, 4);
        assert!(v[0].text.contains("psycopg2.connect"));
    }

    #[test]
    fn scope_keeps_a_rule_off_files_it_does_not_govern() {
        // A rule about handlers must not fire on the repository layer that is
        // *supposed* to talk to the database, or it forbids the architecture
        // it exists to protect.
        let d = tempfile::tempdir().unwrap();
        write_rule(
            d.path(),
            "r.toml",
            "name = \"x\"\nrank = \"non-negotiable\"\n\
             forbid = { pattern = \"psycopg2.connect\" }\nscope = [\"app/handlers/\"]\n",
        );
        std::fs::create_dir_all(d.path().join("app/repo")).unwrap();
        std::fs::write(
            d.path().join("app/repo/db.py"),
            "c = psycopg2.connect('x')\n",
        )
        .unwrap();

        let rules = load(d.path()).unwrap();
        assert!(violations(d.path(), &rules, &["app/repo/db.py".into()]).is_empty());
    }

    #[test]
    fn a_malformed_rule_is_an_error_not_a_silent_skip() {
        // A governance file that quietly did not load leaves a user believing
        // a rule is in force while every run ignores it.
        let d = tempfile::tempdir().unwrap();
        write_rule(d.path(), "bad.toml", "name = \"x\"\nrank = \"whatever\"\n");
        let e = load(d.path()).unwrap_err();
        assert!(format!("{e}").contains("bad.toml"), "{e}");
    }

    #[test]
    fn rank_decides_only_whether_the_audit_fails() {
        assert!(Rank::NonNegotiable.fails_the_audit());
        assert!(
            !Rank::NiceToHave.fails_the_audit(),
            "a nice-to-have is reported, never a failure — otherwise the \
             ranking means nothing"
        );
    }

    #[test]
    fn no_governance_directory_is_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(load(d.path()).unwrap().is_empty());
    }

    #[test]
    fn deny_write_rules_are_collected_for_the_write_path_not_the_audit() {
        // The two mechanisms are deliberately separate: one refuses an edit
        // before it reaches disk, the other reports after the fact. A rule
        // that denies writes contributes no violations here.
        let d = tempfile::tempdir().unwrap();
        write_rule(
            d.path(),
            "vendor.toml",
            "name = \"Vendored code is not ours\"\nrank = \"non-negotiable\"\n\
             deny-write = { paths = [\"vendor/\", \"*.lock\"] }\n",
        );
        let rules = load(d.path()).unwrap();
        assert_eq!(deny_write_paths(&rules), vec!["vendor/", "*.lock"]);
        assert!(violations(d.path(), &rules, &["vendor/x.py".into()]).is_empty());
    }
}
