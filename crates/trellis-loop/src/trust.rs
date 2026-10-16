//! The trust store (ADR 0023 §4).
//!
//! Cloning a repository and running trellis in it must not execute code the
//! repository supplied until a person has said so. The record of that lives
//! **outside** the repository, in the user's configuration directory, so a
//! repository cannot trust itself; and it is a digest over everything that
//! would run, so changing any of it (a hook's argv, a script a hook names, a
//! custom tool's schema, the bash settings) makes the repository untrusted
//! again.
//!
//! What trust does not cover is said aloud in the ADR: `[checks]` commands
//! have always run without it.

use crate::hooks::HookConfig;
use crate::registry::CustomTool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One thing that would execute, as shown by `trellis trust`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustItem {
    /// e.g. `hook pre_tool`, `custom tool lint`, `bash`.
    pub label: String,
    /// The argv, when the item is a command.
    pub argv: Vec<String>,
    /// Anything else that decides what runs: a custom tool's description and
    /// schema, the bash settings.
    pub detail: String,
    /// Repository files the argv names, with their sha256.
    pub files: Vec<(String, String)>,
    /// Variables from the user's environment handed to the command, by
    /// name: a process agent's `pass_env` (ADR 0026 §11). Consent to run a
    /// command is not consent to give it a key, so `trellis trust` shows
    /// these on their own line. Left out of the digest's JSON when empty, so
    /// an item that receives nothing hashes as it did before this field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub receives: Vec<String>,
}

impl TrustItem {
    /// A command, with every argument that names a file inside `root`
    /// hashed. An argument is taken to name a file when it is a relative
    /// path with no `..`, or an absolute path under `root`, and a regular
    /// file is there. A program found on `PATH` is not repository code and is
    /// not hashed; a script the repository ships is.
    ///
    /// Every argument is also split into shell-ish tokens (whitespace,
    /// `; & | ( ) < > =`, quotes trimmed) and every token that is a file in
    /// the repository is hashed, whatever flag precedes it:
    /// `["bash", "-lc", "python .trellis/hooks/gate.py"]` and `--file=x.py`
    /// cover their scripts. For a hook or custom tool, a file found this way
    /// outside `.trellis/` makes the fingerprint refused (`outside_code`,
    /// review round 11), as does any token that resolves, canonically, to
    /// such a file even where this hashing does not take it (`..`, a
    /// differently spelled absolute path; review round 12). Code reached any other way — an import,
    /// `python -m pkg.mod`, a file a script reads — is covered only if it
    /// lives under `.trellis/`, which [`Fingerprint`] hashes whole (ADR
    /// 0023 §4); keep hook and tool code in `.trellis/hooks/`.
    pub fn command(root: &Path, label: String, argv: &[String], detail: String) -> Self {
        Self::command_cached(root, label, argv, detail, &HashCache::default())
    }

    /// [`command`](Self::command), hashing through `cache`: a file whose
    /// stat is unchanged since `cache` last hashed it is not read again.
    /// For [`TrustGuard`]'s rechecks only; a trust decision hashes afresh.
    pub fn command_cached(
        root: &Path,
        label: String,
        argv: &[String],
        detail: String,
        cache: &HashCache,
    ) -> Self {
        let mut files: Vec<(String, String)> = Vec::new();
        let mut add = |candidate: &str| {
            if let Some(f) = repo_file(root, candidate, cache)
                && !files.iter().any(|(p, _)| *p == f.0)
            {
                files.push(f);
            }
        };
        // Every element, and every token of every element: which flag
        // introduces code is the interpreter's business (`-c`, `-lc`,
        // `-ec`, `-Command`, `--file=`), and a list of them is a list of
        // the ones not yet missed (review round 10). Splitting where no
        // code is only adds hashes of files that exist, the safe direction.
        for tok in argv_candidates(argv) {
            add(tok);
        }
        Self {
            label,
            argv: argv.to_vec(),
            detail,
            files,
            receives: Vec::new(),
        }
    }
}

/// Every argv element, then every shell-ish token of it (split on
/// whitespace and `; & | ( ) < > =`, quotes trimmed). The one tokenizer for
/// hashing ([`TrustItem::command_cached`]) and for the refusal
/// ([`outside_code`]), so the two cannot drift apart.
fn argv_candidates(argv: &[String]) -> impl Iterator<Item = &str> {
    argv.iter().flat_map(|arg| {
        std::iter::once(arg.as_str()).chain(
            arg.split(|c: char| {
                c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>' | '=')
            })
            .map(|tok| tok.trim_matches(|c| matches!(c, '"' | '\'' | '`'))),
        )
    })
}

/// `(repository-relative path, sha256)` when `arg` names a regular file
/// inside `root`: a relative path with no `..`, or an absolute one under
/// `root`. A program found on `PATH` is not repository code and is not
/// hashed; a script the repository ships is.
fn repo_file(root: &Path, arg: &str, cache: &HashCache) -> Option<(String, String)> {
    let p = Path::new(arg);
    let full = if p.is_absolute() {
        if !p.starts_with(root) {
            return None;
        }
        p.to_path_buf()
    } else {
        if arg.is_empty() || p.components().any(|c| c.as_os_str() == "..") {
            return None;
        }
        root.join(p)
    };
    if !full.is_file() {
        return None;
    }
    Some((rel(root, &full), cache.digest(&full)?))
}

/// Control characters shown as escapes, the text kept on one line: the
/// same rule as `plain` in trellis-cli. Labels and paths in a refusal come
/// from the repository and are printed indented under `state:`, so a
/// newline in one could add a line that reads as trellis's own, and an
/// escape sequence could repaint one (#192). Applied where a message is
/// assembled, not to the paths the digest keys on, so a digest over text
/// with no control characters is unchanged. A `.trellis/` refusal is also
/// kept in that item's `detail`, so a refused fingerprint whose refusal
/// holds a control character hashes differently than before; a refused
/// fingerprint is never trusted or compared, so no trust decision moves.
fn plain(s: &str) -> String {
    trellis_core::text::escape_controls(s)
}

fn rel(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

/// Every file under `.trellis/`, sorted, with its sha256 — except exactly
/// what trellis itself writes there, which would otherwise make a repository
/// untrusted after each run:
/// - `index.bin`, the index cache
/// - `.gitignore`, while it is byte for byte the one trellis writes
///   (line endings aside); one a person edited is theirs and is covered
/// - `runs/<name>.json`, the trails: regular files directly under `runs/`
/// - `replays/<name>.json`, the replay recordings, and `replays/.gitignore`
///   while it is the one trellis writes (ADR 0023 §4, amended 2026-10-12).
///   The item's shown text below does not name them: it is part of the
///   digest, and changing it would untrust every repository trusted before.
/// - a linked worktree registered in this repository, strictly under
///   `work/` and reached there with no link on the way: the task
///   worktrees `trellis work` keeps (ADR 0053). A checkout of the
///   repository, not code trusted under `.trellis/`; registration is
///   checked both ways (ADR 0023 §4, amended 2026-10-13)
///
/// Nothing is skipped by prefix: `index.bin.py` and `runs/gate.py` are
/// hashed. Hook and tool code kept under `.trellis/` is covered by this
/// whatever reaches it: an import, `-m`, a file a script reads.
///
/// Symbolic links are followed. The returned `Err` names a link that cannot
/// be: one resolving outside the repository or to nothing (hashing what it
/// points at could mean a whole disk, and leaving it out would leave code
/// uncovered); one resolving to `.trellis/` itself or an ancestor of it (the
/// walk would take in the whole repository, `.git/` included, and every
/// commit would untrust it); and one resolving into `.git/`, whose files git
/// rewrites on its own.
///
/// Entries are walked in name order, and each file is recorded under its
/// real path relative to the repository, whichever alias reached it first:
/// the digest depends on what is on disk, not on directory order. The skips
/// above are decided on the same real path, relative to the real
/// `.trellis/`.
///
/// Bytecode (`*.pyc`, `__pycache__/`) is covered like any other file: a
/// sourceless `gate.pyc` is importable, and an unchecked-hash pyc is loaded
/// without being compared to its source. Trellis starts the commands it
/// runs with `PYTHONDONTWRITEBYTECODE=1`, so its own hook runs write none
/// (ADR 0023 §4).
fn dot_trellis_files(root: &Path, cache: &HashCache) -> Result<Vec<(String, String)>, String> {
    struct Walk<'c> {
        cache: &'c HashCache,
        real_root: PathBuf,
        real_dot: PathBuf,
        real_git: Option<PathBuf>,
        /// The `.trellis/` the walk started from, as given (not canonical),
        /// so a directory's path as walked can be compared with its real
        /// one.
        dot: PathBuf,
        /// The repository's common git directory, canonical, read from
        /// disk; `None` skips no worktree.
        common: Option<PathBuf>,
        /// Canonical directories already walked, so a link cycle ends.
        seen: std::collections::HashSet<PathBuf>,
        out: BTreeMap<String, String>,
    }
    impl Walk<'_> {
        /// `real` is canonical; the skips apply to it relative to the real
        /// `.trellis/`, so an alias cannot move a file in or out of them.
        fn skipped(&self, real: &Path) -> bool {
            let Ok(inner) = real.strip_prefix(&self.real_dot) else {
                return false;
            };
            let inner = inner.to_string_lossy().replace('\\', "/");
            match inner.as_str() {
                "index.bin" => true,
                ".gitignore" => std::fs::read_to_string(real)
                    .is_ok_and(|t| t.replace("\r\n", "\n") == trellis_index::cache::IGNORE),
                // Replay recordings (ADR 0023 §4, amended 2026-10-12): every
                // run writes one, so hashing them would untrust the
                // repository after each run, as trails would.
                "replays/.gitignore" => std::fs::read_to_string(real)
                    .is_ok_and(|t| t.replace("\r\n", "\n") == crate::scripted::REPLAY_IGNORE),
                _ => ["runs/", "replays/"].iter().any(|dir| {
                    inner
                        .strip_prefix(dir)
                        .is_some_and(|n| !n.contains('/') && n.ends_with(".json"))
                }),
            }
        }

        /// Whether the directory reached at `path`, whose real path is
        /// `real`, is a linked worktree to leave out (ADR 0023 §4, amended
        /// 2026-10-13): strictly under the real `.trellis/work/`, reached
        /// at that same place with no link on the way (an alias of one is
        /// walked like any directory, so its files are hashed), and
        /// registered in this repository
        /// (`trellis_vcs::is_registered_linked_worktree`).
        fn linked_worktree(&self, path: &Path, real: &Path) -> bool {
            let Some(common) = &self.common else {
                return false;
            };
            let Ok(inner) = real.strip_prefix(self.real_dot.join("work")) else {
                return false;
            };
            if inner.as_os_str().is_empty() {
                return false;
            }
            let as_walked = path.strip_prefix(&self.dot).ok();
            if as_walked != real.strip_prefix(&self.real_dot).ok() {
                return false;
            }
            trellis_vcs::is_registered_linked_worktree(real, common)
        }

        /// Why a link resolving to `real` (or failing to) cannot be followed.
        fn refusal(&self, real: Option<&Path>) -> Option<&'static str> {
            match real {
                None => Some("which does not exist"),
                Some(t) if !t.starts_with(&self.real_root) => {
                    Some("which is outside the repository")
                }
                Some(t) if self.real_dot.starts_with(t) => Some(
                    "which contains .trellis/ itself: following it would hash the repository \
                     and its .git/ directory",
                ),
                Some(t) if self.real_git.as_ref().is_some_and(|g| t.starts_with(g)) => {
                    Some("which is inside .git/, whose files git rewrites on its own")
                }
                Some(_) => None,
            }
        }

        fn dir(&mut self, dir: &Path) -> Result<(), String> {
            if let Ok(real) = std::fs::canonicalize(dir)
                && !self.seen.insert(real)
            {
                return Ok(());
            }
            let Ok(rd) = std::fs::read_dir(dir) else {
                return Ok(());
            };
            let mut entries: Vec<std::fs::DirEntry> = rd.filter_map(Result::ok).collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for e in entries {
                let path = e.path();
                let Ok(ft) = e.file_type() else { continue };
                let linked = ft.is_symlink();
                let real = std::fs::canonicalize(&path).ok();
                if linked && let Some(why) = self.refusal(real.as_deref()) {
                    let to = std::fs::read_link(&path)
                        .map(|t| plain(&t.display().to_string()))
                        .unwrap_or_else(|_| "?".into());
                    return Err(format!(
                        "`{}` is a symbolic link to `{to}`, {why}. Trust covers the files \
                         under .trellis/, and this one cannot be covered: replace the link \
                         with the files, or remove it",
                        plain(&rel(
                            &self.real_root,
                            &path_for_message(&self.real_root, &path)
                        ))
                    ));
                }
                let Some(real) = real else { continue };
                // `metadata` follows a link; `file_type` above did not.
                let Ok(md) = std::fs::metadata(&path) else {
                    continue;
                };
                if md.is_dir() {
                    // A link to a worktree is walked: only the directory
                    // itself, at its own place, is left out.
                    if linked || !self.linked_worktree(&path, &real) {
                        self.dir(&path)?;
                    }
                    continue;
                }
                let key = rel(&self.real_root, &real);
                if self.out.contains_key(&key) || self.skipped(&real) {
                    continue;
                }
                if let Some(digest) = self.cache.digest(&real) {
                    self.out.insert(key, digest);
                }
            }
            Ok(())
        }
    }
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let dot = root.join(".trellis");
    let Ok(real_dot) = std::fs::canonicalize(&dot) else {
        return Ok(Vec::new());
    };
    let real_git = std::fs::canonicalize(root.join(".git")).ok();
    if !real_dot.starts_with(&real_root)
        || real_dot == real_root
        || real_git.as_ref().is_some_and(|g| real_dot.starts_with(g))
    {
        return Err(format!(
            "`.trellis` resolves to `{}`, which is not a directory inside the repository. \
             Trust covers the files under .trellis/, and these cannot be covered",
            plain(&real_dot.display().to_string())
        ));
    }
    let common = trellis_vcs::common_git_dir_on_disk(root);
    let mut w = Walk {
        cache,
        real_git,
        dot: dot.clone(),
        common,
        real_root,
        real_dot,
        seen: Default::default(),
        out: BTreeMap::new(),
    };
    w.dir(&dot)?;
    Ok(w.out.into_iter().collect())
}

/// The user's packages directory (ADR 0039): `packages/` beside the trust
/// store, which is where `trellis install` puts it.
fn user_packages_dir() -> Option<PathBuf> {
    TrustStore::user()
        .ok()
        .and_then(|s| s.path().parent().map(|d| d.join("packages")))
}

/// Why the hooks and custom tools in `items` cannot be trusted, when any of
/// their argv names a file under the user's packages directory (#201).
///
/// Packages carry no code (ADR 0039 §9), and nothing under the packages
/// directory is in the trust digest. A trusted hook that ran a package's
/// file as code would change behaviour whenever the package is updated,
/// with the digest unchanged. So such a hook is refused, the same rule as
/// a repository file outside `.trellis/` and with the same fix. Tokens are
/// made canonical, as in [`outside_code`], so `..`, a link or a drive
/// letter's case cannot step around it.
fn package_code(packages: Option<&Path>, root: &Path, items: &[TrustItem]) -> Option<String> {
    let real_pkgs = std::fs::canonicalize(packages?).ok()?;
    let mut why: Vec<String> = Vec::new();
    for item in items {
        for tok in argv_candidates(&item.argv) {
            if tok.is_empty() {
                continue;
            }
            let p = Path::new(tok);
            let Ok(real) = std::fs::canonicalize(if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }) else {
                continue;
            };
            if real.starts_with(&real_pkgs) {
                let line = format!(
                    "{} names `{}`",
                    plain(&item.label),
                    plain(&real.display().to_string())
                );
                if !why.contains(&line) {
                    why.push(line);
                }
            }
        }
    }
    (!why.is_empty()).then(|| {
        format!(
            "{}, under your packages directory. Packages carry no code (ADR 0039 §9) and \
             nothing there is in the trust digest, so a package update would change what \
             the hook runs without the digest changing: move the code to .trellis/hooks/ \
             (or .trellis/tools/) and name it there",
            why.join(", ")
        )
    })
}

/// Why the hooks and custom tools in `items` cannot be trusted, when any of
/// their argv (any element, any token of one) names a regular file in the
/// repository outside `.trellis/`.
///
/// A rule instead of a smarter tokenizer (review round 11). Which text in
/// an argv is code is the interpreter's business, and every round found a
/// spelling the last tokenizer missed. Code under `.trellis/` is covered
/// whatever reaches it: the digest hashes every byte there, and `edit` and
/// `write_file` refuse to write there. Code outside it is an ordinary file
/// a run may edit, and a tokenizer was the only thing tying it to trust. So
/// a hook or custom tool that names such a file is refused, with the fix:
/// move the code under `.trellis/`.
///
/// Programs on `PATH` and absolute paths outside the repository are the
/// user's system, not repository files. Files only: a directory named as an
/// argument (`ruff check src`) is data for the tool, and refusing it would
/// refuse ordinary lint commands. What this does not catch is said in ADR
/// 0023 §4: code reached by import or `-m`, a directory run as a program
/// (`python pkg/`), a file a script reads.
///
/// Two tests, and a file failing either is refused. The first is on the
/// files the item hashed, by the path as written
/// ([`crate::path_rules::names_trellis_dir`]). The second has its own
/// resolver (review round 12), because what gets hashed is narrower than
/// what a command can name: [`repo_file`] drops a path with `..` and an
/// absolute one that is not lexically under `root`, so
/// `.trellis/../scripts/gate.py`, an absolute path with its drive letter in
/// the other case, or one reached through a relative `--root` was neither
/// hashed nor refused. Every candidate token is made canonical (joined to
/// `root` when relative), as are the root and `.trellis/`; a regular file
/// inside the real root and outside the real `.trellis/` is refused. So a
/// path under `.trellis/` that is a link to a file elsewhere in the
/// repository is refused too: the file a run can edit is the one it
/// resolves to, and `edit` checks the path as written.
fn outside_code(root: &Path, items: &[TrustItem]) -> Option<String> {
    let real_root = std::fs::canonicalize(root).ok();
    let real_dot = std::fs::canonicalize(root.join(".trellis")).ok();
    let resolves_outside = |tok: &str| -> Option<String> {
        let real_root = real_root.as_ref()?;
        if tok.is_empty() {
            return None;
        }
        let p = Path::new(tok);
        let real = std::fs::canonicalize(if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        })
        .ok()?;
        let inside_dot = real_dot.as_ref().is_some_and(|d| real.starts_with(d));
        (real.is_file() && real.starts_with(real_root) && !inside_dot)
            .then(|| rel(real_root, &real))
    };
    let mut why: Vec<String> = Vec::new();
    let mut note = |line: String| {
        if !why.contains(&line) {
            why.push(line);
        }
    };
    for item in items {
        for (path, _) in &item.files {
            if !crate::path_rules::names_trellis_dir(path) {
                note(format!("{} names `{}`", plain(&item.label), plain(path)));
            }
        }
        for tok in argv_candidates(&item.argv) {
            let Some(real) = resolves_outside(tok) else {
                continue;
            };
            if item.files.iter().any(|(p, _)| *p == real) {
                continue; // already named above, by the same path
            }
            note(if tok.replace('\\', "/") == real {
                format!("{} names `{}`", plain(&item.label), plain(&real))
            } else {
                format!(
                    "{} names `{}`, which is `{}`",
                    plain(&item.label),
                    plain(tok),
                    plain(&real)
                )
            });
        }
    }
    (!why.is_empty()).then(|| {
        format!(
            "{}, a file in the repository outside .trellis/. A hook's or custom tool's code \
             must live under .trellis/, where a run cannot edit it and every byte is in \
             the trust digest: move it to .trellis/hooks/ (or .trellis/tools/) and name it \
             there. Programs on PATH and files outside the repository are fine",
            why.join(", ")
        )
    })
}

/// The registered linked worktree under the real `.trellis/work/` that the
/// canonical path `real` is inside, if any.
fn enclosing_linked_worktree(real: &Path, real_dot: &Path, common: &Path) -> Option<PathBuf> {
    let work = real_dot.join("work");
    real.ancestors()
        .take_while(|a| a.starts_with(&work) && *a != work.as_path())
        .find(|a| trellis_vcs::is_registered_linked_worktree(a, common))
        .map(Path::to_path_buf)
}

/// The refusal for a hook or custom tool whose argv names a file inside a
/// linked worktree under `.trellis/work/` (ADR 0023 §4, amended
/// 2026-10-13). The digest leaves those worktrees out, and a run rooted in
/// a task worktree edits its files, so code there is neither covered nor
/// out of a run's reach: the two reasons [`outside_code`] refuses code
/// outside `.trellis/`. Decided on the canonical path, so a link such as
/// `.trellis/hooks -> work/task/1` is refused when a file is named through
/// it.
fn worktree_code(root: &Path, items: &[TrustItem]) -> Option<String> {
    let common = trellis_vcs::common_git_dir_on_disk(root)?;
    let real_root = std::fs::canonicalize(root).ok()?;
    let real_dot = std::fs::canonicalize(root.join(".trellis")).ok()?;
    let mut why: Vec<String> = Vec::new();
    for item in items {
        for tok in argv_candidates(&item.argv) {
            if tok.is_empty() {
                continue;
            }
            let p = Path::new(tok);
            let Ok(real) = std::fs::canonicalize(if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }) else {
                continue;
            };
            if !real.is_file() {
                continue;
            }
            let Some(wt) = enclosing_linked_worktree(&real, &real_dot, &common) else {
                continue;
            };
            let line = format!(
                "{} names `{}`, which is in the linked worktree `{}`",
                plain(&item.label),
                plain(tok),
                plain(&rel(&real_root, &wt))
            );
            if !why.contains(&line) {
                why.push(line);
            }
        }
    }
    (!why.is_empty()).then(|| {
        format!(
            "{}. Linked worktrees under .trellis/work/ are left out of the trust digest, and \
             a run in one edits its files: move the code to .trellis/hooks/ (or \
             .trellis/tools/) and name it there",
            why.join(", ")
        )
    })
}

/// `path` as reached, with its parent made canonical, for naming a link in
/// a message: the link itself, not what it resolves to.
fn path_for_message(real_root: &Path, path: &Path) -> PathBuf {
    match (path.parent().map(std::fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => real_root.join(path),
    }
}

/// Everything a repository would execute, and one digest over all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub items: Vec<TrustItem>,
    pub sha256: String,
    /// Why this repository cannot be trusted as it stands, e.g. a link
    /// under `.trellis/` that leaves it. [`TrustStore::trust`] refuses it
    /// and [`TrustStore::state`] never reports it trusted.
    pub refused: Option<String>,
}

impl Fingerprint {
    /// Items for the hooks, the custom tools, and the bash settings (given
    /// as text: whatever `[tools]` said about bash, rendered stably by the
    /// caller). An empty fingerprint means there is nothing to trust.
    pub fn of(
        root: &Path,
        hooks: &HookConfig,
        custom: &[CustomTool],
        bash: Option<(Vec<String>, String)>,
    ) -> Self {
        Self::of_with(root, hooks, custom, bash, Vec::new())
    }

    /// As [`of`](Self::of), with further items the binary knows about and
    /// this crate does not: process agents (ADR 0026 §11), whose profiles
    /// the queue crate reads.
    pub fn of_with(
        root: &Path,
        hooks: &HookConfig,
        custom: &[CustomTool],
        bash: Option<(Vec<String>, String)>,
        extra: Vec<TrustItem>,
    ) -> Self {
        Self::of_cached(root, hooks, custom, bash, extra, &HashCache::default())
    }

    /// [`of_with`](Self::of_with), hashing through `cache`, for
    /// [`TrustGuard`]'s rechecks. The walk of `.trellis/` is made in full
    /// every time, so an added or removed file is always seen; only the
    /// reading and hashing of a file whose stat has not changed is saved.
    pub fn of_cached(
        root: &Path,
        hooks: &HookConfig,
        custom: &[CustomTool],
        bash: Option<(Vec<String>, String)>,
        extra: Vec<TrustItem>,
        cache: &HashCache,
    ) -> Self {
        let mut items = Vec::new();
        for (event, argv) in hooks.declared() {
            items.push(TrustItem::command_cached(
                root,
                format!("hook {event}"),
                argv,
                format!("timeout_s = {:?}", hooks.timeout_s),
                cache,
            ));
        }
        for c in custom {
            let mut settings = format!("description = {:?}\nschema = {}", c.description, c.schema);
            // Only when declared: a declaration without the key keeps the
            // digest it was trusted under, and one that adds or changes it
            // has changed how long repository code may run, so it has to be
            // trusted again.
            if let Some(t) = c.timeout_s {
                settings.push_str(&format!("\ntimeout_s = {t}"));
            }
            items.push(TrustItem::command_cached(
                root,
                format!("custom tool {}", c.name),
                &c.command,
                settings,
                cache,
            ));
        }
        // Hooks and custom tools only, taken before `bash` and the extras
        // are added: the rule is about where their code lives.
        let outside = [
            outside_code(root, &items),
            worktree_code(root, &items),
            package_code(user_packages_dir().as_deref(), root, &items),
        ]
        .into_iter()
        .flatten()
        .reduce(|a, b| format!("{a}; {b}"));
        if let Some((shell, settings)) = bash {
            items.push(TrustItem::command_cached(
                root,
                "bash".to_string(),
                &shell,
                settings,
                cache,
            ));
        }
        items.extend(extra);
        // Only when something would execute: a repository that declares
        // nothing still has nothing to trust, whatever `.trellis/` holds.
        let mut refused = outside;
        if !items.is_empty() {
            let (files, detail) = match dot_trellis_files(root, cache) {
                Ok(files) => (files, String::new()),
                Err(why) => {
                    refused = Some(match refused {
                        Some(r) => format!("{r}; {why}"),
                        None => why.clone(),
                    });
                    (Vec::new(), format!("\nnot trustable: {why}"))
                }
            };
            items.push(TrustItem {
                label: ".trellis/ (every file)".to_string(),
                argv: Vec::new(),
                detail: format!(
                    "all files under .trellis/ except index.bin, runs/*.json and the \
                     .gitignore trellis wrote, which trellis writes itself{detail}"
                ),
                files,
                receives: Vec::new(),
            });
        }
        let canonical = serde_json::to_vec(&items).expect("a list of strings always serializes");
        Self {
            sha256: hex(&Sha256::digest(&canonical)),
            items,
            refused,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// One sha256 per item label, over that item's JSON as the repository
    /// digest serializes it; items sharing a label are hashed together, in
    /// order. For diagnosis only (#136): stored beside the repository
    /// digest so `trellis trust --show` and `trellis doctor` can name which
    /// item changed. No trust decision reads it; see [`TrustStore::state`].
    pub fn item_digests(&self) -> BTreeMap<String, String> {
        let mut by_label: BTreeMap<&str, Vec<&TrustItem>> = BTreeMap::new();
        for item in &self.items {
            by_label.entry(&item.label).or_default().push(item);
        }
        by_label
            .into_iter()
            .map(|(label, items)| {
                let json = serde_json::to_vec(&items).expect("a list of strings always serializes");
                (label.to_string(), hex(&Sha256::digest(&json)))
            })
            .collect()
    }
}

/// What a person trusted, re-checked before each spawn of trusted code
/// during a run (ADR 0023 §5, "trusted code cannot be rewritten mid-run").
///
/// Hooks re-read their scripts on every spawn, and `bash`, a custom tool or
/// a hook can rewrite any file; none of that can be path-checked the way
/// `edit` and `write_file` are. So before a hook or a custom tool is
/// started, and after any call that may have changed the tree, the
/// fingerprint is computed again and compared with the one trusted. A
/// difference refuses the spawn, and is latched: every later spawn is
/// refused too, and the run does not converge. The latch is shared by every
/// clone, so the `pre_tool` gate's copy of the hooks and the orchestrator's
/// see one breach.
///
/// A recheck hashes through a [`HashCache`], so a file whose stat has not
/// changed is not read again; the tree is still walked every time, so an
/// added or removed file is always seen. A rewrite that keeps a file's
/// length and modification time (possible on Windows, where no change time
/// is exposed) can pass a cached recheck, so a converged run is checked
/// once more with [`check_full`](Self::check_full), which hashes every file
/// afresh (ADR 0023 §5). The async forms run the recheck on the blocking
/// pool, so the runtime still answers Ctrl-C, `abort` and approvals while a
/// large `.trellis/` is hashed.
#[derive(Clone)]
pub struct TrustGuard(std::sync::Arc<GuardInner>);

type Recheck = Box<dyn Fn(&HashCache) -> Result<(), String> + Send + Sync>;

struct GuardInner {
    recheck: Recheck,
    /// Replaced whole by each full check, so it never holds a digest older
    /// than the last full hash of the file.
    cache: Mutex<std::sync::Arc<HashCache>>,
    breach: Mutex<Option<String>>,
}

/// Digests of files by real path, each kept with the stat it was taken
/// under: length and modification time, plus the change time and inode on
/// Unix and the creation time on Windows. A file whose stat differs is
/// read and hashed again.
#[derive(Debug, Default)]
pub struct HashCache(Mutex<std::collections::HashMap<PathBuf, (FileStamp, String)>>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (u64, u64, i64, i64),
    #[cfg(windows)]
    created: u64,
}

impl FileStamp {
    fn of(m: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt;
        Self {
            len: m.len(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            changed: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
            #[cfg(windows)]
            created: m.creation_time(),
        }
    }
}

impl HashCache {
    /// The sha256 of the file at `path`, links followed, or `None` when it
    /// cannot be read. The stat is taken before the read, so a write that
    /// lands between them leaves a stamp older than the bytes, and the next
    /// call hashes again.
    pub fn digest(&self, path: &Path) -> Option<String> {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let stamp = FileStamp::of(&std::fs::metadata(&key).ok()?);
        {
            let map = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((s, d)) = map.get(&key)
                && *s == stamp
            {
                return Some(d.clone());
            }
        }
        let bytes = std::fs::read(&key).ok()?;
        let digest = hex(&Sha256::digest(&bytes));
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, (stamp, digest.clone()));
        Some(digest)
    }
}

impl std::fmt::Debug for TrustGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustGuard")
            .field("breach", &self.breach())
            .finish_non_exhaustive()
    }
}

impl TrustGuard {
    /// A guard whose check is `recheck`: `Err` names what changed. Tests
    /// only: a guard outside them is built by [`over`](Self::over), which
    /// ties it to a trusted fingerprint.
    #[cfg(test)]
    fn new(recheck: impl Fn() -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self::with_cache(Box::new(move |_: &HashCache| recheck()))
    }

    fn with_cache(recheck: Recheck) -> Self {
        Self(std::sync::Arc::new(GuardInner {
            recheck,
            cache: Mutex::new(Default::default()),
            breach: Mutex::new(None),
        }))
    }

    /// A guard over `trusted`, recomputed by `now` through the cache it is
    /// handed ([`Fingerprint::of_cached`]). The error names the files whose
    /// hashes differ, so the trail says what was rewritten.
    pub fn over(
        trusted: Fingerprint,
        now: impl Fn(&HashCache) -> Fingerprint + Send + Sync + 'static,
    ) -> Self {
        Self::with_cache(Box::new(move |cache: &HashCache| {
            let current = now(cache);
            if current.sha256 == trusted.sha256 && current.refused.is_none() {
                return Ok(());
            }
            Err(describe_change(&trusted, &current))
        }))
    }

    /// `Ok` while what runs is what was trusted. Once it has failed it
    /// fails for the rest of the run, even if the files are put back: a
    /// hook that ran rewritten, or was refused, is already in this run.
    ///
    /// Hashes through the guard's cache: see the type's documentation for
    /// what that can miss, and [`check_full`](Self::check_full).
    pub fn check(&self) -> Result<(), TrustChanged> {
        self.check_with(false)
    }

    /// [`check`](Self::check), with every file hashed afresh. The fresh
    /// digests become the cache later checks use.
    pub fn check_full(&self) -> Result<(), TrustChanged> {
        self.check_with(true)
    }

    fn check_with(&self, full: bool) -> Result<(), TrustChanged> {
        let mut b = self.0.breach.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(why) = b.as_ref() {
            return Err(TrustChanged(why.clone()));
        }
        let cache = if full {
            std::sync::Arc::new(HashCache::default())
        } else {
            self.0
                .cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        };
        if let Err(why) = (self.0.recheck)(&cache) {
            *b = Some(why.clone());
            return Err(TrustChanged(why));
        }
        if full {
            *self.0.cache.lock().unwrap_or_else(|p| p.into_inner()) = cache;
        }
        Ok(())
    }

    /// [`check`](Self::check) on tokio's blocking pool, so the runtime is
    /// free while the tree is walked and hashed. A check that panicked is a
    /// breach: what it would have said is not known.
    pub async fn check_async(&self) -> Result<(), TrustChanged> {
        self.off_runtime(false).await
    }

    /// [`check_full`](Self::check_full) on tokio's blocking pool.
    pub async fn check_full_async(&self) -> Result<(), TrustChanged> {
        self.off_runtime(true).await
    }

    async fn off_runtime(&self, full: bool) -> Result<(), TrustChanged> {
        let g = self.clone();
        match tokio::task::spawn_blocking(move || g.check_with(full)).await {
            Ok(r) => r,
            Err(e) => {
                let why = format!("the trust recheck did not finish ({e})");
                let mut b = self.0.breach.lock().unwrap_or_else(|p| p.into_inner());
                Err(TrustChanged(b.get_or_insert(why).clone()))
            }
        }
    }

    /// The first failed check's reason, if any check has failed.
    pub fn breach(&self) -> Option<String> {
        self.0
            .breach
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// What differs between two fingerprints, in words: the files whose hashes
/// changed, appeared or went, or the items that did.
fn describe_change(was: &Fingerprint, now: &Fingerprint) -> String {
    if let Some(why) = &now.refused {
        return format!("the trusted code can no longer be trusted: {why}");
    }
    let files = |f: &Fingerprint| -> BTreeMap<String, String> {
        f.items
            .iter()
            .flat_map(|i| i.files.iter().cloned())
            .collect()
    };
    let (a, b) = (files(was), files(now));
    let mut changed: Vec<String> = a
        .iter()
        .filter(|(p, h)| b.get(*p) != Some(*h))
        .map(|(p, _)| p.clone())
        .chain(b.keys().filter(|p| !a.contains_key(*p)).cloned())
        .collect();
    changed.sort();
    changed.dedup();
    // Escaped after the dedup, so two paths that escape alike stay two.
    let changed: Vec<String> = changed.iter().map(|p| plain(p)).collect();
    if changed.is_empty() {
        "the trusted configuration changed since the run started (a hook, custom tool \
         or bash setting)"
            .to_string()
    } else {
        format!(
            "trusted code changed since the run started: {}",
            changed.join(", ")
        )
    }
}

/// Whether a repository's current fingerprint is the one a person trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustState {
    Trusted,
    /// Never trusted, or revoked.
    Unknown,
    /// Trusted once, and something has changed since.
    Changed,
    /// Cannot be trusted as it stands, whatever the store says, and
    /// `trellis trust` would refuse too: `reason` names what to fix (a link
    /// under `.trellis/` that leaves it, for one). Reported before the
    /// store is consulted, so a person is never sent to a command that
    /// refuses.
    Refused {
        reason: String,
    },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    repos: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// The one authoritative digest: trust is given, and honoured, for
    /// exactly this [`Fingerprint::sha256`] and nothing else.
    sha256: String,
    /// Seconds since the Unix epoch.
    trusted_at: u64,
    /// [`Fingerprint::item_digests`] at the time of trust, for naming what
    /// changed and for nothing else (#136). Absent from an entry written
    /// before it was kept, which then reads exactly as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    items: BTreeMap<String, String>,
}

/// One item that differs from what was trusted, by its label
/// ([`TrustItem::label`], written by the repository's configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemChange {
    /// Declared then and now, with different contents.
    Changed(String),
    /// Declared now, not when trust was recorded.
    Added(String),
    /// Declared when trust was recorded, not now.
    Removed(String),
}

/// What [`TrustStore::changed_items`] can say about a repository's items
/// against its trust record. Informational: [`TrustStore::state`] alone
/// decides whether anything loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustDiagnosis {
    /// No trust is recorded for this repository.
    NotRecorded,
    /// Trust was recorded before per-item digests were kept, so which item
    /// changed cannot be said.
    NoItemDigests,
    /// Each item that differs, by label. Empty when every item matches its
    /// recorded digest; the repository digest can still differ then (the
    /// same items declared in another order), which is not a match.
    Items(Vec<ItemChange>),
}

/// Why the trust store could not be read or written, or would not record a
/// decision. The messages are the ones these returned as strings before
/// (review round 10).
#[derive(Debug, thiserror::Error)]
pub enum TrustStoreError {
    #[error("no user configuration directory (APPDATA, XDG_CONFIG_HOME or HOME) to keep trust in")]
    NoConfigDir,
    #[error("{} is not a trust store: {source}", path.display())]
    NotAStore {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot create {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot replace {}: {source}", path.display())]
    Replace {
        path: PathBuf,
        source: std::io::Error,
    },
    /// [`Fingerprint::refused`]: the repository cannot be trusted as it
    /// stands.
    #[error("cannot trust this repository: {0}")]
    Refused(String),
}

/// A [`TrustGuard`] check that failed: what trusted code changed, or why it
/// can no longer be trusted. Prose composed from the fingerprints (which
/// files differ), not a closed set of cases, so one variant carrying it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TrustChanged(pub String);

/// Where trust is recorded.
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
}

/// Set once per process by tests, so no test reads or writes the real
/// user's store. Not an environment variable: setting one in a test is
/// unsafe under edition 2024 and races every other test in the process.
static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Serializes read-modify-write in this process, so two writers cannot each
/// read the file, add their entry, and have the second erase the first.
static LOCK: Mutex<()> = Mutex::new(());

impl TrustStore {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Points [`user`](Self::user) at `path` for the rest of this process.
    /// For tests; the first call wins.
    pub fn use_for_this_process(path: PathBuf) {
        let _ = OVERRIDE.set(path);
    }

    /// The user's store: `%APPDATA%\trellis\trust.json` on Windows, else
    /// `$XDG_CONFIG_HOME/trellis/trust.json` or `~/.config/trellis/trust.json`.
    pub fn user() -> Result<Self, TrustStoreError> {
        if let Some(p) = OVERRIDE.get() {
            return Ok(Self::at(p.clone()));
        }
        let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let dir = if cfg!(windows) {
            non_empty("APPDATA").map(PathBuf::from)
        } else {
            non_empty("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| non_empty("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        dir.map(|d| Self::at(d.join("trellis").join("trust.json")))
            .ok_or(TrustStoreError::NoConfigDir)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<StoreFile, TrustStoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s).map_err(|source| TrustStoreError::NotAStore {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreFile::default()),
            Err(source) => Err(TrustStoreError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write(&self, f: &StoreFile) -> Result<(), TrustStoreError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| TrustStoreError::CreateDir {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(f).expect("the store always serializes");
        std::fs::write(&tmp, body).map_err(|source| TrustStoreError::Write {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|source| TrustStoreError::Replace {
            path: self.path.clone(),
            source,
        })
    }

    /// Trusted only when the recorded repository digest is `fp.sha256`.
    /// The per-item digests beside it are not consulted: they say what
    /// changed, never that anything is trusted ([`changed_items`](Self::changed_items)).
    pub fn state(&self, root: &Path, fp: &Fingerprint) -> Result<TrustState, TrustStoreError> {
        if let Some(reason) = &fp.refused {
            return Ok(TrustState::Refused {
                reason: reason.clone(),
            });
        }
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        Ok(match self.read()?.repos.get(&store_key(root)) {
            None => TrustState::Unknown,
            Some(e) if e.sha256 == fp.sha256 => TrustState::Trusted,
            Some(_) => TrustState::Changed,
        })
    }

    /// Which items differ from the ones recorded when this repository was
    /// trusted. For `trellis trust --show` and `trellis doctor` to name what
    /// changed; never for deciding trust. Items trusted together are one
    /// decision, and a set recombined from items trusted at different times
    /// is a set nobody looked at, so a per-item match trusts nothing
    /// (ADR 0023, "Untrusted is per repository, not per item").
    pub fn changed_items(
        &self,
        root: &Path,
        fp: &Fingerprint,
    ) -> Result<TrustDiagnosis, TrustStoreError> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = self.read()?.repos.remove(&store_key(root)) else {
            return Ok(TrustDiagnosis::NotRecorded);
        };
        if entry.items.is_empty() {
            return Ok(TrustDiagnosis::NoItemDigests);
        }
        let now = fp.item_digests();
        let mut out = Vec::new();
        for (label, digest) in &now {
            match entry.items.get(label) {
                None => out.push(ItemChange::Added(label.clone())),
                Some(was) if was != digest => out.push(ItemChange::Changed(label.clone())),
                Some(_) => {}
            }
        }
        for label in entry.items.keys().filter(|l| !now.contains_key(*l)) {
            out.push(ItemChange::Removed(label.clone()));
        }
        Ok(TrustDiagnosis::Items(out))
    }

    pub fn trust(&self, root: &Path, fp: &Fingerprint) -> Result<(), TrustStoreError> {
        if let Some(why) = &fp.refused {
            return Err(TrustStoreError::Refused(why.clone()));
        }
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        f.repos.insert(
            store_key(root),
            Entry {
                sha256: fp.sha256.clone(),
                trusted_at: now,
                items: fp.item_digests(),
            },
        );
        self.write(&f)
    }

    /// Removes the entry. `Ok(false)` when there was none.
    pub fn revoke(&self, root: &Path) -> Result<bool, TrustStoreError> {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = self.read()?;
        let had = f.repos.remove(&store_key(root)).is_some();
        if had {
            self.write(&f)?;
        }
        Ok(had)
    }
}

/// The canonical repository root, so `C:\r` and `c:\r\.` are one entry.
fn store_key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(argv: &[&str]) -> HookConfig {
        HookConfig {
            pre_tool: Some(argv.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn a_change_to_a_script_a_hook_names_makes_the_repository_untrusted_again() {
        // The argv alone is not what runs; the script it names is. Trusting
        // `python hook.py` and then running a different hook.py would be
        // trust in a file name.
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis/hooks")).unwrap();
        std::fs::write(repo.path().join(".trellis/hooks/hook.py"), "print('ok')\n").unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let h = hooks(&["python", ".trellis/hooks/hook.py"]);

        let fp = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(fp.items[0].files.len(), 1, "the script is hashed");
        assert_eq!(store.state(repo.path(), &fp).unwrap(), TrustState::Unknown);
        store.trust(repo.path(), &fp).unwrap();
        assert_eq!(store.state(repo.path(), &fp).unwrap(), TrustState::Trusted);

        std::fs::write(
            repo.path().join(".trellis/hooks/hook.py"),
            "import os; os.system('x')\n",
        )
        .unwrap();
        let fp2 = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(store.state(repo.path(), &fp2).unwrap(), TrustState::Changed);
    }

    #[test]
    fn a_change_to_any_definition_changes_the_digest() {
        let repo = tempfile::tempdir().unwrap();
        let base = Fingerprint::of(repo.path(), &hooks(&["a"]), &[], None);
        let other_argv = Fingerprint::of(repo.path(), &hooks(&["b"]), &[], None);
        let with_bash = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[],
            Some((vec!["sh".into(), "-c".into()], "unattended = false".into())),
        );
        let unattended = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[],
            Some((vec!["sh".into(), "-c".into()], "unattended = true".into())),
        );
        let custom = Fingerprint::of(
            repo.path(),
            &hooks(&["a"]),
            &[CustomTool {
                name: "lint".into(),
                description: "d".into(),
                command: vec!["x".into()],
                schema: serde_json::json!({ "type": "object" }),
                timeout_s: None,
            }],
            None,
        );
        let all = [&base, &other_argv, &with_bash, &unattended, &custom];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.sha256, b.sha256);
            }
        }
    }

    #[test]
    fn a_file_outside_the_repository_is_not_taken_as_repository_code() {
        let repo = tempfile::tempdir().unwrap();
        let fp = Fingerprint::of(repo.path(), &hooks(&["python", "../evil.py"]), &[], None);
        assert!(fp.items[0].files.is_empty());
    }

    #[test]
    fn revoking_forgets_and_other_repositories_are_untouched() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("sub").join("trust.json"));
        let fa = Fingerprint::of(a.path(), &hooks(&["x"]), &[], None);
        let fb = Fingerprint::of(b.path(), &hooks(&["x"]), &[], None);
        store.trust(a.path(), &fa).unwrap();
        store.trust(b.path(), &fb).unwrap();
        assert!(store.revoke(a.path()).unwrap());
        assert!(!store.revoke(a.path()).unwrap());
        assert_eq!(store.state(a.path(), &fa).unwrap(), TrustState::Unknown);
        assert_eq!(store.state(b.path(), &fb).unwrap(), TrustState::Trusted);
    }

    #[test]
    fn a_script_named_inside_a_shell_string_is_hashed() {
        // The risk: `["sh", "-c", "./scripts/check.sh"]` names no file as an
        // argv element, so the script it runs was hashed by nothing and
        // could change after trust without anyone being asked again.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis/hooks")).unwrap();
        std::fs::write(repo.path().join(".trellis/hooks/check.sh"), "exit 0\n").unwrap();
        let h = hooks(&["sh", "-c", "./.trellis/hooks/check.sh --fast && echo ok"]);
        let fp = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(
            fp.items[0]
                .files
                .iter()
                .map(|f| f.0.as_str())
                .collect::<Vec<_>>(),
            [".trellis/hooks/check.sh"]
        );
        assert_eq!(fp.refused, None);
        std::fs::write(repo.path().join(".trellis/hooks/check.sh"), "curl x | sh\n").unwrap();
        assert_ne!(
            fp.sha256,
            Fingerprint::of(repo.path(), &h, &[], None).sha256
        );
    }

    #[test]
    fn a_script_named_in_any_argv_element_is_hashed_whatever_flag_precedes_it() {
        // The risk (review round 10, finding 3): only the string after
        // exactly `-c`, `-e`, `/C` or `/c` was split into tokens, so
        // `bash -lc`, `sh -ec`, `pwsh -Command`, `python -c` spelled
        // otherwise, or `--file=...` left the script they run unhashed, and
        // it could change after trust with nobody asked again.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis/hooks")).unwrap();
        std::fs::write(repo.path().join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();
        for argv in [
            &["bash", "-lc", "python .trellis/hooks/gate.py"][..],
            &["sh", "-ec", "python .trellis/hooks/gate.py --fast"],
            &["pwsh", "-Command", "& python .trellis/hooks/gate.py"],
            &["runner", "--file=.trellis/hooks/gate.py"],
        ] {
            let h = hooks(argv);
            let fp = Fingerprint::of(repo.path(), &h, &[], None);
            let files: Vec<&str> = fp.items[0].files.iter().map(|f| f.0.as_str()).collect();
            assert_eq!(files, [".trellis/hooks/gate.py"], "{argv:?}");
            std::fs::write(
                repo.path().join(".trellis/hooks/gate.py"),
                "print('changed')\n",
            )
            .unwrap();
            assert_ne!(
                fp.sha256,
                Fingerprint::of(repo.path(), &h, &[], None).sha256,
                "editing the script untrusts: {argv:?}"
            );
            std::fs::write(repo.path().join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();
        }
    }

    #[test]
    fn a_hook_or_custom_tool_naming_code_outside_dot_trellis_is_refused() {
        // Review round 11, item 4: the tokenizer missed scripts named inside
        // larger expressions, and each round found another spelling. The
        // class is closed by a rule instead: repository code a hook or
        // custom tool names must live under `.trellis/`, where every byte is
        // hashed and `edit` cannot reach it. Named anywhere else, it is
        // refused at `trellis trust` and whenever trust is read.
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis/hooks")).unwrap();
        std::fs::write(repo.path().join("scripts/gate.py"), "print('ok')\n").unwrap();
        std::fs::write(repo.path().join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();

        let outside = Fingerprint::of(
            repo.path(),
            &hooks(&["bash", "-lc", "python scripts/gate.py"]),
            &[],
            None,
        );
        let why = outside.refused.as_deref().expect("refused");
        assert!(
            why.contains("hook pre_tool names `scripts/gate.py`"),
            "{why}"
        );
        assert!(
            why.contains(".trellis/hooks/"),
            "the message names the fix: {why}"
        );
        assert!(matches!(
            store.trust(repo.path(), &outside),
            Err(TrustStoreError::Refused(_))
        ));
        assert!(matches!(
            store.state(repo.path(), &outside).unwrap(),
            TrustState::Refused { .. }
        ));

        let tool = CustomTool {
            name: "lint".into(),
            description: "d".into(),
            command: vec!["python".into(), "scripts/gate.py".into()],
            schema: serde_json::json!({ "type": "object" }),
            timeout_s: None,
        };
        let why = Fingerprint::of(repo.path(), &HookConfig::default(), &[tool], None).refused;
        assert!(
            why.as_deref()
                .is_some_and(|w| w.contains("custom tool lint names `scripts/gate.py`")),
            "{why:?}"
        );

        // Under `.trellis/`, a directory as data, a program on PATH: fine.
        for argv in [
            &["python", ".trellis/hooks/gate.py"][..],
            &["ruff", "check", "scripts"],
            &["python", "-c", "print(1)"],
        ] {
            let fp = Fingerprint::of(repo.path(), &hooks(argv), &[], None);
            assert_eq!(fp.refused, None, "{argv:?}");
        }
        let fp = Fingerprint::of(
            repo.path(),
            &hooks(&["python", ".trellis/hooks/gate.py"]),
            &[],
            None,
        );
        store.trust(repo.path(), &fp).unwrap();
        assert_eq!(store.state(repo.path(), &fp).unwrap(), TrustState::Trusted);
    }

    /// A repository with `scripts/gate.py` and `.trellis/hooks/gate.py`.
    fn repo_with_gate_scripts() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis/hooks")).unwrap();
        std::fs::write(repo.path().join("scripts/gate.py"), "print('ok')\n").unwrap();
        std::fs::write(repo.path().join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();
        repo
    }

    #[test]
    fn code_outside_dot_trellis_is_refused_however_the_path_is_spelled() {
        // Review round 12, item 1: the refusal only saw what `repo_file`
        // hashed, and `repo_file` drops `..` and any absolute path not
        // lexically under the root. So `.trellis/../scripts/gate.py` was
        // neither hashed nor refused, and the hook ran code a model run can
        // edit.
        let repo = repo_with_gate_scripts();
        let refused =
            |root: &Path, argv: &[&str]| Fingerprint::of(root, &hooks(argv), &[], None).refused;
        let why = refused(repo.path(), &["python", ".trellis/../scripts/gate.py"])
            .expect("`..` out of .trellis/ is refused");
        assert!(
            why.contains("names `.trellis/../scripts/gate.py`, which is `scripts/gate.py`"),
            "{why}"
        );
        // Inside a code string.
        assert!(
            refused(
                repo.path(),
                &["bash", "-lc", "python .trellis/hooks/../../scripts/gate.py"]
            )
            .is_some()
        );
        // With backslashes: a separator on Windows only. On Unix
        // `.trellis\hooks\..\..\scripts\gate.py` is one file name that does
        // not exist, so a hook with that argv could not reach
        // `scripts/gate.py` either, and not refusing it is correct there.
        if cfg!(windows) {
            assert!(
                refused(
                    repo.path(),
                    &[
                        "bash",
                        "-lc",
                        "python .trellis\\hooks\\..\\..\\scripts\\gate.py"
                    ]
                )
                .is_some()
            );
        }

        // An absolute path to the file, through the canonical root.
        let real = std::fs::canonicalize(repo.path().join("scripts/gate.py")).unwrap();
        let abs = real.to_string_lossy().into_owned();
        assert!(refused(repo.path(), &["python", &abs]).is_some(), "{abs}");
        // The root given relative to the working directory: the argv's
        // relative path and the root are joined and made canonical, so the
        // refusal does not depend on where trellis was started.
        let cwd = std::env::current_dir().unwrap();
        if let Some(rel_root) = pathdiff(repo.path(), &cwd) {
            assert!(
                refused(&rel_root, &["python", "scripts/gate.py"]).is_some(),
                "{rel_root:?}"
            );
            assert!(
                refused(&rel_root, &["python", &abs]).is_some(),
                "{rel_root:?}"
            );
        } else {
            eprintln!("skipped: the temporary directory and {cwd:?} share no root");
        }

        // Code under `.trellis/`, reached with or without `..`, is fine.
        assert_eq!(
            refused(repo.path(), &["python", ".trellis/hooks/../hooks/gate.py"]),
            None
        );
        let dot = std::fs::canonicalize(repo.path().join(".trellis/hooks/gate.py")).unwrap();
        assert_eq!(
            refused(repo.path(), &["python", &dot.to_string_lossy()]),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_absolute_path_with_the_drive_letter_in_the_other_case_is_refused() {
        // `Path::starts_with` compares the drive letter as written, so
        // `c:\...` was not "under" a root spelled `C:\...`: not hashed, not
        // refused. Windows opens either spelling.
        let repo = repo_with_gate_scripts();
        let root = repo.path().to_string_lossy().into_owned();
        let file = repo.path().join("scripts").join("gate.py");
        let file = file.to_string_lossy().into_owned();
        let flip = |s: &str| {
            let mut c = s.chars();
            let first = c.next().unwrap();
            assert!(first.is_ascii_alphabetic(), "a drive-letter path: {s}");
            let first = if first.is_ascii_uppercase() {
                first.to_ascii_lowercase()
            } else {
                first.to_ascii_uppercase()
            };
            format!("{first}{}", c.as_str())
        };
        for (root, file) in [(root.clone(), flip(&file)), (flip(&root), file.clone())] {
            let fp = Fingerprint::of(Path::new(&root), &hooks(&["python", &file]), &[], None);
            let why = fp.refused.expect("refused");
            assert!(why.contains("`scripts/gate.py`"), "{why}");
        }
    }

    #[test]
    fn a_link_under_dot_trellis_to_code_elsewhere_in_the_repository_is_refused_when_named() {
        // The chosen behaviour for a link: decided on where the path
        // resolves. `.trellis/hooks -> ../scripts` makes
        // `.trellis/hooks/gate.py` the file `scripts/gate.py`, which `edit`
        // and `write_file` reach by that name, so naming it is naming code
        // outside `.trellis/`. Reached by `-m` instead (no path in the argv),
        // it is not refused here, and the digest follows the link and
        // hashes it: `a_symlinked_directory_under_dot_trellis_is_followed`.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        std::fs::write(repo.path().join("scripts/gate.py"), "print('ok')\n").unwrap();
        let Some(()) = symlink_dir(
            &repo.path().join("scripts"),
            &repo.path().join(".trellis/hooks"),
        ) else {
            return;
        };
        let fp = Fingerprint::of(
            repo.path(),
            &hooks(&["python", ".trellis/hooks/gate.py"]),
            &[],
            None,
        );
        let why = fp.refused.expect("refused");
        assert!(
            why.contains("names `.trellis/hooks/gate.py`, which is `scripts/gate.py`"),
            "{why}"
        );
        let fp = Fingerprint::of(repo.path(), &hooks(&["python", "-m", "gate"]), &[], None);
        assert_eq!(fp.refused, None);
    }

    /// `path` relative to `base`, when both are on one volume and `path`
    /// can be reached from `base` with `..` steps.
    fn pathdiff(path: &Path, base: &Path) -> Option<PathBuf> {
        let path = std::fs::canonicalize(path).ok()?;
        let base = std::fs::canonicalize(base).ok()?;
        let p: Vec<_> = path.components().collect();
        let b: Vec<_> = base.components().collect();
        let common = p.iter().zip(&b).take_while(|(x, y)| x == y).count();
        if common == 0 {
            return None;
        }
        let mut out = PathBuf::new();
        for _ in common..b.len() {
            out.push("..");
        }
        for c in &p[common..] {
            out.push(c.as_os_str());
        }
        Some(out)
    }

    #[test]
    fn a_change_anywhere_under_dot_trellis_changes_the_digest_except_run_output() {
        // The risk: `python -m tools.hook` reaches code through an import,
        // which no argv names. Code under `.trellis/` is covered whole;
        // what trellis itself writes there (trails, the index cache) is not,
        // or every run would untrust the repository.
        let repo = tempfile::tempdir().unwrap();
        let hooks_dir = repo.path().join(".trellis/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("gate.py"), "print('ok')\n").unwrap();
        let h = hooks(&["python", "-m", "gate"]);
        let base = Fingerprint::of(repo.path(), &h, &[], None).sha256;

        std::fs::create_dir_all(repo.path().join(".trellis/runs")).unwrap();
        std::fs::write(repo.path().join(".trellis/runs/r.json"), "{}").unwrap();
        std::fs::write(repo.path().join(".trellis/index.bin"), [0u8, 1, 2]).unwrap();
        assert_eq!(
            base,
            Fingerprint::of(repo.path(), &h, &[], None).sha256,
            "trails and the index cache are trellis's output, not repository code"
        );

        std::fs::write(hooks_dir.join("gate.py"), "import os\n").unwrap();
        assert_ne!(base, Fingerprint::of(repo.path(), &h, &[], None).sha256);

        // With nothing declared there is still nothing to trust.
        assert!(Fingerprint::of(repo.path(), &HookConfig::default(), &[], None).is_empty());
    }

    /// A directory symlink, or `None` where the platform refuses to make one
    /// (Windows without developer mode), in which case the test says so.
    /// A committed repository with a hook under `.trellis/hooks/`, the
    /// `.gitignore` trellis writes (which ignores `work/`), and one task
    /// worktree at `.trellis/work/task/1/` added through `trellis_vcs`, as
    /// `trellis work` adds one.
    fn repo_with_task_worktree() -> (tempfile::TempDir, trellis_vcs::TaskWorktree) {
        use crate::testutil::git;
        let repo = tempfile::tempdir().unwrap();
        let r = repo.path();
        git(r, &["init", "-q"]);
        git(r, &["config", "user.email", "t@example.invalid"]);
        git(r, &["config", "user.name", "t"]);
        std::fs::create_dir_all(r.join(".trellis/hooks")).unwrap();
        std::fs::write(r.join(".trellis/hooks/gate.py"), "print('ok')\n").unwrap();
        std::fs::write(r.join(".trellis/.gitignore"), trellis_index::cache::IGNORE).unwrap();
        std::fs::write(r.join("app.py"), "x = 1\n").unwrap();
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", "init"]);
        let wt = trellis_vcs::TaskWorktree::create(r, "1", "HEAD").unwrap();
        (repo, wt)
    }

    fn digest_paths(root: &Path) -> Vec<String> {
        dot_trellis_files(root, &HashCache::default())
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    #[test]
    fn trust_linked_worktree_registered_under_work_is_left_out_of_the_digest() {
        // The risk (#718): a kept task worktree (ADR 0053) holds a checkout
        // of the repository under `.trellis/work/`, so the starting
        // worktree's `.trellis/` digest changed the moment one was kept,
        // and every later trust check there read "changed since it was
        // trusted"; and changed again with every edit a run made in it.
        let (repo, wt) = repo_with_task_worktree();
        let r = repo.path();
        std::fs::remove_dir_all(&wt.path).unwrap();
        crate::testutil::git(r, &["worktree", "prune"]);
        let h = hooks(&["python", "-m", "gate"]);
        let before = Fingerprint::of(r, &h, &[], None).sha256;

        let wt = trellis_vcs::TaskWorktree::create(r, "2", "HEAD").unwrap();
        assert!(wt.path.join(".trellis/hooks/gate.py").is_file());
        assert_eq!(
            Fingerprint::of(r, &h, &[], None).sha256,
            before,
            "a registered worktree under .trellis/work/ is not repository code to trust"
        );
        std::fs::write(wt.path.join(".trellis/hooks/gate.py"), "import os\n").unwrap();
        std::fs::write(wt.path.join("app.py"), "x = 2\n").unwrap();
        assert_eq!(Fingerprint::of(r, &h, &[], None).sha256, before);
        assert!(
            !digest_paths(r).iter().any(|p| p.contains("work/")),
            "{:?}",
            digest_paths(r)
        );

        // Pointers written relative (`worktree.useRelativePaths`), with `..`
        // in them, are resolved before they are compared.
        std::fs::write(
            wt.path.join(".git"),
            "gitdir: ../../../../.git/worktrees/../worktrees/2\n",
        )
        .unwrap();
        std::fs::write(
            r.join(".git/worktrees/2/gitdir"),
            "../../../.trellis/work/task/2/.git\n",
        )
        .unwrap();
        assert_eq!(Fingerprint::of(r, &h, &[], None).sha256, before);

        // Everything else under `.trellis/` is still covered.
        std::fs::write(r.join(".trellis/hooks/gate.py"), "import sys\n").unwrap();
        assert_ne!(Fingerprint::of(r, &h, &[], None).sha256, before);
    }

    #[test]
    fn trust_linked_worktree_forged_or_unregistered_is_still_fingerprinted() {
        // The risk (#718's security review): the skip keyed on something a
        // directory can claim for itself, so a planted directory under
        // `.trellis/work/` hides code from the digest. Each case below
        // plants `gate.py` in a directory that is not a registered linked
        // worktree at its own place, and the file must be in the digest.
        let (repo, wt) = repo_with_task_worktree();
        let r = repo.path();
        let real_root = std::fs::canonicalize(r).unwrap();
        let admin = real_root.join(".git/worktrees/1");
        let plant = |name: &str, dotgit: Option<&str>| {
            let d = r.join(".trellis/work").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("gate.py"), "print('planted')\n").unwrap();
            if let Some(text) = dotgit {
                std::fs::write(d.join(".git"), text).unwrap();
            }
            format!(".trellis/work/{name}/gate.py")
        };
        let mut expected = vec![
            // No `.git` at all.
            plant("bare", None),
            // Points somewhere that is not a worktree's directory.
            plant("elsewhere", Some("gitdir: /nonexistent/worktrees/x\n")),
            // Points at the repository's own git directory.
            plant(
                "common",
                Some(&format!("gitdir: {}\n", real_root.join(".git").display())),
            ),
            // Points at a registered worktree's directory, which names
            // task/1 back, not this one; `..` does not change that.
            plant(
                "borrowed",
                Some(&format!("gitdir: {}/../1\n", admin.display())),
            ),
            // Not in git's format.
            plant("garbled", Some(&format!("{}\n", admin.display()))),
        ];
        // A `.git` directory, not a file: a nested repository.
        let nested = r.join(".trellis/work/nested");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        expected.push(plant("nested", None));

        // A `.git` that is a symbolic link to a well-formed `.git` file.
        let linked = plant("linked", None);
        let link_target = r.join(".trellis/work/linked-gitfile");
        std::fs::write(&link_target, format!("gitdir: {}\n", admin.display())).unwrap();
        let have_links = trellis_core::paths::symlink_for_test(
            &link_target,
            &r.join(".trellis/work/linked/.git"),
        )
        .is_ok();
        expected.push(linked);
        expected.push(".trellis/work/linked-gitfile".into());

        // Registered once, then its registration removed (pruned).
        let gone = trellis_vcs::TaskWorktree::create(r, "3", "HEAD").unwrap();
        std::fs::write(gone.path.join("gate.py"), "print('pruned')\n").unwrap();
        std::fs::remove_dir_all(real_root.join(".git/worktrees/3")).unwrap();
        expected.push(".trellis/work/task/3/gate.py".into());

        let got = digest_paths(r);
        for path in &expected {
            assert!(got.contains(path), "{path} left out: {got:?}");
        }
        assert!(
            !got.iter().any(|p| p.starts_with(".trellis/work/task/1/")),
            "the registered one is still left out: {got:?}"
        );

        // A link under `.trellis/work/` to the registered worktree: an
        // alias is walked, so its files are covered under their real path.
        if have_links {
            symlink_dir(&wt.path, &r.join(".trellis/work/alias")).unwrap();
            let got = digest_paths(r);
            assert!(
                got.contains(&".trellis/work/task/1/.trellis/hooks/gate.py".to_string()),
                "{got:?}"
            );
        }
    }

    #[test]
    fn trust_linked_worktree_outside_work_is_still_fingerprinted() {
        // The risk: the skip reaching beyond `.trellis/work/`, so a person's
        // own `git worktree add .trellis/hooks-wt` hid hook code.
        let (repo, _wt) = repo_with_task_worktree();
        let r = repo.path();
        crate::testutil::git(
            r,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                ".trellis/elsewhere",
                "HEAD",
            ],
        );
        let got = digest_paths(r);
        assert!(
            got.contains(&".trellis/elsewhere/.trellis/hooks/gate.py".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn trust_linked_worktree_code_named_by_a_hook_is_refused() {
        // The risk: the digest leaves a task worktree out, and a run rooted
        // there edits it, so a hook naming a script in one (directly, or
        // through a link under `.trellis/`) would run code that is neither
        // hashed as `.trellis/` is nor out of a run's reach.
        let (repo, wt) = repo_with_task_worktree();
        let r = repo.path();
        let named = Fingerprint::of(
            r,
            &hooks(&["python", ".trellis/work/task/1/.trellis/hooks/gate.py"]),
            &[],
            None,
        );
        let why = named.refused.as_deref().expect("refused");
        assert!(
            why.contains("linked worktree `.trellis/work/task/1`"),
            "{why}"
        );
        assert!(why.contains(".trellis/hooks/"), "names the fix: {why}");

        if symlink_dir(
            &wt.path.join(".trellis/hooks"),
            &r.join(".trellis/wt-hooks"),
        )
        .is_some()
        {
            let through = Fingerprint::of(
                r,
                &hooks(&["python", ".trellis/wt-hooks/gate.py"]),
                &[],
                None,
            );
            let why = through.refused.as_deref().expect("refused through a link");
            assert!(why.contains("linked worktree"), "{why}");
        }

        // The starting worktree's own hook is not affected.
        let own = Fingerprint::of(r, &hooks(&["python", ".trellis/hooks/gate.py"]), &[], None);
        assert_eq!(own.refused, None);
    }

    fn symlink_dir(target: &Path, link: &Path) -> Option<()> {
        match trellis_core::paths::symlink_for_test(target, link) {
            Ok(()) => Some(()),
            Err(why) => {
                eprintln!("{why}");
                None
            }
        }
    }

    #[test]
    fn only_what_trellis_writes_is_left_out_of_the_dot_trellis_digest() {
        // The risk: the skip was every top-level name starting `index.bin`
        // and all of `runs/`, so `.trellis/index.bin.py` or
        // `.trellis/runs/gate.py` could hold hook code that changed after
        // trust without anyone being asked again.
        let repo = tempfile::tempdir().unwrap();
        let dot = repo.path().join(".trellis");
        std::fs::create_dir_all(dot.join("runs")).unwrap();
        let h = hooks(&["python", "-m", "gate"]);
        let digest = || Fingerprint::of(repo.path(), &h, &[], None).sha256;
        let base = digest();

        // What trellis writes: the index cache, trails, its own .gitignore.
        std::fs::write(dot.join("index.bin"), [0u8, 1]).unwrap();
        std::fs::write(dot.join("runs/20260101-x.json"), "{}").unwrap();
        std::fs::write(dot.join(".gitignore"), trellis_index::cache::IGNORE).unwrap();
        // And replay recordings with their own .gitignore (#620).
        std::fs::create_dir_all(dot.join("replays")).unwrap();
        std::fs::write(dot.join("replays/20260101-x.json"), "{}").unwrap();
        std::fs::write(
            dot.join("replays/.gitignore"),
            crate::scripted::REPLAY_IGNORE,
        )
        .unwrap();
        assert_eq!(
            base,
            digest(),
            "trellis's own output is not repository code"
        );

        for (name, body) in [
            (
                "index.bin.py",
                "import os
",
            ),
            (
                "runs/gate.py",
                "import os
",
            ),
            (
                "runs/x.json.py",
                "import os
",
            ),
            (
                "replays/gate.py",
                "import os
",
            ),
            (
                "replays/sub/x.json",
                "{}
",
            ),
        ] {
            let before = digest();
            let path = dot.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
            assert_ne!(before, digest(), "`.trellis/{name}` is not covered");
        }
        // A .gitignore a person wrote is theirs, and it is covered.
        let before = digest();
        std::fs::write(
            dot.join(".gitignore"),
            "index.bin
!gate.py
",
        )
        .unwrap();
        assert_ne!(before, digest(), "an edited .gitignore is not covered");
        let before = digest();
        std::fs::write(dot.join("replays/.gitignore"), "!gate.py\n").unwrap();
        assert_ne!(
            before,
            digest(),
            "an edited replays/.gitignore is not covered"
        );
    }

    #[test]
    fn a_symlinked_directory_under_dot_trellis_is_followed() {
        // The risk: `.trellis/hooks -> ../hooks` was skipped silently, so the
        // hook code it points at was covered by nothing.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("hooks")).unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        std::fs::write(
            repo.path().join("hooks/gate.py"),
            "print('ok')
",
        )
        .unwrap();
        let Some(()) = symlink_dir(
            &repo.path().join("hooks"),
            &repo.path().join(".trellis/hooks"),
        ) else {
            return;
        };
        let h = hooks(&["python", "-m", "gate"]);
        let fp = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(fp.refused, None);
        let dot = fp.items.last().unwrap();
        assert!(
            // Under its real path, whichever alias reached it.
            dot.files.iter().any(|(p, _)| p == "hooks/gate.py"),
            "{:?}",
            dot.files
        );
        std::fs::write(
            repo.path().join("hooks/gate.py"),
            "import os
",
        )
        .unwrap();
        assert_ne!(
            fp.sha256,
            Fingerprint::of(repo.path(), &h, &[], None).sha256
        );
    }

    #[test]
    fn a_symlink_under_dot_trellis_that_leaves_the_repository_refuses_trust() {
        // Following it would hash whatever it points at (a home directory, a
        // whole disk); not following it would leave that code uncovered.
        // Neither is trust in what runs, so it is refused, by name.
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        let Some(()) = symlink_dir(outside.path(), &repo.path().join(".trellis/lib")) else {
            return;
        };
        let fp = Fingerprint::of(repo.path(), &hooks(&["x"]), &[], None);
        let why = fp.refused.clone().expect("refused");
        assert!(
            why.contains(".trellis/lib") && why.contains("outside the repository"),
            "{why}"
        );
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        let e = store.trust(repo.path(), &fp).unwrap_err().to_string();
        assert!(e.contains("outside the repository"), "{e}");
        assert_ne!(store.state(repo.path(), &fp).unwrap(), TrustState::Trusted);
    }

    #[test]
    fn a_file_reached_through_two_links_is_recorded_once_under_its_real_path() {
        // The risk: a directory reached twice was recorded under whichever
        // alias `read_dir` returned first, so the digest of one repository
        // depended on directory order, and could differ between two
        // checkouts of the same commit.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("hooks")).unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        std::fs::write(repo.path().join("hooks/gate.py"), "print('ok')\n").unwrap();
        for alias in ["b", "a"] {
            let Some(()) = symlink_dir(
                &repo.path().join("hooks"),
                &repo.path().join(".trellis").join(alias),
            ) else {
                return;
            };
        }
        let fp = Fingerprint::of(repo.path(), &hooks(&["x"]), &[], None);
        assert_eq!(fp.refused, None);
        let files: Vec<&str> = fp
            .items
            .last()
            .unwrap()
            .files
            .iter()
            .map(|f| f.0.as_str())
            .collect();
        assert_eq!(files, ["hooks/gate.py"]);
    }

    #[test]
    fn a_link_under_dot_trellis_to_itself_an_ancestor_or_the_git_directory_refuses_trust() {
        // The risk: `.trellis/src -> ..` resolves inside the repository, so
        // it was followed, and the digest hashed the whole repository,
        // `.git/` included: every commit, every fetch, every index refresh
        // untrusted it again. A link into `.git/` covers files git rewrites
        // on its own.
        for (name, target, says) in [
            ("src", "", "contains .trellis/"),
            ("self", ".trellis", "contains .trellis/"),
            ("objects", ".git", "inside .git/"),
        ] {
            let repo = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
            std::fs::create_dir_all(repo.path().join(".git/objects")).unwrap();
            let Some(()) = symlink_dir(
                &repo.path().join(target),
                &repo.path().join(".trellis").join(name),
            ) else {
                return;
            };
            let fp = Fingerprint::of(repo.path(), &hooks(&["x"]), &[], None);
            let why = fp.refused.clone().unwrap_or_default();
            assert!(
                why.contains(&format!(".trellis/{name}")) && why.contains(says),
                "{name} -> {target:?}: {why:?}"
            );
            let home = tempfile::tempdir().unwrap();
            let store = TrustStore::at(home.path().join("trust.json"));
            assert!(store.trust(repo.path(), &fp).is_err());
        }
    }

    #[test]
    fn a_dot_trellis_that_is_itself_a_link_out_of_the_repository_refuses_trust() {
        // The risk: only links *under* `.trellis/` were checked, so a
        // `.trellis` that was itself a link was walked wherever it led.
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let Some(()) = symlink_dir(outside.path(), &repo.path().join(".trellis")) else {
            return;
        };
        let fp = Fingerprint::of(repo.path(), &hooks(&["x"]), &[], None);
        let why = fp.refused.clone().unwrap_or_default();
        assert!(why.contains("`.trellis` resolves to"), "{why:?}");
    }

    #[test]
    fn a_trail_reached_through_an_alias_is_still_a_trail() {
        // The skips are decided on the real path, so `.trellis/r -> runs`
        // does not bring the trails into the digest (every run would then
        // untrust the repository), whichever name is walked first.
        let repo = tempfile::tempdir().unwrap();
        let dot = repo.path().join(".trellis");
        std::fs::create_dir_all(dot.join("runs")).unwrap();
        let Some(()) = symlink_dir(&dot.join("runs"), &dot.join("r")) else {
            return;
        };
        let h = hooks(&["x"]);
        let base = Fingerprint::of(repo.path(), &h, &[], None);
        assert_eq!(base.refused, None);
        std::fs::write(dot.join("runs/20260101-x.json"), "{}").unwrap();
        assert_eq!(
            base.sha256,
            Fingerprint::of(repo.path(), &h, &[], None).sha256
        );
    }

    #[test]
    fn bytecode_under_dot_trellis_is_covered() {
        // Guard, not a regression: the fourth review asked for `.pyc` and
        // `__pycache__` to be left out, as derived from hashed sources. They
        // are not only that. `import gate` loads a sourceless `gate.pyc`,
        // and an unchecked-hash pyc in `__pycache__` (PEP 552) is loaded
        // without comparing it to the source. Either is code the repository
        // supplied, so both stay in the digest; trellis instead starts every
        // command with PYTHONDONTWRITEBYTECODE=1, so its own hook runs write
        // none (`exec::tests`).
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path().join(".trellis/hooks");
        std::fs::create_dir_all(dir.join("__pycache__")).unwrap();
        let h = hooks(&["python", "-m", "gate"]);
        let digest = || Fingerprint::of(repo.path(), &h, &[], None).sha256;
        for name in ["gate.pyc", "__pycache__/gate.cpython-312.pyc", "gate.pyo"] {
            let before = digest();
            std::fs::write(dir.join(name), [0x55u8, 0x0d, 0x0d, 0x0a]).unwrap();
            assert_ne!(before, digest(), "`.trellis/hooks/{name}` is not covered");
        }
    }

    #[test]
    fn a_refused_repository_reports_refused_not_changed_or_unknown() {
        // The risk: with a trust entry on record, a refused fingerprint read
        // `Changed`, and without one `Unknown`; both sent the person to
        // `trellis trust`, which then refused. The state names the link.
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = TrustStore::at(home.path().join("trust.json"));
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        let h = hooks(&["x"]);
        let fine = Fingerprint::of(repo.path(), &h, &[], None);
        let Some(()) = symlink_dir(outside.path(), &repo.path().join(".trellis/lib")) else {
            return;
        };
        let refused = Fingerprint::of(repo.path(), &h, &[], None);
        let is_refused = |s: TrustState| matches!(s, TrustState::Refused { ref reason } if reason.contains(".trellis/lib"));
        assert!(
            is_refused(store.state(repo.path(), &refused).unwrap()),
            "no entry"
        );
        store.trust(repo.path(), &fine).unwrap();
        assert!(
            is_refused(store.state(repo.path(), &refused).unwrap()),
            "an entry"
        );
    }

    #[test]
    fn nothing_declared_is_nothing_to_trust() {
        let repo = tempfile::tempdir().unwrap();
        assert!(Fingerprint::of(repo.path(), &HookConfig::default(), &[], None).is_empty());
    }

    /// A guard over `hooks(["python", ".trellis/hook.py"])` in `root`, built as the
    /// binary builds one.
    fn guard(root: &Path) -> TrustGuard {
        let root = root.to_path_buf();
        let h = hooks(&["python", ".trellis/hook.py"]);
        let now = move |c: &HashCache| Fingerprint::of_cached(&root, &h, &[], None, Vec::new(), c);
        TrustGuard::over(now(&HashCache::default()), now)
    }

    #[test]
    fn a_cached_recheck_still_sees_a_rewrite_and_an_added_file() {
        // The risk in caching: a digest reused for a file that changed. The
        // stat differs here (length), so the file is hashed again; and the
        // tree is walked every time, so a new file under `.trellis/` is seen.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        std::fs::write(repo.path().join(".trellis/hook.py"), "print('ok')\n").unwrap();
        let g = guard(repo.path());
        g.check().unwrap();
        g.check().unwrap();
        std::fs::write(
            repo.path().join(".trellis/hook.py"),
            "import os; os.system('x')\n",
        )
        .unwrap();
        let e = g.check().unwrap_err().to_string();
        assert!(e.contains("hook.py"), "{e}");

        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        std::fs::write(repo.path().join(".trellis/hook.py"), "print('ok')\n").unwrap();
        let g = guard(repo.path());
        g.check().unwrap();
        std::fs::write(repo.path().join(".trellis/sitecustomize.py"), "x").unwrap();
        let e = g.check().unwrap_err().to_string();
        assert!(e.contains(".trellis/sitecustomize.py"), "{e}");
    }

    #[test]
    fn a_rewrite_keeping_length_and_mtime_is_caught_by_the_full_check() {
        // What the cache can miss, where no change time is exposed
        // (Windows): same length, modification time set back. The full
        // check, which the binary runs at convergence, hashes afresh.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".trellis")).unwrap();
        let hook = repo.path().join(".trellis/hook.py");
        std::fs::write(&hook, "print('aa')\n").unwrap();
        let g = guard(repo.path());
        g.check().unwrap();
        let mtime = std::fs::metadata(&hook).unwrap().modified().unwrap();
        std::fs::write(&hook, "print('zz')\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&hook)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(std::fs::metadata(&hook).unwrap().len(), 12);
        let e = g.check_full().unwrap_err().to_string();
        assert!(e.contains("hook.py"), "{e}");
    }

    #[test]
    fn a_trust_recheck_leaves_the_runtime_free() {
        // The risk: the recheck walked and hashed `.trellis/` on the
        // single-threaded runtime, so Ctrl-C, `abort` and approvals waited
        // for it. Run as the call sites run it, a timer on the same runtime
        // fires while the check is still going.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let g = TrustGuard::new(|| {
                std::thread::sleep(std::time::Duration::from_millis(400));
                Ok(())
            });
            let started = std::time::Instant::now();
            let timer = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                started.elapsed()
            });
            g.check_async().await.unwrap();
            let fired = timer.await.unwrap();
            assert!(
                fired < std::time::Duration::from_millis(300),
                "the timer waited for the check: {fired:?}"
            );
        });
    }

    #[test]
    fn a_full_check_after_a_breach_still_fails() {
        // The latch holds whichever check found the breach.
        let g = TrustGuard::new(|| Err("changed".into()));
        assert!(g.check().is_err());
        assert_eq!(g.check_full().unwrap_err().to_string(), "changed");
    }

    #[test]
    fn trust_reason_escaped_so_a_label_or_path_cannot_add_a_line_or_repaint_one() {
        // The risk (#192): the refusal is printed as `state:   cannot be
        // trusted: {why}`, and the guard's error is printed as is. A label
        // or path with a newline in it could add an indented line that reads
        // as trellis's own ("state: trusted"), and an escape sequence could
        // repaint the line that says what is wrong. Built by hand: no file
        // system this runs on will create a file with a newline in its name.
        let repo = tempfile::tempdir().unwrap();
        let label = "custom tool x\n    state:   trusted\x1b[2K";
        let path = "scripts/x\n  fake\x1b[31m.py";
        let item = |file: &str| TrustItem {
            label: label.to_string(),
            argv: Vec::new(),
            detail: String::new(),
            files: vec![(file.to_string(), "0".repeat(64))],
            receives: Vec::new(),
        };
        let inert = |msg: &str| {
            assert!(!msg.contains('\n') && !msg.contains('\x1b'), "{msg:?}");
            assert!(msg.contains(r"\n") && msg.contains(r"\u{1b}["), "{msg:?}");
        };

        let why = outside_code(repo.path(), &[item(path)]).expect("refused");
        inert(&why);
        assert!(
            why.contains(r"custom tool x\n    state:   trusted\u{1b}[2K"),
            "{why}"
        );
        assert!(why.contains(r"scripts/x\n  fake\u{1b}[31m.py"), "{why}");

        let fp = |files: &str, refused: Option<String>| Fingerprint {
            items: vec![item(files)],
            sha256: String::new(),
            refused,
        };
        let was = fp(".trellis/hooks/gate.py", None);
        let changed = describe_change(&was, &fp(&format!(".trellis/{path}"), None));
        inert(&changed);
        assert!(
            changed.contains(r"scripts/x\n  fake\u{1b}[31m.py"),
            "{changed}"
        );
        let refused = describe_change(&was, &fp(".trellis/hooks/gate.py", Some(why)));
        inert(&refused);
    }

    fn hook(argv: &[&str]) -> TrustItem {
        TrustItem {
            label: "hook before_run".to_string(),
            argv: argv.iter().map(|a| a.to_string()).collect(),
            detail: String::new(),
            files: Vec::new(),
            receives: Vec::new(),
        }
    }

    #[test]
    fn trust_package_path_named_by_absolute_path_is_refused() {
        // The risk (#201): a trusted hook runs a package's file as code; a
        // package update then changes the hook with the digest unchanged.
        let repo = tempfile::tempdir().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let pkgs = cfg.path().join("packages");
        std::fs::create_dir_all(pkgs.join("somepkg")).unwrap();
        let file = pkgs.join("somepkg").join("gate.py");
        std::fs::write(&file, "print('x')").unwrap();
        let why = package_code(
            Some(&pkgs),
            repo.path(),
            &[hook(&["python", &file.display().to_string()])],
        )
        .expect("refused");
        assert!(why.contains("hook before_run names"), "{why}");
        assert!(why.contains("packages directory"), "{why}");
        // One sentence, not a line broken by a run of spaces.
        assert!(!why.contains("  "), "{why}");
    }

    #[test]
    fn trust_package_path_inside_a_longer_argument_is_refused() {
        // The tokenizer splits `--script=<path>`; a package path hidden in
        // one argument is refused as one standing alone is.
        let repo = tempfile::tempdir().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let pkgs = cfg.path().join("packages");
        std::fs::create_dir_all(pkgs.join("p")).unwrap();
        let file = pkgs.join("p").join("x.sh");
        std::fs::write(&file, "true").unwrap();
        let arg = format!("--script={}", file.display());
        assert!(package_code(Some(&pkgs), repo.path(), &[hook(&["runner", &arg])]).is_some());
    }

    #[test]
    fn trust_package_path_rule_leaves_programs_elsewhere_alone() {
        // A program on PATH or a file outside the packages directory is the
        // user's system, as before; and with no packages directory there is
        // nothing to refuse.
        let repo = tempfile::tempdir().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let pkgs = cfg.path().join("packages");
        std::fs::create_dir_all(&pkgs).unwrap();
        let other = cfg.path().join("tool.sh");
        std::fs::write(&other, "true").unwrap();
        let items = [hook(&[
            "ruff",
            "check",
            "src",
            &other.display().to_string(),
        ])];
        assert!(package_code(Some(&pkgs), repo.path(), &items).is_none());
        assert!(package_code(None, repo.path(), &items).is_none());
    }
}
