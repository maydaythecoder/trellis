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

use crate::plain;

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
/// Entries in the source's tree, copied or not. The whole listing is held
/// in memory for `select` and every skipped path is printed, so the tree
/// is bounded while it is read, not after. Five times `MAX_FILES` leaves
/// room for the README, licence, CI and other paths a package carries and
/// never copies (ADR 0039, 8). Chosen, not measured.
const MAX_TREE_ENTRIES: usize = 10_000;
/// Bytes of `ls-tree` output. A count alone does not bound memory: one
/// entry can carry a path of any length. Derived, not measured: about 100
/// bytes of fixed fields per record plus an average path of about 700
/// bytes, over `MAX_TREE_ENTRIES` records (ADR 0039, 8).
const MAX_LISTING_BYTES: u64 = 8 << 20;

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
    /// The sha256 of each file as installed, by package path (#617).
    /// Informational, never a trust decision (ADR 0039 §9). Absent from a
    /// manifest written before it was recorded, which `--list` reports as
    /// "not recorded" rather than as unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,
}

/// `path -> sha256` for every file an install wrote. Per file, not one hash
/// over all of them, so a change can be named by path.
pub(crate) type Digest = std::collections::BTreeMap<String, String>;

/// How a package's files on disk compare with the digest its entry records.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DigestCheck {
    /// Every recorded file is there with its recorded bytes, and nothing
    /// else is.
    Ok,
    /// Paths whose bytes differ, that are missing, or that were added.
    Modified(Vec<String>),
    /// The entry predates #617.
    NotRecorded,
}

impl std::fmt::Display for DigestCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "digest ok"),
            Self::Modified(paths) => write!(f, "modified: {}", paths.join(", ")),
            Self::NotRecorded => write!(f, "not recorded"),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Compares the files under `dir` with `entry`'s recorded digest. A file
/// that cannot be read, or is not a plain file (a link put there since),
/// counts as modified: "digest ok" is a claim about bytes, so anything
/// short of reading them is not it.
pub(crate) fn check_digest(dir: &Path, entry: &Entry) -> DigestCheck {
    let Some(recorded) = &entry.digest else {
        return DigestCheck::NotRecorded;
    };
    let mut on_disk = std::collections::BTreeSet::new();
    walk_files(dir, dir, &mut on_disk);
    let mut modified = Vec::new();
    for (path, sha) in recorded {
        let file = dir.join(path);
        let is_plain = std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file());
        match std::fs::read(&file) {
            Ok(bytes) if is_plain && sha256_hex(&bytes) == *sha => {}
            Ok(_) => modified.push(plain(path)),
            Err(_) => modified.push(format!("{} (missing)", plain(path))),
        }
    }
    for path in on_disk {
        if !recorded.contains_key(&path) {
            modified.push(format!("{} (added)", plain(&path)));
        }
    }
    if modified.is_empty() {
        DigestCheck::Ok
    } else {
        DigestCheck::Modified(modified)
    }
}

/// Every entry under `dir` that is not a directory, as a `/`-separated path
/// relative to `base`. Links are not followed: one is listed as an entry
/// and read (or not) by `check_digest`. A directory that cannot be read
/// lists nothing, which the recorded paths then report as missing.
fn walk_files(base: &Path, dir: &Path, out: &mut std::collections::BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            walk_files(base, &p, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default, rename = "package")]
    packages: Vec<Entry>,
}

/// `trellis install`, dispatched from `main`.
pub(crate) fn run(
    source: Option<String>,
    list: bool,
    remove: Option<String>,
    upgrade: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let config = crate::user_config::dir().context(
        "no user config directory to install into: set TRELLIS_CONFIG_DIR, APPDATA, \
         XDG_CONFIG_HOME or HOME (ADR 0025 §7)",
    )?;
    let mut out = std::io::stdout().lock();
    match (source, list, remove, upgrade) {
        (_, true, _, _) => list_packages(&config, &mut out),
        (_, _, Some(name), _) => remove_package(&config, &name, &mut out),
        (_, _, _, Some(name)) => upgrade_package(&config, &name, &mut out),
        (Some(spec), false, None, None) if dry_run => install_dry_run(&config, &spec, &mut out),
        (Some(spec), false, None, None) => install(&config, &spec, &mut out).map(|_| ()),
        (None, false, None, None) => {
            bail!("name a source as <git-url|path>@<ref>, or pass --list")
        }
    }
}

pub(crate) fn packages_dir(config: &Path) -> PathBuf {
    config.join("packages")
}

/// `<config>/packages/<name>` for every package the manifest records, in
/// name order: that order decides which of two packages wins a skill or
/// template name they both carry, so it must not depend on the order they
/// were installed in or on `read_dir`.
///
/// The manifest, not a directory listing, is what counts as installed: a
/// staging directory mid-install, or a directory `--list` and `--remove`
/// do not know, is never read into a run. A manifest that cannot be read
/// or parsed is an error rather than no packages, because a run that
/// silently lost every installed skill would look like one that had them.
pub(crate) fn installed_dirs(config: &Path) -> Result<Vec<PathBuf>> {
    let dir = packages_dir(config);
    let mut names: Vec<String> = load(&dir)?.packages.into_iter().map(|e| e.name).collect();
    // The manifest is a file the user can edit, and a name becomes a path.
    for name in &names {
        if let Some(why) = bad_name(name) {
            bail!("{}: {why}", dir.join(MANIFEST).display());
        }
    }
    names.sort();
    Ok(names.into_iter().map(|n| dir.join(n)).collect())
}

/// Installs `spec` (`<git-url|path>@<ref>`) under `<config>/packages/<name>/`.
pub(crate) fn install(config: &Path, spec: &str, out: &mut dyn Write) -> Result<Entry> {
    let dir = packages_dir(config);
    let Prepared {
        name,
        source,
        reference,
        target,
        mut manifest,
    } = prepare(&dir, spec)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let staging = Staging::new(&dir)?;
    let bare = staging.path.join("clone");
    clone_bare(&source, &bare)?;
    let commit = resolve_ref(&bare, &reference)?;
    let plan = plan_for(&bare, &source, &commit)?;
    let tree = staging.path.join("package");
    let digest = stage(&bare, &plan, &tree)?;
    std::fs::rename(&tree, &target)
        .with_context(|| format!("cannot move the package into {}", target.display()))?;

    let entry = Entry {
        name: name.clone(),
        source,
        reference,
        commit,
        digest: Some(digest),
    };
    manifest.packages.push(entry.clone());
    if let Err(e) = save(&dir, &manifest) {
        // A directory the manifest does not name could never be listed or
        // removed by name, so it goes too.
        let _ = remove_tree(&target);
        return Err(e);
    }

    writeln!(
        out,
        "installed {name} from {} @ {} (commit {}): {} in {}",
        plain(&entry.source),
        plain(&entry.reference),
        entry.commit,
        summary(&plan),
        target.display()
    )?;
    report_skipped(&plan, out)?;
    writeln!(
        out,
        "note: runs read it after the repository's own skills and templates, which win a \
         name; a skill in <user config dir>/skills also wins over it (ADR 0039)."
    )?;
    Ok(entry)
}

/// `--dry-run` (#619): every check an install makes, the clone under 6's
/// hardening and the listing under 8's bounds, then what would be copied
/// and what skipped, and nothing written. The staging directory goes under
/// the system temporary directory, not `packages/`: there is no rename
/// into place to keep on one filesystem, and `packages/` and the manifest
/// are left exactly as found, down to a `packages/` that does not exist.
pub(crate) fn install_dry_run(config: &Path, spec: &str, out: &mut dyn Write) -> Result<()> {
    let Prepared {
        name,
        source,
        reference,
        target,
        ..
    } = prepare(&packages_dir(config), spec)?;
    let staging = Staging::new(&std::env::temp_dir())?;
    let bare = staging.path.join("clone");
    clone_bare(&source, &bare)?;
    let commit = resolve_ref(&bare, &reference)?;
    let plan = plan_for(&bare, &source, &commit)?;
    writeln!(
        out,
        "dry run: {name} from {} @ {} (commit {commit}) would put {} in {}",
        plain(&source),
        plain(&reference),
        summary(&plan),
        target.display()
    )?;
    for f in &plan.copy {
        writeln!(out, "  {}", plain(&f.path))?;
    }
    report_skipped(&plan, out)?;
    let (skills, prompts) = counts(&plan);
    writeln!(
        out,
        "would install {skills} skill(s), {prompts} prompt(s); nothing was written"
    )?;
    Ok(())
}

/// What `install` and `--dry-run` share before anything is fetched: the
/// spec checked and split, the name derived, and the manifest and target
/// found free. Creates nothing: a refused spec leaves no `packages/`.
struct Prepared {
    name: String,
    source: String,
    reference: String,
    target: PathBuf,
    manifest: Manifest,
}

fn prepare(dir: &Path, spec: &str) -> Result<Prepared> {
    // `<helper>::<address>` makes git run `git-remote-<helper>` from PATH,
    // so the source string would pick which installed program runs.
    // `protocol.allow=never` in `git()` refuses it too; refusing here says
    // why before any git runs. It is checked on the whole spec, before the
    // split, because `split_spec` would refuse `foo::https://host@v1` for
    // naming no ref, which is not the reason. A ref cannot hold `:`, so the
    // helper prefix of the spec is that of its source.
    if let Some(helper) = remote_helper(spec) {
        bail!(
            "`{}` names the git remote helper `{}`, which would run a program \
             from PATH; give a URL (https://, ssh://, git://, file://), an scp-style \
             user@host:path address, or a local path",
            plain(spec),
            plain(helper)
        );
    }
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

    let manifest = load(dir)?;
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
    Ok(Prepared {
        name,
        source,
        reference: reference.to_string(),
        target,
        manifest,
    })
}

/// `--upgrade <name>` (#618): the recorded ref re-resolved against the
/// recorded source, its current commit staged under the same bounds as an
/// install, and swapped over the old copy. The manifest entry's `commit`
/// and `digest` are rewritten only once the swap has happened, so the
/// manifest never names a commit the directory does not hold.
///
/// A copy whose files differ from the recorded digest is refused with the
/// paths named, and there is no flag to overwrite them: the edits are the
/// user's and an upgrade that lost them would be a removal nobody asked
/// for. An entry with no digest (installed before #617) is refused too,
/// since nothing can say whether it was edited; `--remove` then install is
/// the way past, as it was before.
pub(crate) fn upgrade_package(config: &Path, name: &str, out: &mut dyn Write) -> Result<()> {
    // Before any path is built from it, as `--remove` does.
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
    match check_digest(&target, &manifest.packages[i]) {
        DigestCheck::Ok => {}
        DigestCheck::Modified(paths) => bail!(
            "{name} has been modified since it was installed; an upgrade would overwrite \
             these: {}. `trellis install --remove {name}` and install again to discard \
             them",
            paths.join(", ")
        ),
        DigestCheck::NotRecorded => bail!(
            "{name} was installed before its digest was recorded, so nothing can say \
             whether it was edited; `trellis install --remove {name}` and install again"
        ),
    }
    // The recorded source was checked when it was recorded, but the
    // manifest is a file the user can edit, so it is checked again before
    // it reaches git.
    let (source, reference) = (
        manifest.packages[i].source.clone(),
        manifest.packages[i].reference.clone(),
    );
    if let Some(helper) = remote_helper(&source) {
        bail!(
            "{MANIFEST}: the source of {name} names the git remote helper `{}`",
            plain(helper)
        );
    }
    if let Some(why) = unacceptable_source(&source) {
        bail!("{MANIFEST}: {why}");
    }
    if reference.starts_with('-') || reference.chars().any(|c| c.is_control() || c == ' ') {
        bail!(
            "{MANIFEST}: ref `{}` of {name} is not a ref name",
            plain(&reference)
        );
    }

    let staging = Staging::new(&dir)?;
    let bare = staging.path.join("clone");
    clone_bare(&source, &bare)?;
    let commit = resolve_ref(&bare, &reference)?;
    let old = manifest.packages[i].commit.clone();
    if commit == old {
        writeln!(
            out,
            "{name} is already at commit {commit} ({} @ {}); nothing to do",
            plain(&source),
            plain(&reference)
        )?;
        return Ok(());
    }
    let plan = plan_for(&bare, &source, &commit)?;
    let tree = staging.path.join("package");
    let digest = stage(&bare, &plan, &tree)?;

    // The old copy moves into the staging directory, so it is removed with
    // the clone once the swap has gone through, and moves back if it does
    // not.
    let previous = staging.path.join("previous");
    std::fs::rename(&target, &previous)
        .with_context(|| format!("cannot move the old copy of {name} aside"))?;
    if let Err(e) = std::fs::rename(&tree, &target) {
        let restored = std::fs::rename(&previous, &target);
        return Err(e).with_context(|| {
            format!(
                "cannot move the upgraded package into {}{}",
                target.display(),
                match restored {
                    Ok(()) => "; the old copy is back in place".to_string(),
                    Err(e) => format!("; the old copy is in {} ({e})", previous.display()),
                }
            )
        });
    }
    manifest.packages[i].commit = commit.clone();
    manifest.packages[i].digest = Some(digest);
    if let Err(e) = save(&dir, &manifest) {
        // The manifest still names the old commit; put its copy back so
        // the two agree, rather than leave a directory the entry misdescribes.
        let _ = remove_tree(&target);
        let _ = std::fs::rename(&previous, &target);
        return Err(e);
    }
    writeln!(
        out,
        "upgraded {name} from {} @ {}: commit {old} -> {commit}: {} in {}",
        plain(&source),
        plain(&reference),
        summary(&plan),
        target.display()
    )?;
    report_skipped(&plan, out)
}

/// What `commit` of the bare clone would put on disk, refused when it holds
/// nothing to install or exceeds the bounds (ADR 0039 §5, §8).
fn plan_for(bare: &Path, source: &str, commit: &str) -> Result<Plan> {
    let listing = ls_tree(bare, commit)?;
    let plan = select(&listing);
    if plan.copy.is_empty() {
        bail!(
            "{} at {commit} has no skills/<name>/SKILL.md and no prompts/<name>.md; \
             nothing to install",
            plain(source)
        );
    }
    check_bounds(&plan.copy)?;
    Ok(plan)
}

/// Writes the plan's files under `tree` and returns their digest.
fn stage(bare: &Path, plan: &Plan, tree: &Path) -> Result<Digest> {
    let mut digest = Digest::new();
    for f in &plan.copy {
        let bytes = blob(bare, &f.blob)?;
        if bytes.len() as u64 != f.size {
            bail!("{} changed size while it was read", f.path);
        }
        // Over the bytes written, not git's blob id: the blob id is of
        // git's object, and what `--list` compares later is the file.
        digest.insert(f.path.clone(), sha256_hex(&bytes));
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
    Ok(digest)
}

/// `N skill(s), M prompt template(s), K file(s)`.
fn summary(plan: &Plan) -> String {
    let (skills, prompts) = counts(plan);
    format!(
        "{skills} skill(s), {prompts} prompt template(s), {} file(s)",
        plan.copy.len()
    )
}

/// Skills (a `skills/<name>/SKILL.md` one level deep) and prompt templates
/// the plan copies.
fn counts(plan: &Plan) -> (usize, usize) {
    let skills = plan
        .copy
        .iter()
        .filter(|f| {
            f.path
                .strip_prefix("skills/")
                .and_then(|r| r.strip_suffix("/SKILL.md"))
                .is_some_and(|n| !n.contains('/'))
        })
        .count();
    let prompts = plan
        .copy
        .iter()
        .filter(|f| f.path.starts_with("prompts/"))
        .count();
    (skills, prompts)
}

fn report_skipped(plan: &Plan, out: &mut dyn Write) -> Result<()> {
    if !plan.skipped.is_empty() {
        writeln!(
            out,
            "skipped {} path(s): a package carries skills and prompt templates only, \
             never hooks, tools or other code (ADR 0039)",
            plan.skipped.len()
        )?;
        // Every path, uncapped (#120): a count stands in for the one hook
        // the user would have wanted to see. What bounds this output is
        // `MAX_TREE_ENTRIES`, enforced while `ls-tree` is read (#200).
        for (path, why) in &plan.skipped {
            writeln!(out, "  {}: {why}", plain(path))?;
        }
    }
    Ok(())
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
        // The manifest is a file the user can edit, and a name becomes a
        // path before the digest is checked.
        if let Some(why) = bad_name(&e.name) {
            bail!("{}: {why}", dir.join(MANIFEST).display());
        }
        writeln!(
            out,
            "{}  source {}  ref {}  commit {}  {}",
            plain(&e.name),
            plain(&e.source),
            plain(&e.reference),
            plain(&e.commit),
            check_digest(&dir.join(&e.name), e)
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

/// The `<helper>` of a `<helper>::<address>` source, by git's own rule
/// (`transport_get`): a scheme-like run of characters, a letter then
/// letters, digits, `+`, `-` or `.`, followed directly by `::`.
fn remote_helper(source: &str) -> Option<&str> {
    let (helper, _) = source.split_once("::")?;
    let mut chars = helper.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(helper)
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
/// - `protocol.allow=never`, then `always` for `https`, `ssh`, `git` and
///   `file` only: every other transport is refused, among them `ext::`,
///   which runs a command named in the URL, and any `<helper>::` form,
///   which runs `git-remote-<helper>` from PATH;
/// - `transfer.fsckObjects=true`: git's own checks on what it receives,
///   which reject `..` and `.git` tree entries among others;
/// - `GIT_TERMINAL_PROMPT=0`: a source that wants credentials fails rather
///   than waiting on a prompt nobody may be watching.
fn git() -> std::process::Command {
    let mut c = trellis_vcs::command();
    // A per-name `protocol.<name>.allow` takes precedence over the
    // `protocol.allow` default, whatever their order.
    c.args([
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "protocol.ssh.allow=always",
        "-c",
        "protocol.git.allow=always",
        "-c",
        "protocol.file.allow=always",
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

fn resolve_ref(bare: &Path, reference: &str) -> Result<String> {
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
    const WHAT: &str = "list the package's files";
    let mut child = git()
        .arg("-C")
        .arg(bare)
        .args(["ls-tree", "-r", "-l", "-z", "--end-of-options", commit])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run git to {WHAT}"))?;
    // Drained on its own thread: a child blocked writing stderr would
    // never finish the stdout this thread is reading.
    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut e, &mut buf);
            buf
        })
    });
    let stdout = child.stdout.take().context("git's stdout was not piped")?;
    let read = read_ls_tree(stdout, MAX_TREE_ENTRIES, MAX_LISTING_BYTES);
    if read.is_err() {
        // Stopped early: git may be blocked on a full pipe, so `wait`
        // alone could hang.
        let _ = child.kill();
    }
    let status = child
        .wait()
        .with_context(|| format!("cannot run git to {WHAT}"))?;
    let err = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    let entries = read?;
    if !status.success() {
        let err = String::from_utf8_lossy(&err);
        let err = match err.trim() {
            "" => "no such ref or object".to_string(),
            e => plain(e),
        };
        bail!("could not {WHAT}: {err}");
    }
    Ok(entries)
}

/// Reads `ls-tree -z` records one at a time and refuses the listing as soon
/// as it passes `max_entries` or `max_bytes`, so an oversized tree is never
/// held whole.
fn read_ls_tree(
    out: impl std::io::Read,
    max_entries: usize,
    max_bytes: u64,
) -> Result<Vec<TreeEntry>> {
    use std::io::BufRead;
    // One byte past the cap, so output of exactly `max_bytes` is accepted
    // and anything longer is seen.
    let mut r = std::io::BufReader::new(out.take(max_bytes.saturating_add(1)));
    let mut v = Vec::new();
    let mut seen: u64 = 0;
    let mut rec = Vec::new();
    loop {
        rec.clear();
        let n = r
            .read_until(0, &mut rec)
            .context("cannot read `git ls-tree` output")?;
        if n == 0 {
            break;
        }
        seen += n as u64;
        if seen > max_bytes {
            bail!(
                "the package's file listing is more than {max_bytes} bytes; the limit is \
                 {max_bytes} (ADR 0039)"
            );
        }
        if rec.last() != Some(&0) {
            // Within the byte cap, so this is the end of the output: git
            // ends every `-z` record with NUL.
            bail!("unexpected `git ls-tree` output");
        }
        rec.pop();
        if rec.is_empty() {
            continue;
        }
        if v.len() == max_entries {
            bail!(
                "the package's tree has more than {max_entries} entries; the limit is \
                 {max_entries} (ADR 0039)"
            );
        }
        v.push(parse_ls_tree_record(&rec)?);
    }
    Ok(v)
}

/// `<mode> SP <type> SP <id> SP+ <size> TAB <path>`, one `-z` record. A
/// path that is not UTF-8 is kept lossily, which cannot pass the component
/// allowlist, so it is skipped and named.
fn parse_ls_tree_record(rec: &[u8]) -> Result<TreeEntry> {
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
    Ok(TreeEntry {
        mode: mode.to_string(),
        blob: blob.to_string(),
        size: size.parse().ok(),
        path,
    })
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
        assert!(
            said.contains("after the repository's own skills and templates, which win a name"),
            "{said}"
        );
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
    fn install_counts_a_nested_skill_md_as_a_file_not_a_skill() {
        // The risk: a reference file named SKILL.md inside a skill is copied
        // (it is a .md under a valid skill) and the summary counted it as a
        // second skill, so the user is told of a skill that does not exist.
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("nested");
        std::fs::create_dir_all(&src).unwrap();
        git_in(&src, &["init", "-q"]);
        write(&src, "skills/alpha/SKILL.md", "---\nname: alpha\n---\nA\n");
        write(&src, "skills/alpha/references/SKILL.md", "reference\n");
        git_in(&src, &["add", "-A"]);
        git_in(&src, &["commit", "-q", "-m", "package"]);
        git_in(&src, &["tag", "v1"]);
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        install(&cfg, &spec(&src, "v1"), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            files_under(&cfg.join("packages").join("nested"))
                .contains(&"skills/alpha/references/SKILL.md".to_string()),
            "{said}"
        );
        assert!(said.contains(": 1 skill(s),"), "{said}");
    }

    /// A package source whose code outnumbers any cap on how many skipped
    /// paths are shown: 25 hooks, 25 tools, a `.trellis/` with a hook, and a
    /// script inside a skill. Returns the source and every code path in it.
    fn code_heavy_source(parent: &Path) -> (PathBuf, Vec<String>) {
        let src = parent.join("codepkg");
        std::fs::create_dir_all(&src).unwrap();
        git_in(&src, &["init", "-q"]);
        write(&src, "skills/alpha/SKILL.md", "---\nname: alpha\n---\nA\n");
        let mut code = Vec::new();
        for i in 0..25 {
            code.push(format!("hooks/h{i:02}.sh"));
            code.push(format!("tools/t{i:02}.py"));
        }
        code.push(".trellis/config.toml".to_string());
        code.push(".trellis/hooks/pre_tool.py".to_string());
        code.push("skills/alpha/run.py".to_string());
        for p in &code {
            write(&src, p, "#!/bin/sh\nexit 0\n");
        }
        git_in(&src, &["add", "-A"]);
        git_in(&src, &["commit", "-q", "-m", "package"]);
        git_in(&src, &["tag", "v1"]);
        (src, code)
    }

    #[test]
    fn install_skips_code_and_names_every_skipped_path() {
        // The risk: a package with many hooks shows the first few and a
        // count, so the one the user would have cared about is never named,
        // and a package never carries code only if nothing of it lands on
        // disk (#120).
        let d = tempfile::tempdir().unwrap();
        let (src, code) = code_heavy_source(d.path());
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        install(&cfg, &spec(&src, "v1"), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();

        assert_eq!(
            files_under(&cfg.join("packages").join("codepkg")),
            vec!["skills/alpha/SKILL.md"],
            "{said}"
        );
        assert!(code.len() > 50, "the fixture outnumbers the old cap of 20");
        for p in &code {
            assert!(
                said.lines()
                    .any(|l| l.trim_start().starts_with(&format!("{p}: "))),
                "{p} not named on its own line in:\n{said}"
            );
        }
        assert!(
            said.contains(&format!("skipped {} path(s)", code.len())),
            "{said}"
        );
        assert!(
            !said.contains("... and "),
            "no path is folded into a count:\n{said}"
        );
    }

    #[test]
    fn install_skips_code_and_leaves_the_trust_store_byte_identical() {
        // The risk: installing a package records, widens or rewrites trust,
        // so code nobody ran `trellis trust` on is vouched for. The store is
        // placed where `TrustStore::user()` puts it in production, beside
        // `packages/` in the same user config directory, and holds a real
        // record so a rewrite that kept it valid would still differ.
        let d = tempfile::tempdir().unwrap();
        let (src, _) = code_heavy_source(d.path());
        let cfg = d.path().join("cfg");
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join(".trellis/hooks")).unwrap();
        std::fs::write(repo.join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();
        let hooks = trellis_loop::HookConfig {
            pre_tool: Some(vec!["python".into(), ".trellis/hooks/gate.py".into()]),
            ..Default::default()
        };
        let fp = trellis_loop::Fingerprint::of(&repo, &hooks, &[], None);
        let store = trellis_loop::TrustStore::at(cfg.join("trust.json"));
        store.trust(&repo, &fp).unwrap();
        let before = std::fs::read(store.path()).unwrap();

        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();

        assert_eq!(std::fs::read(store.path()).unwrap(), before);
        assert!(
            !cfg.join("trust.json.tmp").exists(),
            "the store's write-then-rename path was not taken"
        );
        assert_eq!(
            store.state(&repo, &fp).unwrap(),
            trellis_loop::TrustState::Trusted
        );
        // Nothing beside the store and the packages directory appeared.
        let mut top: Vec<String> = std::fs::read_dir(&cfg)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(top, vec!["packages", "trust.json"]);
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
    fn install_digest_is_recorded_per_file_and_list_says_ok_while_the_files_match() {
        // The risk: `--list` answers "is this what I installed" from the
        // manifest alone, which says nothing about the bytes on disk; the
        // digest is over the files as written (ADR 0044's rule), one per
        // path, so a later edit can be named.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        let entry = install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let digest = entry
            .digest
            .as_ref()
            .expect("an install records its digest");
        let paths: Vec<&str> = digest.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            vec![
                "prompts/review.md",
                "skills/alpha/SKILL.md",
                "skills/alpha/references/notes.md",
                "skills/beta/SKILL.md",
            ]
        );
        assert_eq!(
            digest["skills/alpha/SKILL.md"],
            sha256_hex(b"---\nname: alpha\n---\nA\n")
        );
        // Round-tripped through the manifest, not only held in memory.
        let loaded = load(&cfg.join("packages")).unwrap();
        assert_eq!(loaded.packages[0].digest.as_ref(), Some(digest));
        let mut out = Vec::new();
        list_packages(&cfg, &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(said.trim_end().ends_with("  digest ok"), "{said}");
    }

    #[test]
    fn install_digest_names_every_edited_missing_and_added_path() {
        // The risk: one hash over the whole package can say "changed" but
        // not what, and a check that skips a file it cannot read, or reads
        // a link put in place of one, reports it as the bytes installed.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let pkg = cfg.join("packages/pkg-src");
        std::fs::write(pkg.join("skills/alpha/SKILL.md"), "edited\n").unwrap();
        std::fs::remove_file(pkg.join("prompts/review.md")).unwrap();
        std::fs::write(pkg.join("skills/beta/extra.md"), "added\n").unwrap();
        let mut out = Vec::new();
        list_packages(&cfg, &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains(
                "  modified: prompts/review.md (missing), skills/alpha/SKILL.md, \
                 skills/beta/extra.md (added)"
            ),
            "{said}"
        );
        assert!(!said.contains("digest ok"), "{said}");
        // The whole directory gone: every recorded path is missing, and the
        // check is still a report, not an error.
        std::fs::remove_dir_all(&pkg).unwrap();
        let entry = &load(&cfg.join("packages")).unwrap().packages[0];
        match check_digest(&pkg, entry) {
            DigestCheck::Modified(paths) => {
                assert_eq!(paths.len(), 4, "{paths:?}");
                assert!(paths.iter().all(|p| p.ends_with(" (missing)")), "{paths:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn install_digest_absent_from_an_older_manifest_loads_and_reads_not_recorded() {
        // The risk: a manifest written before the digest was recorded fails
        // to parse (and with it every run that reads installed skills), or
        // an entry with no digest is reported as unchanged.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let packages = cfg.join("packages");
        let commit = git_in(&src, &["rev-parse", "v1^{commit}"]);
        let older = format!(
            "[[package]]\nname = \"pkg-src\"\nsource = \"{}\"\nref = \"v1\"\ncommit = \"{commit}\"\n",
            std::path::absolute(&src)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/")
        );
        std::fs::write(packages.join(MANIFEST), older).unwrap();
        let m = load(&packages).unwrap();
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.packages[0].digest, None);
        assert_eq!(
            installed_dirs(&cfg).unwrap(),
            vec![packages.join("pkg-src")]
        );
        let mut out = Vec::new();
        list_packages(&cfg, &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(said.trim_end().ends_with("  not recorded"), "{said}");
        assert!(!said.contains("digest ok"), "{said}");
        // Saved again, the entry stays without one: nothing invents a digest
        // for files it did not write.
        save(&packages, &m).unwrap();
        assert!(
            !std::fs::read_to_string(packages.join(MANIFEST))
                .unwrap()
                .contains("digest"),
            "{said}"
        );
    }

    /// Commits a second version of the source on branch `release`: alpha's
    /// SKILL.md changes, beta is dropped, a new skill and a hook appear.
    fn advance(src: &Path) {
        write(src, "skills/alpha/SKILL.md", "---\nname: alpha\n---\nA2\n");
        std::fs::remove_dir_all(src.join("skills/beta")).unwrap();
        write(src, "skills/delta/SKILL.md", "---\nname: delta\n---\nD\n");
        write(src, "hooks/new.sh", "#!/bin/sh\n");
        git_in(src, &["add", "-A"]);
        git_in(src, &["commit", "-q", "-m", "v2"]);
    }

    #[test]
    fn install_upgrade_installs_the_ref_s_current_commit_and_rewrites_the_entry_after_the_swap() {
        // The risk: remove-then-install loses the package for the window
        // between the two and records nothing of the old commit; an upgrade
        // must leave the directory at the new commit with the manifest's
        // `commit` and `digest` describing exactly it, and nothing of the
        // old copy or the staging behind.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        git_in(&src, &["branch", "release"]);
        git_in(&src, &["checkout", "-q", "release"]);
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "release"), &mut Vec::new()).unwrap();
        let old = git_in(&src, &["rev-parse", "release^{commit}"]);
        advance(&src);
        let new = git_in(&src, &["rev-parse", "release^{commit}"]);
        assert_ne!(old, new);

        let mut out = Vec::new();
        upgrade_package(&cfg, "pkg-src", &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(said.starts_with("upgraded pkg-src from "), "{said}");
        assert!(
            said.contains(&format!(
                "commit {old} -> {new}: 2 skill(s), 1 prompt template(s), 5 file(s)"
            )),
            "{said}"
        );
        assert!(said.contains("hooks/new.sh: outside"), "{said}");
        let pkg = cfg.join("packages/pkg-src");
        // `advance` re-added the tree from the worktree, where tool.md is a
        // plain file and the index-only link does not exist, so one more
        // text file is copied and the link is gone.
        assert_eq!(
            files_under(&pkg),
            vec![
                "prompts/review.md",
                "skills/alpha/SKILL.md",
                "skills/alpha/references/notes.md",
                "skills/alpha/tool.md",
                "skills/delta/SKILL.md",
            ]
        );
        assert_eq!(
            std::fs::read_to_string(pkg.join("skills/alpha/SKILL.md")).unwrap(),
            "---\nname: alpha\n---\nA2\n"
        );
        let m = load(&cfg.join("packages")).unwrap();
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.packages[0].commit, new);
        assert_eq!(m.packages[0].reference, "release");
        assert_eq!(check_digest(&pkg, &m.packages[0]), DigestCheck::Ok);
        let mut left: Vec<String> = std::fs::read_dir(cfg.join("packages"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["packages.toml", "pkg-src"]);
    }

    #[test]
    fn install_upgrade_refuses_a_modified_copy_and_names_the_paths() {
        // The risk: an upgrade overwrites edits the user made to an
        // installed skill, which is a removal nobody asked for (ClawHub's
        // `--force` is the part not copied). An entry with no digest is
        // refused for the same reason: nothing can say it was not edited.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        git_in(&src, &["branch", "release"]);
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "release"), &mut Vec::new()).unwrap();
        git_in(&src, &["checkout", "-q", "release"]);
        advance(&src);
        let pkg = cfg.join("packages/pkg-src");
        std::fs::write(pkg.join("skills/beta/SKILL.md"), "mine\n").unwrap();
        std::fs::write(pkg.join("prompts/extra.md"), "mine too\n").unwrap();
        let manifest_before = std::fs::read(cfg.join("packages").join(MANIFEST)).unwrap();

        let mut out = Vec::new();
        let e = upgrade_package(&cfg, "pkg-src", &mut out).unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("modified since it was installed"), "{msg}");
        assert!(msg.contains("skills/beta/SKILL.md"), "{msg}");
        assert!(msg.contains("prompts/extra.md (added)"), "{msg}");
        assert!(msg.contains("--remove pkg-src"), "{msg}");
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        assert_eq!(
            std::fs::read_to_string(pkg.join("skills/beta/SKILL.md")).unwrap(),
            "mine\n"
        );
        assert_eq!(
            std::fs::read(cfg.join("packages").join(MANIFEST)).unwrap(),
            manifest_before
        );
        // Nothing was fetched: no staging directory came and went.
        let mut left: Vec<String> = std::fs::read_dir(cfg.join("packages"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["packages.toml", "pkg-src"]);

        // No digest recorded (an install from before #617): refused, not
        // read as unmodified.
        let mut m = load(&cfg.join("packages")).unwrap();
        m.packages[0].digest = None;
        save(&cfg.join("packages"), &m).unwrap();
        let e = upgrade_package(&cfg, "pkg-src", &mut Vec::new()).unwrap_err();
        assert!(
            format!("{e:#}").contains("before its digest was recorded"),
            "{e:#}"
        );
    }

    #[test]
    fn install_upgrade_of_an_unknown_name_is_refused_and_touches_nothing() {
        // The risk: `--upgrade` joins its argument onto a path and hands
        // the entry's source to git, so a name that is not a package, `..`
        // above all, must be refused before either.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        let manifest_before = std::fs::read(cfg.join("packages").join(MANIFEST)).unwrap();
        for name in ["nope", "..", "../cfg", "packages.toml", "PKG-SRC"] {
            let mut out = Vec::new();
            let e = upgrade_package(&cfg, name, &mut out).unwrap_err();
            let msg = format!("{e:#}");
            assert!(
                msg.contains("no package named") || msg.contains("package name"),
                "{name}: {msg}"
            );
            assert!(out.is_empty(), "{name}: {}", String::from_utf8_lossy(&out));
        }
        let e = upgrade_package(&cfg, "nope", &mut Vec::new()).unwrap_err();
        assert!(format!("{e:#}").contains("installed: pkg-src"), "{e:#}");
        assert_eq!(
            std::fs::read(cfg.join("packages").join(MANIFEST)).unwrap(),
            manifest_before
        );
        assert!(cfg.join("packages/pkg-src/skills/alpha/SKILL.md").is_file());
    }

    #[test]
    fn install_upgrade_at_the_same_commit_is_a_no_op_that_says_so() {
        // The risk: a tag or commit ref re-resolves to what is installed,
        // and an "upgrade" to it rewrote the directory and manifest for
        // nothing, or said "upgraded" of a package that did not change.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        // The source moves on, but `v1` does not.
        advance(&src);
        let packages = cfg.join("packages");
        let manifest_before = std::fs::read(packages.join(MANIFEST)).unwrap();
        let pkg = packages.join("pkg-src");
        let files_before = files_under(&pkg);
        let commit = git_in(&src, &["rev-parse", "v1^{commit}"]);

        let mut out = Vec::new();
        upgrade_package(&cfg, "pkg-src", &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.starts_with(&format!("pkg-src is already at commit {commit}")),
            "{said}"
        );
        assert!(said.contains("nothing to do"), "{said}");
        assert!(!said.contains("upgraded"), "{said}");
        assert_eq!(
            std::fs::read(packages.join(MANIFEST)).unwrap(),
            manifest_before
        );
        assert_eq!(files_under(&pkg), files_before);
        assert_eq!(
            std::fs::read_to_string(pkg.join("skills/alpha/SKILL.md")).unwrap(),
            "---\nname: alpha\n---\nA\n"
        );
        let mut left: Vec<String> = std::fs::read_dir(&packages)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["packages.toml", "pkg-src"]);
    }

    /// Every path under `root` with its bytes, or `None` when `root` is
    /// not there: the whole state a dry run must leave as it found it.
    fn snapshot(root: &Path) -> Option<Vec<(String, Vec<u8>)>> {
        if !root.exists() {
            return None;
        }
        Some(
            files_under(root)
                .into_iter()
                .map(|rel| {
                    let bytes = std::fs::read(root.join(&rel)).unwrap();
                    (rel, bytes)
                })
                .collect(),
        )
    }

    #[test]
    fn install_dry_run_lists_what_would_be_copied_and_skipped_and_writes_nothing() {
        // The risk: the only way to learn what a package would put under
        // `packages/` was to install it; and a dry run that leaves a
        // staging clone, a manifest, or even an empty `packages/` behind
        // is not one. Checked against a config with no `packages/` at all
        // and against one holding another package.
        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        assert_eq!(snapshot(&cfg.join("packages")), None);
        let mut out = Vec::new();
        install_dry_run(&cfg, &spec(&src, "v1"), &mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert!(said.starts_with("dry run: pkg-src from "), "{said}");
        assert!(
            said.contains("would put 2 skill(s), 1 prompt template(s), 4 file(s) in "),
            "{said}"
        );
        for copied in [
            "  prompts/review.md\n",
            "  skills/alpha/SKILL.md\n",
            "  skills/alpha/references/notes.md\n",
            "  skills/beta/SKILL.md\n",
        ] {
            assert!(said.contains(copied), "{copied:?} not listed in:\n{said}");
        }
        // The skipped block is the install's, every path on its own line.
        assert!(
            said.contains("skipped 9 path(s): a package carries"),
            "{said}"
        );
        for skipped in [
            "  hooks/after_run.sh: outside",
            "  skills/alpha/tool.md: executable",
            "  skills/alpha/link.md: symbolic link",
        ] {
            assert!(said.contains(skipped), "{skipped} not named in:\n{said}");
        }
        assert!(
            said.trim_end()
                .ends_with("would install 2 skill(s), 1 prompt(s); nothing was written"),
            "{said}"
        );
        assert!(!said.contains("installed pkg-src"), "{said}");
        assert_eq!(
            snapshot(&cfg.join("packages")),
            None,
            "a dry run created packages/"
        );
        assert!(!cfg.exists(), "a dry run created the config directory");

        // With another package installed, the manifest and its files are
        // byte-identical after, and the dry-run package is not installed.
        let other = d.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        git_in(&other, &["init", "-q"]);
        write(&other, "prompts/one.md", "one\n");
        git_in(&other, &["add", "-A"]);
        git_in(&other, &["commit", "-q", "-m", "other"]);
        git_in(&other, &["tag", "v1"]);
        install(&cfg, &spec(&other, "v1"), &mut Vec::new()).unwrap();
        let before = snapshot(&cfg.join("packages"));
        assert!(before.is_some());
        install_dry_run(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap();
        assert_eq!(snapshot(&cfg.join("packages")), before);
        assert_eq!(load(&cfg.join("packages")).unwrap().packages.len(), 1);
        // A dry run of what is already installed is refused as an install
        // would be, since the install it describes could not happen.
        let e = install_dry_run(&cfg, &spec(&other, "v1"), &mut Vec::new()).unwrap_err();
        assert!(format!("{e:#}").contains("already installed"), "{e:#}");
        assert_eq!(snapshot(&cfg.join("packages")), before);
    }

    #[test]
    fn install_dry_run_reports_a_bounds_refusal_as_an_install_would_and_writes_nothing() {
        // The risk: a dry run that lists a package the install would refuse
        // says it would install; or one that skips the listing bound holds
        // an unbounded tree in memory to describe it (#200).
        let d = tempfile::tempdir().unwrap();
        let src = wide_source(d.path(), MAX_TREE_ENTRIES + 1);
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        let e = install_dry_run(&cfg, &spec(&src, "v1"), &mut out).unwrap_err();
        let dry = format!("{e:#}");
        assert!(
            dry.contains(&format!("the limit is {MAX_TREE_ENTRIES}")),
            "{dry}"
        );
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        assert!(
            !cfg.exists(),
            "a refused dry run wrote under the config directory"
        );
        // Word for word what the install says.
        let e = install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap_err();
        assert_eq!(dry, format!("{e:#}"));
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

    /// A source whose tree holds `n` entries, all one blob, written through
    /// the index so the test creates no worktree files.
    fn wide_source(parent: &Path, n: usize) -> PathBuf {
        let src = parent.join("wide-src");
        std::fs::create_dir_all(&src).unwrap();
        git_in(&src, &["init", "-q"]);
        let blob = git_in(&src, &["hash-object", "-w", "--stdin", "--path", "x"]);
        let mut info = String::new();
        for i in 0..n {
            info.push_str(&format!("100644 {blob}\tprompts/p{i}.md\n"));
        }
        let mut child = trellis_vcs::command()
            .arg("-C")
            .arg(&src)
            .args(["update-index", "--add", "--index-info"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(info.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "update-index --index-info");
        git_in(&src, &["commit", "-q", "-m", "wide"]);
        git_in(&src, &["tag", "v1"]);
        src
    }

    #[test]
    fn install_tree_listing_bounded_refuses_an_over_limit_tree_and_writes_nothing() {
        // The risk: the whole `ls-tree` listing was read into memory before
        // any bound applied, and every skipped entry is printed, so a source
        // with a huge tree cost unbounded memory and output (#200).
        let d = tempfile::tempdir().unwrap();
        let src = wide_source(d.path(), MAX_TREE_ENTRIES + 1);
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        let e = install(&cfg, &spec(&src, "v1"), &mut out).unwrap_err();
        let msg = format!("{e:#}");
        assert!(
            msg.contains(&format!("the limit is {MAX_TREE_ENTRIES}")),
            "{msg}"
        );
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        assert!(!cfg.join("packages/wide-src").exists());
        assert!(load(&cfg.join("packages")).unwrap().packages.is_empty());

        // Exactly at the limit the listing is read whole and the install
        // goes on to the next bound: every entry is a prompt, so it is
        // `MAX_FILES` that refuses it.
        let d = tempfile::tempdir().unwrap();
        let src = wide_source(d.path(), MAX_TREE_ENTRIES);
        let cfg = d.path().join("cfg");
        let e = install(&cfg, &spec(&src, "v1"), &mut Vec::new()).unwrap_err();
        let msg = format!("{e:#}");
        assert!(
            msg.contains(&format!("files to install; the limit is {MAX_FILES}")),
            "{msg}"
        );
    }

    #[test]
    fn install_tree_listing_bounded_stops_reading_at_the_limit() {
        // The risk: a bound checked after the listing is collected still
        // holds the whole listing. The reader here offers far more records
        // than the limit; the error must come back without them.
        let record = format!("100644 blob {} 1\tprompts/p.md\0", "0".repeat(40));
        let many = record.repeat(1_000);
        let e = read_ls_tree(many.as_bytes(), 10, u64::MAX).unwrap_err();
        assert!(format!("{e:#}").contains("the limit is 10 "), "{e:#}");
        assert_eq!(
            read_ls_tree(many.as_bytes(), 1_000, u64::MAX)
                .unwrap()
                .len(),
            1_000
        );

        // One record with a path of any length is one entry; the byte cap
        // is what bounds it.
        let long = format!(
            "100644 blob {} 1\tprompts/{}.md\0",
            "0".repeat(40),
            "a".repeat(4_096)
        );
        let e = read_ls_tree(long.as_bytes(), 10, 1_024).unwrap_err();
        assert!(format!("{e:#}").contains("the limit is 1024 "), "{e:#}");
        assert_eq!(
            read_ls_tree(long.as_bytes(), 10, long.len() as u64)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn install_transport_helper_sources_are_refused_before_git_runs() {
        // The risk: `<helper>::<url>` makes git run `git-remote-<helper>`
        // from the user's PATH, so the package source string would choose
        // which installed program runs. It must be refused before any git
        // spawn, and `ext::` (which names a command outright) with it.
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cfg");
        for source in [
            "ext::sh -c touch% pwned",
            "foo::https://x",
            "git+x.y-z::https://x/p",
        ] {
            let mut out = Vec::new();
            let e = install(&cfg, &format!("{source}@v1"), &mut out).unwrap_err();
            let msg = format!("{e:#}");
            assert!(msg.contains("remote helper"), "{source}: {msg}");
            assert!(msg.contains("https://") && msg.contains("file://"), "{msg}");
            assert!(out.is_empty(), "nothing is reported as fetched");
            // The packages directory is created before the clone; its
            // absence shows the refusal came before the git path.
            assert!(
                !cfg.join("packages").exists(),
                "{source} reached the clone path"
            );
        }
        // Addresses that hold `::` without a helper prefix, or none at all,
        // are not mistaken for one.
        assert_eq!(remote_helper("https://h/[::1]/p"), None);
        assert_eq!(remote_helper("git@host:o/p.git"), None);
        assert_eq!(remote_helper(r"C:\src\pkg"), None);
        assert_eq!(remote_helper("1x::y"), None);
        assert_eq!(remote_helper("foo::https://x"), Some("foo"));
    }

    #[test]
    fn install_transport_local_file_source_still_installs() {
        // The risk: `protocol.allow=never` also shuts the `file` transport a
        // local path goes through under `--no-local`, so a local package
        // would stop installing. The allowlist must also be what git gets.
        let args: Vec<String> = git()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "protocol.allow=never"), "{args:?}");
        for name in ["https", "ssh", "git", "file"] {
            let allow = format!("protocol.{name}.allow=always");
            assert!(args.contains(&allow), "{allow} missing: {args:?}");
        }
        assert_eq!(
            args.iter().filter(|a| a.ends_with(".allow=always")).count(),
            4,
            "no transport beyond the four: {args:?}"
        );

        let d = tempfile::tempdir().unwrap();
        let src = source(d.path());
        let cfg = d.path().join("cfg");
        let mut out = Vec::new();
        let entry = install(&cfg, &spec(&src, "v1"), &mut out).unwrap();
        assert_eq!(entry.name, "pkg-src");
        assert!(
            cfg.join("packages/pkg-src/skills/alpha/SKILL.md").is_file(),
            "{:?}",
            files_under(&cfg)
        );
    }

    // ---- #248: survivors of the trellis-cli mutation run ----

    #[test]
    fn a_ref_with_a_space_a_control_character_or_a_leading_dash_is_not_a_ref() {
        for bad in ["src@v 1", "src@v\u{7}1", "src@-v1"] {
            let e = split_spec(bad).unwrap_err().to_string();
            assert!(e.contains("is not a ref name"), "{bad}: {e}");
        }
        assert_eq!(split_spec("src@v1").unwrap(), ("src", "v1"));
    }

    #[test]
    fn a_source_that_is_an_option_holds_a_control_character_or_a_password_is_refused() {
        // `-` first would be read by `git clone` as an option, and a
        // password would be written into packages.toml.
        assert!(unacceptable_source("--upload-pack=x").is_some());
        assert!(unacceptable_source("pkg\u{1b}[2J").is_some());
        // Built at run time: a basic-auth URL literal in tracked source
        // trips scripts/checks/no-auth-urls.sh (the G5 gate).
        let with_password = format!("https://me:{}@host.example/p.git", "pw");
        assert!(unacceptable_source(&with_password).is_some());
        assert_eq!(unacceptable_source("https://me@host.example/p.git"), None);
    }

    #[test]
    fn a_path_component_is_one_to_128_characters() {
        assert!(bad_component("").is_some(), "empty");
        assert_eq!(bad_component(&"a".repeat(128)), None);
        assert!(bad_component(&"a".repeat(129)).is_some());
    }

    #[test]
    fn a_skill_name_may_hold_lowercase_letters_digits_and_inner_hyphens() {
        for ok in ["abc", "a1", "9", "a-b"] {
            assert!(skill_name_ok(ok), "{ok}");
        }
        for bad in ["Abc", "a_b", "a.b"] {
            assert!(!skill_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn only_a_markdown_file_directly_under_prompts_is_a_template() {
        let entry = |path: &str| TreeEntry {
            mode: "100644".into(),
            blob: "b".repeat(40),
            size: Some(10),
            path: path.into(),
        };
        let plan = select(&[entry("prompts/fix.md"), entry("prompts/notes.txt")]);
        let copied: Vec<&str> = plan.copy.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(copied, ["prompts/fix.md"]);
        assert_eq!(plan.skipped.len(), 1, "{:?}", plan.skipped);
        assert_eq!(plan.skipped[0].0, "prompts/notes.txt");
    }

    #[test]
    fn package_bounds_admit_exactly_the_limit_and_refuse_one_over() {
        let files = |n: usize, size: u64| -> Vec<Accepted> {
            (0..n)
                .map(|i| Accepted {
                    path: format!("prompts/p{i}.md"),
                    blob: String::new(),
                    size,
                })
                .collect()
        };
        // Count.
        assert!(check_bounds(&files(MAX_FILES, 1)).is_ok());
        assert!(check_bounds(&files(MAX_FILES + 1, 1)).is_err());
        // One file's size.
        assert!(check_bounds(&files(1, MAX_FILE_BYTES)).is_ok());
        assert!(check_bounds(&files(1, MAX_FILE_BYTES + 1)).is_err());
        // The total: 32 files of the per-file limit are exactly the total.
        let n = (MAX_TOTAL_BYTES / MAX_FILE_BYTES) as usize;
        let mut at = files(n, MAX_FILE_BYTES);
        assert!(check_bounds(&at).is_ok());
        at.push(Accepted {
            path: "prompts/one-more.md".into(),
            blob: String::new(),
            size: 1,
        });
        let e = check_bounds(&at).unwrap_err().to_string();
        assert!(e.contains("total"), "{e}");
    }

    #[test]
    fn a_manifest_that_is_there_but_unreadable_is_an_error_not_no_packages() {
        // The risk: read as empty, the next install's save would write a
        // manifest with every earlier entry gone.
        let d = tempfile::tempdir().unwrap();
        let packages = packages_dir(d.path());
        std::fs::create_dir_all(&packages).unwrap();
        std::fs::write(packages.join(MANIFEST), [0xff, 0xfe, b'x']).unwrap();
        let e = installed_dirs(d.path()).unwrap_err();
        assert!(format!("{e:#}").contains("cannot read"), "{e:#}");
    }

    #[test]
    fn remove_tree_removes_a_plain_file_as_well_as_a_directory() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, "x").unwrap();
        remove_tree(&f).unwrap();
        assert!(!f.exists());
        let t = d.path().join("t/u");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("g"), "y").unwrap();
        remove_tree(&d.path().join("t")).unwrap();
        assert!(!d.path().join("t").exists());
    }
}
