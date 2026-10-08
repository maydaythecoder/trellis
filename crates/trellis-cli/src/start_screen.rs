//! The start screen bare `trellis` opens outside a repository (ADR 0050 §3).
//!
//! Bare `trellis` in a home directory, or in a directory of repositories,
//! used to refuse with "cannot determine the repository root", and the
//! person had to know where to go next. This lists the repositories trellis
//! last resolved a root in, takes a typed path inside a repository, and
//! offers `git init` in the current directory unless that is the home
//! directory.
//! Every other command keeps the refusal: nothing here walks a tree that is
//! not a repository, and the reason `root_from` refuses (a home directory
//! indexed by accident) stands.
//!
//! The list lives in `recent.json` in the user config directory, beside the
//! trust store: `{"version":1,"repositories":["<absolute path>", ...]}`,
//! newest first, at most [`MAX`]. ADR 0050 §3 is the record of that file.
//!
//! Split like `first_run`: [`record`] and [`listed`] take the file as a
//! parameter, and [`screen`] reads its answers from whatever reader it is
//! given, so each is testable without a home directory or a terminal.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::plain;

/// How many repositories the list keeps.
pub(crate) const MAX: usize = 10;

/// The file's format version; a file with another is ignored, not guessed at.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Recent {
    version: u32,
    repositories: Vec<String>,
}

/// `<user config dir>/recent.json`; `None` where there is no user config
/// directory, and in this crate's unit tests (`user_config::dir`).
pub(crate) fn file() -> Option<PathBuf> {
    crate::user_config::dir().map(|d| d.join("recent.json"))
}

/// What the file holds, in its order. A missing file is an empty list; one
/// that does not parse, or has another version, is an error naming it.
fn read(file: &Path) -> Result<Vec<String>> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let r: Recent = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a list of recent repositories", file.display()))?;
    if r.version != VERSION {
        anyhow::bail!(
            "{} has version {}, not {VERSION}",
            file.display(),
            r.version
        );
    }
    Ok(r.repositories)
}

/// Put `root` first in the list at `file`: deduplicated, at most [`MAX`].
///
/// Entries that are no longer a repository are dropped before the cap, so
/// deleted agent worktrees and demo directories never push a live
/// repository off the end of the list.
///
/// A root that is not valid UTF-8 is not recorded, since the file is JSON
/// text. Written beside the file under this process's id and renamed over
/// it, so two trellis processes recording at once each leave a whole file.
pub(crate) fn record(file: &Path, root: &Path) -> Result<()> {
    let root =
        std::path::absolute(root).with_context(|| format!("making {} absolute", root.display()))?;
    let Some(root) = root.to_str().map(str::to_string) else {
        return Ok(());
    };
    let mut list = read(file)?;
    list.retain(|r| *r != root && is_repository(Path::new(r)));
    list.insert(0, root);
    list.truncate(MAX);
    let text = serde_json::to_string_pretty(&Recent {
        version: VERSION,
        repositories: list,
    })?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("replacing {}", file.display())
    })
}

/// The recorded repositories that still are one, newest first, and whether
/// the file named any repository at all. A path whose `.git` is gone is
/// dropped here; nothing else on disk is read.
pub(crate) fn listed(file: &Path) -> Result<(Vec<PathBuf>, bool)> {
    let entries = read(file)?;
    let live = entries
        .iter()
        .map(PathBuf::from)
        .filter(|p| is_repository(p))
        .take(MAX)
        .collect();
    Ok((live, !entries.is_empty()))
}

/// Whether `root` is under `temp`, the temporary directory. Test fixtures
/// and scratch checkouts live there and are deleted within minutes, so
/// recording them only fills the list with entries that are soon gone.
/// Compared as given and resolved, since a temporary directory is often
/// named through a link (`/tmp`, a short Windows name).
pub(crate) fn is_temporary(root: &Path, temp: &Path) -> bool {
    let resolved = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    root.starts_with(temp) || resolved(root).starts_with(resolved(temp))
}

/// What `record` keeps and `listed` shows: an absolute path with a `.git`.
fn is_repository(p: &Path) -> bool {
    p.is_absolute() && p.join(".git").exists()
}

/// The screen: `recent` numbered, a typed path, then the `git init` offer
/// for `cwd`. Returns the root to open, or `None` when the person quits (`q`
/// or end of input). `git init` runs only after an explicit `y`.
///
/// `recorded` is whether the list named any repository at all, so an empty
/// `recent` can say whether trellis never opened one or every one it opened
/// is gone. `home` is the user's home directory, a parameter so tests pass a
/// temporary one: there `git init` is not offered, since a repository in a
/// home directory takes in everything under it.
pub(crate) fn screen(
    recent: &[PathBuf],
    recorded: bool,
    cwd: &Path,
    home: Option<&Path>,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Option<PathBuf>> {
    let here = plain(&cwd.display().to_string());
    let at_home = home.is_some_and(|h| same_dir(cwd, h));
    writeln!(out, "{here} is not inside a git repository.")?;
    if recent.is_empty() && recorded {
        writeln!(
            out,
            "Every repository trellis opened before is gone: none still has a .git."
        )?;
    } else if recent.is_empty() {
        writeln!(out, "trellis has not opened any repository yet.")?;
    } else {
        writeln!(out, "Repositories trellis opened recently:")?;
        for (i, r) in recent.iter().enumerate() {
            writeln!(out, "  {}. {}", i + 1, plain(&r.display().to_string()))?;
        }
    }
    writeln!(out, "  or type the path of a repository")?;
    if at_home {
        writeln!(
            out,
            "  This is your home directory, so `git init` is not offered: a repository there would take in every file under it."
        )?;
    } else {
        writeln!(out, "  i. run `git init` here, in {here}")?;
    }
    let numbers = match recent.len() {
        0 => String::new(),
        1 => "1, ".to_string(),
        n => format!("1 to {n}, "),
    };
    let init_choice = if at_home { "" } else { "i, " };
    loop {
        write!(out, "Choose {numbers}a path, {init_choice}or q to quit: ")?;
        out.flush()?;
        let Some(t) = answer(input)? else {
            return Ok(None);
        };
        if t.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        if t.eq_ignore_ascii_case("i") && at_home {
            writeln!(out, "That is not one of the choices.")?;
            continue;
        }
        if t.eq_ignore_ascii_case("i") {
            write!(out, "Create a git repository in {here}? [y/N] ")?;
            out.flush()?;
            if !matches!(answer(input)?.as_deref(), Some("y" | "Y")) {
                writeln!(out, "Nothing was created.")?;
                return Ok(None);
            }
            init(cwd)?;
            writeln!(out, "Initialised a git repository in {here}.")?;
            return Ok(Some(cwd.to_path_buf()));
        }
        match t.parse::<usize>() {
            Ok(n) if (1..=recent.len()).contains(&n) => return Ok(Some(recent[n - 1].clone())),
            Ok(_) => writeln!(out, "That is not one of the choices.")?,
            Err(_) if t.is_empty() => writeln!(out, "That is not one of the choices.")?,
            Err(_) => {
                let path = typed(&t, cwd, home);
                let shown = plain(&path.display().to_string());
                if !path.exists() {
                    writeln!(out, "{shown} does not exist.")?;
                    continue;
                }
                // `root_from` is the walk every command uses: up from the
                // path to the first `.git`, never into the tree (ADR 0050 §3).
                match crate::root_from(&path) {
                    Some(root) => return Ok(Some(root)),
                    None => writeln!(out, "{shown} is not inside a git repository.")?,
                }
            }
        }
    }
}

/// A typed answer as a path: a leading `~` is `home`, and a relative path
/// is taken from `cwd`, where the person is standing.
fn typed(t: &str, cwd: &Path, home: Option<&Path>) -> PathBuf {
    let rest = if t == "~" {
        Some("")
    } else {
        t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\"))
    };
    match (rest, home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => cwd.join(t),
    }
}

/// The home directory: `USERPROFILE` on Windows, else `HOME`, as
/// `user_config` reads it. An empty variable counts as unset.
fn home() -> Option<PathBuf> {
    let get = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        get("USERPROFILE").or_else(|| get("HOME"))
    } else {
        get("HOME")
    }
}

/// Whether `a` and `b` name one directory, compared as given and resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    let resolved = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || resolved(a) == resolved(b)
}

/// One trimmed line, or `None` at end of input.
fn answer(input: &mut dyn BufRead) -> Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

/// `git init` in `dir`, through `trellis_vcs` like every git trellis starts.
fn init(dir: &Path) -> Result<()> {
    let out = trellis_vcs::command()
        .arg("init")
        .current_dir(dir)
        .output()
        .context("starting git")?;
    if !out.status.success() {
        anyhow::bail!(
            "could not create a git repository in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// [`screen`] on the process's stdin and stderr, listing the user's file.
/// The caller has checked they are terminals. A list that cannot be read is
/// said once and the screen opens without it: `git init` is still on offer.
pub(crate) fn offer() -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    let mut err = std::io::stderr();
    let (recent, recorded) = match file().map(|f| listed(&f)).transpose() {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => {
            writeln!(err, "recent repositories not shown: {e:#}")?;
            (Vec::new(), false)
        }
    };
    screen(
        &recent,
        recorded,
        &cwd,
        home().as_deref(),
        &mut std::io::stdin().lock(),
        &mut err,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that `listed` counts as a repository: it has a `.git`.
    fn repo_at(base: &Path, name: &str) -> PathBuf {
        let p = base.join(name);
        std::fs::create_dir_all(p.join(".git")).unwrap();
        p
    }

    #[test]
    fn start_screen_recent_list_is_newest_first_deduplicated_and_capped() {
        // The risk: the list grows without bound, repeats a repository, or
        // shows the oldest first, so the one just used is not the one at 1.
        let user = tempfile::tempdir().unwrap();
        let file = user.path().join("recent.json");
        let base = tempfile::tempdir().unwrap();
        let repos: Vec<PathBuf> = (0..12)
            .map(|i| repo_at(base.path(), &format!("r{i}")))
            .collect();
        for r in &repos {
            record(&file, r).unwrap();
        }
        // r3 again: it moves to the front rather than appearing twice.
        record(&file, &repos[3]).unwrap();

        let want: Vec<PathBuf> = [3, 11, 10, 9, 8, 7, 6, 5, 4, 2]
            .iter()
            .map(|&i| repos[i].clone())
            .collect();
        assert_eq!(listed(&file).unwrap().0, want);
        assert_eq!(read(&file).unwrap().len(), MAX);

        // A repository that no longer exists is dropped when listed.
        std::fs::remove_dir_all(&repos[11]).unwrap();
        let mut gone = want.clone();
        gone.retain(|p| *p != repos[11]);
        assert_eq!(listed(&file).unwrap().0, gone);

        // The screen numbers them in that order, and a number opens one.
        let mut shown = Vec::new();
        let chosen = screen(
            &gone,
            true,
            base.path(),
            None,
            &mut "2\n".as_bytes(),
            &mut shown,
        )
        .unwrap();
        assert_eq!(chosen, Some(repos[10].clone()));
        let shown = String::from_utf8(shown).unwrap();
        assert!(
            shown.contains(&format!("  1. {}", repos[3].display())),
            "{shown}"
        );
    }

    #[test]
    fn start_screen_git_init_runs_only_after_an_explicit_y() {
        // The risk: a repository is created in a home directory because the
        // person pressed Enter, or answered anything but yes.
        for input in ["i\n\n", "i\nn\n", "i\nyes please\n", "i\n", "q\n", ""] {
            let cwd = tempfile::tempdir().unwrap();
            let mut shown = Vec::new();
            let got = screen(
                &[],
                false,
                cwd.path(),
                None,
                &mut input.as_bytes(),
                &mut shown,
            )
            .unwrap();
            assert_eq!(got, None, "{input:?}");
            assert!(
                !cwd.path().join(".git").exists(),
                "{input:?} created a repository: {}",
                String::from_utf8_lossy(&shown)
            );
        }
        let cwd = tempfile::tempdir().unwrap();
        let got = screen(
            &[],
            false,
            cwd.path(),
            None,
            &mut "i\ny\n".as_bytes(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(got.as_deref(), Some(cwd.path()));
        assert!(cwd.path().join(".git").exists());
    }

    #[test]
    fn start_screen_record_prunes_deleted_repositories_before_capping() {
        // The risk (the owner's first run): agent worktrees and demo
        // directories, since deleted, fill the list, and recording one more
        // root pushes the person's real repository off the end. The start
        // screen then lists nothing at all.
        let user = tempfile::tempdir().unwrap();
        let file = user.path().join("recent.json");
        let base = tempfile::tempdir().unwrap();
        let repos: Vec<PathBuf> = (0..MAX)
            .map(|i| repo_at(base.path(), &format!("r{i}")))
            .collect();
        for r in &repos {
            record(&file, r).unwrap();
        }
        // r0, recorded first, is the oldest and the one left alive.
        for r in &repos[1..] {
            std::fs::remove_dir_all(r).unwrap();
        }
        let new = repo_at(base.path(), "new");
        record(&file, &new).unwrap();
        assert_eq!(listed(&file).unwrap().0, vec![new, repos[0].clone()]);
    }

    #[test]
    fn start_screen_typed_path_opens_the_enclosing_root_and_refuses_one_outside() {
        // The risk: with nothing listed the screen offers only `git init`
        // or quit, a dead end for anyone not wanting a repository in their
        // home directory; or a typed path outside a repository is accepted
        // and its tree walked.
        let base = tempfile::tempdir().unwrap();
        let above: Vec<_> = base
            .path()
            .ancestors()
            .filter(|a| a.join(".git").exists())
            .collect();
        assert!(
            above.is_empty(),
            "the temp directory is in a repository: {above:?}"
        );
        let repo = repo_at(base.path(), "repo");
        let deep = repo.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        let outside = base.path().join("plain");
        std::fs::create_dir_all(&outside).unwrap();

        let input = format!("{}\n{}\n", outside.display(), deep.display());
        let mut shown = Vec::new();
        let got = screen(
            &[],
            false,
            base.path(),
            None,
            &mut input.as_bytes(),
            &mut shown,
        )
        .unwrap();
        assert_eq!(got, Some(repo));
        let shown = String::from_utf8(shown).unwrap();
        assert!(
            shown.contains("or type the path of a repository"),
            "{shown}"
        );
        assert!(
            shown.contains(&format!(
                "{} is not inside a git repository.",
                outside.display()
            )),
            "{shown}"
        );
        // It asked again after the refusal: the prompt appears twice.
        assert_eq!(shown.matches("or q to quit: ").count(), 2, "{shown}");
        assert!(!outside.join(".git").exists());
    }

    #[test]
    fn start_screen_typed_path_expands_a_leading_tilde() {
        // The risk: `~/code/x` is read as a directory named `~` under the
        // working directory, and the person's repository is refused.
        let cwd = Path::new("/w");
        let home = Path::new("/h");
        assert_eq!(typed("~", cwd, Some(home)), home.join(""));
        assert_eq!(typed("~/a/b", cwd, Some(home)), home.join("a/b"));
        assert_eq!(typed("a/~", cwd, Some(home)), cwd.join("a/~"));
        assert_eq!(typed("~x", cwd, Some(home)), cwd.join("~x"));
        assert_eq!(typed("~/a", cwd, None), cwd.join("~/a"));
    }

    #[test]
    fn start_screen_home_no_git_init_offer_in_the_home_directory() {
        // The risk (the owner's first run): bare `trellis` in the home
        // directory offers `git init` there, and one `i`, `y` makes every
        // file under it part of a repository.
        let home = tempfile::tempdir().unwrap();
        let mut shown = Vec::new();
        let got = screen(
            &[],
            false,
            home.path(),
            Some(home.path()),
            &mut "i
y
"
            .as_bytes(),
            &mut shown,
        )
        .unwrap();
        let shown = String::from_utf8(shown).unwrap();
        assert_eq!(got, None, "{shown}");
        assert!(!home.path().join(".git").exists(), "{shown}");
        assert!(!shown.contains("i. run `git init`"), "{shown}");
        assert!(
            shown.contains("This is your home directory, so `git init` is not offered"),
            "{shown}"
        );
        assert!(shown.contains("That is not one of the choices."), "{shown}");

        // Anywhere else, the offer stands.
        let cwd = tempfile::tempdir().unwrap();
        let mut shown = Vec::new();
        screen(
            &[],
            false,
            cwd.path(),
            Some(home.path()),
            &mut "q
"
            .as_bytes(),
            &mut shown,
        )
        .unwrap();
        let shown = String::from_utf8(shown).unwrap();
        assert!(shown.contains("i. run `git init`"), "{shown}");
    }

    #[test]
    fn start_screen_home_says_when_every_listed_repository_is_gone() {
        // The risk: a person whose repositories were all deleted or moved is
        // told trellis never opened one, and goes looking for a list that
        // was there.
        let cwd = tempfile::tempdir().unwrap();
        let said = |recorded: bool| {
            let mut shown = Vec::new();
            screen(
                &[],
                recorded,
                cwd.path(),
                None,
                &mut "q
"
                .as_bytes(),
                &mut shown,
            )
            .unwrap();
            String::from_utf8(shown).unwrap()
        };
        let gone = said(true);
        assert!(
            gone.contains("Every repository trellis opened before is gone"),
            "{gone}"
        );
        assert!(!gone.contains("has not opened any repository"), "{gone}");
        let never = said(false);
        assert!(
            never.contains("trellis has not opened any repository yet."),
            "{never}"
        );
        assert!(!never.contains("is gone"), "{never}");
    }

    #[test]
    fn start_screen_an_unreadable_list_is_an_error_naming_the_file() {
        // The risk: a corrupt file silently reads as "no repositories", or
        // a record over it throws away whatever the person had.
        let user = tempfile::tempdir().unwrap();
        let file = user.path().join("recent.json");
        std::fs::write(&file, "not json").unwrap();
        let err = listed(&file).unwrap_err();
        assert!(format!("{err:#}").contains("recent.json"), "{err:#}");
        assert!(record(&file, user.path()).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
    }
}
