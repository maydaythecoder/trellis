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

/// One rule, in the three shapes a project actually needs.
///
/// Deliberately not a glob library. A full glob syntax invites rules whose
/// behaviour a user cannot predict from reading them, and the refusal message
/// has to name a rule the user recognises. These three cover vendored trees,
/// generated files by extension, and single named files; anything else is
/// better written as several rules than as one clever one.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// The rule's pattern folded as a path is, so a rule written `Vendor/`
    /// holds on Windows as `vendor/` does. Computed once, when the rules
    /// are built ([`Governance::new`]), not at every comparison.
    fn folded(&self) -> String {
        match self {
            Rule::Prefix(p) => fold(p),
            Rule::Suffix(s) => fold_case(&s.replace('\\', "/")),
            Rule::Exact(e) => fold(e),
        }
    }

    /// `path` is already [`fold`]ed, and `folded` is [`Self::folded`].
    fn matches(&self, folded: &str, path: &str) -> bool {
        match self {
            Rule::Prefix(_) => path.starts_with(folded),
            Rule::Suffix(_) => path.ends_with(folded),
            Rule::Exact(_) => path == folded,
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
    /// Each rule's [`Rule::folded`] pattern, in the same order.
    folded: Vec<String>,
    /// An agent profile's `allow_write` (ADR 0026 §4), or `None` when the
    /// run has no profile or its profile has none: then nothing here
    /// narrows, which is the behaviour before the field existed.
    scope: Option<WriteScope>,
    /// The configured vault's notes root (ADR 0049), or `None` when
    /// `[context] vault` is not set.
    vault: Option<VaultRoot>,
}

/// The vault's notes root, refused as a `deny_write` rule is but kept apart
/// from them, so its refusal names `[context] vault` and not a rule the
/// user never wrote.
#[derive(Debug, Clone)]
struct VaultRoot {
    /// `[context] vault` as written, for the refusal.
    source: String,
    /// The notes root relative to the repository, ending in `/`.
    notes_root: String,
    /// `notes_root` [`fold`]ed, as a `Prefix` rule's pattern is.
    folded: String,
}

/// Why a write is refused: a `deny_write` rule, or the vault's notes root.
/// Its [`Display`](std::fmt::Display) is the wording a carry-back uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal<'a> {
    Rule(&'a Rule),
    Vault {
        source: &'a str,
        notes_root: &'a str,
    },
}

impl std::fmt::Display for Refusal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Rule(r) => write!(f, "deny_write `{}`", r.as_written()),
            Refusal::Vault { source, notes_root } => write!(
                f,
                "[context] vault = \"{source}\", notes root `{notes_root}`"
            ),
        }
    }
}

/// A profile's `allow_write`: the only paths its runs may write. Kept apart
/// from `deny_write` rather than folded into it, because `deny_write` also
/// decides which changed ignored files count as unrestorable, and a scope
/// there would make every cache a tool writes outside it (`.mypy_cache/`)
/// a tree not restored.
#[derive(Debug, Clone)]
struct WriteScope {
    /// The profile's name, for a refusal that points at its file.
    profile: String,
    rules: Vec<Rule>,
    folded: Vec<String>,
}

impl WriteScope {
    fn admits(&self, path: &str) -> bool {
        let norm = fold(path);
        self.rules
            .iter()
            .zip(&self.folded)
            .any(|(r, f)| r.matches(f, &norm))
    }
}

impl Governance {
    pub fn new(deny_write: &[String]) -> Self {
        let deny_write: Vec<Rule> = deny_write.iter().map(|s| Rule::parse(s)).collect();
        let folded = deny_write.iter().map(Rule::folded).collect();
        Self {
            deny_write,
            folded,
            scope: None,
            vault: None,
        }
    }

    /// Refuses every write into the configured vault's notes root (ADR
    /// 0049). `source` is `[context] vault` as written; `notes_root` is the
    /// notes root relative to the repository, matched as a `Prefix` rule
    /// is. A missing trailing `/` is added, since without it `vault` would
    /// also refuse `vaulted/`.
    pub fn with_vault(mut self, source: &str, notes_root: &str) -> Self {
        let notes_root = if notes_root.ends_with(['/', '\\']) {
            notes_root.to_string()
        } else {
            format!("{notes_root}/")
        };
        self.vault = Some(VaultRoot {
            source: source.to_string(),
            folded: fold(&notes_root),
            notes_root,
        });
        self
    }

    /// Narrows the run to `allow` (profile `profile`'s `allow_write`, in
    /// the rule syntax `deny_write` uses). It only narrows: `deny_write`
    /// is consulted first everywhere a write is judged, so a path the
    /// repository denies stays denied whatever `allow` names. An empty
    /// `allow` admits no write at all.
    pub fn with_allow_write(mut self, profile: &str, allow: &[String]) -> Self {
        let rules: Vec<Rule> = allow.iter().map(|s| Rule::parse(s)).collect();
        let folded = rules.iter().map(Rule::folded).collect();
        self.scope = Some(WriteScope {
            profile: profile.to_string(),
            rules,
            folded,
        });
        self
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
        self.deny_write
            .iter()
            .zip(&self.folded)
            .find(|(r, f)| r.matches(f, &norm))
            .map(|(r, _)| r)
    }

    /// What forbids a write to `rel` under `root`, if anything: the
    /// spelling, and then the repository-relative path the write really
    /// lands on through any link (the one `write_refusal` computes). Every
    /// place a write is judged asks this — the gate for `edit` and
    /// `write_file`, and a process agent's audit of what its command wrote —
    /// so a refusal cannot hold at one and not the other. `deny_write` is
    /// asked first, then the vault's notes root.
    pub fn denies_write_at(&self, root: &std::path::Path, rel: &str) -> Option<Refusal<'_>> {
        // Resolved at most once, and only when the spelling is not refused.
        let real = std::cell::OnceCell::new();
        let real = || {
            real.get_or_init(|| crate::orchestrator::real_rel(root, rel).ok())
                .as_deref()
        };
        if let Some(r) = self
            .denies_write(rel)
            .or_else(|| self.denies_write(real()?))
        {
            return Some(Refusal::Rule(r));
        }
        let v = self.vault.as_ref()?;
        // As a `Prefix` rule matches: inside the directory, at any depth.
        let inside = |p: &str| fold(p).starts_with(&v.folded);
        (inside(rel) || real().is_some_and(inside)).then_some(Refusal::Vault {
            source: &v.source,
            notes_root: &v.notes_root,
        })
    }

    /// Why a write to `rel` under `root` is outside the profile's
    /// `allow_write`, or `None` when it is inside or there is no scope.
    ///
    /// Inside means the spelling **and** the path the write really lands
    /// on are both admitted: `deny_write` refuses when either matches, and
    /// the narrowing direction of that is both. A path whose landing place
    /// cannot be worked out is outside, since "unknown" is not "admitted".
    /// Callers ask [`denies_write_at`](Self::denies_write_at) first, so a
    /// refusal names the repository's rule when there is one.
    pub fn outside_allow_write_at(&self, root: &std::path::Path, rel: &str) -> Option<String> {
        let scope = self.scope.as_ref()?;
        let inside = scope.admits(rel)
            && crate::orchestrator::real_rel(root, rel).is_ok_and(|real| scope.admits(&real));
        (!inside).then(|| {
            let written: Vec<String> = scope
                .rules
                .iter()
                .map(|r| format!("`{}`", r.as_written()))
                .collect();
            format!(
                "outside the allow_write [{}] of agent `{}` in .trellis/agents/{}.toml",
                written.join(", "),
                scope.profile,
                scope.profile
            )
        })
    }
}

/// A path in the form rules compare: `/` separators, no empty or `.`
/// components (`.//setup.py` is `setup.py`), a trailing `/` kept so a
/// `Prefix` rule stays one, and each name [`fold_name`]d. `..` is left as
/// it is: `unsafe_repo_path` refuses it before any write.
fn fold(path: &str) -> String {
    #[cfg(test)]
    FOLDS.with(|c| c.set(c.get() + 1));
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
thread_local! {
    /// Calls to `fold`, for the test that holds a lookup to one.
    static FOLDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_folds_the_path_once_and_no_rule_again() {
        // The risk (review round 9, finding 9): each comparison folded the
        // rule it compared with, so a lookup against `n` rules folded `n`
        // strings that never change after parsing.
        let g = Governance::new(&["vendor/".into(), "setup.py".into(), "*.lock".into()]);
        FOLDS.with(|c| c.set(0));
        for p in ["app/x.py", "vendor/y.py", "setup.py", "a.lock"] {
            let _ = g.denies_write(p);
        }
        assert_eq!(FOLDS.with(std::cell::Cell::get), 4, "one fold per lookup");
    }

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

    #[test]
    fn vault_notes_root_is_refused_through_denies_write_at() {
        // The risk (ADR 0049): a model writing the notes later runs read as
        // prompt input, refused nowhere, or refused under a `deny_write`
        // rule the user never wrote and would go looking for.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let g = Governance::new(&[]).with_vault("vault", "vault/");
        for p in ["vault/x.md", "vault/sub/y.md"] {
            let r = g.denies_write_at(root, p).expect(p);
            assert!(
                r.to_string().contains("[context] vault = \"vault\""),
                "{p}: {r}"
            );
        }
        for p in ["vaulted/x.md", "app/x.py"] {
            assert!(g.denies_write_at(root, p).is_none(), "{p}");
        }
        assert!(
            Governance::new(&[])
                .denies_write_at(root, "vault/x.md")
                .is_none()
        );
        // `deny_write` is asked first and still reads as it always has.
        let g = Governance::new(&["vendor/".into()]).with_vault("vault", "vault/");
        let r = g.denies_write_at(root, "vendor/a.py").expect("denied");
        assert_eq!(r.to_string(), "deny_write `vendor/`");
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
