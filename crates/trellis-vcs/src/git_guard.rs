//! The files under `.git/` a writer can use to run code at trellis's next
//! git call, watched around something that can write them.
//!
//! `git status` never shows `.git/`, and the rollback (`git reset --hard`,
//! `git clean -fd`) never touches it, so a `bash` command, custom tool,
//! hook or process agent that sets `core.fsmonitor`, points
//! `core.hooksPath` somewhere, or plants a hook would survive any rollback,
//! and the next `git status` or `git commit` trellis runs would execute it
//! (ADR 0026 §11, ADR 0023 §5). Moved here from the process-agent code so a
//! model run uses the same one.
//!
//! Covered, as bytes: the config git reads for this worktree
//! (`config`, `config.worktree`), every file a config `include.path` or
//! `includeIf.*.path` names that lives under the git directory (followed
//! through includes of includes), every file in the hooks directory
//! (`--git-path hooks`, which is `core.hooksPath` when that is set),
//! `info/exclude`, which decides what `git status` and `git clean` can see
//! at all, and `info/attributes`, which can bind a path to a filter the
//! config defines. Not covered: the rest of `.git/` (refs, objects, the
//! index), a submodule's own git directory, and an include target outside
//! the git directory (a tracked one is the rollback's; one elsewhere on
//! disk is not the repository's).
//!
//! **Links are never followed** (review round 9). Every path is looked at
//! with `symlink_metadata`: a link is recorded as a link, by its text, and
//! the walk of the hooks directory does not enter one. Before, a planted
//! `.git/hooks/lib -> $HOME` listed every file in the home directory as a
//! new hook, and restore deleted them. Nothing is written or removed
//! outside the git directories as they resolved at capture: a planted link
//! is removed itself (never its target), a directory on the way to a file
//! being put back that has become a link is replaced by a directory, and a
//! file is created with `create_new`, which does not follow a link. A hooks
//! directory outside the git directory (`core.hooksPath` pointing into the
//! worktree or elsewhere) is not guarded; re-pointing it is a change to
//! `config`, which is.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

/// How deep includes are followed. Git's own limit is 10.
const MAX_INCLUDE_DEPTH: usize = 10;

/// One guarded path, as `symlink_metadata` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// A file's bytes and permissions: a hook written back with the
    /// default mode is not executable on Unix, and would be put back only
    /// in name; and a mode change alone can enable a hook.
    File(Vec<u8>, std::fs::Permissions),
    /// A link, by its own text. `dir` is whether Windows made it as a
    /// directory link, which is how it is removed and made again there.
    Link { target: PathBuf, dir: bool },
    /// A directory where one file is guarded (`config.worktree` made a
    /// directory). The hooks directory's own directories are walked.
    Dir,
}

pub struct GitGuard {
    /// Canonical, for the labels.
    root: PathBuf,
    /// The git directory and the common one, canonical at capture: nothing
    /// outside them is written or removed.
    bases: Vec<PathBuf>,
    /// The hooks directory, when it is inside one of `bases`.
    hooks: Option<PathBuf>,
    /// Every guarded path outside the hooks directory, as it was at
    /// capture, or `None` for one that did not exist: a `config.worktree`
    /// the writer creates is removed as a planted hook is.
    fixed: Vec<(PathBuf, Option<Entry>)>,
    /// What the hooks directory held at capture, links unentered.
    hook_files: Vec<(PathBuf, Entry)>,
}

impl GitGuard {
    /// Reads every guarded file. Before the writer runs: nothing can be put
    /// back that was not read.
    pub fn capture(root: &Path) -> Result<GitGuard, String> {
        let mut git = crate::command_with_hooks();
        git.args([
            "rev-parse",
            "--git-dir",
            "--git-common-dir",
            "--git-path",
            "config",
            "--git-path",
            "config.worktree",
            "--git-path",
            "hooks",
            "--git-path",
            "info/exclude",
            "--git-path",
            "info/attributes",
        ])
        .current_dir(root)
        .stdin(Stdio::null());
        let o = git
            .output()
            .map_err(|e| format!("git rev-parse could not run: {e}"))?;
        let text = String::from_utf8_lossy(&o.stdout);
        let paths: Vec<PathBuf> = text.lines().map(|l| root.join(l.trim())).collect();
        let [
            git_dir,
            common,
            config,
            worktree_config,
            hooks,
            exclude,
            attributes,
        ] = <[PathBuf; 7]>::try_from(paths).map_err(|_| {
            format!(
                "git rev-parse --git-path failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )
        })?;
        let root_real = std::fs::canonicalize(root)
            .map_err(|e| format!("{} could not be resolved: {e}", root.display()))?;
        let mut bases = Vec::new();
        for d in [&git_dir, &common] {
            let b = std::fs::canonicalize(d)
                .map_err(|e| format!("{} could not be resolved: {e}", d.display()))?;
            if !bases.contains(&b) {
                bases.push(b);
            }
        }
        let inside = |p: &Path| locate(p).filter(|l| bases.iter().any(|b| l.starts_with(b)));
        let mut fixed_paths: Vec<PathBuf> = Vec::new();
        let standard = [config.clone(), worktree_config.clone(), exclude, attributes];
        for p in standard
            .into_iter()
            .chain(includes(&[config, worktree_config])?)
        {
            if let Some(l) = inside(&p)
                && !fixed_paths.contains(&l)
            {
                fixed_paths.push(l);
            }
        }
        let mut fixed = Vec::new();
        for p in fixed_paths {
            let e = read_entry(&p)?;
            fixed.push((p, e));
        }
        let hooks = inside(&hooks);
        let hook_files = match &hooks {
            Some(h) => read_tree(h)?,
            None => Vec::new(),
        };
        Ok(GitGuard {
            root: root_real,
            bases,
            hooks,
            fixed,
            hook_files,
        })
    }

    fn label(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .display()
            .to_string()
            .replace('\\', "/")
    }

    /// What differs from the capture, each put back: a changed or deleted
    /// file gets its bytes again, a new file or link is removed (a link as
    /// a link), a changed link is made again as a link. Returns the paths
    /// that differed, and any that could not be put back. Call it before
    /// any other git command once the writer has finished: `core.fsmonitor`
    /// runs at the next `git status`, a hook at the next `git commit`.
    ///
    /// What it cannot close, stated: each path is checked and then acted
    /// on, and a process still running (a process agent's grandchild,
    /// ADR 0026 §11) could swap a directory for a link in between. Closing
    /// that needs `openat`-relative calls, which std does not offer.
    pub fn restore(&self) -> (Vec<String>, Vec<String>) {
        let (mut differed, mut failed) = (Vec::new(), Vec::new());
        for (p, was) in &self.fixed {
            let now = match read_entry(p) {
                Ok(n) => n,
                Err(e) => {
                    differed.push(self.label(p));
                    failed.push(e);
                    continue;
                }
            };
            if &now == was {
                continue;
            }
            differed.push(self.label(p));
            if let Err(e) = self.put(p, now.as_ref(), was.as_ref()) {
                failed.push(format!("{}: {e}", self.label(p)));
            }
        }
        let Some(hooks) = &self.hooks else {
            return (differed, failed);
        };
        let now = match read_tree(hooks) {
            Ok(n) => n,
            Err(e) => {
                differed.push(self.label(hooks));
                failed.push(e);
                return (differed, failed);
            }
        };
        // What is new goes first, deepest first: a file or link planted
        // where a captured hook's directory belongs is out of the way
        // before that hook is written back.
        for (p, e) in now.iter().rev() {
            if !self.hook_files.iter().any(|(q, _)| q == p) {
                differed.push(self.label(p));
                if let Err(err) = self.put(p, Some(e), None) {
                    failed.push(format!("{}: {err}", self.label(p)));
                }
            }
        }
        for (p, was) in &self.hook_files {
            let cur = now.iter().find(|(q, _)| q == p).map(|(_, e)| e);
            if cur != Some(was) {
                differed.push(self.label(p));
                if let Err(e) = self.put(p, cur, Some(was)) {
                    failed.push(format!("{}: {e}", self.label(p)));
                }
            }
        }
        differed.sort();
        differed.dedup();
        (differed, failed)
    }

    /// Takes `p` from `now` back to `was`, never outside the git directory
    /// and never through a link.
    fn put(&self, p: &Path, now: Option<&Entry>, was: Option<&Entry>) -> std::io::Result<()> {
        let base = self
            .bases
            .iter()
            .find(|b| p.starts_with(b))
            .ok_or_else(|| std::io::Error::other("outside the git directory; not touched"))?;
        dirs_to(base, p, was.is_some())?;
        match now {
            None => {}
            Some(Entry::Link { .. }) => remove_link(p)?,
            Some(Entry::File(..)) => std::fs::remove_file(p)?,
            // Only an empty one: what was in it is not the guard's to judge.
            Some(Entry::Dir) => std::fs::remove_dir(p)?,
        }
        match was {
            None => Ok(()),
            Some(Entry::File(bytes, perms)) => {
                use std::io::Write;
                // `create_new` fails on anything at `p`, a link included,
                // so the bytes land in a new file and nowhere else.
                let mut f = std::fs::File::options()
                    .write(true)
                    .create_new(true)
                    .open(p)?;
                f.write_all(bytes)?;
                drop(f);
                std::fs::set_permissions(p, perms.clone())
            }
            Some(Entry::Link { target, dir }) => make_link(target, p, *dir),
            Some(Entry::Dir) => std::fs::create_dir(p),
        }
    }
}

/// Checks every directory between `base` and `p`'s parent. A link there
/// was planted (at capture each was a directory, or the canonical path
/// would not run through it): when `create`, it is removed as a link and a
/// directory made in its place, as is any that is missing; otherwise
/// nothing is done under it. A file in the way is an error.
fn dirs_to(base: &Path, p: &Path, create: bool) -> std::io::Result<()> {
    let rel = p
        .parent()
        .and_then(|d| d.strip_prefix(base).ok())
        .ok_or_else(|| std::io::Error::other("outside the git directory; not touched"))?;
    let mut cur = base.to_path_buf();
    for c in rel.components() {
        let Component::Normal(n) = c else {
            return Err(std::io::Error::other("not a plain path; not touched"));
        };
        cur.push(n);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                if !create {
                    return Err(std::io::Error::other(format!(
                        "{} is a link; nothing under it is touched",
                        cur.display()
                    )));
                }
                remove_link(&cur)?;
                std::fs::create_dir(&cur)?;
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is a file where a directory was",
                    cur.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&cur)?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Removes the link at `p`, never what it points at: `remove_file` on
/// Unix; a Windows directory link or junction takes `remove_dir`, which
/// removes the link and would refuse a real directory that is not empty.
fn remove_link(p: &Path) -> std::io::Result<()> {
    std::fs::remove_file(p).or_else(|e| std::fs::remove_dir(p).map_err(|_| e))
}

fn make_link(target: &Path, p: &Path, dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = dir;
        std::os::unix::fs::symlink(target, p)
    }
    // Needs developer mode or the privilege; without it the link is
    // reported as not put back. A junction comes back as a directory link.
    #[cfg(windows)]
    if dir {
        std::os::windows::fs::symlink_dir(target, p)
    } else {
        std::os::windows::fs::symlink_file(target, p)
    }
}

/// What is at `p`, `None` when nothing is, `Err` when it cannot be read: a
/// guard that cannot read a file cannot put it back. Never through a link.
fn read_entry(p: &Path) -> Result<Option<Entry>, String> {
    let fail = |e: std::io::Error| format!("{} could not be read: {e}", p.display());
    let m = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(fail(e)),
    };
    if m.file_type().is_symlink() {
        #[cfg(windows)]
        let dir = std::os::windows::fs::FileTypeExt::is_symlink_dir(&m.file_type());
        #[cfg(not(windows))]
        let dir = false;
        let target = std::fs::read_link(p).map_err(fail)?;
        return Ok(Some(Entry::Link { target, dir }));
    }
    if m.is_dir() {
        return Ok(Some(Entry::Dir));
    }
    let bytes = std::fs::read(p).map_err(fail)?;
    Ok(Some(Entry::File(bytes, m.permissions())))
}

/// Every file and link under `dir`, sorted, links not entered. A missing
/// directory is empty; one that is itself a link is that one entry.
fn read_tree(dir: &Path) -> Result<Vec<(PathBuf, Entry)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        match read_entry(&d)? {
            None => {}
            Some(Entry::Dir) => {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    continue;
                };
                stack.extend(entries.filter_map(Result::ok).map(|e| e.path()));
            }
            Some(e) => out.push((d, e)),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Where `p` itself lives: its parent resolved through any link, its own
/// name not followed. Where the parent does not exist, the nearest
/// ancestor that does is resolved and the rest appended; `None` when that
/// rest is not plain names, since `..` there could lead anywhere.
fn locate(p: &Path) -> Option<PathBuf> {
    let mut tail = vec![p.file_name()?.to_owned()];
    let mut dir = p.parent()?;
    loop {
        if let Ok(real) = std::fs::canonicalize(dir) {
            let mut out = real;
            out.extend(tail.iter().rev());
            return Some(out);
        }
        match dir.components().next_back()? {
            Component::Normal(n) => tail.push(n.to_owned()),
            _ => return None,
        }
        dir = dir.parent()?;
    }
}

/// The file's bytes, `None` when it does not exist, for reading the
/// configs' includes (read through a link: reading is what git does too).
fn read_opt(p: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} could not be read: {e}", p.display())),
    }
}

/// Every file the configs name as an include, followed through includes
/// of includes, each resolved as git resolves it: `~/` against the home
/// directory, a relative path against the directory of the file naming it.
fn includes(configs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut frontier: Vec<(PathBuf, usize)> = configs.iter().map(|c| (c.clone(), 0)).collect();
    while let Some((file, depth)) = frontier.pop() {
        if depth >= MAX_INCLUDE_DEPTH {
            continue;
        }
        let Some(bytes) = read_opt(&file)? else {
            continue;
        };
        let base = file.parent().map(Path::to_path_buf).unwrap_or_default();
        for raw in include_paths(&String::from_utf8_lossy(&bytes)) {
            let target = resolve(&base, &raw);
            if !out.contains(&target) {
                out.push(target.clone());
                frontier.push((target, depth + 1));
            }
        }
    }
    Ok(out)
}

fn resolve(base: &Path, raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(h) = home {
            return PathBuf::from(h).join(rest);
        }
    }
    base.join(raw)
}

/// The `path` values of `[include]` and `[includeIf "…"]` sections, as a
/// config file writes them. Deliberately small: section names are
/// case-insensitive, a value may be quoted, and a `#` or `;` outside quotes
/// starts a comment. A spelling this misses makes the guard cover less,
/// never put back a file it did not read: the config that names the
/// include is itself guarded, whole.
fn include_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_include = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('[') {
            let name = rest
                .split([']', ' ', '"', '.'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            in_include = name == "include" || name == "includeif";
            // `[include] path = x` on one line.
            let Some(after) = rest.split_once(']').map(|(_, a)| a.trim()) else {
                continue;
            };
            if in_include && let Some(v) = path_value(after) {
                out.push(v);
            }
            continue;
        }
        if in_include && let Some(v) = path_value(t) {
            out.push(v);
        }
    }
    out
}

fn path_value(line: &str) -> Option<String> {
    let (k, v) = line.split_once('=')?;
    if !k.trim().eq_ignore_ascii_case("path") {
        return None;
    }
    let v = v.trim();
    let v = if let Some(q) = v.strip_prefix('"') {
        q.split('"').next().unwrap_or("")
    } else {
        v.split(['#', ';']).next().unwrap_or("").trim()
    };
    (!v.is_empty()).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn get(dir: &Path, key: &str) -> Option<String> {
        let o = Command::new("git")
            .args(["config", "--get", key])
            .current_dir(dir)
            .output()
            .unwrap();
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    #[test]
    fn config_hooks_and_a_created_worktree_config_are_put_back() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        let g = GitGuard::capture(root).unwrap();

        git(root, &["config", "core.fsmonitor", "./evil.sh"]);
        std::fs::write(root.join(".git/hooks/post-checkout"), "#!/bin/sh\n").unwrap();
        std::fs::write(root.join(".git/config.worktree"), "[core]\n").unwrap();
        let (differed, failed) = g.restore();

        assert!(failed.is_empty(), "{failed:?}");
        for p in [
            ".git/config",
            ".git/hooks/post-checkout",
            ".git/config.worktree",
        ] {
            assert!(differed.iter().any(|x| x == p), "{p} in {differed:?}");
        }
        assert_eq!(get(root, "core.fsmonitor"), None);
        assert!(!root.join(".git/hooks/post-checkout").exists());
        assert!(!root.join(".git/config.worktree").exists());
        assert_eq!(g.restore(), (vec![], vec![]), "nothing left to put back");
    }

    #[test]
    fn a_deleted_hook_comes_back_with_its_permissions() {
        // The risk: a hook the writer deleted was written back with the
        // default mode, so on Unix it was no longer executable and the
        // user's hook stopped running while the report said "put back".
        // Read-only stands in for the mode here, since it is the one
        // permission both platforms keep.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        let hook = root.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        let mut ro = std::fs::metadata(&hook).unwrap().permissions();
        ro.set_readonly(true);
        std::fs::set_permissions(&hook, ro).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let before = std::fs::metadata(&hook).unwrap().permissions();
        let g = GitGuard::capture(root).unwrap();

        let mut rw = before.clone();
        #[allow(clippy::permissions_set_readonly_false)]
        rw.set_readonly(false);
        std::fs::set_permissions(&hook, rw).unwrap();
        std::fs::remove_file(&hook).unwrap();
        let (differed, failed) = g.restore();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(differed, [".git/hooks/pre-commit"]);
        assert_eq!(std::fs::metadata(&hook).unwrap().permissions(), before);
        // Writable again, so the temporary directory can be removed.
        let mut rw = before;
        #[allow(clippy::permissions_set_readonly_false)]
        rw.set_readonly(false);
        std::fs::set_permissions(&hook, rw).unwrap();
    }

    #[test]
    fn a_file_the_config_includes_from_inside_git_is_guarded() {
        // The risk: `.git/config` is guarded whole, but a file it includes
        // is config too, and writing `core.fsmonitor` there leaves the
        // guarded file's bytes alone.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".git/extra.cfg"), "[user]\n\tname = t\n").unwrap();
        std::fs::write(root.join(".git/deeper.cfg"), "").unwrap();
        std::fs::write(
            root.join(".git/extra.cfg"),
            "[user]\n\tname = t\n[include]\n\tpath = deeper.cfg\n",
        )
        .unwrap();
        git(root, &["config", "includeIf.gitdir:/.path", "extra.cfg"]);
        let g = GitGuard::capture(root).unwrap();

        std::fs::write(
            root.join(".git/deeper.cfg"),
            "[core]\n\tfsmonitor = ./evil.sh\n",
        )
        .unwrap();
        let (differed, failed) = g.restore();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(differed, [".git/deeper.cfg"]);
        assert_eq!(std::fs::read(root.join(".git/deeper.cfg")).unwrap(), b"");
    }

    /// A link to a directory at `link`: a junction on Windows, which any
    /// user may create, and a symlink on Unix.
    fn dir_link(target: &Path, link: &Path) {
        #[cfg(windows)]
        {
            let ok = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                // `mklink` reads a `/` as the start of a switch.
                .arg(link.to_string_lossy().replace('/', "\\"))
                .arg(target.to_string_lossy().replace('/', "\\"))
                .stdout(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "mklink /J");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    /// A link to a file, or `None` where the platform refuses one (Windows
    /// without developer mode), in which case the test says so.
    fn file_link(target: &Path, link: &Path) -> Option<()> {
        #[cfg(windows)]
        let r = std::os::windows::fs::symlink_file(target, link);
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(target, link);
        match r {
            Ok(()) => Some(()),
            Err(e) => {
                eprintln!("skipped: cannot create a file symlink here: {e}");
                None
            }
        }
    }

    #[test]
    fn a_planted_directory_link_in_the_hooks_is_removed_and_its_target_is_untouched() {
        // The risk (review round 9, finding 1): the walk followed a link,
        // so `.git/hooks/lib -> $HOME` listed every file under it as a new
        // hook, and restore deleted them all. The link is the only thing
        // the writer planted; it is what goes.
        let d = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        std::fs::write(victim.path().join("precious.txt"), "keep").unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        let g = GitGuard::capture(root).unwrap();

        let link = root.join(".git/hooks/lib");
        dir_link(victim.path(), &link);
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "std reads the link as a link"
        );
        let (differed, failed) = g.restore();

        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(differed, [".git/hooks/lib"]);
        assert_eq!(
            std::fs::read(victim.path().join("precious.txt")).unwrap(),
            b"keep",
            "the link's target was touched"
        );
        assert!(std::fs::symlink_metadata(&link).is_err(), "link removed");
        assert_eq!(g.restore(), (vec![], vec![]), "nothing left to put back");
    }

    #[test]
    fn a_hooks_subdirectory_replaced_by_a_link_is_not_written_through() {
        // The risk: a hook at `.git/hooks/sub/x` was written back with
        // `create_dir_all` and `write`, both of which follow a link, so a
        // writer that replaced `sub` with a link to `$HOME` had the hook's
        // bytes written into `$HOME/x`.
        let d = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir(root.join(".git/hooks/sub")).unwrap();
        std::fs::write(root.join(".git/hooks/sub/x"), "hook").unwrap();
        let g = GitGuard::capture(root).unwrap();

        std::fs::remove_dir_all(root.join(".git/hooks/sub")).unwrap();
        dir_link(victim.path(), &root.join(".git/hooks/sub"));
        let (_, failed) = g.restore();

        assert!(failed.is_empty(), "{failed:?}");
        assert!(
            !victim.path().join("x").exists(),
            "the hook was written through the link"
        );
        let sub = std::fs::symlink_metadata(root.join(".git/hooks/sub")).unwrap();
        assert!(sub.is_dir() && !sub.file_type().is_symlink());
        assert_eq!(
            std::fs::read(root.join(".git/hooks/sub/x")).unwrap(),
            b"hook"
        );
    }

    #[test]
    fn a_hook_replaced_by_a_file_link_comes_back_as_a_file_and_the_target_is_untouched() {
        // The risk: `write` follows a link, so a hook replaced by a link to
        // `~/.bashrc` had the hook's bytes written over `~/.bashrc`; and a
        // config replaced the same way likewise.
        let d = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        let target = victim.path().join("bashrc");
        std::fs::write(&target, "mine").unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        let hook = root.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "hook").unwrap();
        let config = root.join(".git/config");
        let config_bytes = std::fs::read(&config).unwrap();
        let g = GitGuard::capture(root).unwrap();

        std::fs::remove_file(&hook).unwrap();
        if file_link(&target, &hook).is_none() {
            return;
        }
        std::fs::remove_file(&config).unwrap();
        file_link(&target, &config).unwrap();
        let (_, failed) = g.restore();

        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"mine", "written through");
        for (p, want) in [(&hook, &b"hook"[..]), (&config, &config_bytes[..])] {
            let m = std::fs::symlink_metadata(p).unwrap();
            assert!(
                !m.file_type().is_symlink(),
                "{} is still a link",
                p.display()
            );
            assert_eq!(std::fs::read(p).unwrap(), want);
        }
    }

    #[test]
    fn a_hook_that_was_a_link_at_capture_is_compared_as_a_link_and_never_written_through() {
        // The risk: a hook the user keeps as a link into the worktree
        // (`pre-commit -> ../../scripts/pc`) was read through the link, so
        // an edit to `scripts/pc` looked like a changed hook and restore
        // wrote the captured bytes back through the link, outside `.git/`.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts/pc"), "v1").unwrap();
        let hook = root.join(".git/hooks/pre-commit");
        if file_link(&root.join("scripts/pc"), &hook).is_none() {
            return;
        }
        let g = GitGuard::capture(root).unwrap();

        std::fs::write(root.join("scripts/pc"), "v2").unwrap();
        assert_eq!(g.restore(), (vec![], vec![]), "the link did not change");
        assert_eq!(std::fs::read(root.join("scripts/pc")).unwrap(), b"v2");
    }

    #[test]
    fn a_hooks_directory_outside_the_git_directory_is_not_touched() {
        // `core.hooksPath` pointing into the worktree (husky's `.husky`) or
        // elsewhere names a directory that is not `.git/`'s: its tracked
        // files are the rollback's, and anything else there is not the
        // guard's to delete.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("hooks-dir")).unwrap();
        git(root, &["config", "core.hooksPath", "hooks-dir"]);
        let g = GitGuard::capture(root).unwrap();

        std::fs::write(root.join("hooks-dir/pre-commit"), "x").unwrap();
        assert_eq!(g.restore(), (vec![], vec![]));
        assert!(root.join("hooks-dir/pre-commit").exists());
    }

    #[test]
    fn include_paths_reads_the_shapes_config_files_use() {
        let text = "[core]\n\tpath = no\n[include]\n\tpath = a.cfg\n\
                    [includeIf \"gitdir:~/w/\"]\n\tPath = \"b c.cfg\" ; note\n\
                    [Include] path = d.cfg # c\n[user]\n\tpath = no\n";
        assert_eq!(include_paths(text), ["a.cfg", "b c.cfg", "d.cfg"]);
    }
}
