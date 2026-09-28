//! Path rules every crate that reads repository content shares.
//!
//! Here rather than in the loop because the binary reads repository files
//! into the prompt too (context files, system files, prompt templates,
//! profile instructions, the vault), and two copies of "is this inside the
//! repository" drift. This crate depends on nothing in the workspace, so
//! every crate can use it.

use std::path::{Path, PathBuf};

/// Why `rel` cannot be used as a path inside the repository, or `None`.
///
/// `is_absolute()` alone is not the rule: on Windows `/x` and `\x` are
/// rooted but not absolute, `C:x` is relative to drive C's current
/// directory, and `root.join` of any of them leaves the repository. So the
/// rule is on the string, the same on every platform:
///
/// - no leading `/` or `\` (rooted, UNC, `\\?\`), and no drive prefix `X:`;
/// - no `:` anywhere: on NTFS `a.py:s` is an alternate data stream, a write
///   git cannot see and a rollback cannot remove;
/// - no component that is only dots and spaces other than `.`, compared
///   after the trailing dots and spaces Windows strips, so `..`, `.. ` and
///   `...` are all parent references here;
/// - and, belt and braces, no `RootDir` or `Prefix` component as this host
///   parses it.
///
/// Components are split on both `/` and `\`, since Windows accepts either.
pub fn unsafe_repo_path(rel: &str) -> Option<String> {
    let refuse = || {
        Some(format!(
            "path `{rel}` must be relative to the repository root, with no leading \
             `/` or `\\`, no drive letter or `:`, and no `..`"
        ))
    };
    if rel.starts_with(['/', '\\']) || rel.contains(':') {
        return refuse();
    }
    let parentish = |seg: &str| {
        let t = seg.trim_end_matches(['.', ' ']);
        t.is_empty() && seg != "." && !seg.is_empty()
    };
    if rel.split(['/', '\\']).any(parentish) {
        return refuse();
    }
    let p = Path::new(rel);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::ParentDir
            )
        })
    {
        return refuse();
    }
    None
}

/// Whether any component of `rel` names git's own directory, by any
/// spelling a filesystem this runs on would resolve to it: compared
/// case-insensitively (NTFS, APFS), after the trailing dots and spaces
/// Windows strips (`.git.`, `.git `), and including the 8.3 short name
/// `git~1` — the list git itself checks since CVE-2014-9390. Every
/// component, so `app/.GIT/x` is caught as well: a nested `.git` is a
/// repository git would honour.
pub fn names_git_dir(rel: &str) -> bool {
    rel.split(['/', '\\']).any(|seg| {
        let s = seg.trim_end_matches(['.', ' ']).to_ascii_lowercase();
        s == ".git" || s == "git~1"
    })
}

/// Where a repository-supplied prompt input really is, or why it may not be
/// read.
///
/// A repository can commit `AGENTS.md -> ~/.aws/credentials` or
/// `CLAUDE.md -> .git/config`, and a reader that follows the link puts that
/// file into every hosted request body. So everything a repository supplies
/// for the prompt — context files, `.trellis/system.md`, skill bodies,
/// prompt templates, profile instructions, `context.md`, the vault — is
/// resolved here first, and read from the path this returns:
///
/// - `Ok(None)`: no entry at `path` at all, not even a link. Absence is the
///   caller's to judge.
/// - `Ok(Some(real))`: the canonical path, inside the canonical `root` and
///   not inside `.git` by any spelling ([`names_git_dir`]). Read `real`,
///   not `path`, so what was checked is what is read.
/// - `Err(why)`: a link (anywhere along the path) that resolves outside the
///   repository, into `.git`, or to nothing. `why` names the path as the
///   repository spells it and where it lands, never its contents.
///
/// `.git` is refused whether reached through a link or named directly: a
/// setting such as `[context] vault = ".git"` is the same leak.
///
/// Presence is decided by `symlink_metadata`, which does not follow links,
/// so a dangling link is refused with a reason rather than read as absent.
/// Whether the target is a file or a directory is the caller's to check.
pub fn resolve_repo_input(root: &Path, path: &Path) -> Result<Option<PathBuf>, String> {
    let shown = shown(root, path);
    if std::fs::symlink_metadata(path).is_err() {
        // Nothing at the path itself. A missing parent is absence too; a
        // parent that is a dangling link makes `symlink_metadata` fail the
        // same way, and nothing can be read through it either.
        return Ok(None);
    }
    let real_root = std::fs::canonicalize(root).map_err(|e| {
        format!(
            "`{shown}` was not read: the repository root {} could not be resolved ({e})",
            root.display()
        )
    })?;
    // Failing here with an entry present is a link to nothing, in practice;
    // whatever else it is, what cannot be resolved cannot be checked.
    let real = std::fs::canonicalize(path).map_err(|e| {
        format!(
            "`{shown}` is a link that resolves to nothing ({e}), so it was not read. Replace \
             the link with the file, or remove it"
        )
    })?;
    let Ok(inside) = real.strip_prefix(&real_root) else {
        return Err(format!(
            "`{shown}` resolves to {}, outside the repository, so it was not read: a \
             repository's prompt inputs are its own files. Replace the link with the file, \
             or remove it",
            plain(&real)
        ));
    };
    if names_git_dir(&inside.to_string_lossy()) {
        return Err(format!(
            "`{shown}` resolves into .git ({}), so it was not read: git's directory can \
             hold credentials, and a prompt input is sent to the model",
            inside.display()
        ));
    }
    Ok(Some(real))
}

/// Creates a symbolic link at `link` pointing to `target`, for tests in
/// every crate that defends against links. `Err` says why none could be
/// made: on Windows without Developer Mode or elevation, creating one needs
/// a privilege (os error 1314), and a test that cannot make its link skips
/// with that reason rather than passing on nothing.
#[doc(hidden)]
pub fn symlink_for_test(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let r = {
        // Windows resolves a link's text itself, and `/` in a relative
        // target is not a separator to it: `docs/a.md` would dangle.
        let target = PathBuf::from(target.to_string_lossy().replace('/', "\\"));
        let target = target.as_path();
        // A relative target is resolved against the link's directory, as
        // the OS will; the kind of link must match what it points at.
        let resolved = link
            .parent()
            .map(|p| p.join(target))
            .unwrap_or_else(|| target.to_path_buf());
        if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    };
    r.map_err(|e| {
        format!(
            "skipped: a symbolic link could not be created on this host ({e}); on Windows \
             this needs Developer Mode or an elevated process"
        )
    })
}

/// A canonical path as a person writes it: without the `\\?\` prefix
/// Windows' canonical form carries.
fn plain(p: &Path) -> String {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
    }
}

/// `path` relative to `root` for a message, `/`-separated; the whole path
/// when it is not under `root` as written.
fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scratch(name: &str) -> Scratch {
        let d =
            std::env::temp_dir().join(format!("trellis-core-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }

    #[test]
    fn a_plain_file_resolves_and_an_absent_one_is_none() {
        let d = scratch("plain");
        std::fs::write(d.0.join("AGENTS.md"), "x").unwrap();
        let got = resolve_repo_input(&d.0, &d.0.join("AGENTS.md")).unwrap();
        assert!(got.is_some_and(|p| p.ends_with("AGENTS.md")));
        assert_eq!(resolve_repo_input(&d.0, &d.0.join("CLAUDE.md")), Ok(None));
    }

    #[test]
    fn a_path_inside_git_is_refused_without_any_link() {
        // `[context] vault = ".git"` needs no link to be the same leak.
        let d = scratch("dotgit");
        std::fs::create_dir_all(d.0.join(".git")).unwrap();
        std::fs::write(d.0.join(".git/config"), "[remote]").unwrap();
        let e = resolve_repo_input(&d.0, &d.0.join(".git/config")).unwrap_err();
        assert!(e.contains(".git"), "{e}");
    }

    #[test]
    fn unsafe_repo_path_refuses_drive_relative_and_streams() {
        for bad in ["C:x", "a:b", "/x", "\\x", "..", "a/../b", "...", ".. "] {
            assert!(unsafe_repo_path(bad).is_some(), "`{bad}` must be refused");
        }
        for ok in ["a", "a/b.md", "./a", "a.b"] {
            assert!(unsafe_repo_path(ok).is_none(), "`{ok}` is ordinary");
        }
    }
}
