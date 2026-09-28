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

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How deep includes are followed. Git's own limit is 10.
const MAX_INCLUDE_DEPTH: usize = 10;

pub struct GitGuard {
    root: PathBuf,
    hooks: PathBuf,
    /// Every guarded path outside the hooks directory, with its bytes at
    /// capture, or `None` for one that did not exist: a `config.worktree`
    /// the writer creates is removed as a planted hook is.
    fixed: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// The hooks directory's files at capture.
    hook_files: Vec<(PathBuf, Vec<u8>)>,
    /// Their permissions at capture: a hook written back with the default
    /// mode is not executable on Unix, and would be put back only in name.
    hook_modes: Vec<(PathBuf, std::fs::Permissions)>,
}

impl GitGuard {
    /// Reads every guarded file. Before the writer runs: nothing can be put
    /// back that was not read.
    pub fn capture(root: &Path) -> Result<GitGuard, String> {
        let mut git = Command::new("git");
        trellis_core::secrets::scrub(&mut git);
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
        let mut fixed_paths = vec![config.clone(), worktree_config.clone(), exclude, attributes];
        let under = [real(&git_dir), real(&common)];
        for target in includes(&[config, worktree_config])? {
            let r = real(&target);
            if under.iter().any(|g| r.starts_with(g)) && !fixed_paths.contains(&target) {
                fixed_paths.push(target);
            }
        }
        let mut fixed = Vec::new();
        for p in fixed_paths {
            let bytes = read_opt(&p)?;
            fixed.push((p, bytes));
        }
        let hook_files = read_tree(&hooks)?;
        let hook_modes = hook_files
            .iter()
            .filter_map(|(p, _)| Some((p.clone(), std::fs::metadata(p).ok()?.permissions())))
            .collect();
        Ok(GitGuard {
            root: root.to_path_buf(),
            hooks,
            fixed,
            hook_files,
            hook_modes,
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
    /// file gets its bytes again, a new one is removed. Returns the paths
    /// that differed, and any that could not be put back. Call it before
    /// any other git command once the writer has finished: `core.fsmonitor`
    /// runs at the next `git status`, a hook at the next `git commit`.
    pub fn restore(&self) -> (Vec<String>, Vec<String>) {
        let (mut differed, mut failed) = (Vec::new(), Vec::new());
        for (p, was) in &self.fixed {
            let now = match read_opt(p) {
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
            let put = match was {
                Some(bytes) => std::fs::write(p, bytes),
                None => std::fs::remove_file(p),
            };
            if let Err(e) = put {
                failed.push(format!("{}: {e}", self.label(p)));
            }
        }
        let now = match read_tree(&self.hooks) {
            Ok(n) => n,
            Err(e) => {
                differed.push(self.label(&self.hooks));
                failed.push(e);
                return (differed, failed);
            }
        };
        for (p, bytes) in &self.hook_files {
            if now.iter().find(|(q, _)| q == p).map(|(_, b)| b) != Some(bytes) {
                differed.push(self.label(p));
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let mode = self.hook_modes.iter().find(|(q, _)| q == p);
                let put = std::fs::write(p, bytes).and_then(|()| match mode {
                    Some((_, m)) => std::fs::set_permissions(p, m.clone()),
                    None => Ok(()),
                });
                if let Err(e) = put {
                    failed.push(format!("{}: {e}", self.label(p)));
                }
            }
        }
        for (p, _) in &now {
            if !self.hook_files.iter().any(|(q, _)| q == p) {
                differed.push(self.label(p));
                if let Err(e) = std::fs::remove_file(p) {
                    failed.push(format!("{}: {e}", self.label(p)));
                }
            }
        }
        (differed, failed)
    }
}

/// The file's bytes, `None` when it does not exist, `Err` when it cannot
/// be read: a guard that cannot read a file cannot put it back.
fn read_opt(p: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} could not be read: {e}", p.display())),
    }
}

/// Every file under `dir`, with its bytes, sorted. A missing directory is
/// empty.
fn read_tree(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut paths = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                paths.push(p);
            }
        }
    }
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        if let Some(b) = read_opt(&p)? {
            out.push((p, b));
        }
    }
    Ok(out)
}

/// `p` through any link, or as written where it does not exist yet.
fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| {
        match (
            p.parent().and_then(|d| std::fs::canonicalize(d).ok()),
            p.file_name(),
        ) {
            (Some(d), Some(n)) => d.join(n),
            _ => p.to_path_buf(),
        }
    })
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

    #[test]
    fn include_paths_reads_the_shapes_config_files_use() {
        let text = "[core]\n\tpath = no\n[include]\n\tpath = a.cfg\n\
                    [includeIf \"gitdir:~/w/\"]\n\tPath = \"b c.cfg\" ; note\n\
                    [Include] path = d.cfg # c\n[user]\n\tpath = no\n";
        assert_eq!(include_paths(text), ["a.cfg", "b c.cfg", "d.cfg"]);
    }
}
