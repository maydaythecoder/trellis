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
pub(crate) struct RepoConfig {
    #[serde(default)]
    pub checks: Option<ChecksSection>,
    #[serde(default)]
    pub budget: Option<BudgetSection>,
    #[serde(default)]
    #[allow(dead_code)]
    pub agents: Option<AgentsSection>,
    #[serde(default)]
    pub governance: Option<GovernanceSection>,
    #[serde(default)]
    pub model: Option<ModelSection>,
    #[serde(default)]
    pub context: Option<ContextSection>,
    #[serde(default)]
    pub control: Option<ControlSection>,
    /// Which tools a run offers, bash, and the repository's own tools
    /// (ADR 0023).
    #[serde(default)]
    pub tools: Option<ToolsSection>,
    /// Executable hooks, loaded only when trusted (ADR 0023).
    #[serde(default)]
    pub hooks: Option<trellis_loop::HookConfig>,
    /// Hosted endpoints a `provider:<name>#<model>` spec names (ADR 0024).
    #[serde(default)]
    pub providers: Option<Vec<ProviderSection>>,
    /// `[index] backstop`, the recall backstop `trellis index` runs after
    /// the resolver (ADR 0031).
    #[serde(default)]
    pub index: Option<IndexSection>,
    /// What the user's own file said, kept after layering, because where a
    /// hosted endpoint came from decides whether a key may go to it
    /// (ADR 0024 §3). Never read from any file.
    #[serde(skip)]
    pub provenance: crate::user_config::Provenance,
}

/// Where a run sits between hands-off and asking about each step (ADR 0018).
#[derive(Debug, Deserialize)]
pub(crate) struct ControlSection {
    /// `"auto"` or `"approve"`.
    pub mode: Option<String>,
    /// Tools that need a human yes in approve mode. Default `["edit"]`.
    pub approve: Option<Vec<String>>,
}

/// The two ends of the dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlMode {
    /// No human gate. Budgets, governance and the checks still apply.
    Auto,
    /// Each listed tool call waits for a person's yes.
    Approve,
}

/// The dial as configured, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Control {
    pub mode: ControlMode,
    pub approve: Vec<String>,
}

impl Control {
    /// How the trail and the run's banner name the mode.
    pub(crate) fn describe(&self) -> String {
        match self.mode {
            ControlMode::Auto => "auto".to_string(),
            ControlMode::Approve => format!("approve ({})", self.approve.join(", ")),
        }
    }

    /// A profile's `[control] approve` list, added to the layered dial. A
    /// profile is repository content, and it replaced the list before,
    /// dropping the user's own entries and the repository's default `edit`
    /// (review round 10). A profile can only add restrictions, as a user's
    /// file can (ADR 0025 §7).
    pub(crate) fn add_profile_approvals(&mut self, list: &[String]) {
        for t in list {
            if !self.approve.contains(t) {
                self.approve.push(t.clone());
            }
        }
    }
}

/// A note vault to compile one focus note from (ADR 0017).
#[derive(Debug, Deserialize)]
pub(crate) struct ContextSection {
    /// The vault directory, relative to the repository root.
    pub vault: Option<String>,
    /// The note stem to compile. `trellis run --focus` overrides it, because
    /// editing this file to switch focus dirties the worktree, and a run
    /// refuses a dirty worktree (ADR 0009).
    pub focus: Option<String>,
    /// `false` turns off `AGENTS.md`, `CLAUDE.md` and `AGENTS.override.md`
    /// (ADR 0025 §1). Absent means on.
    pub files: Option<bool>,
    /// `true` annotates each compiled note header with its created day from
    /// the vault's `dates.json` (ADR 0022 §7, integrated). Absent means off,
    /// so the compiled bytes are what the goldens hold.
    pub dates: Option<bool>,
}

/// Where the vault is and which note to compile, after validation of the
/// configuration itself. Whether the vault is valid is `trellis-vault`'s
/// question, asked later.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct VaultSetting {
    /// Absolute: `root` joined with the configured path.
    pub dir: std::path::PathBuf,
    /// As written in `config.toml`, for messages and the prompt.
    pub source: String,
    pub focus: Option<String>,
}

/// A default model for this repository.
///
/// Saves retyping a runtime URL every session. Deliberately not a secret and
/// not a key: it is a host and port, or a path to a script. The user's API
/// key never goes in a file TRELLIS reads (ADR 0010) — a runtime that needs
/// one is configured with it directly, out of this tool's sight.
#[derive(Debug, Deserialize)]
pub(crate) struct ModelSection {
    /// e.g. `llamacpp:http://127.0.0.1:8080` or `scripted:fix.json`.
    pub spec: Option<String>,
    /// Send no schema and validate the reply instead (tier 4). Opt-in, since
    /// a run can then fail on format alone.
    pub unconstrained: Option<bool>,
    /// `off`, `low`, `medium` or `high` (ADR 0024 §4). Mapped where the
    /// protocol carries it; ignored with a notice elsewhere.
    pub thinking: Option<String>,
    /// A ceiling on one request's size in bytes; over it, the oldest
    /// observations are elided (ADR 0024 §5). Unset means no ceiling.
    pub context_bytes: Option<u64>,
}

/// Rules the loop obeys, read from the repository rather than configured per
/// session. Enforced by the orchestrator; see `trellis_loop::governance`.
#[derive(Debug, Deserialize)]
pub(crate) struct GovernanceSection {
    /// Paths no run may edit: `vendor/`, `*.lock`, `setup.py`.
    pub deny_write: Option<Vec<String>>,
}

/// What `trellis index` runs beyond the resolver.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexSection {
    /// `"pyright"`, or absent for none.
    pub backstop: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChecksSection {
    pub typecheck: Option<Vec<String>>,
    pub tests: Option<Vec<String>>,
    pub python: Option<String>,
    /// Seconds a check may run before its process tree is killed and it
    /// fails; `trellis_checks::DEFAULT_CHECK_TIMEOUT_S` when absent. A run
    /// also ends a check when its wall-clock budget does (ADR 0004).
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BudgetSection {
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
pub(crate) struct AgentsSection {
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
    pub(crate) fn load(root: &Path) -> Result<Self> {
        // ADR 0025 §7: the user's own file sits under this one. The layering
        // and the repository read are in `user_config.rs`.
        Self::load_with_user(root, crate::user_config::file().as_deref())
    }

    pub(crate) fn checks(&self) -> CheckConfig {
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
            if let Some(v) = s.timeout_s {
                c.timeout_s = v;
            }
        }
        c
    }

    /// Whether `[index] backstop = "pyright"` asks for the Pyright backstop.
    ///
    /// Any other value is an error rather than "no backstop": a misspelt
    /// name that quietly ran the resolver alone would print coverage the
    /// user believes had the backstop's help.
    pub(crate) fn pyright_backstop(&self) -> Result<bool> {
        match self.index.as_ref().and_then(|i| i.backstop.as_deref()) {
            None => Ok(false),
            Some("pyright") => Ok(true),
            Some(other) => anyhow::bail!(
                "[index] backstop = \"{other}\" is not a backstop; the only one is \"pyright\""
            ),
        }
    }

    /// The repository's default model spec, if it set one.
    pub(crate) fn model(&self) -> Option<String> {
        self.model.as_ref().and_then(|m| m.spec.clone())
    }

    /// Whether this repository asks for tier 4.
    pub(crate) fn unconstrained(&self) -> bool {
        self.model
            .as_ref()
            .and_then(|m| m.unconstrained)
            .unwrap_or(false)
    }

    /// The `[control]` dial, validated, with `--approve`/`--auto` over it.
    ///
    /// An unknown mode or tool is an error, not a fallback. A typo in
    /// `approve = ["edits"]` that silently meant "approve nothing" would run
    /// hands-off in a repository whose owner asked to be consulted.
    pub(crate) fn control(&self, flag: Option<ControlMode>) -> Result<Control> {
        let section = self.control.as_ref();
        let configured = match section.and_then(|c| c.mode.as_deref()) {
            None | Some("auto") => ControlMode::Auto,
            Some("approve") => ControlMode::Approve,
            Some(other) => anyhow::bail!(
                "[control] mode = \"{other}\" is not a mode; use \"auto\" or \"approve\""
            ),
        };
        let approve = section
            .and_then(|c| c.approve.clone())
            .unwrap_or_else(|| vec!["edit".to_string()]);
        for t in &approve {
            self.check_approvable(t)
                .map_err(|e| anyhow::anyhow!("[control] {e}"))?;
        }
        Ok(Control {
            mode: flag.unwrap_or(configured),
            approve,
        })
    }

    /// Whether `t` may be listed in a `[control] approve` list: a built-in
    /// approvable tool, or a repository's own tool by name (ADR 0023). The
    /// one test the config's list and a profile's list both use, so a name
    /// one accepts the other cannot refuse.
    pub(crate) fn check_approvable(&self, t: &str) -> std::result::Result<(), String> {
        let custom = self.custom_tools();
        if custom.iter().any(|c| c.name == t) || trellis_loop::APPROVABLE_TOOLS.contains(&t) {
            return Ok(());
        }
        let mut names: Vec<&str> = trellis_loop::APPROVABLE_TOOLS.to_vec();
        names.extend(custom.iter().map(|c| c.name.as_str()));
        Err(format!(
            "approve lists `{t}`, which is not a tool; the tools are {}",
            names.join(", ")
        ))
    }

    /// The repository's write rules, or none.
    pub(crate) fn governance(&self) -> trellis_loop::Governance {
        let rules = self
            .governance
            .as_ref()
            .and_then(|g| g.deny_write.clone())
            .unwrap_or_default();
        trellis_loop::Governance::new(&rules)
    }

    /// The configured vault, with `--focus` applied over the config's focus.
    ///
    /// `Ok(None)` when no vault is configured and no focus asked for. A focus
    /// with no vault is an error rather than a no-op: a flag that silently
    /// does nothing is how a user ends up believing the model saw a note it
    /// never saw.
    ///
    /// The path must stay inside the repository. The clean-worktree check
    /// covers the repository and nothing else, so a vault outside it could
    /// change between two runs from the same commit and neither the snapshot
    /// nor the trail would show why they differed.
    pub(crate) fn vault(&self, root: &Path, focus: Option<&str>) -> Result<Option<VaultSetting>> {
        let section = self.context.as_ref();
        let configured = section.and_then(|c| c.vault.clone());
        let focus = focus
            .map(str::to_string)
            .or_else(|| section.and_then(|c| c.focus.clone()));
        let Some(source) = configured else {
            if let Some(f) = focus {
                anyhow::bail!(
                    "focus `{f}` was asked for, but no vault is configured. \
                     Set [context] vault = \"<dir>\" in .trellis/config.toml"
                );
            }
            return Ok(None);
        };
        let rel = Path::new(&source);
        let escapes = rel.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if source.trim().is_empty() || escapes {
            anyhow::bail!(
                "[context] vault = \"{source}\" must be a relative path inside the \
                 repository; a vault outside it is not covered by the clean-worktree \
                 check, so two runs from one commit could see different notes"
            );
        }
        // The string rule above cannot see a committed link: `vault -> ~`
        // passes it and puts the home directory's notes in the prompt. The
        // real path is checked as every other prompt input's is. The path
        // kept is the one written, not the canonical one (`\\?\` on
        // Windows), because the vault commands name files relative to it;
        // the walk inside the vault skips links (`trellis_vault`).
        let dir = root.join(rel);
        trellis_core::paths::resolve_repo_input(root, &dir)
            .map_err(|why| anyhow::anyhow!("[context] vault = \"{source}\": {why}"))?;
        Ok(Some(VaultSetting { dir, source, focus }))
    }

    /// A named task template from `.trellis/prompts/<name>.md`.
    ///
    /// Repeating the same paragraph to start the same kind of task is a tax
    /// the repository can pay once. Extra words on the command line are
    /// appended rather than replacing it, so a template can be a standing
    /// instruction with a specific ask after it.
    ///
    /// A prompt file is repository content and instruction-shaped, so it is a
    /// **task**, never configuration: it cannot set the model, lift a denied
    /// path, or change a budget. Everything that binds a run stays in
    /// `config.toml` and `governance/`, where a reviewer looks for it.
    pub(crate) fn prompt(root: &Path, name: &str) -> Result<String> {
        // Frontmatter stripped and defaults applied (ADR 0025 §3). A
        // template without frontmatter comes back exactly as before.
        Self::prompt_raw(root, name).map(|s| crate::templates::render_text(&s, None))
    }

    /// `.trellis/prompts/<name>.md` as written: the one reader `prompt` and
    /// `templates::render` share, so there is one name check and one
    /// real-path check.
    pub(crate) fn prompt_raw(root: &Path, name: &str) -> Result<String> {
        // No path separators: `--prompt ../../etc/passwd` must not read
        // outside the prompts directory, and a name is a name. Nor anything
        // the repository-path rule refuses: `C:x` is relative to drive C's
        // current directory on Windows, so `join` leaves the repository.
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || trellis_core::paths::unsafe_repo_path(name).is_some()
        {
            anyhow::bail!("`{name}` is not a prompt name; use the file stem only");
        }
        let path = root
            .join(".trellis")
            .join("prompts")
            .join(format!("{name}.md"));
        // A committed link out of the repository is refused by name rather
        // than read into the task (`read_repo_text`).
        match crate::context_files::read_repo_text(root, &path)
            .with_context(|| format!("prompt `{name}` could not be loaded"))?
        {
            Some(text) => Ok(text),
            None => {
                let dir = root.join(".trellis").join("prompts");
                let known: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(Result::ok)
                            .filter_map(|e| {
                                e.path()
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if known.is_empty() {
                    anyhow::bail!("no prompt `{name}`; {} holds none", dir.display())
                } else {
                    anyhow::bail!("no prompt `{name}`. Known: {}", known.join(", "))
                }
            }
        }
    }

    /// `.trellis/context.md`, if the repository wrote one.
    ///
    /// Absent is normal and not an error. Unreadable is reported by the
    /// caller rather than swallowed: a project that wrote a context file and
    /// silently did not get it is worse off than one that never wrote one.
    pub(crate) fn notes(root: &Path) -> Result<Option<String>> {
        let path = root.join(".trellis").join("context.md");
        // Through the one prompt-input reader: a committed link to a file
        // outside the repository or into `.git` is refused by name.
        let text = crate::context_files::read_repo_text(root, &path)
            .context(".trellis/context.md could not be loaded")?;
        Ok(text.filter(|s| !s.trim().is_empty()))
    }

    pub(crate) fn budget(&self) -> Budget {
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
    fn the_root_is_the_repository_root_not_the_working_directory() {
        // `trellis index` three directories deep used to index whatever was
        // under the cwd, which for most subdirectories is nothing — reported
        // as "indexed 0 files" with no hint the user was standing in the
        // wrong place.
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let deep = repo.join("app/deep/nested");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        assert_eq!(crate::root_from(&deep), Some(repo.clone()));
        assert_eq!(crate::root_from(&repo), Some(repo));
    }

    #[test]
    fn a_tree_with_no_repository_is_refused_rather_than_walked() {
        // This fell back to the working directory, on the reasoning that
        // `index` and `radius` are useful on a plain source tree. True, and
        // still the wrong default: `trellis index` typed in a home folder
        // walked the entire home folder, and the line printed to say so
        // appeared after the walk had already started. It hung for two
        // minutes before anyone could read it.
        //
        // Git refuses here and so does this. `--root .` serves the plain-tree
        // case in one flag and states the intent.
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(crate::root_from(&sub), None);
    }

    #[test]
    fn the_nearest_repository_wins_over_an_outer_one() {
        // A nested checkout — a vendored dependency, a submodule — must
        // resolve to the inner repository, because that is the one whose
        // worktree a run would snapshot and roll back.
        let d = tempfile::tempdir().unwrap();
        let outer = d.path().join("outer");
        let inner = outer.join("vendor/inner");
        let deep = inner.join("src");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::create_dir_all(inner.join(".git")).unwrap();

        assert_eq!(crate::root_from(&deep), Some(inner));
    }

    #[test]
    fn an_absent_file_yields_defaults() {
        // Including S20's `--no-incremental`: a repository with no config is
        // exactly the one that gets no say in the matter, so the default it
        // inherits has to be the safe one.
        let d = tempfile::tempdir().unwrap();
        let c = RepoConfig::load(d.path()).unwrap();
        assert_eq!(c.checks().typecheck, ["mypy", "--no-incremental"]);
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
    fn a_prompt_template_loads_by_name() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".trellis").join("prompts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("typefix.md"),
            "  Read before editing.

",
        )
        .unwrap();
        assert_eq!(
            RepoConfig::prompt(d.path(), "typefix").unwrap(),
            "Read before editing.",
            "trimmed, so a trailing newline does not become part of the task"
        );
    }

    #[test]
    fn a_prompt_name_cannot_escape_the_prompts_directory() {
        // `--prompt` takes a name, not a path. Without this, a name is a
        // relative path and the flag reads arbitrary files into the task.
        let d = tempfile::tempdir().unwrap();
        for bad in ["../secrets", "..", "a/b", r"a\b"] {
            let e = RepoConfig::prompt(d.path(), bad).unwrap_err();
            assert!(
                format!("{e}").contains("not a prompt name"),
                "{bad} must be refused: {e}"
            );
        }
    }

    #[test]
    fn a_prompt_name_with_a_colon_is_refused() {
        // `C:x` is relative to drive C's current directory on Windows, and
        // `a:b` an NTFS stream: `root.join` of the first leaves the
        // repository. The name obeys the repository-path rule the loop uses.
        let d = tempfile::tempdir().unwrap();
        for bad in ["C:x", "a:b", ":x", "...", ".. "] {
            let e = RepoConfig::prompt(d.path(), bad).unwrap_err();
            assert!(
                format!("{e}").contains("not a prompt name"),
                "{bad} must be refused: {e}"
            );
        }
    }

    /// A link at `link` to `target`, or `false` with the reason printed.
    fn link(target: &Path, link: &Path) -> bool {
        match trellis_core::paths::symlink_for_test(target, link) {
            Ok(()) => true,
            Err(why) => {
                eprintln!("{why}");
                false
            }
        }
    }

    #[test]
    fn a_prompt_or_context_md_linked_outside_the_repository_is_refused() {
        // The risk: a committed `.trellis/prompts/p.md -> ~/.ssh/id_rsa`
        // became the task text of a hosted run.
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("id_rsa");
        std::fs::write(&secret, "PRIVATE").unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/prompts")).unwrap();
        if !link(&secret, &d.path().join(".trellis/prompts/p.md")) {
            return;
        }
        let e = format!("{:#}", RepoConfig::prompt(d.path(), "p").unwrap_err());
        assert!(e.contains("outside the repository"), "{e}");
        assert!(!e.contains("PRIVATE"), "{e}");
        let e = format!(
            "{:#}",
            crate::templates::render(d.path(), "p", None).unwrap_err()
        );
        assert!(e.contains("outside the repository"), "{e}");

        assert!(link(&secret, &d.path().join(".trellis/context.md")));
        let e = format!("{:#}", RepoConfig::notes(d.path()).unwrap_err());
        assert!(
            e.contains(".trellis/context.md") && e.contains("outside the repository"),
            "{e}"
        );
    }

    #[test]
    fn a_vault_linked_outside_the_repository_or_into_git_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("n.md"), "# n\n").unwrap();
        write(d.path(), "[context]\nvault = \"vault\"\n");
        if !link(outside.path(), &d.path().join("vault")) {
            return;
        }
        let c = RepoConfig::load(d.path()).unwrap();
        let e = format!("{:#}", c.vault(d.path(), None).unwrap_err());
        assert!(e.contains("outside the repository"), "{e}");

        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        write(d.path(), "[context]\nvault = \".git\"\n");
        let c = RepoConfig::load(d.path()).unwrap();
        let e = format!("{:#}", c.vault(d.path(), None).unwrap_err());
        assert!(e.contains(".git"), "{e}");
    }

    #[test]
    fn an_unknown_prompt_says_which_ones_exist() {
        // "File not found" sends a user looking for a typo in a path they
        // never typed. The names are right there.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".trellis").join("prompts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("typefix.md"), "x").unwrap();
        std::fs::write(dir.join("refactor.md"), "y").unwrap();

        let e = RepoConfig::prompt(d.path(), "nosuch").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("refactor") && msg.contains("typefix"), "{msg}");
    }

    #[test]
    fn a_repository_may_name_its_own_model() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(RepoConfig::load(d.path()).unwrap().model(), None);
        assert!(!RepoConfig::load(d.path()).unwrap().unconstrained());

        write(
            d.path(),
            "[model]
spec = \"llamacpp:http://127.0.0.1:8792\"
unconstrained = true
",
        );
        let c = RepoConfig::load(d.path()).unwrap();
        assert_eq!(c.model().as_deref(), Some("llamacpp:http://127.0.0.1:8792"));
        assert!(c.unconstrained());
    }

    #[test]
    fn a_model_section_does_not_disturb_the_rest_of_the_config() {
        // Sections are independent: a repository that names a model must
        // still get its own checks, not the defaults.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "[checks]
typecheck = [\"pyright\"]

[model]
spec = \"scripted:x.json\"
",
        );
        let c = RepoConfig::load(d.path()).unwrap();
        assert_eq!(c.checks().typecheck, ["pyright"]);
        assert_eq!(c.model().as_deref(), Some("scripted:x.json"));
    }

    #[test]
    fn governance_rules_are_read_and_absent_means_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            RepoConfig::load(d.path()).unwrap().governance().is_empty(),
            "no config means no rules, not a guessed default — a harness that              invents restrictions is as surprising as one that ignores them"
        );

        write(
            d.path(),
            "[governance]
deny_write = [\"vendor/\", \"*.lock\"]
",
        );
        let g = RepoConfig::load(d.path()).unwrap().governance();
        assert_eq!(g.rules().len(), 2);
        assert!(g.denies_write("vendor/pkg/a.py").is_some());
        assert!(g.denies_write("poetry.lock").is_some());
        assert!(g.denies_write("app/main.py").is_none());
    }

    #[test]
    fn a_context_file_is_optional_and_an_empty_one_is_not_content() {
        let d = tempfile::tempdir().unwrap();
        assert!(RepoConfig::notes(d.path()).unwrap().is_none());

        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        std::fs::write(
            d.path().join(".trellis/context.md"),
            "   

",
        )
        .unwrap();
        assert!(
            RepoConfig::notes(d.path()).unwrap().is_none(),
            "a whitespace-only file must not put an empty fence in the prompt"
        );

        std::fs::write(d.path().join(".trellis/context.md"), "Use `make check`.").unwrap();
        assert_eq!(
            RepoConfig::notes(d.path()).unwrap().as_deref(),
            Some("Use `make check`.")
        );
    }

    #[test]
    fn a_vault_is_read_and_the_flag_overrides_the_configured_focus() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            RepoConfig::load(d.path())
                .unwrap()
                .vault(d.path(), None)
                .unwrap(),
            None
        );

        write(
            d.path(),
            "[context]\nvault = \"notes\"\nfocus = \"task-a\"\n",
        );
        let c = RepoConfig::load(d.path()).unwrap();
        let v = c.vault(d.path(), None).unwrap().unwrap();
        assert_eq!(v.dir, d.path().join("notes"));
        assert_eq!(v.focus.as_deref(), Some("task-a"));
        // The flag wins: switching focus must not require editing a tracked
        // file, or the next run refuses the dirty worktree.
        let v = c.vault(d.path(), Some("task-b")).unwrap().unwrap();
        assert_eq!(v.focus.as_deref(), Some("task-b"));
    }

    #[test]
    fn a_focus_with_no_vault_is_refused_rather_than_ignored() {
        let d = tempfile::tempdir().unwrap();
        let e = RepoConfig::load(d.path())
            .unwrap()
            .vault(d.path(), Some("task-a"))
            .unwrap_err();
        assert!(format!("{e}").contains("no vault is configured"), "{e}");
    }

    #[test]
    fn a_vault_outside_the_repository_is_refused() {
        // The clean-worktree check is what makes a run reproducible from its
        // commit. A vault it does not cover breaks that silently.
        let d = tempfile::tempdir().unwrap();
        for bad in ["../notes", "/abs/notes", "notes/../../x", ""] {
            write(d.path(), &format!("[context]\nvault = {bad:?}\n"));
            let e = RepoConfig::load(d.path())
                .unwrap()
                .vault(d.path(), None)
                .unwrap_err();
            assert!(format!("{e}").contains("inside the"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_silent_default() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "[checks]\ntypecheck = \"not a list\"\n");
        let e = RepoConfig::load(d.path()).unwrap_err();
        assert!(format!("{e}").contains("config.toml"), "{e}");
    }

    #[test]
    fn the_pyright_backstop_runs_only_when_named_and_a_misspelling_is_refused() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            !RepoConfig::load(d.path())
                .unwrap()
                .pyright_backstop()
                .unwrap()
        );
        write(
            d.path(),
            "[index]
backstop = \"pyright\"
",
        );
        assert!(
            RepoConfig::load(d.path())
                .unwrap()
                .pyright_backstop()
                .unwrap()
        );
        write(
            d.path(),
            "[index]
backstop = \"pyrite\"
",
        );
        let e = RepoConfig::load(d.path())
            .unwrap()
            .pyright_backstop()
            .unwrap_err();
        assert!(format!("{e}").contains("pyrite"), "{e}");
        write(
            d.path(),
            "[index]
backstops = \"pyright\"
",
        );
        assert!(
            RepoConfig::load(d.path()).is_err(),
            "an unknown key is refused"
        );
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

    #[test]
    fn control_defaults_to_auto_with_edit_listed_for_approval() {
        let d = tempfile::tempdir().unwrap();
        let c = RepoConfig::load(d.path()).unwrap().control(None).unwrap();
        assert_eq!(c.mode, ControlMode::Auto);
        assert_eq!(c.approve, vec!["edit".to_string()]);
    }

    #[test]
    fn the_run_flag_overrides_the_configured_mode_for_that_run_only() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "[control]\nmode = \"approve\"\n");
        let cfg = RepoConfig::load(d.path()).unwrap();
        assert_eq!(cfg.control(None).unwrap().mode, ControlMode::Approve);
        assert_eq!(
            cfg.control(Some(ControlMode::Auto)).unwrap().mode,
            ControlMode::Auto
        );
    }

    #[test]
    fn an_unknown_mode_or_tool_is_refused_rather_than_run_hands_off() {
        // `approve = ["edits"]` silently meaning "approve nothing" would run
        // hands-off in a repository whose owner asked to be consulted.
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "[control]\nmode = \"ask\"\n");
        let e = RepoConfig::load(d.path())
            .unwrap()
            .control(None)
            .unwrap_err();
        assert!(format!("{e}").contains("\"ask\" is not a mode"), "{e}");

        write(
            d.path(),
            "[control]\nmode = \"approve\"\napprove = [\"edits\"]\n",
        );
        let e = RepoConfig::load(d.path())
            .unwrap()
            .control(None)
            .unwrap_err();
        assert!(
            format!("{e}").contains("`edits`, which is not a tool"),
            "{e}"
        );
    }

    #[test]
    fn every_tool_the_adr_names_may_be_listed() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "[control]\nmode = \"approve\"\napprove = [\"edit\", \"read_file\", \"run_checks\", \"blast_radius\"]\n",
        );
        let c = RepoConfig::load(d.path()).unwrap().control(None).unwrap();
        assert_eq!(c.approve.len(), 4);
        assert_eq!(
            c.describe(),
            "approve (edit, read_file, run_checks, blast_radius)"
        );
    }

    #[test]
    fn a_repository_tool_may_be_listed_for_approval_by_its_name() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            concat!(
                "[control]\nmode = \"approve\"\napprove = [\"lint\", \"bash\"]\n",
                "[[tools.custom]]\nname = \"lint\"\ndescription = \"Lint.\"\n",
                "command = [\"python\", \"lint.py\"]\n",
                "schema = { type = \"object\", properties = { path = { type = \"string\" } } }\n",
            ),
        );
        let cfg = RepoConfig::load(d.path()).unwrap();
        assert_eq!(cfg.control(None).unwrap().approve, ["lint", "bash"]);
        let c = &cfg.custom_tools()[0];
        assert_eq!(c.schema["properties"]["path"]["type"], "string");
    }

    #[test]
    fn tools_and_hooks_sections_are_read_and_a_typo_is_an_error() {
        // A misspelt key that silently meant "no timeout" or "no allowlist"
        // would change what runs unattended without anyone seeing it.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            concat!(
                "[tools]\nenable = [\"grep\"]\nbash = true\nbash_allow = [\"cargo test\"]\n",
                "bash_timeout_s = 30\n",
                "[hooks]\npre_tool = [\"python\", \"pre.py\"]\ntimeout_s = 5\n",
            ),
        );
        let cfg = RepoConfig::load(d.path()).unwrap();
        let t = cfg.tools.as_ref().unwrap();
        assert_eq!(t.bash, Some(true));
        assert_eq!(t.bash_timeout_s, Some(30));
        assert_eq!(cfg.hooks.as_ref().unwrap().count(), 1);

        write(d.path(), "[tools]\nbash_alow = [\"x\"]\n");
        assert!(RepoConfig::load(d.path()).is_err());
        write(d.path(), "[hooks]\npretool = [\"x\"]\n");
        assert!(RepoConfig::load(d.path()).is_err());
    }
}

/// `[tools]` (ADR 0023). Unknown keys are refused: every one of these
/// changes what a run may execute.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolsSection {
    /// Built-ins that are off by default: `write_file`, `grep`, `find`, `ls`.
    pub enable: Option<Vec<String>>,
    /// Any tool, custom ones included. Never `run_checks`.
    pub disable: Option<Vec<String>>,
    /// Turns the `bash` tool on. Subject to trust.
    pub bash: Option<bool>,
    /// Command prefixes an unattended run may execute.
    pub bash_allow: Option<Vec<String>>,
    /// Lets an unattended run execute any command.
    pub bash_unattended: Option<bool>,
    /// Default 120.
    pub bash_timeout_s: Option<u64>,
    /// argv prefix for `bash`; the command is appended. Default: the
    /// platform's (`trellis_loop::default_shell`).
    pub shell: Option<Vec<String>>,
    /// `[[tools.custom]]`.
    pub custom: Option<Vec<trellis_loop::CustomTool>>,
}

/// Default `[tools] bash_timeout_s`.
pub(crate) const DEFAULT_BASH_TIMEOUT_S: u64 = 120;

impl RepoConfig {
    /// `[[tools.custom]]` as declared, trusted or not.
    pub(crate) fn custom_tools(&self) -> &[trellis_loop::CustomTool] {
        self.tools
            .as_ref()
            .and_then(|t| t.custom.as_deref())
            .unwrap_or(&[])
    }

    /// Whether `[tools] bash = true`.
    pub(crate) fn bash_requested(&self) -> bool {
        self.tools.as_ref().and_then(|t| t.bash).unwrap_or(false)
    }

    /// `[hooks]`, or none.
    pub(crate) fn hook_config(&self) -> trellis_loop::HookConfig {
        self.hooks.clone().unwrap_or_default()
    }
}

/// `[[providers]]` (ADR 0024 §2): a hosted endpoint by name.
///
/// Names the variable a key is read from, never the key: the file is in the
/// repository, and ADR 0024 keeps keys out of every file trellis reads.
/// Unknown fields are refused, so `api_key = "..."` fails to load rather
/// than being quietly ignored in a committed file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderSection {
    pub name: String,
    /// `openai` or `anthropic`.
    pub protocol: String,
    pub base_url: String,
    /// The environment variable holding the key. Empty means the
    /// protocol's own (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`). Only the
    /// user's file may set it; the repository's is refused if it does
    /// (ADR 0024 §3).
    #[serde(default)]
    pub key_env: String,
}

impl RepoConfig {
    /// `[model] thinking`, validated. Absent is `off`.
    pub(crate) fn thinking(&self) -> Result<trellis_model::Thinking> {
        match self.model.as_ref().and_then(|m| m.thinking.as_deref()) {
            None => Ok(trellis_model::Thinking::Off),
            Some(s) => trellis_model::Thinking::parse(s).ok_or_else(|| {
                anyhow::anyhow!("[model] thinking = \"{s}\" is not one of off, low, medium, high")
            }),
        }
    }

    /// `[model] context_bytes`, when set.
    pub(crate) fn context_bytes(&self) -> Option<usize> {
        self.model
            .as_ref()
            .and_then(|m| m.context_bytes)
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// The `[[providers]]` entry called `name`.
    pub(crate) fn provider(&self, name: &str) -> Result<&ProviderSection> {
        let all = self.providers.as_deref().unwrap_or(&[]);
        all.iter().find(|p| p.name == name).ok_or_else(|| {
            let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
            anyhow::anyhow!(
                "no [[providers]] entry is named `{name}`; {}",
                if names.is_empty() {
                    "none are configured in .trellis/config.toml".to_string()
                } else {
                    format!("the configured ones are {}", names.join(", "))
                }
            )
        })
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn a_provider_names_its_key_variable_and_a_key_field_is_refused() {
        // A key in this file is a key committed to version control
        // (ADR 0010); refusing the field beats ignoring it.
        let ok: RepoConfig = toml::from_str(
            "[[providers]]\nname = \"groq\"\nprotocol = \"openai\"\n\
             base_url = \"https://api.groq.com/openai/v1\"\nkey_env = \"GROQ_API_KEY\"\n",
        )
        .unwrap();
        assert_eq!(ok.provider("groq").unwrap().key_env, "GROQ_API_KEY");
        assert!(
            ok.provider("nope")
                .unwrap_err()
                .to_string()
                .contains("groq")
        );

        let bad = toml::from_str::<RepoConfig>(
            "[[providers]]\nname = \"x\"\nprotocol = \"openai\"\nbase_url = \"https://x\"\n\
             key_env = \"K\"\napi_key = \"sk-123\"\n",
        );
        assert!(bad.is_err());
    }

    #[test]
    fn thinking_and_the_context_budget_are_read_and_a_typo_is_an_error() {
        let c: RepoConfig =
            toml::from_str("[model]\nthinking = \"high\"\ncontext_bytes = 200000\n").unwrap();
        assert_eq!(c.thinking().unwrap(), trellis_model::Thinking::High);
        assert_eq!(c.context_bytes(), Some(200_000));
        let c: RepoConfig = toml::from_str("[model]\nthinking = \"max\"\n").unwrap();
        assert!(c.thinking().is_err());
        assert_eq!(RepoConfig::default().context_bytes(), None);
    }
}
