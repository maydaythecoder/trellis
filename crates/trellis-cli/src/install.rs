//! `trellis install`: skills and prompt templates from a package source,
//! copied into the user config directory (ADR 0039, amending ADR 0021 row 10).
//!
//! A package source is text from outside any repository the user chose, so
//! everything read out of it is untrusted: its paths, its file modes, its
//! sizes. Nothing is checked out. The source is cloned bare, the ref is
//! resolved to a commit, and each accepted file is read by blob id with
//! `git cat-file`, so a path from the package never reaches a git argument
//! and a link in the package is never followed on disk.
//!
//! Only two shapes are copied: `skills/<name>/...` text files in a directory
//! holding `SKILL.md`, and `prompts/<name>.md`. Everything else, hooks and
//! tools included, is skipped and named. A package never carries code
//! (ADR 0039).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The manifest's file name, in `<user config dir>/packages/`.
pub(crate) const MANIFEST: &str = "packages.toml";

/// A single file larger than this refuses the install. Skills and templates
/// are prose; a megabyte of it is already more than a context window holds.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Accepted files per package, and their total size. The package decides
/// how many entries its tree has, so both are bounded before anything is
/// read.
const MAX_FILES: usize = 2_000;
const MAX_TOTAL_BYTES: u64 = 32 << 20;
/// Skipped paths named in the output; the rest are counted.
const SHOW_SKIPPED: usize = 20;

/// One installed package, as `packages.toml` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub name: String,
    /// The source as given, made absolute when it is a local path.
    pub source: String,
    /// The ref as given.
    #[serde(rename = "ref")]
    pub reference: String,
    /// What the ref resolved to when the package was installed.
    pub commit: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default, rename = "package")]
    packages: Vec<Entry>,
}

/// `trellis install`, dispatched from `main`.
pub(crate) fn run(source: Option<String>, list: bool, remove: Option<String>) -> Result<()> {
    let config = crate::user_config::dir().context(
        "no user config directory to install into: set TRELLIS_CONFIG_DIR, APPDATA, \
         XDG_CONFIG_HOME or HOME (ADR 0025 §7)",
    )?;
    let mut out = std::io::stdout().lock();
    match (source, list, remove) {
        (_, true, _) => list_packages(&config, &mut out),
        (_, _, Some(name)) => remove_package(&config, &name, &mut out),
        (Some(spec), false, None) => install(&config, &spec, &mut out).map(|_| ()),
        (None, false, None) => bail!("name a source as <git-url|path>@<ref>, or pass --list"),
    }
}

fn packages_dir(config: &Path) -> PathBuf {
    config.join("packages")
}

/// Installs `spec` (`<git-url|path>@<ref>`) under `<config>/packages/<name>/`.
pub(crate) fn install(config: &Path, spec: &str, out: &mut dyn Write) -> Result<Entry> {
    let (source, reference) = split_spec(spec)?;
    if let Some(why) = unacceptable_source(source) {
        bail!("{why}");
    }
    let name = package_name(source)?;
    // A local directory is recorded absolute: a relative path in the
    // manifest would mean something different from every other directory.
    let source = if Path::new(source).is_dir() {
        std::path::absolute(source)
            .with_context(|| format!("cannot make {source} absolute"))?
            .to_string_lossy()
            .into_owned()
    } else {
        source.to_string()
    };

    let dir = packages_dir(config);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut manifest = load(&dir)?;
    if let Some(e) = manifest
        .packages
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(&name))
    {
        bail!(
            "{} is already installed at commit {}; `trellis install --remove {}` first",
            plain(&e.name),
            plain(&e.commit),
            plain(&e.name)
        );
    }
    let target = dir.join(&name);
    if std::fs::symlink_metadata(&target).is_ok() {
        bail!(
            "{} exists but is not in {MANIFEST}; move it aside before installing {name}",
            target.display()
        );
    }

    let staging = Staging::new(&dir)?;
    let bare = staging.path.join("clone");
    clone_bare(&source, &bare)?;
    let commit = resolve(&bare, reference)?;
    let listing = ls_tree(&bare, &commit)?;
    let plan = select(&listing);
    if plan.copy.is_empty() {
        bail!(
            "{} at {commit} has no skills/<name>/SKILL.md and no prompts/<name>.md; \
             nothing to install",
            plain(&source)
        );
    }
    check_bounds(&plan.copy)?;

    let tree = staging.path.join("package");
    for f in &plan.copy {
        let bytes = blob(&bare, &f.blob)?;
        if bytes.len() as u64 != f.size {
            bail!("{} changed size while it was read", f.path);
        }
        let dest = tree.join(&f.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        // `create_new`: two package paths that one filesystem folds to the
        // same name (`SKILL.md` and `skill.md` on Windows) refuse the
        // install rather than one silently replacing the other.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .with_context(|| {
                format!(
                    "cannot write {} (two package paths that differ only in case?)",
                    f.path
                )
            })?;
        file.write_all(&bytes)
            .with_context(|| format!("cannot write {}", dest.display()))?;
    }
    std::fs::rename(&tree, &target)
        .with_context(|| format!("cannot move the package into {}", target.display()))?;

    let entry = Entry {
        name: name.clone(),
        source,
        reference: reference.to_string(),
        commit,
    };
    manifest.packages.push(entry.clone());
    if let Err(e) = save(&dir, &manifest) {
        // A directory the manifest does not name could never be listed or
        // removed by name, so it goes too.
        let _ = remove_tree(&target);
        return Err(e);
    }

    let skills = plan
        .copy
        .iter()
        .filter(|f| f.path.ends_with("/SKILL.md") && f.path.starts_with("skills/"))
        .count();
    let prompts = plan
        .copy
        .iter()
        .filter(|f| f.path.starts_with("prompts/"))
        .count();
    writeln!(
        out,
        "installed {name} from {} @ {} (commit {}): {skills} skill(s), {prompts} prompt \
         template(s), {} file(s) in {}",
        plain(&entry.source),
        plain(&entry.reference),
        entry.commit,
        plan.copy.len(),
        target.display()
    )?;
    if !plan.skipped.is_empty() {
        writeln!(
            out,
            "skipped {} path(s): a package carries skills and prompt templates only, \
             never hooks, tools or other code (ADR 0039)",
            plan.skipped.len()
        )?;
        for (path, why) in plan.skipped.iter().take(SHOW_SKIPPED) {
            writeln!(out, "  {}: {why}", plain(path))?;
        }
        if plan.skipped.len() > SHOW_SKIPPED {
            writeln!(out, "  ... and {} more", plan.skipped.len() - SHOW_SKIPPED)?;
        }
    }
    writeln!(
        out,
        "note: runs do not read packages/ yet. Skills load from <user config dir>/skills \
         and templates from .trellis/prompts (ADR 0039)."
    )?;
    Ok(entry)
}

/// `--list`.
pub(crate) fn list_packages(config: &Path, out: &mut dyn Write) -> Result<()> {
    let dir = packages_dir(config);
    let manifest = load(&dir)?;
    if manifest.packages.is_empty() {
        writeln!(out, "no packages installed in {}", dir.display())?;
        return Ok(());
    }
    for e in &manifest.packages {
        writeln!(
            out,
            "{}  source {}  ref {}  commit {}",
            plain(&e.name),
            plain(&e.source),
            plain(&e.reference),
            plain(&e.commit)
        )?;
    }
    Ok(())
}

/// `--remove <name>`: the package's directory and its manifest entry.
pub(crate) fn remove_package(config: &Path, name: &str, out: &mut dyn Write) -> Result<()> {
    // Before any path is built from it: `--remove ..` must never reach a
    // `join`.
    if let Some(why) = bad_name(name) {
        bail!("{why}");
    }
    let dir = packages_dir(config);
    let mut manifest = load(&dir)?;
    let Some(i) = manifest.packages.iter().position(|e| e.name == name) else {
        let known: Vec<String> = manifest.packages.iter().map(|e| plain(&e.name)).collect();
        bail!(
            "no package named {name} is installed{}",
            if known.is_empty() {
                String::new()
            } else {
                format!(" (installed: {})", known.join(", "))
            }
        );
    };
    let target = dir.join(name);
    if std::fs::symlink_metadata(&target).is_ok() {
        remove_tree(&target).with_context(|| format!("cannot remove {}", target.display()))?;
    }
    manifest.packages.remove(i);
    save(&dir, &manifest)?;
    writeln!(out, "removed {name} ({})", target.display())?;
    Ok(())
}

/// `<source>@<ref>` split at the last `@`, or refused when no ref is given.
///
/// The last `@`, because a URL's user part (`git@host:x.git`) holds one too.
/// A ref cannot hold `:` (git forbids it in ref names), so a part after the
/// last `@` that has one is the rest of an address, not a ref: that is how
/// `git@host:x.git` with no ref is told apart. For the same reason a
/// `scheme://` source must keep a path after its host.
pub(crate) fn split_spec(spec: &str) -> Result<(&str, &str)> {
    let missing = || {
        anyhow::anyhow!(
            "`{}` names no ref: give one explicitly, as <git-url|path>@<ref> \
             (a tag, branch or commit), so what is installed is what was asked for",
            plain(spec)
        )
    };
    let (source, reference) = spec.rsplit_once('@').ok_or_else(missing)?;
    if reference.is_empty() || reference.contains([':', '\\']) || source.is_empty() {
        return Err(missing());
    }
    if let Some((_, rest)) = source.split_once("://")
        && !rest.contains('/')
    {
        return Err(missing());
    }
    if reference.starts_with('-') || reference.chars().any(|c| c.is_control() || c == ' ') {
        bail!("ref `{}` is not a ref name", plain(reference));
    }
    Ok((source, reference))
}

/// Why `source` is refused before any git runs, or `None`.
fn unacceptable_source(source: &str) -> Option<String> {
    if source.starts_with('-') {
        // `git clone --upload-pack=...` runs what it names.
        return Some(format!("source `{}` begins with `-`", plain(source)));
    }
    if source.chars().any(char::is_control) {
        return Some("a source may not hold control characters".to_string());
    }
    // A password in the URL would be written into packages.toml.
    if let Some((_, rest)) = source.split_once("://") {
        let authority = rest.split('/').next().unwrap_or("");
        if let Some((userinfo, _)) = authority.rsplit_once('@')
            && userinfo.contains(':')
        {
            return Some(
                "the source URL holds a password, which would be recorded in packages.toml; \
                 use a git credential helper instead"
                    .to_string(),
            );
        }
    }
    None
}

/// The package's name: the source's last path component, less `.git`.
fn package_name(source: &str) -> Result<String> {
    let trimmed = source.trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    if let Some(why) = bad_name(name) {
        bail!("cannot name a package after `{}`: {why}", plain(source));
    }
    Ok(name.to_string())
}

/// Windows device names, which a directory may not take on that system
/// whatever its extension.
const DEVICES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Why `seg` cannot be one path component written under the install
/// directory, or `None`. An allowlist: `[A-Za-z0-9._-]`, no leading `.`
/// (so no `..`, no dotfiles), no trailing `.` (Windows strips it), and no
/// device name.
fn bad_component(seg: &str) -> Option<String> {
    if seg.is_empty() || seg.len() > 128 {
        return Some(format!("`{}` is empty or too long", plain(seg)));
    }
    if !seg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Some(format!(
            "`{}` holds a character outside A-Z, a-z, 0-9, `.`, `_` and `-`",
            plain(seg)
        ));
    }
    if seg.starts_with('.') || seg.ends_with('.') {
        return Some(format!("`{seg}` begins or ends with `.`"));
    }
    let stem = seg.split('.').next().unwrap_or(seg).to_ascii_lowercase();
    if DEVICES.contains(&stem.as_str()) {
        return Some(format!("`{seg}` is a Windows device name"));
    }
    None
}

/// Why `name` cannot be a package name, or `None`.
fn bad_name(name: &str) -> Option<String> {
    if let Some(why) = bad_component(name) {
        return Some(format!("package name {why}"));
    }
    if name.eq_ignore_ascii_case(MANIFEST) {
        return Some(format!("package name `{name}` is the manifest's"));
    }
    None
}

/// A skill directory's name: ADR 0019's alphabet, which `read_skill` also
/// holds a name to.
fn skill_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// One `ls-tree -r -l` row.
#[derive(Debug, Clone)]
struct TreeEntry {
    mode: String,
    blob: String,
    size: Option<u64>,
    path: String,
}

/// A file to copy.
#[derive(Debug)]
struct Accepted {
    path: String,
    blob: String,
    size: u64,
}

#[derive(Debug, Default)]
struct Plan {
    copy: Vec<Accepted>,
    /// Path and why it was left out.
    skipped: Vec<(String, String)>,
}

/// What of `tree` is copied. Pure: every rule about what a package may put
/// on disk is here, and none of it depends on git.
fn select(tree: &[TreeEntry]) -> Plan {
    let with_skill_md: std::collections::BTreeSet<&str> = tree
        .iter()
        .filter(|e| e.mode == "100644")
        .filter_map(|e| {
            let rest = e.path.strip_prefix("skills/")?;
            rest.strip_suffix("/SKILL.md").filter(|n| !n.contains('/'))
        })
        .collect();
    let mut plan = Plan::default();
    for e in tree {
        match why_skipped(e, &with_skill_md) {
            Some(why) => plan.skipped.push((e.path.clone(), why)),
            None => plan.copy.push(Accepted {
                path: e.path.clone(),
                blob: e.blob.clone(),
                size: e.size.unwrap_or(0),
            }),
        }
    }
    plan
}

fn why_skipped(e: &TreeEntry, with_skill_md: &std::collections::BTreeSet<&str>) -> Option<String> {
    let parts: Vec<&str> = e.path.split('/').collect();
    let shape = match parts.as_slice() {
        ["skills", name, _, ..] => {
            if !skill_name_ok(name) {
                return Some(format!(
                    "skill directory `{}` is outside ADR 0019's name alphabet",
                    plain(name)
                ));
            }
            if !with_skill_md.contains(name) {
                return Some(format!("skills/{name}/ has no SKILL.md"));
            }
            let file = parts.last().copied().unwrap_or("");
            if !(file.ends_with(".md") || file.ends_with(".txt")) {
                return Some("not a text file (.md or .txt) of a skill".to_string());
            }
            "skill"
        }
        ["prompts", file] if file.ends_with(".md") => "prompt",
        _ => return Some("outside skills/<name>/ and prompts/<name>.md".to_string()),
    };
    match e.mode.as_str() {
        "100644" => {}
        "100755" => return Some("executable".to_string()),
        "120000" => return Some("symbolic link".to_string()),
        "160000" => return Some("submodule".to_string()),
        m => return Some(format!("mode {m} is not a plain file")),
    }
    if let Some(why) = trellis_core::paths::unsafe_repo_path(&e.path) {
        return Some(why);
    }
    if let Some(why) = parts.iter().find_map(|p| bad_component(p)) {
        return Some(why);
    }
    if e.size.is_none() {
        return Some(format!("{shape} file with no size"));
    }
    None
}

/// Refuses a package whose accepted files exceed the bounds, before any is
/// read.
fn check_bounds(copy: &[Accepted]) -> Result<()> {
    if copy.len() > MAX_FILES {
        bail!(
            "the package has {} files to install; the limit is {MAX_FILES}",
            copy.len()
        );
    }
    if let Some(f) = copy.iter().find(|f| f.size > MAX_FILE_BYTES) {
        bail!(
            "{} is {} bytes; the limit per file is {MAX_FILE_BYTES}",
            f.path,
            f.size
        );
    }
    let total: u64 = copy.iter().map(|f| f.size).sum();
    if total > MAX_TOTAL_BYTES {
        bail!("the package's files total {total} bytes; the limit is {MAX_TOTAL_BYTES}");
    }
    Ok(())
}

/// `git` from `trellis_vcs`, so a planted `core.fsmonitor` or hooks path
/// never runs and the environment is scrubbed of keys, with the settings a
/// fetch from an untrusted source needs:
/// - `protocol.ext.allow=never`: the `ext::` transport runs a command named
///   in the URL;
/// - `transfer.fsckObjects=true`: git's own checks on what it receives,
///   which reject `..` and `.git` tree entries among others;
/// - `GIT_TERMINAL_PROMPT=0`: a source that wants credentials fails rather
///   than waiting on a prompt nobody may be watching.
fn git() -> std::process::Command {
    let mut c = trellis_vcs::command();
    c.args([
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "transfer.fsckObjects=true",
    ]);
    c.env("GIT_TERMINAL_PROMPT", "0");
    c
}

fn git_out(c: &mut std::process::Command, what: &str) -> Result<Vec<u8>> {
    let out = c
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("cannot run git to {what}"))?;
    if !out.status.success() {
        // `rev-parse --quiet` says nothing on an unknown ref.
        let err = String::from_utf8_lossy(&out.stderr);
        let err = match err.trim() {
            "" => "no such ref or object".to_string(),
            e => plain(e),
        };
        bail!("could not {what}: {err}");
    }
    Ok(out.stdout)
}

fn clone_bare(source: &str, into: &Path) -> Result<()> {
    // `--no-local`: a local source goes through the transport, so it is
    // fsck'd like a remote one and no file of it is hard-linked in.
    git_out(
        git()
            .args(["clone", "--bare", "--no-local", "--quiet", "--"])
            .arg(source)
            .arg(into),
        &format!("clone {}", plain(source)),
    )?;
    Ok(())
}

fn resolve(bare: &Path, reference: &str) -> Result<String> {
    let out = git_out(
        git()
            .arg("-C")
            .arg(bare)
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
            .arg(format!("{reference}^{{commit}}")),
        &format!("resolve `{}` to a commit", plain(reference)),
    )?;
    let commit = String::from_utf8_lossy(&out).trim().to_string();
    if !matches!(commit.len(), 40 | 64) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("`{}` did not resolve to a commit id", plain(reference));
    }
    Ok(commit)
}

fn ls_tree(bare: &Path, commit: &str) -> Result<Vec<TreeEntry>> {
    let out = git_out(
        git()
            .arg("-C")
            .arg(bare)
            .args(["ls-tree", "-r", "-l", "-z", "--end-of-options", commit]),
        "list the package's files",
    )?;
    parse_ls_tree(&out)
}

/// `<mode> SP <type> SP <id> SP+ <size> TAB <path> NUL`, repeated. A path
/// that is not UTF-8 is kept lossily, which cannot pass the component
/// allowlist, so it is skipped and named.
fn parse_ls_tree(out: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut v = Vec::new();
    for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = rec
            .iter()
            .position(|b| *b == b'\t')
            .context("unexpected `git ls-tree` output")?;
        let head = String::from_utf8_lossy(&rec[..tab]);
        let path = String::from_utf8_lossy(&rec[tab + 1..]).into_owned();
        let mut f = head.split_whitespace();
        let (Some(mode), Some(_kind), Some(blob), Some(size)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            bail!("unexpected `git ls-tree` output");
        };
        v.push(TreeEntry {
            mode: mode.to_string(),
            blob: blob.to_string(),
            size: size.parse().ok(),
            path,
        });
    }
    Ok(v)
}

fn blob(bare: &Path, id: &str) -> Result<Vec<u8>> {
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("unexpected blob id from `git ls-tree`");
    }
    git_out(
        git().arg("-C").arg(bare).args(["cat-file", "blob", id]),
        "read a package file",
    )
}

fn load(dir: &Path) -> Result<Manifest> {
    let path = dir.join(MANIFEST);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Manifest::default()),
        // A manifest that cannot be read is not an empty one (ADR 0025 §7's
        // rule for config files): treating it so would let the next save
        // drop every entry.
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
}

/// Written beside and renamed over, so an interrupted save leaves the old
/// manifest rather than half of a new one.
fn save(dir: &Path, m: &Manifest) -> Result<()> {
    let path = dir.join(MANIFEST);
    let tmp = dir.join(format!(".{MANIFEST}.{}", std::process::id()));
    let text = toml::to_string(m).context("cannot serialise the package manifest")?;
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// Removes a directory tree, or only the link when `path` is one.
///
/// Links are not followed: `remove_dir_all` removes a link inside the tree,
/// not what it names. git writes its object files read-only; std's Windows
/// implementation deletes those regardless, which
/// `install_copies_only_skills_and_prompts_and_names_what_it_skipped` holds
/// by checking the staging clone is gone.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        // A directory link on Windows (a junction or symlink to a directory)
        // is removed with `remove_dir`; `remove_file` handles the rest.
        return std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path));
    }
    std::fs::remove_dir_all(path)
}

/// A scratch directory under `packages/`, on the target's filesystem so the
/// final move is a rename, removed when dropped whatever the outcome.
struct Staging {
    path: PathBuf,
}

impl Staging {
    fn new(dir: &Path) -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!(".staging-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&path).with_context(|| format!("cannot create {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(e) = remove_tree(&self.path) {
            tracing::warn!(path = %self.path.display(), error = %e, "staging directory left behind");
        }
    }
}

fn plain(s: &str) -> String {
    crate::plain(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = trellis_vcs::command()
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A package source as a local git repository, tagged `v1`: two skills,
    /// a reference file, a template, and every kind of thing that must not
    /// be copied.
    fn source(parent: &Path) -> PathBuf {
        let src = parent.join("pkg-src");
        std::fs::create_dir_all(&src).unwrap();
        git_in(&src, &["init", "-q"]);
        write(&src, "skills/alpha/SKILL.md", "---\nname: alpha\n---\nA\n");
        write(&src, "skills/alpha/references/notes.md", "notes\n");
        write(&src, "skills/beta/SKILL.md", "---\nname: beta\n---\nB\n");
        write(&src, "prompts/review.md", "Review $1\n");
        // Decoys: code, hooks, a repository config, a skill with no
        // SKILL.md, a script inside a skill, a nested template.
        write(&src, "hooks/after_run.sh", "#!/bin/sh\nrm -rf ~\n");
        write(&src, "tools/tool.py", "import os\n");
        write(&src, ".trellis/config.toml", "[hooks]\n");
        write(&src, "skills/gamma/README.md", "no SKILL.md here\n");
        write(&src, "skills/alpha/run.py", "print(1)\n");
        write(&src, "skills/alpha/tool.md", "marked executable\n");
        write(&src, "prompts/nested/deep.md", "nested\n");
        write(&src, "README.md", "package\n");
        git_in(&src, &["add", "-A"]);
        git_in(
            &src,
            &["update-index", "--chmod=+x", "skills/alpha/tool.md"],
        );
        // A link, recorded in the index directly so the test needs no
        // symlink privilege on Windows.
        let target = git_in(&src, &["hash-object", "-w", "--stdin", "--path", "x"]);
        git_in(
            &src,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("120000,{target},skills/alpha/link.md"),
            ],
        );
        git_in(&src, &["commit", "-q", "-m", "package"]);
        git_in(&src, &["tag", "v1"]);
        src
    }

    fn files_under(root: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    let rel = p.strip_prefix(base).unwrap().to_string_lossy();
                    out.push(rel.replace('\\', "/"));
                }
            }
        }
        let mut v = Vec::new();
        walk(root, root, &mut v);
        v.sort();
        v
    }

    fn spec(src: &Path, r: &str) -> String {
        format!("{}@{r}", src.display())
    }

    #[test]
    fn install_without_a_ref_is_refused_before_anything_is_fetched() {
        // The risk: an implicit default branch installs whatever the source
        // holds today, so the recorded commit would not be one anybody chose.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        let e = install(&cfg, &src.display().to_string(), &mut out).unwrap_err();
        assert!(format!("{e:#}").contains("names no ref"), "{e:#}");
        assert!(
            !cfg.join("packages").exists(),
            "nothing is created for a refused install"
        );
        // An scp-style address has an `@` that is not a ref separator.
        assert!(split_spec("git@example.invalid:o/p.git").is_err());
        assert!(split_spec("https://u@example.invalid/o/p").is_err());
        assert_eq!(
            split_spec("git@example.invalid:o/p.git@v1").unwrap(),
            ("git@example.invalid:o/p.git", "v1")
        );
        assert_eq!(
            split_spec("https://u@example.invalid/o/p@release/1").unwrap(),
            ("https://u@example.invalid/o/p", "release/1")
        );
        assert!(split_spec("x@-bad").is_err(), "a ref may not be an option");
    }

    #[test]
    fn install_copies_only_skills_and_prompts_and_names_what_it_skipped() {
        // The risk: a package that carries hooks, tools or scripts puts code
        // on the user's machine under a command that promises only text
        // (ADR 0039; #120's "no package code").
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        let entry = install(&cfg, &spec(&src, "v1"), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();

        let pkg = cfg.join("packages").join("pkg-src");
        assert_eq!(
            files_under(&pkg),
            vec![
                "prompts/review.md",
                "skills/alpha/SKILL.md",
                "skills/alpha/references/notes.md",
                "skills/beta/SKILL.md",
            ],
            "{said}"
        );
        assert_eq!(
            std::fs::read_to_string(pkg.join("skills/alpha/SKILL.md")).unwrap(),
            "---\nname: alpha\n---\nA\n"
        );
        for skipped in [
            "hooks/after_run.sh",
            "tools/tool.py",
            ".trellis/config.toml",
            "skills/gamma/README.md",
            "skills/alpha/run.py",
            "skills/alpha/tool.md: executable",
            "skills/alpha/link.md: symbolic link",
            "prompts/nested/deep.md",
            "README.md",
        ] {
            assert!(said.contains(skipped), "{skipped} not named in:\n{said}");
        }
        assert!(said.contains("never hooks, tools or other code"), "{said}");
        assert!(said.contains("runs do not read packages/ yet"), "{said}");
        assert_eq!(entry.commit, git_in(&src, &["rev-parse", "v1^{commit}"]));
        // The staging clone is gone; only the package and its manifest stay.
        let mut left: Vec<String> = std::fs::read_dir(cfg.join("packages"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["packages.toml", "pkg-src"]);
    }

    #[test]
    fn install_list_prints_source_ref_and_resolved_commit() {
        // The risk: a ref such as a branch moves; without the commit beside
        // it, nobody can say what was installed.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        list_packages(&cfg, &mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("no packages installed"));

        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let commit = git_in(&src, &["rev-parse", "v1^{commit}"]);
        let mut out = Vec::new();
        list_packages(&cfg, &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        let abs = std::path::absolute(&src).unwrap();
        assert!(said.starts_with("pkg-src  source "), "{said}");
        assert!(said.contains(&*abs.to_string_lossy()), "{said}");
        assert!(said.contains("ref v1"), "{said}");
        assert!(said.contains(&format!("commit {commit}")), "{said}");

        let m = load(&cfg.join("packages")).unwrap();
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.packages[0].reference, "v1");
        assert_eq!(m.packages[0].commit, commit);
        // A second install of the same name is refused, not merged.
        let e = install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap_err();
        assert!(format!("{e:#}").contains("already installed"), "{e:#}");
    }

    #[test]
    fn install_remove_deletes_the_package_directory_and_its_entry() {
        // The risk: a removal that leaves either half behind, a directory
        // no entry names or an entry naming no directory.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let pkg = cfg.join("packages").join("pkg-src");
        assert!(pkg.is_dir());

        let mut out = Vec::new();
        remove_package(&cfg, "pkg-src", &mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).starts_with("removed pkg-src"));
        assert!(!pkg.exists());
        assert!(load(&cfg.join("packages")).unwrap().packages.is_empty());
        // Removed means installable again.
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
    }

    #[test]
    fn install_remove_of_an_unknown_name_is_refused_and_touches_nothing() {
        // The risk: `--remove` joins its argument onto a path, so a name that
        // is not a package, `..` above all, must be refused before any join.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        std::fs::write(cfg.join("config.toml"), "").unwrap();

        for name in ["nope", "..", "../cfg", "packages.toml", "PKG-SRC"] {
            let e = remove_package(&cfg, name, &mut Vec::new()).unwrap_err();
            let msg = format!("{e:#}");
            assert!(
                msg.contains("no package named") || msg.contains("package name"),
                "{name}: {msg}"
            );
        }
        let e = remove_package(&cfg, "nope", &mut Vec::new()).unwrap_err();
        assert!(format!("{e:#}").contains("installed: pkg-src"), "{e:#}");
        assert!(cfg.join("config.toml").exists());
        assert!(cfg.join("packages").join("pkg-src").is_dir());
        assert_eq!(load(&cfg.join("packages")).unwrap().packages.len(), 1);
    }

    #[test]
    fn install_refuses_a_path_that_would_leave_the_package_directory() {
        // The risk: git's own checks are the first line against `..` and
        // absolute tree entries; this is the second, and it must hold even
        // when the first is bypassed (a tree written by hand).
        let entry = |path: &str, mode: &str| TreeEntry {
            mode: mode.to_string(),
            blob: "0".repeat(40),
            size: Some(1),
            path: path.to_string(),
        };
        let tree = vec![
            entry("skills/a/SKILL.md", "100644"),
            entry("skills/a/../../x.md", "100644"),
            entry("skills/a/.hidden.md", "100644"),
            entry("skills/a/c:x.md", "100644"),
            entry("skills/a/CON.md", "100644"),
            entry("skills/a/sub\\..\\..\\y.md", "100644"),
            entry("prompts/..md", "100644"),
            entry("skills/A/SKILL.md", "100644"),
        ];
        let plan = select(&tree);
        let copied: Vec<&str> = plan.copy.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(copied, vec!["skills/a/SKILL.md"], "{:?}", plan.skipped);
    }
}
