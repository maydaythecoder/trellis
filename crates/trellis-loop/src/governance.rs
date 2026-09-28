//! Project rules the loop obeys, read from the repository.
//!
//! **Enforced, not requested.** A rule that lives only in a prompt is a
//! suggestion the model may decline, and "do not edit generated files" is not
//! useful as a suggestion. Every rule here is checked by the orchestrator,
//! which owns the only write path to the working tree, so a model that asks
//! to break one is refused rather than trusted not to ask.
//!
//! That division is the whole design. The repository's prose conventions go
//! into the prompt, where they inform; its hard rules come here, where they
//! bind. A context file that says "never touch `vendor/`" is advice. A
//! `deny_write` entry is a refusal.
//!
//! **This is not a security boundary and must not be described as one.** The
//! agent runs with the user's own permissions and the rules come from a file
//! inside the repository being edited. Anyone who can write `config.toml` can
//! rewrite the rules. What this buys is protection against an agent doing
//! something the project already knows is wrong — vendored code, generated
//! output, lockfiles — which is the failure that actually happens.

use serde::{Deserialize, Serialize};

/// One rule, in the three shapes a project actually needs.
///
/// Deliberately not a glob library. A full glob syntax invites rules whose
/// behaviour a user cannot predict from reading them, and the refusal message
/// has to name a rule the user recognises. These three cover vendored trees,
/// generated files by extension, and single named files; anything else is
/// better written as several rules than as one clever one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rule {
    /// `vendor/` — the path is inside this directory, at any depth.
    Prefix(String),
    /// `*.lock` — the path ends with this.
    Suffix(String),
    /// `setup.py` — exactly this path.
    Exact(String),
}

impl Rule {
    /// How a rule is written in `config.toml`, and how it is read back.
    pub fn parse(s: &str) -> Rule {
        let s = s.trim();
        // `vendor/**` and `vendor/*` are what people write out of habit; both
        // mean the directory, so accept them rather than matching nothing and
        // letting the user think the rule is on.
        let s = s.strip_suffix("**").unwrap_or(s);
        let s = s.strip_suffix('*').unwrap_or(s);
        if let Some(rest) = s.strip_prefix('*') {
            Rule::Suffix(rest.to_string())
        } else if s.ends_with('/') {
            Rule::Prefix(s.to_string())
        } else {
            Rule::Exact(s.to_string())
        }
    }

    /// `path` is already [`fold`]ed; the rule is folded the same way here,
    /// so a rule written `Vendor/` holds on Windows as `vendor/` does.
    fn matches(&self, path: &str) -> bool {
        match self {
            Rule::Prefix(p) => path.starts_with(fold(p).as_str()),
            Rule::Suffix(s) => path.ends_with(fold_case(&s.replace('\\', "/")).as_str()),
            Rule::Exact(e) => path == fold(e),
        }
    }

    /// The rule as the user wrote it, for a refusal message. A refusal that
    /// cannot be traced back to a line in the config is one the user cannot
    /// act on.
    pub fn as_written(&self) -> String {
        match self {
            Rule::Prefix(p) => p.clone(),
            Rule::Suffix(s) => format!("*{s}"),
            Rule::Exact(e) => e.clone(),
        }
    }
}

/// The rules in force for a run.
#[derive(Debug, Clone, Default)]
pub struct Governance {
    deny_write: Vec<Rule>,
}

impl Governance {
    pub fn new(deny_write: &[String]) -> Self {
        Self {
            deny_write: deny_write.iter().map(|s| Rule::parse(s)).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.deny_write.is_empty()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.deny_write
    }

    /// The rule forbidding a write to `path` as spelt, if any.
    ///
    /// The spelling is [`fold`]ed first: a rule written `vendor/` must hold
    /// on Windows, where a model will happily emit `vendor\thing.py`, and
    /// for `.//vendor/x.py`, which is the same file; a rule that silently
    /// stops applying to a spelling is worse than no rule. What this cannot
    /// see is a link on the way, which [`denies_write_at`](Self::denies_write_at)
    /// resolves: every write path consults that one.
    pub fn denies_write(&self, path: &str) -> Option<&Rule> {
        let norm = fold(path);
        self.deny_write.iter().find(|r| r.matches(&norm))
    }

    /// The rule forbidding a write to `rel` under `root`, if any: the
    /// spelling, and then the repository-relative path the write really
    /// lands on through any link (the one `write_refusal` computes). Every
    /// place `deny_write` is consulted asks this — the gate for `edit` and
    /// `write_file`, and a process agent's audit of what its command wrote —
    /// so a rule cannot hold at one and not the other.
    pub fn denies_write_at(&self, root: &std::path::Path, rel: &str) -> Option<&Rule> {
        self.denies_write(rel).or_else(|| {
            let real = crate::orchestrator::real_rel(root, rel).ok()?;
            self.denies_write(&real)
        })
    }
}

/// A path in the form rules compare: `/` separators, no empty or `.`
/// components (`.//setup.py` is `setup.py`), a trailing `/` kept so a
/// `Prefix` rule stays one, and each name [`fold_name`]d. `..` is left as
/// it is: `unsafe_repo_path` refuses it before any write.
fn fold(path: &str) -> String {
    let dir = path.ends_with(['/', '\\']);
    let mut s = path
        .split(['/', '\\'])
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .map(fold_name)
        .collect::<Vec<_>>()
        .join("/");
    if dir && !s.is_empty() {
        s.push('/');
    }
    s
}

/// One name as the filesystem compares it. On Windows the trailing dots
/// and spaces Win32 strips go (`setup.py.` opens `setup.py`), and case goes
/// on Windows and macOS, whose default filesystems ignore it. On a
/// case-sensitive macOS volume that denies a file the rule did not name,
/// which is the direction a deny list may err in.
fn fold_name(seg: &str) -> String {
    let seg = if cfg!(windows) {
        match seg.trim_end_matches(['.', ' ']) {
            // `..`, `. .`: not a name; refused as a path elsewhere.
            "" => seg,
            t => t,
        }
    } else {
        seg
    };
    fold_case(seg)
}

fn fold_case(s: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_shapes_match_what_they_say() {
        let g = Governance::new(&["vendor/".into(), "*.lock".into(), "setup.py".into()]);

        assert!(g.denies_write("vendor/pkg/mod.py").is_some());
        assert!(g.denies_write("poetry.lock").is_some());
        assert!(g.denies_write("setup.py").is_some());

        assert!(g.denies_write("app/vendor_helpers.py").is_none());
        assert!(
            g.denies_write("app/setup.py").is_none(),
            "Exact means exact"
        );
        assert!(g.denies_write("app/locket.py").is_none());
    }

    #[test]
    fn glob_habits_are_accepted_rather_than_silently_matching_nothing() {
        // People write `vendor/**`. If that parsed as an exact path it would
        // match nothing, and the user would believe the rule was in force —
        // the worst outcome for a rule whose whole value is being in force.
        for written in ["vendor/**", "vendor/*", "vendor/"] {
            let g = Governance::new(&[written.into()]);
            assert!(
                g.denies_write("vendor/a/b.py").is_some(),
                "`{written}` must deny vendor/a/b.py"
            );
        }
    }

    #[test]
    fn separators_are_normalised_so_a_rule_holds_on_every_platform() {
        // A model emits whatever separator it feels like, and a rule that
        // stops applying on one platform is worse than no rule at all.
        let g = Governance::new(&["vendor/".into()]);
        assert!(g.denies_write("vendor\\pkg\\mod.py").is_some());
        assert!(g.denies_write("./vendor/pkg/mod.py").is_some());
    }

    #[test]
    fn a_refusal_can_name_the_rule_the_user_wrote() {
        let g = Governance::new(&["*.lock".into()]);
        let r = g.denies_write("poetry.lock").expect("denied");
        assert_eq!(r.as_written(), "*.lock");
    }

    #[test]
    fn no_rules_denies_nothing() {
        let g = Governance::default();
        assert!(g.is_empty());
        assert!(g.denies_write("anything/at/all.py").is_none());
    }

    #[test]
    fn a_spelling_that_lands_on_a_protected_path_is_denied() {
        // The risk: the rule was matched against the string the model
        // wrote, and `.//setup.py` or `vendor//x.py` is not that string
        // while the write lands on exactly the protected file.
        let g = Governance::new(&["vendor/".into(), "setup.py".into(), "*.lock".into()]);
        for p in [
            ".//vendor/x.py",
            ".//setup.py",
            "././/setup.py",
            "vendor//x.py",
            "./vendor/./x.py",
            ".\\.\\setup.py",
        ] {
            assert!(g.denies_write(p).is_some(), "`{p}` must be denied");
        }
        assert!(
            g.denies_write("app//setup.py").is_none(),
            "Exact means exact"
        );
        assert!(g.denies_write("vendored/x.py").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_case_or_trailing_dot_spelling_is_the_same_file() {
        // NTFS compares names case-insensitively and Win32 strips a trailing
        // dot or space, so each of these writes the file the rule protects.
        let g = Governance::new(&["vendor/".into(), "setup.py".into(), "*.lock".into()]);
        for p in [
            "setup.py.",
            "Setup.py",
            "SETUP.PY ",
            "Vendor/x.py",
            "vendor./x.py",
            "VENDOR \\x.py",
            "poetry.LOCK",
            "poetry.lock.",
        ] {
            assert!(g.denies_write(p).is_some(), "`{p}` must be denied");
        }
    }
}
