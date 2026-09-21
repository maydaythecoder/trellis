//! Writing trails, and finding the last one.
//!
//! **Ordering matters more than it looks.** Rollback is `git reset --hard`
//! plus cleaning untracked files, and `.trellis/runs/` is inside the
//! repository — so a trail written before a rollback is deleted by it. The
//! trail of a run that *failed* is exactly the one worth keeping, so the
//! caller must write after rolling back, and `write` says so rather than
//! assuming the caller worked it out.
//!
//! This module only *writes*. The caller commits, because committing is a
//! git concern and belongs in `trellis-vcs` — but it must happen, and only
//! for the trail: leaving it untracked would mean the worktree is never clean
//! again, so no later run could start, and the next rollback's `git clean`
//! would delete every trail accumulated so far.
//!
//! The code a run proposed stays uncommitted either way. Accepting that is
//! the user's decision, and the trail records the run as unapproved until
//! they make it.

use crate::{Trail, UntrustedTrail};
use std::path::{Path, PathBuf};

/// Filesystem-safe, readable, and stable for the same task.
pub fn slugify(task: &str) -> String {
    let mut s: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    let s: String = s.chars().take(48).collect();
    if s.is_empty() {
        "run".into()
    } else {
        s.trim_matches('-').to_string()
    }
}

/// UTC timestamp, `YYYYMMDD-HHMMSS-mmm`, so trails sort by name.
///
/// **Milliseconds are not decoration.** Two runs in the same second produced
/// two trails whose lexical order fell back to the slug, so `latest` returned
/// whichever task name sorted last rather than the most recent run — and a
/// resume that silently reads the wrong session is worse than one that finds
/// nothing. Second precision was not enough for a tool a person runs
/// repeatedly.
///
/// Hand-rolled rather than pulling in a date library for one format string.
/// The civil-from-days conversion is Howard Hinnant's, which is the standard
/// way to do this without a calendar table.
pub fn timestamp_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{:03}",
        timestamp_from_epoch(d.as_secs()),
        d.subsec_millis()
    )
}

fn timestamp_from_epoch(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ISO-8601 UTC, for the trail's own `started_at`.
pub fn iso_now() -> String {
    now().1
}

/// The filename timestamp and the ISO timestamp, from **one** clock read.
///
/// Reading the clock twice let a trail carry a `started_at` in one second and
/// a filename in the next, which is a small lie in a document whose whole
/// value is being an accurate record.
pub fn now() -> (String, String) {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let base = timestamp_from_epoch(d.as_secs());
    let stamp = format!("{base}-{:03}", d.subsec_millis());
    let iso = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &base[0..4],
        &base[4..6],
        &base[6..8],
        &base[9..11],
        &base[11..13],
        &base[13..15]
    );
    (stamp, iso)
}

/// Writes a trail under `<root>/.trellis/runs/`.
///
/// **Call this after any rollback**, never before: rollback removes untracked
/// files, which is what a freshly written trail is.
pub fn write(root: &Path, trail: &Trail, timestamp: &str, slug: &str) -> std::io::Result<PathBuf> {
    let rel = Trail::path(timestamp, slug);
    let path = root.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = trail
        .to_json()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// The most recent trail, read back as untrusted data.
///
/// Returns `UntrustedTrail`, never `Trail`, because a trail lives in a
/// repository that may take contributions: reading one is reading
/// attacker-influenceable text into an agent's context (S10). The type is the
/// reminder, so no call site has to remember.
pub fn latest(root: &Path) -> std::io::Result<Option<(PathBuf, UntrustedTrail)>> {
    let dir = root.join(".trellis").join("runs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    // Names begin with a sortable UTC timestamp, so lexical order is
    // chronological order.
    paths.sort();
    let Some(path) = paths.pop() else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path)?;
    match UntrustedTrail::from_json(&text) {
        Ok(t) => {
            // The integrity check needs the bytes as they were on disk, not a
            // re-serialization of what we parsed, so the raw JSON travels with
            // the trail rather than being re-read by every caller.
            let integrity = t.integrity(&text);
            if integrity == crate::Integrity::Altered {
                tracing::warn!(
                    path = %path.display(),
                    "the factual section does not match its recorded digest"
                );
            }
            Ok(Some((path, t)))
        }
        // A corrupt or hand-edited trail must not abort a resume. Reporting
        // it and continuing without context is better than refusing to work.
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Approval, Factual, Judgment, ModeCounts, Provenance, RadiusCounts, SCHEMA};

    fn trail(task: &str) -> Trail {
        Trail {
            task: task.into(),
            started_at: iso_now(),
            schema: SCHEMA,
            run_id: "20260920-000000-000-test".into(),
            base_commit: "0000000".into(),
            provenance: Provenance {
                trellis_version: "test".into(),
                model: None,
                tier: None,
            },
            factual: Factual {
                proposed: vec!["app/a.py".into()],
                radius: RadiusCounts {
                    reached_resolved: 3,
                    reached_ambiguous: 1,
                    widest_unbound_in_touched_files: 2,
                    radius_queries: 1,
                },
                checks: Vec::new(),
                check_outcomes: Vec::new(),
                approved_by: Approval::None,
                mode_counts: ModeCounts::default(),
                context: None,
            },
            factual_sha256: String::new(),
            judgment: Judgment::default(),
        }
    }

    #[test]
    fn slugs_are_readable_and_filesystem_safe() {
        assert_eq!(
            slugify("Fix the type error in calc.py"),
            "fix-the-type-error-in-calc-py"
        );
        assert_eq!(slugify("///"), "run");
        assert!(slugify(&"x".repeat(200)).len() <= 48);
    }

    #[test]
    fn timestamps_are_sortable_and_correct() {
        // Cross-checked against an independent implementation rather than
        // against this one's own output, which would only prove it is
        // self-consistent.
        assert_eq!(timestamp_from_epoch(0), "19700101-000000");
        assert_eq!(timestamp_from_epoch(1_000_000), "19700112-134640");
        assert_eq!(timestamp_from_epoch(1_789_000_000), "20260910-002640");
        // Sortable by name is the property `latest` depends on.
        assert!(timestamp_from_epoch(1_000_000) < timestamp_from_epoch(2_000_000));
        assert!(timestamp_from_epoch(1_000_000_000) < timestamp_from_epoch(1_789_000_000));
    }

    /// The three dates above are correct and were not enough: five mutants
    /// survived inside `civil_from_days`.
    ///
    /// They all land in the same arithmetic, and the reason they hid is that
    /// the two branches of `if mp < 10 { mp + 3 } else { mp - 9 }` are only
    /// distinguishable in **January and February** — and `mp - 9` against
    /// `mp / 9` agrees even then for January, where `mp` is 10. February is
    /// the only month that separates them.
    ///
    /// Expected values generated from Python's `datetime`, which is the
    /// cross-check the test above already asks for: an independent
    /// implementation rather than this one's own output.
    #[test]
    fn the_calendar_holds_at_the_dates_that_separate_the_arithmetic() {
        // mp = 10. January, where several of the mutants coincide.
        assert_eq!(timestamp_from_epoch(0), "19700101-000000");
        // mp = 11. February is the only month that tells `mp - 9` from
        // `mp / 9`, and no test reached it before.
        assert_eq!(timestamp_from_epoch(1_707_000_000), "20240203-224000");
        // A leap day, and the day after it.
        assert_eq!(timestamp_from_epoch(1_709_208_000), "20240229-120000");
        assert_eq!(timestamp_from_epoch(1_709_294_400), "20240301-120000");
        // 2000 is a leap year and 2100 is not: the 400-year rule, which the
        // `doe / 146_096` term exists for.
        assert_eq!(timestamp_from_epoch(951_782_400), "20000229-000000");
        assert_eq!(timestamp_from_epoch(4_107_542_400), "21000301-000000");
        // A year boundary, one second apart, where the day-of-month `+ 1`
        // and the year carry both have to be right at once.
        assert_eq!(timestamp_from_epoch(1_767_225_599), "20251231-235959");
        assert_eq!(timestamp_from_epoch(1_767_225_600), "20260101-000000");
    }

    #[test]
    fn a_trail_round_trips_as_untrusted() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            &trail("do a thing"),
            "20260920-010203",
            "do-a-thing",
        )
        .unwrap();
        assert!(p.ends_with("20260920-010203-do-a-thing.json"));

        let (found, t) = latest(d.path()).unwrap().expect("a trail was written");
        assert_eq!(found, p);
        assert_eq!(t.task, "do a thing");
        assert_eq!(t.factual.radius.widest_unbound_in_touched_files, 2);
    }

    #[test]
    fn latest_picks_the_newest_by_name() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), &trail("older"), "20260101-000000", "older").unwrap();
        write(d.path(), &trail("newer"), "20260920-000000", "newer").unwrap();
        let (_, t) = latest(d.path()).unwrap().unwrap();
        assert_eq!(
            t.task, "newer",
            "names sort chronologically, so the last is the newest"
        );
    }

    #[test]
    fn no_runs_directory_is_not_an_error() {
        // A first run in a fresh repository must not fail because there is
        // nothing to resume from.
        let d = tempfile::tempdir().unwrap();
        assert!(latest(d.path()).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_trail_is_reported_rather_than_silently_skipped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/runs")).unwrap();
        std::fs::write(
            d.path().join(".trellis/runs/20260920-000000-x.json"),
            "{not json",
        )
        .unwrap();
        let e = latest(d.path()).unwrap_err();
        assert!(format!("{e}").contains("20260920-000000-x.json"), "{e}");
    }
}
