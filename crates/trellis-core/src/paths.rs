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
///   repository, into `.git`, or to nothing; or a path that could not be
///   looked at (permission denied, an I/O error), which is not absence.
///   `why` names the path as the repository spells it and where it lands,
///   never its contents.
///
/// `.git` is refused whether reached through a link or named directly: a
/// setting such as `[context] vault = ".git"` is the same leak.
///
/// Presence is decided by `symlink_metadata`, which does not follow links,
/// so a dangling link is refused with a reason rather than read as absent.
/// Only `NotFound` (or a parent that is a file) is absence.
/// Whether the target is a file or a directory is the caller's to check.
pub fn resolve_repo_input(root: &Path, path: &Path) -> Result<Option<PathBuf>, RepoInputError> {
    Ok(resolve_inside(root, path)?.map(|(real, _)| real))
}

/// Where `rel`, a path the model names that need not exist yet, really
/// lands: repository-relative and `/`-separated, through any link or
/// junction on the way. Its nearest entry that exists is resolved by
/// [`resolve_repo_input`]'s rule, and the part that does not exist yet is
/// appended as written; the whole is then held to [`names_git_dir`] too.
///
/// The one rule for a model-named path (#441): `read_file`, `ls`, `grep`
/// given a path and every writer ask this, and the whole-repository search
/// asks [`resolve_repo_input`] of each entry it finds, which is this same
/// rule on a path that exists. Before, the reads had a resolver of their
/// own, which walked past a link to nothing that the search refused.
///
/// `Err` as [`resolve_repo_input`]'s: outside the repository, into `.git`,
/// a link to nothing, a path that could not be looked at, or a root that
/// could not be resolved. Callers word it for their own reader.
pub fn resolve_repo_path(root: &Path, rel: &str) -> Result<String, RepoInputError> {
    let mut at = root.join(rel);
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Some((_, inside)) = resolve_inside(root, &at)? {
            let mut p = inside;
            for t in tail.iter().rev() {
                p.push(t);
            }
            let landed = p.to_string_lossy().replace('\\', "/");
            if names_git_dir(&landed) {
                return Err(RepoInputError::IntoGit {
                    shown: shown(root, &root.join(rel)),
                    inside: landed,
                });
            }
            return Ok(landed);
        }
        // Absent: try the directory it would be created in.
        match (at.file_name().map(|n| n.to_os_string()), at.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                at = parent.to_path_buf();
            }
            // Nothing exists above it, not even the root.
            _ => {
                return Err(RepoInputError::RootUnresolved {
                    shown: shown(root, &root.join(rel)),
                    root: root.display().to_string(),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                });
            }
        }
    }
}

/// [`resolve_repo_input`], with the canonical path's part under the
/// canonical root alongside it.
fn resolve_inside(root: &Path, path: &Path) -> Result<Option<(PathBuf, PathBuf)>, RepoInputError> {
    let shown = shown(root, path);
    if !present(std::fs::symlink_metadata(path).map(|_| ()), &shown)? {
        return Ok(None);
    }
    let real_root =
        std::fs::canonicalize(root).map_err(|source| RepoInputError::RootUnresolved {
            shown: shown.clone(),
            root: root.display().to_string(),
            source,
        })?;
    // Failing here with an entry present is a link to nothing, in practice;
    // whatever else it is, what cannot be resolved cannot be checked.
    let real = std::fs::canonicalize(path).map_err(|source| RepoInputError::Dangling {
        shown: shown.clone(),
        source,
    })?;
    let Ok(inside) = real.strip_prefix(&real_root) else {
        return Err(RepoInputError::Outside {
            shown,
            real: plain(&real),
        });
    };
    if names_git_dir(&inside.to_string_lossy()) {
        return Err(RepoInputError::IntoGit {
            shown,
            inside: inside.display().to_string(),
        });
    }
    let inside = inside.to_path_buf();
    Ok(Some((real, inside)))
}

/// Why [`resolve_repo_input`] refused a path. Each message names the path as
/// the repository spells it and where it lands, never its contents; the
/// texts are the ones it returned as strings before (review round 11).
#[derive(Debug, thiserror::Error)]
pub enum RepoInputError {
    /// Something is, or may be, at the path, and it could not be looked
    /// at: permission denied on it or a parent, an I/O error. Only
    /// `NotFound` (and a parent that is a file) is absence.
    #[error("`{shown}` was not read: it could not be looked at ({source})")]
    Uninspectable {
        shown: String,
        source: std::io::Error,
    },
    #[error("`{shown}` was not read: the repository root {root} could not be resolved ({source})")]
    RootUnresolved {
        shown: String,
        root: String,
        source: std::io::Error,
    },
    #[error(
        "`{shown}` is a link that resolves to nothing ({source}), so it was not read. Replace \
         the link with the file, or remove it"
    )]
    Dangling {
        shown: String,
        source: std::io::Error,
    },
    #[error(
        "`{shown}` resolves to {real}, outside the repository, so it was not read: a \
         repository's prompt inputs are its own files. Replace the link with the file, \
         or remove it"
    )]
    Outside { shown: String, real: String },
    #[error(
        "`{shown}` resolves into .git ({inside}), so it was not read: git's directory can \
         hold credentials, and a prompt input is sent to the model"
    )]
    IntoGit { shown: String, inside: String },
}

/// Whether an error from looking at a path means nothing is there:
/// `NotFound`, or `NotADirectory` for a parent that is a file (Unix
/// reports ENOTDIR; Windows reports not found). Any other error (permission
/// denied, an I/O error) is an entry that cannot be looked at, not an
/// absent one. The one classifier for that rule, so every caller that
/// decides "is a prompt input there" agrees (review round 12).
pub fn is_absence(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Whether an entry is at the path, from what `symlink_metadata` returned.
///
/// `NotFound` is absence, and so is `NotADirectory`: a parent that is a
/// file (Unix reports ENOTDIR; Windows reports not found). Every other
/// error refuses, naming the path and the reason. Before review round 11
/// every error read as absence, so a context file behind a directory the
/// process may not read was dropped from the prompt in silence, and a run
/// went ahead without instructions the repository wrote.
fn present(meta: std::io::Result<()>, shown: &str) -> Result<bool, RepoInputError> {
    match meta {
        Ok(()) => Ok(true),
        Err(e) if is_absence(&e) => Ok(false),
        Err(source) => Err(RepoInputError::Uninspectable {
            shown: shown.to_string(),
            source,
        }),
    }
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
        assert!(matches!(
            resolve_repo_input(&d.0, &d.0.join("CLAUDE.md")),
            Ok(None)
        ));
        // A parent that is a file is absence too, not a refusal.
        assert!(matches!(
            resolve_repo_input(&d.0, &d.0.join("AGENTS.md/x")),
            Ok(None)
        ));
    }

    #[test]
    fn a_path_inside_git_is_refused_without_any_link() {
        // `[context] vault = ".git"` needs no link to be the same leak.
        let d = scratch("dotgit");
        std::fs::create_dir_all(d.0.join(".git")).unwrap();
        std::fs::write(d.0.join(".git/config"), "[remote]").unwrap();
        let e = resolve_repo_input(&d.0, &d.0.join(".git/config"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(".git"), "{e}");
    }

    #[test]
    fn only_not_found_is_absence_and_any_other_error_refuses_with_the_path() {
        // The risk (review round 11): every `symlink_metadata` error was
        // `Ok(None)`, so an `AGENTS.md` behind a directory the process may
        // not read was absent, and the run went ahead without it in
        // silence. Permission denied cannot be staged portably (Windows
        // needs an ACL, and a Unix root user reads anything), so the
        // decision is tested at its seam, with the errors the OS returns.
        use std::io::{Error, ErrorKind};
        assert!(!present(Err(Error::from(ErrorKind::NotFound)), "AGENTS.md").unwrap());
        assert!(!present(Err(Error::from(ErrorKind::NotADirectory)), "AGENTS.md").unwrap());
        assert!(present(Ok(()), "AGENTS.md").unwrap());
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::Other,
            ErrorKind::InvalidFilename,
        ] {
            let e = present(Err(Error::new(kind, "the reason")), "docs/AGENTS.md")
                .expect_err("not absence")
                .to_string();
            assert!(e.contains("`docs/AGENTS.md`"), "names the path: {e}");
            assert!(e.contains("the reason"), "names why: {e}");
        }
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

    #[test]
    fn a_path_not_yet_made_is_judged_where_it_would_land() {
        // The risk (#441): the reads' own resolver passed every path when
        // the root could not be resolved, and a writer that asks without
        // the spelling check first must still see a new file under a
        // nested `.git` refused.
        let d = scratch("landing");
        std::fs::create_dir_all(d.0.join("src")).unwrap();
        assert_eq!(
            resolve_repo_path(&d.0, "src/new/a.py").unwrap(),
            "src/new/a.py"
        );
        assert!(matches!(
            resolve_repo_path(&d.0, "src/.git/new"),
            Err(RepoInputError::IntoGit { .. })
        ));
        assert!(matches!(
            resolve_repo_path(&d.0.join("no-such-root"), "a"),
            Err(RepoInputError::RootUnresolved { .. })
        ));
    }
}
