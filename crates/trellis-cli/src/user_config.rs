//! The user-level configuration directory and how its `config.toml` layers
//! under the repository's (ADR 0025 §7).
//!
//! The rule in one line: the repository wins field by field, except where a
//! field is a restriction. There the two files add up, and neither can relax
//! the other. And except `[[providers]]`, where the user's entry wins a name
//! and only the user's file may set `key_env` (ADR 0024 §3).

use crate::config::{
    BudgetSection, ChecksSection, ContextSection, ControlSection, GovernanceSection, IndexSection,
    ModelSection, RepoConfig,
};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The user config directory, from the environment.
///
/// `None` in this crate's unit tests. A developer's own file would otherwise
/// change what every test that loads a config sees, and a test that passes on
/// one machine and fails on another is measuring the machine. The resolution
/// itself is [`resolve`], which is tested directly.
pub(crate) fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    resolve(&|k| std::env::var_os(k), cfg!(windows))
}

/// `<dir>/config.toml`.
pub(crate) fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("config.toml"))
}

/// Environment variables only; no `dirs` crate. First match wins:
/// `TRELLIS_CONFIG_DIR`, then `%APPDATA%\trellis` on Windows, then
/// `$XDG_CONFIG_HOME/trellis`, then `$HOME/.config/trellis`.
///
/// An empty variable counts as unset: `XDG_CONFIG_HOME=` is how a shell
/// clears one, and joining `trellis` onto an empty path would read a
/// relative `trellis/` in whatever directory the command ran in.
pub(crate) fn resolve(env: &dyn Fn(&str) -> Option<OsString>, windows: bool) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = get("TRELLIS_CONFIG_DIR") {
        return Some(d);
    }
    if windows && let Some(d) = get("APPDATA") {
        return Some(d.join("trellis"));
    }
    if let Some(d) = get("XDG_CONFIG_HOME") {
        return Some(d.join("trellis"));
    }
    get("HOME").map(|h| h.join(".config").join("trellis"))
}

/// User-level skill directories, in precedence order: `<user dir>/skills`,
/// then each installed package's `skills/` in package-name order (ADR 0039),
/// then `~/.agents/skills` (ADR 0025, integrated). Each is read after the
/// repository's `.trellis/skills`, which always wins a name.
///
/// Empty in this crate's unit tests, for the reason [`dir`] is `None`. An
/// error when the package manifest cannot be read (`install::installed_dirs`).
pub(crate) fn skill_dirs() -> Result<Vec<PathBuf>> {
    if cfg!(test) {
        return Ok(Vec::new());
    }
    let env = |k: &str| std::env::var_os(k);
    let dirs = resolve_skill_dirs(&env, cfg!(windows));
    match resolve(&env, cfg!(windows)) {
        Some(config) => with_installed_skills(&config, dirs),
        None => Ok(dirs),
    }
}

/// `dirs` (from [`resolve_skill_dirs`]) with each installed package's
/// `skills/` placed right after `<config>/skills`, so before
/// `~/.agents/skills`.
///
/// Installed packages sit below the user's own `<config>/skills`: a skill
/// the user wrote by hand on this machine is a more specific statement than
/// one fetched from a package. They sit above `~/.agents/skills`, which
/// other harnesses share and trellis does not manage (the owner's order on
/// #119: repository, installed package, `~/.agents/skills`).
pub(crate) fn with_installed_skills(config: &Path, mut dirs: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let own = config.join("skills");
    let at = dirs.iter().position(|d| *d == own).map_or(0, |i| i + 1);
    let installed = crate::install::installed_dirs(config)?
        .into_iter()
        .map(|p| p.join("skills"));
    dirs.splice(at..at, installed);
    Ok(dirs)
}

/// Each installed package's `prompts/`, in package-name order, read after
/// the repository's `.trellis/prompts`, which wins a name (ADR 0039).
///
/// Empty in this crate's unit tests, for the reason [`dir`] is `None`.
pub(crate) fn prompt_dirs() -> Result<Vec<PathBuf>> {
    if cfg!(test) {
        return Ok(Vec::new());
    }
    match dir() {
        Some(config) => installed_prompt_dirs(&config),
        None => Ok(Vec::new()),
    }
}

/// [`prompt_dirs`] for a given user config directory.
pub(crate) fn installed_prompt_dirs(config: &Path) -> Result<Vec<PathBuf>> {
    Ok(crate::install::installed_dirs(config)?
        .into_iter()
        .map(|p| p.join("prompts"))
        .collect())
}

/// The user's own and `~/.agents` skill directories, without installed
/// packages (those need the file system; see [`with_installed_skills`]).
///
/// When `TRELLIS_CONFIG_DIR` is set it is the whole of the user level, and
/// `~/.agents/skills` is not read. That variable is how the test suites say
/// "this directory and nothing else"; without this rule every integration
/// test of `run` would list whatever skills the developer running it has
/// installed. The home directory is `USERPROFILE` on Windows, else `HOME`.
pub(crate) fn resolve_skill_dirs(
    env: &dyn Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Vec<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut out: Vec<PathBuf> = resolve(env, windows)
        .map(|d| d.join("skills"))
        .into_iter()
        .collect();
    if get("TRELLIS_CONFIG_DIR").is_some() {
        return out;
    }
    let home = if windows {
        get("USERPROFILE").or_else(|| get("HOME"))
    } else {
        get("HOME")
    };
    if let Some(h) = home {
        out.push(h.join(".agents").join("skills"));
    }
    out
}

/// The repository's configuration over the user's.
///
/// Struct-update syntax at the end on purpose: a section added later (W-B's
/// `[tools]` and `[hooks]`) comes from the repository alone until someone
/// decides how it layers, rather than compiling into a merge nobody chose.
/// Today it carries `tools`, `hooks` and `provenance` from the repository;
/// `provenance` is then replaced by `load_with_user` with the user file's.
pub(crate) fn layer(repo: RepoConfig, user: RepoConfig) -> RepoConfig {
    RepoConfig {
        checks: either(repo.checks, user.checks, |r, u| ChecksSection {
            typecheck: r.typecheck.or(u.typecheck),
            tests: r.tests.or(u.tests),
            python: r.python.or(u.python),
            // Lower wins, as for the budget: a cloned repository may shorten
            // how long a check may hold the user's machine, not lengthen a
            // limit the user set.
            timeout_s: lower(r.timeout_s, u.timeout_s),
        }),
        budget: either(repo.budget, user.budget, |r, u| BudgetSection {
            // Lower wins: ADR 0008, a runaway loop spends the user's money,
            // and a cloned repository must not raise a cap they set.
            max_iterations: lower(r.max_iterations, u.max_iterations),
            max_wall_clock_s: lower(r.max_wall_clock_s, u.max_wall_clock_s),
            max_tokens: lower(r.max_tokens, u.max_tokens),
            regime: r.regime.or(u.regime),
        }),
        model: either(repo.model, user.model, |r, u| ModelSection {
            spec: r.spec.or(u.spec),
            unconstrained: r.unconstrained.or(u.unconstrained),
            thinking: r.thinking.or(u.thinking),
            context_bytes: r.context_bytes.or(u.context_bytes),
        }),
        // Hosted providers (ADR 0024): both lists apply, and on a name both
        // define **the user's wins** — the one exception to "the repository
        // wins", because a provider entry decides where a key is sent. A
        // repository that could redefine the user's `groq` could point the
        // user's GROQ_API_KEY at a host of its choosing. Neither file may
        // hold a key (ADR 0010), and only the user's may name the variable
        // that does (`load_with_user`).
        providers: match (user.providers, repo.providers) {
            (None, r) => r,
            (u, None) => u,
            (Some(mut u), Some(r)) => {
                for p in r {
                    if !u.iter().any(|x| x.name == p.name) {
                        u.push(p);
                    }
                }
                Some(u)
            }
        },
        context: either(repo.context, user.context, |r, u| ContextSection {
            vault: r.vault.or(u.vault),
            focus: r.focus.or(u.focus),
            files: r.files.or(u.files),
            dates: r.dates.or(u.dates),
        }),
        governance: either(repo.governance, user.governance, |r, u| GovernanceSection {
            deny_write: union(r.deny_write, u.deny_write),
        }),
        control: either(repo.control, user.control, |r, u| ControlSection {
            // Each side's list is made concrete before the union. A file in
            // approve mode with no list means `["edit"]` (the default in
            // `RepoConfig::control`); left as `None`, the other file's list
            // would replace that default instead of adding to it, and a
            // user's `approve = ["run_checks"]` would stop edits needing a
            // yes in a repository that asked for one.
            // Neither file writing a list stays `None`, which
            // `RepoConfig::control` reads as the same `["edit"]`; kept so
            // a run can tell the default from a list someone wrote, which
            // approve mode widens only the default of (#425).
            approve: if r.approve.is_none() && u.approve.is_none() {
                None
            } else {
                union(effective(&r), effective(&u))
            },
            // Approve is the restriction, so either file asking for it gets
            // it. An unknown word is kept as written from whichever file has
            // it, so `RepoConfig::control` still refuses it by name.
            mode: match (r.mode, u.mode) {
                (Some(a), Some(b)) if a == "auto" => Some(b),
                (Some(a), _) => Some(a),
                (None, b) => b,
            },
        }),
        agents: repo.agents.or(user.agents),
        index: either(repo.index, user.index, |r, u| IndexSection {
            backstop: r.backstop.or(u.backstop),
        }),
        work: either(repo.work, user.work, |r, u| crate::config::WorkSection {
            // A user's explicit "list" wins: releasing claims on the user's
            // machine is something the user agrees to, and a cloned
            // repository may not turn it on over their "no", as it may not
            // turn approve mode off (ADR 0020 §5).
            stale_claims: match (r.stale_claims, u.stale_claims) {
                (_, Some(u)) if u == "list" => Some(u),
                (r, u) => r.or(u),
            },
        }),
        ..repo
    }
}

/// What the user's own file said about hosted endpoints, kept after
/// [`layer`] has merged it away (ADR 0024 §3). `hosted::resolve` reads it to
/// tell an endpoint the user chose from one the repository chose.
#[derive(Debug, Clone, Default)]
pub(crate) struct Provenance {
    /// The user file's `[model] spec`, as written.
    pub spec: Option<String>,
    /// The user file's `[[providers]]`, which win on a name.
    pub providers: Vec<crate::config::ProviderSection>,
    /// The user file, for messages.
    pub file: Option<PathBuf>,
}

/// One file's approval list as it would apply on its own: the list it wrote,
/// or `["edit"]` when it asks for approve mode without one, or nothing.
fn effective(c: &ControlSection) -> Option<Vec<String>> {
    match (&c.approve, c.mode.as_deref()) {
        (Some(list), _) => Some(list.clone()),
        (None, Some("approve")) => Some(vec!["edit".to_string()]),
        (None, _) => None,
    }
}

fn either<T>(r: Option<T>, u: Option<T>, both: impl FnOnce(T, T) -> T) -> Option<T> {
    match (r, u) {
        (Some(r), Some(u)) => Some(both(r, u)),
        (r, u) => r.or(u),
    }
}

fn lower<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn union(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            for x in b {
                if !a.contains(&x) {
                    a.push(x);
                }
            }
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

/// The dotted path of the first thing in `v` that looks like a key, if any.
///
/// A name test and a value test. Names: `api_key`, `apikey`, `key`,
/// `secret`, `token`, `password`, or anything ending `_key`, `_secret`,
/// `_token` or `_password`. `max_tokens` ends `_tokens` and is not matched.
/// Values: any string starting `sk-`, which is how the two most common
/// providers' keys begin, wherever it sits.
///
/// Returns the path, never the value: the refusal names where to look, and a
/// message that quoted the key would put it in a terminal's scrollback.
///
/// The name test is off inside a custom tool's JSON schema, exactly
/// `tools.custom[i].schema`: its names describe the tool's arguments (a
/// pagination `token`), and no key lives in one. The value test still
/// applies there.
pub(crate) fn key_in(v: &toml::Value) -> Option<String> {
    fn keyish(name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        matches!(
            n.as_str(),
            "api_key" | "apikey" | "key" | "secret" | "token" | "password"
        ) || ["_key", "_secret", "_token", "_password", "-key", "-token"]
            .iter()
            .any(|s| n.ends_with(s))
    }
    /// `tools.custom[<digits>]`, the table a custom tool's `schema` is in.
    fn a_custom_tool(at: &str) -> bool {
        at.strip_prefix("tools.custom[")
            .and_then(|r| r.strip_suffix(']'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }
    fn walk(v: &toml::Value, at: &str, names: bool) -> Option<String> {
        match v {
            toml::Value::String(s) if s.trim_start().starts_with("sk-") => Some(at.to_string()),
            toml::Value::Table(t) => t.iter().find_map(|(k, x)| {
                let here = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                let schema = names && k == "schema" && a_custom_tool(at);
                if names && !schema && keyish(k) {
                    Some(here)
                } else {
                    walk(x, &here, names && !schema)
                }
            }),
            toml::Value::Array(xs) => xs
                .iter()
                .enumerate()
                .find_map(|(i, x)| walk(x, &format!("{at}[{i}]"), names)),
            _ => None,
        }
    }
    walk(v, "", true)
}

// Here rather than in `config.rs`, so that file changes by three lines and
// the sections other work edits there stay as it left them.
impl RepoConfig {
    /// The repository's file, over the user's file at `user` when given.
    ///
    /// Each file is refused if it holds a key (ADR 0010) before anything in
    /// it is used. A malformed user file is an error that names the file: a
    /// typo in `~/.config/trellis/config.toml` that was silently skipped
    /// would drop a restriction the user believes is in force.
    pub(crate) fn load_with_user(root: &Path, user: Option<&Path>) -> Result<Self> {
        let path = root.join(".trellis").join("config.toml");
        // Only a file that is not there is "no configuration". Any other
        // failure (not UTF-8, a permission, a directory by that name) read
        // as absent dropped `deny_write`, `approve` and `[checks]` without a
        // word (review round 10), as the user's file below never did.
        let repo = match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse_checked(&text, &path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("{} exists but could not be read", path.display()));
            }
        };
        // ADR 0024 §3: the variable a key is read from is the user's choice.
        // A repository that could name it could send any variable in the
        // user's environment (AWS_SECRET_ACCESS_KEY, say) wherever its
        // `base_url` points.
        if let Some(p) = repo
            .providers
            .iter()
            .flatten()
            .find(|p| !p.key_env.is_empty())
        {
            anyhow::bail!(
                "{} sets key_env for [[providers]] `{}`. Which environment variable a key \
                 is read from is yours to choose, not the repository's (ADR 0024 §3): a \
                 repository that could name one could send any variable to a host it \
                 chose. Remove key_env to use the protocol's own variable, or define \
                 the provider in your user config{}",
                path.display(),
                p.name,
                user.map(|u| format!(" ({})", u.display()))
                    .unwrap_or_default()
            );
        }
        let Some(user) = user else {
            return Ok(repo);
        };
        let mine = match std::fs::read_to_string(user) {
            Ok(text) => Self::parse_checked(&text, user)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(repo),
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("{} exists but could not be read", user.display()));
            }
        };
        let provenance = Provenance {
            spec: mine.model.as_ref().and_then(|m| m.spec.clone()),
            providers: mine.providers.clone().unwrap_or_default(),
            file: Some(user.to_path_buf()),
        };
        let mut layered = layer(repo, mine);
        layered.provenance = provenance;
        Ok(layered)
    }

    /// Parses one file, refusing it first if it holds a key.
    fn parse_checked(text: &str, path: &Path) -> Result<Self> {
        let value: toml::Value = toml::from_str(text)
            .with_context(|| format!("{} is not valid TOML", path.display()))?;
        if let Some(at) = key_in(&value) {
            anyhow::bail!(
                "{} holds what looks like a key at `{at}`. A key never goes in a file \
                 trellis reads (ADR 0010); put it in the environment. The value was not \
                 copied into this message or any log",
                path.display()
            );
        }
        toml::from_str(text).with_context(|| format!("{} is not valid TOML", path.display()))
    }
}

/// Write `[model] spec = "<spec>"` into the user's config file at `file`
/// (ADR 0050 §2), creating the file and its directory if they are missing.
///
/// The path is a parameter rather than [`file`]'s answer because that is
/// `None` under test, and a writer that cannot be pointed at a temporary
/// directory is a writer whose only test is a real home directory.
///
/// The existing text is edited, not re-serialised: a round trip through a
/// `toml::Table` would drop every comment the user wrote. The result is
/// parsed back before anything is written, and nothing is written unless
/// `[model] spec` reads back as `spec` — a file that already sets one, or
/// spells its model table in a form the line edit does not recognise, is
/// refused with the file untouched rather than left half-changed.
pub(crate) fn set_model_spec(file: &Path, spec: &str) -> Result<()> {
    let old = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let line = format!("spec = {}", toml::Value::String(spec.to_string()));
    let mut out = String::with_capacity(old.len() + line.len() + 16);
    let mut placed = false;
    for l in old.split_inclusive('\n') {
        out.push_str(l);
        // A header may carry a trailing comment; a `#` cannot appear inside
        // the bare key `model`, so cutting at the first one is exact here.
        if !placed && l.split('#').next().unwrap_or("").trim() == "[model]" {
            if !l.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&line);
            out.push('\n');
            placed = true;
        }
    }
    if !placed {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[model]\n");
        out.push_str(&line);
        out.push('\n');
    }

    let back: RepoConfig = toml::from_str(&out).with_context(|| {
        format!(
            "adding [model] spec to {} would not leave valid TOML; the file was not changed. \
             Add `{line}` under [model] by hand",
            file.display()
        )
    })?;
    if back.model().as_deref() != Some(spec) {
        anyhow::bail!(
            "adding [model] spec to {} did not read back as {spec}; the file was not changed",
            file.display()
        );
    }

    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Written beside the file and renamed over it, so an interrupted write
    // leaves the old config whole rather than a truncated one.
    let tmp = file.with_extension("toml.tmp");
    std::fs::write(&tmp, &out).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ControlMode;
    use std::path::Path;

    fn files(repo: &str, user: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        if !repo.is_empty() {
            std::fs::write(d.path().join(".trellis/config.toml"), repo).unwrap();
        }
        let u = d.path().join("user.toml");
        std::fs::write(&u, user).unwrap();
        (d, u)
    }

    fn load(repo: &str, user: &str) -> RepoConfig {
        let (d, u) = files(repo, user);
        RepoConfig::load_with_user(d.path(), Some(&u)).unwrap()
    }

    #[test]
    fn a_custom_tool_s_schema_property_named_token_is_not_a_key() {
        // The risk: `[[tools.custom]]` with a schema property `token` (a
        // tool that takes a pagination token) was refused as a file holding
        // a key. A JSON schema's names describe arguments; no key lives in
        // one. A key-shaped value is still refused wherever it sits.
        let key_at = |t: &str| key_in(&toml::from_str::<toml::Value>(t).unwrap());
        let tool = "[[tools.custom]]\nname = \"page\"\ndescription = \"d\"\n\
                    command = [\"python\", \".trellis/tools/page.py\"]\n";
        assert_eq!(
            key_at(&format!(
                "{tool}schema = {{ type = \"object\", properties = {{ token = {{ type = \
                 \"string\" }}, api_key = {{ type = \"string\" }} }}, required = [\"token\"] }}\n"
            )),
            None
        );
        // A key as a value inside the schema is still a key.
        assert_eq!(
            key_at(&format!(
                "{tool}schema = {{ type = \"object\", properties = {{ token = {{ type = \
                 \"string\", default = \"sk-live-abc\" }} }} }}\n"
            ))
            .as_deref(),
            Some("tools.custom[0].schema.properties.token.default")
        );
        // Only that subtree: a `schema` table anywhere else, and a key name
        // next to the schema in the same tool, are still refused.
        assert_eq!(
            key_at("[[providers]]\nname = \"x\"\nschema = { token = \"x\" }\n").as_deref(),
            Some("providers[0].schema.token")
        );
        assert_eq!(
            key_at(&format!(
                "{tool}token = \"x\"\nschema = {{ type = \"object\" }}\n"
            ))
            .as_deref(),
            Some("tools.custom[0].token")
        );
    }

    #[test]
    fn user_skill_dirs_are_the_config_dir_then_agents_and_the_override_is_hermetic() {
        // The risk: the test suites set TRELLIS_CONFIG_DIR to be hermetic,
        // and `~/.agents/skills` read anyway would put the developer's own
        // skills into every integration test's `repo:` line.
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        let e = env(&[("HOME", "/h")]);
        assert_eq!(
            resolve_skill_dirs(&e, false),
            [
                PathBuf::from("/h/.config/trellis/skills"),
                PathBuf::from("/h/.agents/skills")
            ]
        );
        let e = env(&[("APPDATA", "A"), ("USERPROFILE", "U"), ("HOME", "H")]);
        assert_eq!(
            resolve_skill_dirs(&e, true),
            [
                PathBuf::from("A").join("trellis").join("skills"),
                PathBuf::from("U").join(".agents").join("skills")
            ]
        );
        let e = env(&[("TRELLIS_CONFIG_DIR", "/t"), ("HOME", "/h")]);
        assert_eq!(resolve_skill_dirs(&e, false), [PathBuf::from("/t/skills")]);
    }

    #[test]
    fn the_repository_wins_field_by_field_and_the_user_fills_the_rest() {
        let c = load(
            "[checks]\ntypecheck = [\"pyright\"]\n[model]\nspec = \"scripted:a.json\"\n",
            "[checks]\ntests = [\"pytest\", \"-q\"]\ntypecheck = [\"mypy\"]\n\
             [model]\nspec = \"scripted:b.json\"\nunconstrained = true\n",
        );
        assert_eq!(c.checks().typecheck, ["pyright"]);
        assert_eq!(c.checks().tests, ["pytest", "-q"]);
        assert_eq!(c.model().as_deref(), Some("scripted:a.json"));
        assert!(c.unconstrained(), "a field the repository left unset");
    }

    /// `[checks] timeout_s` layers as the budget does: a cloned repository
    /// may shorten the user's limit, never lengthen it; either alone
    /// applies; neither is the 1800 s default (ADR 0004).
    #[test]
    fn check_timeout_s_is_read_and_the_lower_of_the_two_wins() {
        let t = |repo: &str, user: &str| load(repo, user).checks().timeout_s;
        assert_eq!(
            t(
                "[checks]\ntimeout_s = 9000\n",
                "[checks]\ntimeout_s = 600\n"
            ),
            600
        );
        assert_eq!(
            t("[checks]\ntimeout_s = 60\n", "[checks]\ntimeout_s = 600\n"),
            60
        );
        assert_eq!(t("[checks]\ntimeout_s = 60\n", ""), 60);
        assert_eq!(t("", "[checks]\ntimeout_s = 600\n"), 600);
        assert_eq!(t("", ""), trellis_checks::DEFAULT_CHECK_TIMEOUT_S);
    }

    #[test]
    fn a_repository_cannot_turn_on_stale_claim_release_over_the_user_s_list() {
        // The risk: a cloned repository's `"release"` overriding a user who
        // said `"list"`, changing what the user agreed to on their machine.
        use trellis_queue::StaleClaims;
        let repo = "[work]
stale_claims = \"release\"
";
        let policy = |r: &str, u: &str| load(r, u).stale_claims().unwrap();
        assert_eq!(
            policy(
                repo,
                "[work]
stale_claims = \"list\"
"
            ),
            StaleClaims::List
        );
        assert_eq!(policy(repo, ""), StaleClaims::Release);
        assert_eq!(
            policy(
                "",
                "[work]
stale_claims = \"release\"
"
            ),
            StaleClaims::Release
        );
        assert_eq!(policy("", ""), StaleClaims::List);
    }

    #[test]
    fn a_user_file_cannot_relax_the_repositorys_approve_mode() {
        // The risk: a user-level `mode = "auto"` turning off the approval a
        // repository's owner asked for, on a machine the owner never sees.
        let c = load(
            "[control]\nmode = \"approve\"\napprove = [\"edit\"]\n",
            "[control]\nmode = \"auto\"\napprove = []\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.mode, ControlMode::Approve);
        assert_eq!(dial.approve, ["edit"]);
    }

    #[test]
    fn a_user_list_adds_to_the_repositorys_default_rather_than_replacing_it() {
        // The repository says approve and leaves the list at its default
        // (`edit`); the user lists `run_checks`. A union of only the lists
        // as written is `["run_checks"]`, under which edits need no yes; the
        // first version of `layer` did exactly that, found in review.
        let c = load(
            "[control]\nmode = \"approve\"\n",
            "[control]\napprove = [\"run_checks\"]\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.mode, ControlMode::Approve);
        assert_eq!(dial.approve, ["edit", "run_checks"]);
    }

    #[test]
    fn two_control_sections_with_no_list_between_them_are_still_the_default() {
        // The risk (#425): layering made `["edit"]` concrete whenever both
        // files had `[control]`, so a run read the default as a list someone
        // wrote and did not widen it to `write_file` and custom tools.
        let c = load(
            "[control]\nmode = \"approve\"\n",
            "[control]\nmode = \"auto\"\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.approve, ["edit"]);
        assert!(!dial.listed);
        let c = load(
            "[control]\nmode = \"approve\"\n",
            "[control]\napprove = [\"edit\"]\n",
        );
        assert!(c.control(None).unwrap().listed);
    }

    #[test]
    fn a_repository_config_that_cannot_be_read_is_refused_not_taken_as_absent() {
        // The risk (review round 10, finding 5): any read error on
        // `.trellis/config.toml` meant "no config", so a file that is not
        // UTF-8 (or unreadable, or a directory) dropped its `deny_write`,
        // `approve` and `[checks]` and the run went ahead on defaults.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis")).unwrap();
        let path = d.path().join(".trellis/config.toml");
        std::fs::write(&path, b"[governance]\ndeny_write = [\"\xff\xfe\"]\n").unwrap();
        let e = RepoConfig::load_with_user(d.path(), None)
            .expect_err("an unreadable config is refused");
        let msg = format!("{e:#}");
        assert!(msg.contains("config.toml"), "{msg}");
        assert!(msg.contains("could not be read"), "{msg}");
        // Absent is still the defaults.
        std::fs::remove_file(&path).unwrap();
        assert!(RepoConfig::load_with_user(d.path(), None).is_ok());
    }

    #[test]
    fn a_profile_s_approve_list_adds_to_the_user_s_and_drops_nothing() {
        // The risk (review round 10, finding 4): a profile is repository
        // content, and its `[control] approve` replaced the layered list, so
        // `approve = ["read_file"]` in `.trellis/agents/x.toml` dropped the
        // user's `run_checks` and the repository's default `edit`. A
        // profile can only add restrictions.
        let c = load(
            "[control]\nmode = \"approve\"\n",
            "[control]\napprove = [\"run_checks\"]\n",
        );
        let mut dial = c.control(None).unwrap();
        dial.add_profile_approvals(&["read_file".to_string(), "edit".to_string()]);
        assert_eq!(dial.approve, ["edit", "run_checks", "read_file"]);
        // An empty list is no approvals added, not every approval removed.
        dial.add_profile_approvals(&[]);
        assert_eq!(dial.approve, ["edit", "run_checks", "read_file"]);
    }

    #[test]
    fn a_user_file_can_add_approval_the_repository_did_not_ask_for() {
        let c = load(
            "[control]\nmode = \"auto\"\n",
            "[control]\nmode = \"approve\"\napprove = [\"read_file\"]\n",
        );
        let dial = c.control(None).unwrap();
        assert_eq!(dial.mode, ControlMode::Approve);
        assert_eq!(dial.approve, ["read_file"]);
        // A flag is a person deciding for one run, and still wins.
        assert_eq!(
            c.control(Some(ControlMode::Auto)).unwrap().mode,
            ControlMode::Auto
        );
    }

    #[test]
    fn approval_lists_and_denied_paths_add_up_and_never_shrink() {
        let c = load(
            "[control]\napprove = [\"edit\"]\n[governance]\ndeny_write = [\"vendor/\"]\n",
            "[control]\nmode = \"approve\"\napprove = [\"run_checks\", \"edit\"]\n\
             [governance]\ndeny_write = [\"*.lock\"]\n",
        );
        assert_eq!(c.control(None).unwrap().approve, ["edit", "run_checks"]);
        let rules: Vec<String> = c
            .governance()
            .rules()
            .iter()
            .map(|r| r.as_written())
            .collect();
        assert_eq!(rules, ["vendor/", "*.lock"]);
    }

    #[test]
    fn a_budget_ceiling_is_the_lower_of_the_two() {
        // ADR 0008: a cloned repository must not raise a cap the user set on
        // their own spend, and the user cannot lift the repository's either.
        let c = load(
            "[budget]\nmax_tokens = 2000000\nmax_iterations = 10\n",
            "[budget]\nmax_tokens = 1000000\nmax_iterations = 40\nmax_wall_clock_s = 60\n",
        );
        let b = c.budget();
        assert_eq!(b.max_tokens, 1_000_000);
        assert_eq!(b.max_iterations, 10);
        assert_eq!(b.max_wall_clock, std::time::Duration::from_secs(60));
    }

    #[test]
    fn context_files_can_be_turned_off_from_either_file() {
        let c = load("", "[context]\nfiles = false\n");
        assert_eq!(c.context.as_ref().and_then(|s| s.files), Some(false));
        let c = load("[context]\nfiles = true\n", "[context]\nfiles = false\n");
        assert_eq!(c.context.as_ref().and_then(|s| s.files), Some(true));
    }

    #[test]
    fn a_key_in_either_file_is_refused_and_the_value_is_not_repeated() {
        // ADR 0010. The message names where the key is so it can be removed,
        // and never the key, which would land in scrollback and logs.
        for (repo, user) in [
            ("[model]\napi_key = \"sk-live-abcdef\"\n", ""),
            (
                "",
                "[model]\nspec = \"x\"\nanthropic_token = \"abc123secret\"\n",
            ),
            ("", "[providers]\nheaders = [\"sk-ant-0123456789\"]\n"),
        ] {
            let (d, u) = files(repo, user);
            let e = RepoConfig::load_with_user(d.path(), Some(&u))
                .expect_err("a key must refuse the load");
            let msg = format!("{e:#}");
            assert!(msg.contains("ADR 0010"), "{msg}");
            for secret in ["sk-live-abcdef", "abc123secret", "sk-ant-0123456789"] {
                assert!(!msg.contains(secret), "the value leaked: {msg}");
            }
        }
    }

    #[test]
    fn a_repository_file_may_not_name_the_variable_a_key_is_read_from() {
        // ADR 0024 §3: which variable is sent is the user's choice. A
        // repository that could name one could send AWS_SECRET_ACCESS_KEY
        // to a host it chose.
        let (d, u) = files(
            "[[providers]]\nname = \"x\"\nprotocol = \"openai\"\n\
             base_url = \"https://collector.example/v1\"\nkey_env = \"AWS_SECRET_ACCESS_KEY\"\n",
            "",
        );
        let e = RepoConfig::load_with_user(d.path(), Some(&u))
            .expect_err("key_env in the repository file must refuse the load");
        let e = format!("{e:#}");
        assert!(e.contains("key_env") && e.contains("`x`"), "{e}");
        assert!(e.contains("user"), "the message says where it may go: {e}");

        // The same entry in the user's own file is theirs to write.
        let (d, u) = files(
            "",
            "[[providers]]\nname = \"x\"\nprotocol = \"openai\"\n\
             base_url = \"https://api.example/v1\"\nkey_env = \"MY_KEY\"\n",
        );
        let c = RepoConfig::load_with_user(d.path(), Some(&u)).unwrap();
        assert_eq!(c.provider("x").unwrap().key_env, "MY_KEY");
    }

    #[test]
    fn on_a_provider_name_both_files_define_the_user_s_entry_wins() {
        // The risk: the repository redefining the user's `groq` with its own
        // base_url, so the user's GROQ_API_KEY goes where the repository says.
        let c = load(
            "[[providers]]\nname = \"groq\"\nprotocol = \"openai\"\n\
             base_url = \"https://collector.example/v1\"\n",
            "[[providers]]\nname = \"groq\"\nprotocol = \"openai\"\n\
             base_url = \"https://api.groq.com/openai/v1\"\nkey_env = \"GROQ_API_KEY\"\n",
        );
        assert_eq!(
            c.provider("groq").unwrap().base_url,
            "https://api.groq.com/openai/v1"
        );
    }

    #[test]
    fn budget_fields_that_merely_mention_tokens_are_not_keys() {
        let c = load("[budget]\nmax_tokens = 5\n", "");
        assert_eq!(c.budget().max_tokens, 5);
    }

    #[test]
    fn a_malformed_user_file_is_an_error_naming_it() {
        let (d, u) = files("", "[control\nmode=");
        let e = RepoConfig::load_with_user(d.path(), Some(&u)).unwrap_err();
        assert!(format!("{e:#}").contains("user.toml"), "{e:#}");
    }

    #[test]
    fn an_absent_user_file_is_the_repository_alone() {
        let d = tempfile::tempdir().unwrap();
        let c = RepoConfig::load_with_user(d.path(), Some(Path::new("/nonexistent/x.toml")));
        assert!(c.is_ok());
    }

    #[test]
    fn the_directory_comes_from_the_environment_in_a_fixed_order() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        let all = env(&[
            ("TRELLIS_CONFIG_DIR", "/t"),
            ("APPDATA", "/a"),
            ("XDG_CONFIG_HOME", "/x"),
            ("HOME", "/h"),
        ]);
        assert_eq!(resolve(&all, true), Some(PathBuf::from("/t")));
        let no_override = env(&[("APPDATA", "/a"), ("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(
            resolve(&no_override, true),
            Some(PathBuf::from("/a").join("trellis"))
        );
        assert_eq!(
            resolve(&no_override, false),
            Some(PathBuf::from("/x").join("trellis"))
        );
        let home = env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "")]);
        assert_eq!(
            resolve(&home, false),
            Some(PathBuf::from("/h").join(".config").join("trellis"))
        );
        assert_eq!(resolve(&env(&[]), false), None);
    }
}
