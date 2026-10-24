//! The replay recording every run writes (#620, ADR 0056).
//!
//! `.trellis/replays/<run_id>.json` holds each step the model gave, as the
//! script `trellis run --model scripted:<file>` plays. Replayed on the same
//! commit, a run's edits, checks and rollback happen again for real, with no
//! model and no tokens. The trail cannot serve: it keeps a one-line outcome
//! per action on purpose, not an edit's `old`/`new` or a `bash` script.
//!
//! The directory ignores itself through its own `.gitignore`, and a run
//! refuses to start while git tracks anything in it: a recording is a record
//! of one machine's run, and a tracked one would be repository-supplied input
//! to whoever replays it.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// `.trellis/replays`.
fn dir(root: &Path) -> PathBuf {
    root.join(".trellis").join(trellis_loop::REPLAYS_DIR)
}

/// Refuses a run while git tracks anything under `.trellis/replays/`.
///
/// Before the snapshot, so the refusal costs nothing. A tracked recording
/// would be reset by a rollback and changed by the next run, and a recording
/// that came with the repository is not a record of anything run here.
pub(crate) fn refuse_if_tracked(root: &Path) -> Result<()> {
    let listed = trellis_vcs::command()
        .args(["ls-files", "-z", "--", ".trellis/replays"])
        .current_dir(root)
        .output();
    // A directory git cannot list is refused at the snapshot, whose message
    // names the reason; refusing here would say less.
    let Ok(listed) = listed else { return Ok(()) };
    if !listed.status.success() {
        return Ok(());
    }
    let tracked: Vec<String> = listed
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    if tracked.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        ".trellis/replays/ holds {} that git tracks ({}). Replay recordings are records of runs on \
         this machine and stay out of the repository (ADR 0056). Untrack them with `git rm -r \
         --cached .trellis/replays` and commit, then run again. Nothing was snapshotted.",
        if tracked.len() == 1 {
            "a file".to_string()
        } else {
            format!("{} files", tracked.len())
        },
        tracked.join(", ")
    )
}

/// Writes the recording, after the run has disposed of the tree: a rollback
/// with writer guards removes ignored files the run created, and this is
/// one. Nothing for a run that never reached its snapshot, which ran nothing.
pub(crate) fn finish(
    root: &Path,
    rec: &trellis_loop::Recording,
    out: &mut (dyn std::io::Write + Send),
) {
    if rec.base_commit.is_empty() {
        return;
    }
    match write(root, rec) {
        Ok(path) => {
            let shown = path.strip_prefix(root).unwrap_or(&path);
            let _ = writeln!(
                out,
                "replay:   {} (--model scripted:<this file> on {} replays it)",
                crate::log_cmd::plain(&shown.display().to_string()),
                &rec.base_commit[..rec.base_commit.len().min(12)]
            );
        }
        Err(e) => {
            let _ = writeln!(out, "could not write the replay recording: {e}");
        }
    }
}

fn write(root: &Path, rec: &trellis_loop::Recording) -> Result<PathBuf> {
    // `run_id` is a stamp and `slugify`'s output; checked anyway, since it
    // becomes a file name.
    let id = &rec.run_id;
    anyhow::ensure!(
        !id.is_empty()
            && !id.starts_with('.')
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "run id `{id}` is not a file name"
    );
    let dir = dir(root);
    // Not through a link: a `.trellis/replays` that points elsewhere would
    // put this write wherever it points.
    for p in [root.join(".trellis"), dir.clone()] {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            anyhow::bail!("{} is a symbolic link", p.display());
        }
    }
    std::fs::create_dir_all(&dir)?;
    let ignore = dir.join(".gitignore");
    // Never overwritten, as `.trellis/.gitignore` is not: a person who
    // edited it chose to.
    if std::fs::symlink_metadata(&ignore).is_err() {
        std::fs::write(&ignore, trellis_loop::REPLAY_IGNORE)?;
    }
    let path = dir.join(format!("{id}.json"));
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("{} is a symbolic link", path.display());
    }
    let text = serde_json::to_string_pretty(&rec.to_json())?;
    std::fs::write(&path, text + "\n")?;
    Ok(path)
}
