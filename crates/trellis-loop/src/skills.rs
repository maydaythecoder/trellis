//! Skills: procedures a repository writes down, disclosed progressively.
//!
//! ADR 0019. Each skill is `.trellis/skills/<name>/SKILL.md` in the Agent
//! Skills format: YAML frontmatter with `name` and `description`, then a body.
//! The name and description of every valid skill go into the prompt; the body
//! enters the transcript only when the model calls `read_skill`.
//!
//! **A skill informs; it does not bind.** It is repository text with the
//! standing of `context.md`. Nothing in it is executed, and nothing in it can
//! authorise skipping a check: convergence reads a `Verdict`, and a skill is
//! neither a turn nor a check.
//!
//! The frontmatter reader here is deliberately small. It reads the two keys
//! the format defines as required, as plain, quoted or block scalars, and
//! ignores every other key. A full YAML parser would accept documents whose
//! meaning a reader cannot predict from looking at them, and the only thing
//! this needs to get right is two strings.

use std::path::{Path, PathBuf};

/// Where skills live, relative to the repository root.
pub const SKILLS_DIR: &str = ".trellis/skills";

/// The most the disclosed list may cost in the prompt, in bytes.
///
/// A library such as low-level-dev-skills has 142 descriptions; without a cap
/// every one of them would be sent on every turn, and the window is the scarce
/// resource on local weights (S5).
pub const DISCLOSURE_CAP: usize = 4_096;

const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 1_024;

/// A skill that parsed and whose name matches its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

impl Skill {
    /// The line this skill costs in the prompt. The cap is measured on this
    /// string, so the number of bytes capped is the number of bytes sent.
    pub fn listing(&self) -> String {
        format!("  {}: {}\n", self.name, self.description)
    }
}

/// A directory under `.trellis/skills/` that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSkill {
    /// The directory name, as found.
    pub dir: String,
    pub reason: String,
}

/// Every skill a repository holds, sorted by name, and every one it skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillCatalog {
    pub valid: Vec<Skill>,
    pub invalid: Vec<InvalidSkill>,
    /// Names in `valid` that came from a user-level directory rather than the
    /// repository, for the `repo:` line.
    pub user: Vec<String>,
    /// User-level skills not loaded because a skill of the same name was
    /// found first (the repository's, or an earlier user directory's).
    pub shadowed: Vec<String>,
}

/// What goes into the prompt: the skills that fit under the cap, and how many
/// did not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disclosed {
    pub listed: Vec<Skill>,
    /// Valid skills left out because the list was full. Stated, never
    /// silent: a cut list read as the whole library is a false statement.
    pub omitted: usize,
}

impl SkillCatalog {
    /// Reads `<root>/.trellis/skills`. Absent is an empty catalog, not an
    /// error: most repositories have no skills.
    pub fn discover(root: &Path) -> Self {
        let mut catalog = Self::default();
        for (dir_name, path) in skill_dirs(&root.join(SKILLS_DIR)) {
            match load(&dir_name, &path, Some(root)) {
                Ok(s) => catalog.valid.push(s),
                Err(reason) => catalog.invalid.push(InvalidSkill {
                    dir: dir_name,
                    reason,
                }),
            }
        }
        catalog
    }

    /// The repository's skills, then each user-level directory's, in order.
    ///
    /// The first directory holding a name wins, and `read_body_in` looks in
    /// the same order, so the body read is the one whose description was
    /// listed. A repository's skill is never replaced by one from the user's
    /// machine: the repository's is the one its contributors can review. A
    /// directory counts as found even when its skill is invalid, so which
    /// skill a model reads never depends on whether an earlier one parsed.
    /// The merged list is re-sorted by name, so the cap cuts by name as it
    /// does for a repository alone.
    pub fn discover_with(root: &Path, user_dirs: &[PathBuf]) -> Self {
        let mut catalog = Self::discover(root);
        let mut found: Vec<String> = catalog
            .valid
            .iter()
            .map(|s| s.name.clone())
            .chain(catalog.invalid.iter().map(|i| i.dir.clone()))
            .collect();
        for base in user_dirs {
            for (dir_name, path) in skill_dirs(base) {
                if found.contains(&dir_name) {
                    catalog.shadowed.push(dir_name);
                    continue;
                }
                found.push(dir_name.clone());
                // A user directory is the user's own, outside any repository
                // by definition: the repository-input rule does not apply.
                match load(&dir_name, &path, None) {
                    Ok(s) => {
                        catalog.user.push(s.name.clone());
                        catalog.valid.push(s);
                    }
                    Err(reason) => catalog.invalid.push(InvalidSkill {
                        dir: format!("{}/{dir_name}", base.display()),
                        reason,
                    }),
                }
            }
        }
        catalog.valid.sort_by(|a, b| a.name.cmp(&b.name));
        catalog
    }

    /// The skills that fit in [`DISCLOSURE_CAP`] bytes, in name order.
    ///
    /// Stops at the first that does not fit rather than skipping it for a
    /// shorter one later: which skills a model is shown should follow from
    /// their names, not from the length of their neighbours' descriptions.
    pub fn disclosed(&self) -> Disclosed {
        let mut used = 0;
        let mut listed = Vec::new();
        for s in &self.valid {
            let cost = s.listing().len();
            if used + cost > DISCLOSURE_CAP {
                break;
            }
            used += cost;
            listed.push(s.clone());
        }
        Disclosed {
            omitted: self.valid.len() - listed.len(),
            listed,
        }
    }
}

/// The body of a named skill, for `read_skill`.
///
/// The name is checked against the skill-name alphabet before it touches a
/// path, which is what keeps `read_skill{name: "../../x"}` inside the skills
/// directory. A skill that is present but invalid is refused with its reason,
/// the same one `trellis doctor` gives.
pub fn read_body(root: &Path, name: &str) -> Result<String, SkillError> {
    read_body_in(root, &[], name)
}

/// Why `read_skill` returned no body. The model is shown the message, so
/// each is the string these functions returned before (review round 11).
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("`{name}` is not a skill name: {why}")]
    BadName { name: String, why: String },
    /// The repository's entry resolves outside it, into `.git`, or could
    /// not be looked at.
    #[error("skill `{name}` was not read: {source}")]
    Refused {
        name: String,
        source: trellis_core::paths::RepoInputError,
    },
    #[error("no skill named `{0}`. The skills listed in the prompt are the ones this run has.")]
    NotFound(String),
    #[error("skill `{name}` has no readable SKILL.md: {source}")]
    Unreadable {
        name: String,
        source: std::io::Error,
    },
    #[error("skill `{name}` is invalid: {why}")]
    Invalid { name: String, why: String },
    #[error("skill `{name}` is invalid: its frontmatter names it `{declared}`")]
    Misnamed { name: String, declared: String },
    /// A rule, not a missing file: names the profile and its list.
    #[error("skill `{name}` is denied to agent profile `{profile}`: its [skills] deny = {deny:?}")]
    Denied {
        name: String,
        profile: String,
        deny: Vec<String>,
    },
    #[error(
        "skill `{name}` is not available to agent profile `{profile}`: its [skills] allow = \
         {allow:?}"
    )]
    NotAllowed {
        name: String,
        profile: String,
        allow: Vec<String>,
    },
}

/// [`read_body`], looking in the user-level directories after the
/// repository, in the order [`SkillCatalog::discover_with`] uses.
pub fn read_body_in(root: &Path, user_dirs: &[PathBuf], name: &str) -> Result<String, SkillError> {
    if let Err(why) = check_name(name) {
        return Err(SkillError::BadName {
            name: name.to_string(),
            why,
        });
    }
    // The repository's entry counts when it is a directory or any link,
    // including one that leaves the repository or resolves to nothing: that
    // is refused below by name, never passed over for a user skill.
    let repo = root.join(SKILLS_DIR).join(name);
    let repo_found =
        repo.is_dir() || std::fs::symlink_metadata(&repo).is_ok_and(|m| m.file_type().is_symlink());
    let file = if repo_found {
        repo_skill_md(root, &repo).map_err(|source| SkillError::Refused {
            name: name.to_string(),
            source,
        })?
    } else {
        // The same test `skill_dirs` lists by: a directory, or a link that
        // resolves to nothing. A dangling link the listing counted as found
        // is read (and fails) here, never passed over for a shadowed skill.
        let Some(dir) = user_dirs.iter().map(|base| base.join(name)).find(|d| {
            d.is_dir()
                || (std::fs::symlink_metadata(d).is_ok_and(|m| m.file_type().is_symlink())
                    && std::fs::metadata(d).is_err())
        }) else {
            return Err(SkillError::NotFound(name.to_string()));
        };
        dir.join("SKILL.md")
    };
    let text = std::fs::read_to_string(&file).map_err(|source| SkillError::Unreadable {
        name: name.to_string(),
        source,
    })?;
    let parsed = parse(&text).map_err(|why| SkillError::Invalid {
        name: name.to_string(),
        why,
    })?;
    if parsed.name != name {
        return Err(SkillError::Misnamed {
            name: name.to_string(),
            declared: parsed.name,
        });
    }
    Ok(parsed.body)
}

/// Where `read_skill` looks, and which names an agent profile may read.
///
/// Held by the orchestrator, because that is where `read_skill` is served: a
/// filter applied only to the listing leaves a model that already knows a
/// name free to read it (ADR 0026 §4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillAccess {
    /// User-level skill directories, each holding `<name>/SKILL.md`, after
    /// the repository's in precedence.
    pub user_dirs: Vec<PathBuf>,
    /// The agent profile's `[skills]`, when the run executes as one.
    pub policy: Option<SkillPolicy>,
}

impl SkillAccess {
    /// The body `read_skill` returns, or why it may not.
    pub fn read(&self, root: &Path, name: &str) -> Result<String, SkillError> {
        if let Some(p) = &self.policy {
            p.check(name)?;
        }
        read_body_in(root, &self.user_dirs, name)
    }
}

/// An agent profile's `[skills]` allow and deny lists (ADR 0026 §4).
///
/// Defined here rather than read from `trellis-queue`, so the loop does not
/// depend on the queue. One predicate serves both the listing and
/// `read_skill`; two would drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPolicy {
    pub profile: String,
    /// `None` is every skill.
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
}

impl SkillPolicy {
    /// `Err` names the profile and the list that excludes the skill, so the
    /// model is told it is a rule, not a missing file.
    pub fn check(&self, name: &str) -> Result<(), SkillError> {
        if self.deny.iter().any(|d| d == name) {
            return Err(SkillError::Denied {
                name: name.to_string(),
                profile: self.profile.clone(),
                deny: self.deny.clone(),
            });
        }
        if let Some(a) = &self.allow
            && !a.iter().any(|x| x == name)
        {
            return Err(SkillError::NotAllowed {
                name: name.to_string(),
                profile: self.profile.clone(),
                allow: a.clone(),
            });
        }
        Ok(())
    }
}

/// The subdirectories of `base`, sorted, so the list and which skills the
/// cap leaves out are the same on every machine; `read_dir` order is the
/// filesystem's. Absent is empty: most repositories have no skills.
///
/// A link is included when it leads to a directory or to nothing, so that
/// `read_body` (whose `is_dir()` follows links) never reads a skill this
/// listing did not report; `load` then decides whether it may be read.
fn skill_dirs(base: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_ok_and(|t| {
                t.is_dir()
                    || (t.is_symlink() && std::fs::metadata(e.path()).map_or(true, |m| m.is_dir()))
            })
        })
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    dirs.sort();
    dirs
}

/// `<dir>/SKILL.md` of a repository skill, as its real path, once both the
/// directory and the file resolve inside the repository and outside `.git`
/// (`trellis_core::paths::resolve_repo_input`): a skill's description goes
/// into every prompt and its body into the transcript.
fn repo_skill_md(root: &Path, dir: &Path) -> Result<PathBuf, trellis_core::paths::RepoInputError> {
    let dir = trellis_core::paths::resolve_repo_input(root, dir)?.unwrap_or(dir.to_path_buf());
    let file = dir.join("SKILL.md");
    Ok(trellis_core::paths::resolve_repo_input(root, &file)?.unwrap_or(file))
}

/// `root` is the repository when `path` is one of its skills, and `None`
/// for a user-level directory.
fn load(dir_name: &str, path: &Path, root: Option<&Path>) -> Result<Skill, String> {
    let file = match root {
        Some(root) => repo_skill_md(root, path).map_err(|e| e.to_string())?,
        None => path.join("SKILL.md"),
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("no SKILL.md in the directory".into());
        }
        Err(e) => return Err(format!("SKILL.md could not be read: {e}")),
    };
    let p = parse(&text)?;
    if p.name != dir_name {
        return Err(format!(
            "frontmatter name `{}` does not match the directory name `{dir_name}`",
            p.name
        ));
    }
    Ok(Skill {
        name: p.name,
        description: p.description,
    })
}

struct Parsed {
    name: String,
    description: String,
    body: String,
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("it is empty".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!(
            "it is {} characters; the limit is {MAX_NAME}",
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("only lowercase letters, digits and `-` are allowed".into());
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err("`-` may not lead, trail or repeat".into());
    }
    Ok(())
}

fn parse(text: &str) -> Result<Parsed, String> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let mut lines = text.split('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("SKILL.md does not open with `---` frontmatter".into());
    }
    let mut front: Vec<&str> = Vec::new();
    let mut closed = false;
    for l in lines.by_ref() {
        if l.trim_end() == "---" {
            closed = true;
            break;
        }
        front.push(l);
    }
    if !closed {
        return Err("the frontmatter is never closed with `---`".into());
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    let body = body.trim_start_matches('\n').to_string();

    let name = scalar(&front, "name")?.ok_or("the frontmatter has no `name`")?;
    check_name(&name).map_err(|e| format!("name `{name}` is invalid: {e}"))?;

    let description =
        scalar(&front, "description")?.ok_or("the frontmatter has no `description`")?;
    // Newlines inside a folded or literal scalar become spaces: the listing is
    // one line per skill, and a description that could break the line could
    // close the block it sits in.
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        return Err("`description` is empty".into());
    }
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(format!(
            "`description` is {} characters; the limit is {MAX_DESCRIPTION}",
            description.chars().count()
        ));
    }
    if description.chars().any(char::is_control) {
        return Err("`description` contains a control character".into());
    }
    Ok(Parsed {
        name,
        description,
        body,
    })
}

/// A top-level scalar from the frontmatter, or `None` when the key is absent.
///
/// Reads `key: value`, `key: "value"`, `key: 'value'`, a plain value continued
/// on indented lines, and `key: >` / `key: |` block scalars. Anything else a
/// key's value could be — a list, a map, an anchor — is refused by name
/// rather than read as something it is not.
fn scalar(front: &[&str], key: &str) -> Result<Option<String>, String> {
    let prefix = format!("{key}:");
    let Some(at) = front.iter().position(|l| l.starts_with(&prefix)) else {
        return Ok(None);
    };
    let first = front[at][prefix.len()..].trim();
    let continuation: Vec<&str> = front[at + 1..]
        .iter()
        .take_while(|l| l.is_empty() || l.starts_with(' ') || l.starts_with('\t'))
        .map(|l| l.trim())
        .collect();

    if let Some(style) = first.chars().next().filter(|c| *c == '>' || *c == '|') {
        let joiner = if style == '>' { " " } else { "\n" };
        return Ok(Some(continuation.join(joiner).trim().to_string()));
    }
    if let Some(rest) = first.strip_prefix('"') {
        let inner = rest
            .strip_suffix('"')
            .ok_or_else(|| format!("`{key}` opens a double quote it does not close on its line"))?;
        return Ok(Some(inner.replace("\\\"", "\"").replace("\\\\", "\\")));
    }
    if let Some(rest) = first.strip_prefix('\'') {
        let inner = rest
            .strip_suffix('\'')
            .ok_or_else(|| format!("`{key}` opens a single quote it does not close on its line"))?;
        return Ok(Some(inner.replace("''", "'")));
    }
    if first.starts_with('[')
        || first.starts_with('{')
        || first.starts_with('&')
        || first.starts_with('*')
    {
        return Err(format!("`{key}` must be a string"));
    }
    let mut parts = vec![first];
    parts.extend(continuation.iter().filter(|l| !l.is_empty()));
    let v = parts.join(" ").trim().to_string();
    if v.is_empty() {
        return Err(format!("`{key}` has no value"));
    }
    Ok(Some(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(root: &Path, dir: &str, text: &str) {
        let d = root.join(SKILLS_DIR).join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), text).unwrap();
    }

    fn md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nBODY-{name}\n")
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
    fn a_skill_md_linked_outside_the_repository_is_neither_listed_nor_read() {
        // The risk: `.trellis/skills/x/SKILL.md -> ~/notes/secret.md` put the
        // target's description in every prompt and its body in the transcript.
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("SKILL.md"), md("leak", "OUTSIDE")).unwrap();
        let dir = d.path().join(SKILLS_DIR).join("leak");
        std::fs::create_dir_all(&dir).unwrap();
        if !link(&outside.path().join("SKILL.md"), &dir.join("SKILL.md")) {
            return;
        }
        let c = SkillCatalog::discover(d.path());
        assert!(c.valid.is_empty(), "{c:?}");
        assert!(
            c.invalid[0].reason.contains("outside the repository"),
            "{c:?}"
        );
        let e = read_body(d.path(), "leak").unwrap_err().to_string();
        assert!(e.contains("outside the repository"), "{e}");
        assert!(!e.contains("BODY-leak"), "{e}");
    }

    #[test]
    fn a_skill_directory_linked_outside_is_reported_not_silently_readable() {
        // `skill_dirs` did not follow a linked directory, so discover never
        // listed it, while `read_body`'s `is_dir()` did follow it: a skill
        // readable by name that nothing reported.
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("leak");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("SKILL.md"), md("leak", "OUTSIDE")).unwrap();
        std::fs::create_dir_all(d.path().join(SKILLS_DIR)).unwrap();
        if !link(&target, &d.path().join(SKILLS_DIR).join("leak")) {
            return;
        }
        let c = SkillCatalog::discover(d.path());
        assert!(c.valid.is_empty(), "{c:?}");
        assert_eq!(c.invalid.len(), 1, "named, not skipped: {c:?}");
        assert!(c.invalid[0].reason.contains("outside the repository"));
        let e = read_body(d.path(), "leak").unwrap_err().to_string();
        assert!(e.contains("outside the repository"), "{e}");
    }

    #[test]
    fn a_skill_linked_inside_the_repository_still_loads() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("docs/shared")).unwrap();
        std::fs::write(d.path().join("docs/shared/SKILL.md"), md("shared", "s")).unwrap();
        std::fs::create_dir_all(d.path().join(SKILLS_DIR)).unwrap();
        if !link(
            &d.path().join("docs/shared"),
            &d.path().join(SKILLS_DIR).join("shared"),
        ) {
            return;
        }
        let c = SkillCatalog::discover(d.path());
        assert_eq!(c.valid.len(), 1, "{c:?}");
        assert!(
            read_body(d.path(), "shared")
                .unwrap()
                .contains("BODY-shared")
        );
    }

    #[test]
    fn a_user_skill_outside_the_repository_is_not_refused() {
        // User-level skills are the user's own files, outside any repository
        // by definition (ADR 0025); the rule is for what a repository supplies.
        let d = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let s = user.path().join("mine");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("SKILL.md"), md("mine", "m")).unwrap();
        let dirs = [user.path().to_path_buf()];
        assert_eq!(SkillCatalog::discover_with(d.path(), &dirs).valid.len(), 1);
        assert!(read_body_in(d.path(), &dirs, "mine").is_ok());
    }

    /// The real-format fixture: `rust-unsafe` from low-level-dev-skills,
    /// copied unchanged. See `tests/fixtures/skills/README.md`.
    fn fixture_root() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills/rust-unsafe");
        let dst = d.path().join(SKILLS_DIR).join("rust-unsafe");
        std::fs::create_dir_all(dst.join("references")).unwrap();
        std::fs::copy(src.join("SKILL.md"), dst.join("SKILL.md")).unwrap();
        std::fs::copy(
            src.join("references/unsafe-patterns.md"),
            dst.join("references/unsafe-patterns.md"),
        )
        .unwrap();
        d
    }

    #[test]
    fn a_skill_in_the_agent_skills_format_loads_unchanged() {
        // ADR 0019 promises that skills written for other harnesses load as
        // they are. This one is copied byte for byte from the library the ADR
        // names, extra keys and all.
        let d = fixture_root();
        let c = SkillCatalog::discover(d.path());
        assert!(c.invalid.is_empty(), "{:?}", c.invalid);
        assert_eq!(c.valid.len(), 1);
        assert_eq!(c.valid[0].name, "rust-unsafe");
        assert!(
            c.valid[0]
                .description
                .starts_with("Rust unsafe code skill for systems programming."),
            "{}",
            c.valid[0].description
        );
        let body = read_body(d.path(), "rust-unsafe").unwrap();
        assert!(body.starts_with("# Rust unsafe"), "{}", &body[..40]);
        assert!(
            !body.contains("description:"),
            "the frontmatter is not the body"
        );
    }

    /// Apex's own writing skill: `skills/apex-notes-writing/SKILL.md` from
    /// boriemannetje/apex-notes at commit `f449644a` (blob `976aa86f`),
    /// copied byte for byte with its LF endings. apex-notes is MIT-licensed;
    /// its licence is `tests/fixtures/skills/LICENSE-apex-notes`. See
    /// `tests/fixtures/skills/README.md`.
    const APEX_DESCRIPTION: &str = "use skill when writing .md for Apex Notes; create/edit/import/delete/connect notes; enforce title/parent frontmatter, parent-only hierarchy, body wikilink refs, manifest sync.";

    fn apex_skill_md() -> Vec<u8> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/skills/apex-notes-writing/SKILL.md");
        let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        // A checkout that turned LF into CRLF would hand the loader bytes
        // Apex never published; the upstream blob is 3,581 bytes, all LF.
        assert!(!bytes.contains(&b'\r'), "the fixture lost its LF endings");
        assert_eq!(bytes.len(), 3_581, "the fixture is not the upstream blob");
        bytes
    }

    #[test]
    fn apex_s_writing_skill_loads_unchanged_from_the_repository() {
        // ADR 0021 row 35–37 is built in effect only if Apex's SKILL.md
        // (apex-notes `f449644a`, skills/apex-notes-writing/SKILL.md, MIT)
        // loads as it is when a repository copies it into `.trellis/skills/`.
        // trellis carries no skill in its binary (ADR 0022 §6).
        let d = tempfile::tempdir().unwrap();
        let dst = d.path().join(SKILLS_DIR).join("apex-notes-writing");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("SKILL.md"), apex_skill_md()).unwrap();
        let c = SkillCatalog::discover(d.path());
        assert!(c.invalid.is_empty(), "{:?}", c.invalid);
        assert_eq!(
            c.valid,
            [Skill {
                name: "apex-notes-writing".into(),
                description: APEX_DESCRIPTION.into(),
            }]
        );
        let body = read_body(d.path(), "apex-notes-writing").unwrap();
        assert!(body.starts_with("# Apex Notes Writing"), "{}", &body[..40]);
    }

    #[test]
    fn apex_s_writing_skill_loads_unchanged_from_an_installed_package() {
        // The same file (apex-notes `f449644a`,
        // skills/apex-notes-writing/SKILL.md, MIT) where `trellis install`
        // puts it: an installed package's `skills/` directory, outside the
        // repository, which ADR 0039 §10 passes to `discover_with`.
        let repo = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let pkg_skills = tmp.path().join("packages/apex-notes/skills");
        user_skill(
            &pkg_skills,
            "apex-notes-writing",
            std::str::from_utf8(&apex_skill_md()).unwrap(),
        );
        let dirs = [pkg_skills];
        let c = SkillCatalog::discover_with(repo.path(), &dirs);
        assert!(c.invalid.is_empty(), "{:?}", c.invalid);
        assert_eq!(
            c.valid,
            [Skill {
                name: "apex-notes-writing".into(),
                description: APEX_DESCRIPTION.into(),
            }]
        );
        assert_eq!(c.user, ["apex-notes-writing"]);
        assert!(
            read_body_in(repo.path(), &dirs, "apex-notes-writing")
                .unwrap()
                .starts_with("# Apex Notes Writing")
        );
    }

    #[test]
    fn a_name_that_does_not_match_its_directory_is_skipped_with_the_reason() {
        let d = tempfile::tempdir().unwrap();
        skill(d.path(), "profiling", &md("perf", "How to profile."));
        skill(d.path(), "ok", &md("ok", "Fine."));
        let c = SkillCatalog::discover(d.path());
        assert_eq!(c.valid.len(), 1);
        assert_eq!(c.invalid.len(), 1);
        assert_eq!(c.invalid[0].dir, "profiling");
        assert!(
            c.invalid[0]
                .reason
                .contains("`perf` does not match the directory name `profiling`"),
            "{}",
            c.invalid[0].reason
        );
        // And it cannot be read by either name.
        assert!(read_body(d.path(), "profiling").is_err());
        assert!(read_body(d.path(), "perf").is_err());
    }

    #[test]
    fn each_kind_of_invalid_skill_says_what_is_wrong_with_it() {
        let d = tempfile::tempdir().unwrap();
        let long = "x".repeat(1_025);
        skill(d.path(), "no-front", "# just a body\n");
        skill(d.path(), "no-desc", "---\nname: no-desc\n---\nbody\n");
        skill(d.path(), "too-long", &md("too-long", &long));
        skill(d.path(), "Upper", &md("Upper", "x"));
        skill(
            d.path(),
            "unclosed",
            "---\nname: unclosed\ndescription: x\n",
        );
        std::fs::create_dir_all(d.path().join(SKILLS_DIR).join("empty-dir")).unwrap();
        let c = SkillCatalog::discover(d.path());
        assert!(c.valid.is_empty(), "{:?}", c.valid);
        let reason = |dir: &str| {
            c.invalid
                .iter()
                .find(|i| i.dir == dir)
                .map(|i| i.reason.clone())
                .unwrap_or_else(|| panic!("{dir} was not reported: {:?}", c.invalid))
        };
        assert!(reason("no-front").contains("does not open with `---`"));
        assert!(reason("no-desc").contains("no `description`"));
        assert!(reason("too-long").contains("1025 characters; the limit is 1024"));
        assert!(reason("Upper").contains("lowercase"));
        assert!(reason("unclosed").contains("never closed"));
        assert!(reason("empty-dir").contains("no SKILL.md"));
    }

    #[test]
    fn quoted_and_block_descriptions_read_as_one_line() {
        let d = tempfile::tempdir().unwrap();
        skill(
            d.path(),
            "quoted",
            "---\nname: quoted\ndescription: \"Guide no_std work. Use when: writing #![no_std].\"\nuser-invocable: true\n---\nb\n",
        );
        skill(
            d.path(),
            "folded",
            "---\nname: folded\ndescription: >\n  First line\n  second line\nlicense: MIT\n---\nb\n",
        );
        let c = SkillCatalog::discover(d.path());
        assert!(c.invalid.is_empty(), "{:?}", c.invalid);
        let by = |n: &str| {
            c.valid
                .iter()
                .find(|s| s.name == n)
                .unwrap()
                .description
                .clone()
        };
        assert_eq!(
            by("quoted"),
            "Guide no_std work. Use when: writing #![no_std]."
        );
        assert_eq!(by("folded"), "First line second line");
    }

    #[test]
    fn past_the_cap_skills_are_counted_not_dropped() {
        // 142 descriptions is what the library the ADR names would put into
        // every prompt. The cap bounds the bytes; the count says what the
        // bound cost.
        let d = tempfile::tempdir().unwrap();
        let desc = "d".repeat(200);
        for i in 0..40 {
            let n = format!("skill-{i:02}");
            skill(d.path(), &n, &md(&n, &desc));
        }
        let c = SkillCatalog::discover(d.path());
        let shown = c.disclosed();
        let bytes: usize = shown.listed.iter().map(|s| s.listing().len()).sum();
        assert!(bytes <= DISCLOSURE_CAP, "{bytes}");
        assert!(shown.omitted > 0);
        assert_eq!(shown.listed.len() + shown.omitted, 40, "none silently lost");
        assert_eq!(
            shown.listed[0].name, "skill-00",
            "name order decides who is shown"
        );
    }

    #[test]
    fn a_name_cannot_climb_out_of_the_skills_directory() {
        let d = fixture_root();
        std::fs::write(d.path().join("secret.md"), "---\nname: x\n---\n").unwrap();
        for bad in ["../../secret", "..", "rust-unsafe/../x", "a/b", "A", ""] {
            let e = read_body(d.path(), bad).unwrap_err().to_string();
            assert!(e.contains("is not a skill name"), "{bad}: {e}");
        }
    }

    fn user_skill(base: &Path, name: &str, text: &str) {
        let d = base.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), text).unwrap();
    }

    #[test]
    fn a_user_skill_loads_beside_the_repositorys_and_reads_back_from_its_own_directory() {
        let d = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        skill(d.path(), "zeta", &md("zeta", "Repo."));
        user_skill(user.path(), "alpha", &md("alpha", "User."));
        let dirs = [user.path().to_path_buf()];
        let c = SkillCatalog::discover_with(d.path(), &dirs);
        let names: Vec<&str> = c.valid.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"], "sorted by name across sources");
        assert_eq!(c.user, ["alpha"]);
        assert!(
            read_body_in(d.path(), &dirs, "alpha")
                .unwrap()
                .contains("BODY-alpha")
        );
        // Without the user directory the repository alone is what it was.
        assert!(read_body(d.path(), "alpha").is_err());
    }

    #[test]
    fn a_repository_skill_shadows_a_user_skill_of_the_same_name_in_listing_and_reading() {
        // The risk: the listing shows the repository's description while
        // `read_skill` serves the user's body, or the reverse. Both must
        // resolve the same name to the same directory.
        let d = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        skill(d.path(), "refactor", &md("refactor", "Repo."));
        user_skill(user.path(), "refactor", &md("refactor", "User."));
        let dirs = [user.path().to_path_buf()];
        let c = SkillCatalog::discover_with(d.path(), &dirs);
        assert_eq!(c.valid.len(), 1);
        assert_eq!(c.valid[0].description, "Repo.");
        assert_eq!(c.shadowed, ["refactor"]);
        assert!(c.user.is_empty());
        let body = read_body_in(d.path(), &dirs, "refactor").unwrap();
        assert!(body.contains("BODY-refactor"));
    }

    #[test]
    fn a_dangling_user_skill_link_shadows_a_later_user_directory_in_reading_too() {
        // The risk: the listing counted a dangling link in the first user
        // directory as found (invalid) and the second's skill as shadowed,
        // while `read_skill` passed over the link and served the shadowed
        // body, one the listing never described.
        let d = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        if !link(&first.path().join("gone"), &first.path().join("foo")) {
            return;
        }
        user_skill(second.path(), "foo", &md("foo", "Second."));
        let dirs = [first.path().to_path_buf(), second.path().to_path_buf()];
        let c = SkillCatalog::discover_with(d.path(), &dirs);
        assert!(c.valid.is_empty(), "{:?}", c.valid);
        assert_eq!(c.invalid.len(), 1);
        assert_eq!(c.shadowed, ["foo"]);
        let e = read_body_in(d.path(), &dirs, "foo").unwrap_err();
        assert!(matches!(e, SkillError::Unreadable { .. }), "{e}");
    }

    #[test]
    fn a_profile_deny_list_refuses_read_skill_by_exact_name_and_says_whose_list() {
        // The gap ADR 0026 §4 recorded: the listing was filtered but an exact
        // name still read the body. The refusal has to be at the read.
        let d = tempfile::tempdir().unwrap();
        skill(d.path(), "refactor", &md("refactor", "R."));
        skill(d.path(), "triage", &md("triage", "T."));
        let access = SkillAccess {
            user_dirs: Vec::new(),
            policy: Some(SkillPolicy {
                profile: "reviewer".into(),
                allow: None,
                deny: vec!["refactor".into()],
            }),
        };
        let e = access.read(d.path(), "refactor").unwrap_err().to_string();
        assert!(
            e.contains("agent profile `reviewer`") && e.contains(r#"deny = ["refactor"]"#),
            "{e}"
        );
        assert!(access.read(d.path(), "triage").is_ok());

        let allow = SkillAccess {
            policy: Some(SkillPolicy {
                profile: "reviewer".into(),
                allow: Some(vec!["triage".into()]),
                deny: Vec::new(),
            }),
            ..SkillAccess::default()
        };
        let e = allow.read(d.path(), "refactor").unwrap_err().to_string();
        assert!(e.contains(r#"allow = ["triage"]"#), "{e}");
    }

    #[test]
    fn no_skills_directory_is_an_empty_catalog() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(SkillCatalog::discover(d.path()), SkillCatalog::default());
        assert!(
            read_body(d.path(), "anything")
                .unwrap_err()
                .to_string()
                .contains("no skill named")
        );
    }
}
