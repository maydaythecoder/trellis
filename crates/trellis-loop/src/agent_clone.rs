//! A process agent in a throwaway clone (ADR 0032, "Process agents").
//!
//! `bash` and custom tools are calls inside a model run, and the model has
//! `edit` and `write_file` for the rest, so their carry-back brings back
//! edits to files trellis tracks and nothing else. A process agent (ADR
//! 0026 §11) has no other write path: what it leaves in its clone is the
//! run's whole change. So its carry-back also brings back the files it
//! created and the tracked files it deleted, under the same refusals, and
//! every refusal is reported so the run does not converge on part of a
//! change.
//!
//! Both extra kinds stay inside what ADR 0009's rollback restores: a
//! tracked file deleted is put back by `git reset --hard`, and a new file
//! is written only where the repository's own ignore rules do not ignore
//! it, so `git clean -fd` removes it. The repository's rules, read by a
//! `git check-ignore` in the repository, never the clone's `.gitignore`,
//! which is the agent's to edit.

use crate::governance::Governance;
use crate::isolate::{GIT_LOCATION_VARS, Sandbox, hash_file, is_git_instruction_file};
use crate::orchestrator::{names_trellis_dir, real_rel, write_refusal};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// The most files and directories git does not ignore that the search for
/// new files visits in a clone. The walk is over a directory the agent
/// wrote, so it is bounded; a clone holding more is refused whole rather
/// than carried back in part.
pub const NEW_FILE_WALK_CAP: usize = 100_000;

/// A throwaway clone of the repository for one process agent run.
///
/// Removed when dropped.
pub struct AgentClone(Sandbox);

/// Why a clone could not be prepared. The agent is then not run: running
/// it in the repository instead is what the clone exists to stop.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AgentCloneError(String);

/// A path the carry-back did not write, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotCarried {
    pub path: String,
    pub why: String,
}

/// What the carry-back did, for the run to judge and report.
#[derive(Debug, Default)]
pub struct CarriedBack {
    /// Tracked files whose new bytes were written into the repository.
    pub written: Vec<String>,
    /// New files written into the repository.
    pub created: Vec<String>,
    /// Tracked files removed from the repository.
    pub deleted: Vec<String>,
    /// New files the repository's rules ignore, left in the clone. Not a
    /// refusal: they cannot reach the repository, as a model run's caches
    /// do not become part of its change.
    pub ignored: Vec<String>,
    /// Refused by what judges the run: `.trellis/`, `deny_write`, the
    /// profile's `allow_write`. `why` is in the words the post-command
    /// judge uses (`deny_write \`app/\``), so a person sees one vocabulary.
    pub breaches: Vec<NotCarried>,
    /// Refused for any other reason (under `.git`, a link, too large, a git
    /// instruction file, changed in the repository meanwhile). `why` is a
    /// whole sentence naming the path.
    pub refused: Vec<NotCarried>,
}

impl CarriedBack {
    /// Whether every change the agent made in the clone came back.
    pub fn complete(&self) -> bool {
        self.breaches.is_empty() && self.refused.is_empty()
    }
}

impl AgentClone {
    /// Clones `root` and copies its files in, as for a `bash` call.
    ///
    /// Blocking: git processes and a read of every tracked file.
    pub fn prepare(root: &Path) -> Result<AgentClone, AgentCloneError> {
        Sandbox::prepare(root)
            .map(AgentClone)
            .map_err(|e| AgentCloneError(e.to_string()))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// Points `cmd` at the clone: its working directory, and no variable
    /// that would send a `git` it runs to the repository instead.
    pub fn confine(&self, cmd: &mut std::process::Command) {
        cmd.current_dir(self.0.path());
        for v in GIT_LOCATION_VARS {
            cmd.env_remove(v);
        }
    }

    /// Writes into `root` what the agent changed in the clone, under the
    /// refusals every writer meets, and says what it did.
    ///
    /// For each path, in order: `.trellis/` (by any spelling, through any
    /// link), `write_refusal` (outside the repository, under `.git`, an
    /// unsafe name), the repository's `deny_write`, then the profile's
    /// `allow_write` (both in `governance`, `deny_write` consulted first,
    /// ADR 0026 §4). Then per kind: an edited or deleted file must still
    /// hold in the repository the bytes that were copied in; a new file
    /// must not exist there and must not be ignored by the repository's
    /// rules. `.gitignore`, `.gitattributes` and `.gitmodules` are never
    /// carried back in any kind.
    ///
    /// Blocking. Reads the clone's filesystem only: no `git` runs in the
    /// clone, whose `.git/config` is the agent's (ADR 0032 step 5).
    pub fn carry_back(&self, root: &Path, governance: &Governance) -> CarriedBack {
        let mut out = CarriedBack::default();
        let ch = self.0.changes();
        let copied: std::collections::HashMap<&str, [u8; 32]> = self
            .0
            .copied_in()
            .iter()
            .map(|(rel, h)| (rel.as_str(), *h))
            .collect();
        for (rel, why) in ch.refused {
            out.refused.push(NotCarried {
                why: format!("`{rel}` {why}"),
                path: rel,
            });
        }
        for (rel, before) in ch.modified {
            if !admitted(root, governance, &rel, &mut out)
                || !unchanged(root, &rel, before, &mut out)
            {
                continue;
            }
            let bytes = match self.0.read_changed(&rel) {
                Ok(b) => b,
                Err(why) => {
                    refuse(&mut out, &rel, format!("`{rel}` {why}"));
                    continue;
                }
            };
            match std::fs::write(root.join(&rel), &bytes) {
                Ok(()) => out.written.push(rel),
                Err(e) => refuse(&mut out, &rel, format!("`{rel}` could not be written: {e}")),
            }
        }
        for rel in ch.deleted {
            if is_git_instruction_file(&rel) {
                refuse(&mut out, &rel, instruction_file(&rel));
                continue;
            }
            let Some(&before) = copied.get(rel.as_str()) else {
                continue;
            };
            if !admitted(root, governance, &rel, &mut out)
                || !unchanged(root, &rel, before, &mut out)
            {
                continue;
            }
            match std::fs::remove_file(root.join(&rel)) {
                Ok(()) => out.deleted.push(rel),
                Err(e) => refuse(&mut out, &rel, format!("`{rel}` could not be deleted: {e}")),
            }
        }
        match self.new_files(root, &copied) {
            Ok(found) => {
                out.ignored = found.ignored;
                for (rel, why) in found.refused {
                    refuse(&mut out, &rel, why);
                }
                for rel in found.files {
                    self.create(root, governance, rel, &mut out);
                }
            }
            Err(why) => refuse(&mut out, "", why),
        }
        out
    }

    /// Writes one new file into `root`, or records why not.
    fn create(&self, root: &Path, governance: &Governance, rel: String, out: &mut CarriedBack) {
        if is_git_instruction_file(&rel) {
            refuse(out, &rel, instruction_file(&rel));
            return;
        }
        if !admitted(root, governance, &rel, out) {
            return;
        }
        let bytes = match self.0.read_changed(&rel) {
            Ok(b) => b,
            Err(why) => {
                refuse(out, &rel, format!("`{rel}` {why}"));
                return;
            }
        };
        let full = root.join(&rel);
        if let Some(dir) = full.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            refuse(
                out,
                &rel,
                format!("`{rel}` could not be created, nor its directory: {e}"),
            );
            return;
        }
        // `create_new`: a file that appeared in the repository since the
        // walk (a writer outside the clone) is not overwritten.
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .and_then(|mut f| f.write_all(&bytes));
        match written {
            Ok(()) => out.created.push(rel),
            Err(e) => refuse(out, &rel, format!("`{rel}` could not be created: {e}")),
        }
    }

    /// Files in the clone that were not copied in and do not exist in the
    /// repository, by a walk of the clone's filesystem that never follows
    /// a link and never enters a `.git` directory. Directories the
    /// repository's rules ignore are not entered; files they ignore are
    /// listed as ignored. `Err` when the clone is too large to walk, or
    /// when git could not say what it ignores: nothing new is then carried
    /// back, and the refusal says so.
    fn new_files(
        &self,
        root: &Path,
        copied: &std::collections::HashMap<&str, [u8; 32]>,
    ) -> Result<NewFiles, String> {
        let clone = self.0.path();
        let mut found = NewFiles::default();
        let mut level: Vec<String> = vec![String::new()];
        let mut visited = 0usize;
        while !level.is_empty() {
            // (path, is a directory)
            let mut entries: Vec<(String, bool)> = Vec::new();
            for dir in &level {
                let rd = std::fs::read_dir(clone.join(dir)).map_err(|e| {
                    format!("the clone's directory `{dir}` could not be read: {e}; nothing new was carried back")
                })?;
                for e in rd {
                    let e = e.map_err(|e| {
                        format!("the clone's directory `{dir}` could not be read: {e}; nothing new was carried back")
                    })?;
                    let Some(name) = e.file_name().to_str().map(str::to_string) else {
                        found.refused.push((
                            format!("{dir}{}", e.file_name().to_string_lossy()),
                            format!(
                                "`{dir}{}` has a name that is not UTF-8, which no \
                                 trellis write can name",
                                e.file_name().to_string_lossy()
                            ),
                        ));
                        continue;
                    };
                    let rel = format!("{dir}{name}");
                    // The clone's `.git`, and any other: what the agent
                    // did there is git's state, not a change to a file.
                    if trellis_core::paths::names_git_dir(&name) {
                        continue;
                    }
                    visited += 1;
                    if visited > NEW_FILE_WALK_CAP {
                        return Err(format!(
                            "the clone holds more than {NEW_FILE_WALK_CAP} files and \
                             directories git does not ignore; nothing new was carried back"
                        ));
                    }
                    let ft = e.file_type().map_err(|e| {
                        format!("`{rel}` in the clone could not be read: {e}; nothing new was carried back")
                    })?;
                    if copied.contains_key(rel.as_str()) {
                        continue;
                    }
                    if ft.is_dir() {
                        entries.push((rel, true));
                        continue;
                    }
                    // Present in the repository: a tracked link or
                    // submodule the copy-in left as it was, an ignored file
                    // the clone never had, or another spelling of a copied
                    // file on a filesystem that folds case. Not new.
                    if std::fs::symlink_metadata(root.join(&rel)).is_ok() {
                        continue;
                    }
                    if ft.is_symlink() {
                        found.refused.push((
                            rel.clone(),
                            format!(
                                "`{rel}` is a new link, and a link is never carried back: \
                                 what it points at is not a file git tracks"
                            ),
                        ));
                        continue;
                    }
                    // `git check-ignore` reads a leading `:` as pathspec
                    // magic (`:(top)x` asks about `x`), so it would answer
                    // for a different path than the one written.
                    if rel.starts_with(':') {
                        found.refused.push((
                            rel.clone(),
                            format!(
                                "`{rel}` starts with `:`, which git reads as pathspec magic, so \
                                 whether it ignores the file cannot be asked"
                            ),
                        ));
                        continue;
                    }
                    entries.push((rel, false));
                }
            }
            let ignored = ignored_in(root, &entries)?;
            let mut next = Vec::new();
            for (rel, is_dir) in entries {
                let shown = if is_dir { format!("{rel}/") } else { rel };
                if ignored.contains(&shown) {
                    found.ignored.push(shown);
                } else if is_dir {
                    next.push(shown);
                } else {
                    found.files.push(shown);
                }
            }
            level = next;
        }
        found.files.sort();
        found.ignored.sort();
        Ok(found)
    }
}

#[derive(Default)]
struct NewFiles {
    files: Vec<String>,
    /// Ignored by the repository's rules: files, and directories (with a
    /// trailing `/`) that were not entered.
    ignored: Vec<String>,
    refused: Vec<(String, String)>,
}

/// Which of `entries` the repository's ignore rules ignore, as written
/// (directories with a trailing `/`, which is how `check-ignore` matches a
/// directory-only pattern for a path that does not exist in the
/// repository). One `git check-ignore` in `root`, through
/// `trellis_vcs::command()`, reading the repository's `.gitignore` files,
/// `info/exclude` and `core.excludesFile`, never the clone's.
fn ignored_in(root: &Path, entries: &[(String, bool)]) -> Result<HashSet<String>, String> {
    if entries.is_empty() {
        return Ok(HashSet::new());
    }
    let mut input = Vec::new();
    for (rel, is_dir) in entries {
        input.extend_from_slice(rel.as_bytes());
        if *is_dir {
            input.push(b'/');
        }
        input.push(0);
    }
    let mut git = trellis_vcs::command();
    git.args(["check-ignore", "-z", "--stdin"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for v in GIT_LOCATION_VARS {
        git.env_remove(v);
    }
    let failed = |e: String| {
        format!(
            "the repository could not say which new files its git ignores ({e}); nothing new was carried back"
        )
    };
    let mut child = git.spawn().map_err(|e| failed(e.to_string()))?;
    // On its own thread: git answers as it reads, and a list longer than
    // the pipe's buffer would otherwise block both sides.
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let o = child
        .wait_with_output()
        .map_err(|e| failed(e.to_string()))?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    // 0: some are ignored; 1: none are. Anything else is git failing.
    match o.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            return Err(failed(
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            ));
        }
    }
    Ok(o.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

/// Whether `rel` passes what judges the run; records the refusal if not.
fn admitted(root: &Path, governance: &Governance, rel: &str, out: &mut CarriedBack) -> bool {
    let real = real_rel(root, rel).ok();
    if names_trellis_dir(rel) || real.as_deref().is_some_and(names_trellis_dir) {
        breach(
            out,
            rel,
            "under .trellis/, which holds what judges this run".into(),
        );
        return false;
    }
    if let Some(why) = write_refusal(root, rel) {
        refuse(out, rel, why);
        return false;
    }
    if let Some(rule) = governance.denies_write_at(root, rel) {
        breach(out, rel, format!("deny_write `{}`", rule.as_written()));
        return false;
    }
    if let Some(why) = governance.outside_allow_write_at(root, rel) {
        breach(out, rel, why);
        return false;
    }
    true
}

/// Whether the repository's copy of `rel` still has the bytes copied into
/// the clone. Nothing else in the run writes while the agent does, so a
/// difference is a writer outside the clone, and its bytes are kept.
fn unchanged(root: &Path, rel: &str, before: [u8; 32], out: &mut CarriedBack) -> bool {
    let full = root.join(rel);
    let same = std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file())
        && hash_file(&full).is_ok_and(|h| h == before);
    if !same {
        refuse(
            out,
            rel,
            format!("`{rel}` changed in the repository while the agent ran"),
        );
    }
    same
}

fn instruction_file(rel: &str) -> String {
    format!(
        "`{rel}` tells git what to ignore or filter, which decides what a rollback \
         reaches, and is never carried back from a clone"
    )
}

fn breach(out: &mut CarriedBack, rel: &str, why: String) {
    out.breaches.push(NotCarried {
        path: rel.to_string(),
        why,
    });
}

fn refuse(out: &mut CarriedBack, rel: &str, why: String) {
    out.refused.push(NotCarried {
        path: rel.to_string(),
        why,
    });
}

#[cfg(test)]
mod tests {
    use super::AgentClone;
    use crate::governance::Governance;
    use std::path::Path;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be on PATH to run these tests");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn committed() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("app/a.py"), "a = 1\n").unwrap();
        std::fs::write(root.join("docs/d.md"), "d\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            &["config", "user.email", "t@example.invalid"],
            &["config", "user.name", "t"],
            &["config", "core.autocrlf", "false"],
            &["add", "-A"],
            &["commit", "-qm", "fixture"],
        ] {
            git(root, args);
        }
        d
    }

    #[test]
    fn a_deletion_outside_allow_write_is_refused_and_the_file_kept() {
        // The risk (#182, #175): a scope enforced on the files an agent
        // edited and created, but not on the ones it deleted, which reach
        // the repository through the same carry-back.
        let d = committed();
        let root = d.path();
        let clone = AgentClone::prepare(root).unwrap();
        std::fs::remove_file(clone.path().join("app/a.py")).unwrap();
        std::fs::remove_file(clone.path().join("docs/d.md")).unwrap();
        let gov = Governance::new(&[]).with_allow_write("docs", &["docs/".into()]);
        let back = clone.carry_back(root, &gov);
        assert_eq!(back.deleted, ["docs/d.md"]);
        assert_eq!(back.breaches.len(), 1, "{back:?}");
        assert_eq!(back.breaches[0].path, "app/a.py");
        assert!(root.join("app/a.py").exists());
        assert!(!root.join("docs/d.md").exists());
    }

    #[test]
    fn a_directory_the_repository_ignores_is_not_entered_and_nothing_in_it_comes_back() {
        // The walk for new files is over a tree the agent wrote: a `build/`
        // (or a virtualenv) it filled is not walked file by file, and none
        // of it reaches the repository, where `git clean -fd` would leave
        // it after a rollback.
        let d = committed();
        let root = d.path();
        let clone = AgentClone::prepare(root).unwrap();
        std::fs::create_dir_all(clone.path().join("build/deep")).unwrap();
        std::fs::write(clone.path().join("build/deep/out.bin"), "x").unwrap();
        std::fs::write(clone.path().join("app/b.py"), "b = 2\n").unwrap();
        let back = clone.carry_back(root, &Governance::new(&[]));
        assert!(back.complete(), "{back:?}");
        assert_eq!(back.created, ["app/b.py"]);
        assert_eq!(back.ignored, ["build/"]);
        assert!(!root.join("build").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("app/b.py")).unwrap(),
            "b = 2\n"
        );
    }
}
