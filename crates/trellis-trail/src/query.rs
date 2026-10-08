//! Reading the trail back as a record rather than as a log.
//!
//! `docs/03-v0.3-memory.md` names the failure this guards against: the trail
//! becomes a log nobody reads — written faithfully, loaded on resume, and
//! useless because the grain is wrong. Its test is `trellis why`. If you
//! cannot answer "why does this function look like this" two weeks later, the
//! grain is wrong, and tuning it against completeness rather than against that
//! question is how it stays wrong.
//!
//! Everything here reads `UntrustedTrail`. The trail lives in a repository
//! that takes contributions, so nothing in it is an instruction and nothing
//! from it is quoted without saying where it came from (S10).

use crate::{Integrity, UntrustedTrail};
use std::path::{Path, PathBuf};

/// One trail on disk, with the integrity of its factual section.
pub struct Entry {
    pub path: PathBuf,
    pub trail: UntrustedTrail,
    pub integrity: Integrity,
}

impl Entry {
    /// `<timestamp>-<slug>`, from the trail or from its filename.
    ///
    /// Older trails predate `run_id`, and falling back to the filename is
    /// better than showing them without a handle — `resume <run_id>` has to
    /// name something for every run, not only recent ones.
    pub fn run_id(&self) -> String {
        self.trail.run_id.clone().unwrap_or_else(|| {
            self.path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        })
    }

    /// What the run concluded, from the factual record alone.
    pub fn outcome(&self) -> &'static str {
        let f = &self.trail.factual;
        let passed = f
            .check_outcomes
            .last()
            .map(|c| c.passed)
            .or_else(|| f.checks.last().map(|c| c.passed));
        match passed {
            Some(true) => "converged",
            Some(false) => "not converged",
            // A run that never executed a check did not converge either, and
            // saying "unknown" would be softer than the truth.
            None => "no check ran",
        }
    }
}

/// Every trail under `<root>/.trellis/runs`, newest first.
///
/// A malformed trail is skipped with a warning rather than failing the whole
/// listing: one corrupt file must not make the rest of the history
/// unreadable, which is the opposite of what a record is for.
pub fn all(root: &Path) -> Vec<Entry> {
    let dir = root.join(".trellis").join("runs");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    // Names begin with a sortable UTC timestamp, so lexical order is
    // chronological.
    paths.sort();
    paths.reverse();

    paths
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            match UntrustedTrail::from_json(&text) {
                Ok(trail) => {
                    let integrity = trail.integrity(&text);
                    Some(Entry {
                        path,
                        trail,
                        integrity,
                    })
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping a trail that will not parse");
                    None
                }
            }
        })
        .collect()
}

/// The newest trail under `<root>/.trellis/runs`, with its integrity, read
/// with a single read of one file.
///
/// Unlike [`all`], a newest trail that will not parse is an error, not
/// skipped: an overview that passed over it would show an older run as the
/// last one. Nothing is logged here, so a caller printing one JSON object on
/// stdout decides what to say about it. `Ok(None)` when there is no trail.
pub fn latest_entry(root: &Path) -> std::io::Result<Option<Entry>> {
    let dir = root.join(".trellis").join("runs");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };
    // Names begin with a sortable UTC timestamp, so the greatest is newest.
    let Some(path) = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .max()
    else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path)?;
    let trail = UntrustedTrail::from_json(&text).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })?;
    let integrity = trail.integrity(&text);
    Ok(Some(Entry {
        path,
        trail,
        integrity,
    }))
}

/// Trails whose run touched a file, or whose task mentions a symbol.
///
/// Two ways in, because a user asking "why does this look like this" has a
/// symbol in hand and the factual section records *files*. Matching only
/// files would miss the run that discussed a symbol without editing it;
/// matching only the task text would rank a passing mention above a recorded
/// change. Both, labelled, and the caller shows which.
pub struct Mention {
    pub entry_index: usize,
    /// The first proposed path containing the query, when there is one —
    /// evidence, as opposed to the task string merely naming it.
    pub touched_file: Option<String>,
    pub task_mentions: bool,
    /// A person's decisions in approve mode about calls aimed at the query,
    /// as `(target, outcome)`: `("edit app/x.py", "declined")`.
    ///
    /// A declined edit never reaches `proposed`, so without this `why` could
    /// not find the run in which someone refused exactly the change being
    /// asked about (ADR 0018).
    pub decisions: Vec<(String, String)>,
}

/// The entries that mention any of `queries`: a proposed path, the task, or
/// an approval decision's target containing one of them, case-insensitively.
/// A substring search, so `parser` finds `app/parser.py`.
///
/// `why <symbol>` resolves the symbol to the file that defines it and passes
/// both, because the factual section records files and the user holds a
/// symbol. Matching only the literal query reported "no file changed" about
/// runs that had changed exactly the right file.
pub fn mentions_any(entries: &[Entry], queries: &[String]) -> Vec<Mention> {
    let needles: Vec<String> = queries.iter().map(|q| q.to_ascii_lowercase()).collect();
    entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let touched = e
                .trail
                .factual
                .proposed
                .iter()
                .find(|p| {
                    let lp = p.to_ascii_lowercase();
                    needles.iter().any(|n| lp.contains(n.as_str()))
                })
                .cloned();
            let in_task = {
                let lt = e.trail.task.to_ascii_lowercase();
                needles.iter().any(|n| lt.contains(n.as_str()))
            };
            let decisions: Vec<(String, String)> = e
                .trail
                .factual
                .actions
                .iter()
                .filter(|a| a.tool == "approval")
                .filter(|a| {
                    let lt = a.target.to_ascii_lowercase();
                    needles.iter().any(|n| lt.contains(n.as_str()))
                })
                .map(|a| (a.target.clone(), a.outcome.clone()))
                .collect();
            (touched.is_some() || in_task || !decisions.is_empty()).then_some(Mention {
                entry_index: i,
                touched_file: touched,
                task_mentions: in_task,
                decisions,
            })
        })
        .collect()
}

/// A trail whose record names one file, and where it names it.
pub struct Touch<'a> {
    pub entry: &'a Entry,
    /// The run's `factual.proposed` lists the file: it ended up changing it.
    pub proposed: bool,
    /// Actions whose target names the file, in trail order. Includes reads
    /// and declined edits: the caller decides which of those count.
    pub actions: Vec<&'a crate::Action>,
}

/// The entries whose `factual.proposed` or action targets name `path`.
///
/// This is equality of paths, not a substring search like [`mentions_any`]:
/// pairing a changed file with the runs that touched it must not pair
/// `a.py` with a run on `ba.py` or `src/a.py`. Case is kept, because two
/// paths that differ only in case are two files on most filesystems. What
/// is normalised is spelling that names the same repository path: `\`
/// separators and a leading `./`.
///
/// An action target is a label (`edit a.py`, `run_checks Tests on a.py,
/// b.py`, `grep pat in a.py`), so it names the path when any of its words,
/// split on whitespace and commas, does.
pub fn touching<'a>(entries: &'a [Entry], path: &str) -> Vec<Touch<'a>> {
    let want = normalize_path(path);
    // An empty path (or `.`) names no file; matched literally it would pair
    // every target word that is itself `.`.
    if want.is_empty() || want == "/" {
        return Vec::new();
    }
    let names = |s: &str| normalize_path(s) == want;
    entries
        .iter()
        .filter_map(|entry| {
            let f = &entry.trail.factual;
            let proposed = f.proposed.iter().any(|p| names(p));
            let actions: Vec<&crate::Action> = f
                .actions
                .iter()
                .filter(|a| {
                    a.target
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .any(|w| !w.is_empty() && names(w))
                })
                .collect();
            (proposed || !actions.is_empty()).then_some(Touch {
                entry,
                proposed,
                actions,
            })
        })
        .collect()
}

/// `a\b.py`, `./a/b.py` and `a//b.py` all name `a/b.py`.
///
/// A leading `/` is kept: `/a.py` is an absolute path, not the repository's
/// `a.py`, and a trail that named one did not touch the other.
fn normalize_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    let rel = p
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/");
    if p.starts_with('/') {
        format!("/{rel}")
    } else {
        rel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        let runs = dir.join(".trellis").join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join(name), body).unwrap();
    }

    fn trail_json(task: &str, file: &str, passed: bool) -> String {
        format!(
            r#"{{"task":"{task}","factual":{{"proposed":["{file}"],
               "check_outcomes":[{{"kind":"Typecheck","passed":{passed}}}]}}}}"#
        )
    }

    #[test]
    fn trails_list_newest_first() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-old.json",
            &trail_json("old", "a.py", true),
        );
        write(
            d.path(),
            "20260920-000000-000-new.json",
            &trail_json("new", "b.py", true),
        );
        let all = all(d.path());
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[0].trail.task, "new",
            "newest first, so `log` reads top-down"
        );
    }

    #[test]
    fn latest_entry_is_the_newest_trail_and_none_without_one() {
        let d = tempfile::tempdir().unwrap();
        assert!(latest_entry(d.path()).unwrap().is_none(), "no runs dir");
        write(
            d.path(),
            "20260920-000000-000-new.json",
            &trail_json("new", "b.py", false),
        );
        write(
            d.path(),
            "20260101-000000-000-old.json",
            &trail_json("old", "a.py", true),
        );
        write(d.path(), "notes.txt", "not a trail");
        let e = latest_entry(d.path()).unwrap().expect("a trail");
        assert_eq!(e.trail.task, "new");
        assert_eq!(e.outcome(), "not converged");
        assert_eq!(e.integrity, Integrity::Unrecorded);
    }

    #[test]
    fn latest_entry_is_an_error_when_the_newest_trail_is_corrupt() {
        // Skipping it, as `all` does, would report the older run as the
        // last one.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-old.json",
            &trail_json("old", "a.py", true),
        );
        write(d.path(), "20260920-000000-000-new.json", "{ not json");
        let e = latest_entry(d.path()).err().expect("an error");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            e.to_string().contains("20260920-000000-000-new.json"),
            "{e}"
        );
    }

    #[test]
    fn one_unparseable_trail_does_not_hide_the_rest() {
        // A record that becomes unreadable because one file is corrupt is
        // not a record. The bad one is skipped with a warning.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-good.json",
            &trail_json("good", "a.py", true),
        );
        write(d.path(), "20260102-000000-000-bad.json", "{not json");
        let all = all(d.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].trail.task, "good");
    }

    #[test]
    fn outcome_reads_from_the_checks_and_never_softens_a_missing_one() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-a.json",
            &trail_json("a", "a.py", true),
        );
        write(
            d.path(),
            "20260102-000000-000-b.json",
            &trail_json("b", "b.py", false),
        );
        write(
            d.path(),
            "20260103-000000-000-c.json",
            r#"{"task":"c","factual":{"proposed":[]}}"#,
        );
        let all = all(d.path());
        assert_eq!(all[2].outcome(), "converged");
        assert_eq!(all[1].outcome(), "not converged");
        assert_eq!(
            all[0].outcome(),
            "no check ran",
            "a run with no check did not converge; `unknown` would be softer than the truth"
        );
    }

    #[test]
    fn why_matches_a_touched_file_and_a_task_mention_separately() {
        // A user asking "why does this look like this" holds a symbol, and
        // the factual section records files. Matching only one of the two
        // misses half the history.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-edit.json",
            &trail_json("fix the parser", "app/parser.py", true),
        );
        write(
            d.path(),
            "20260102-000000-000-talk.json",
            &trail_json("investigate parser slowness", "app/other.py", true),
        );
        let all = all(d.path());
        let m = mentions_any(&all, &["parser".to_string()]);
        assert_eq!(m.len(), 2);

        let edited = m.iter().find(|x| x.touched_file.is_some()).unwrap();
        assert_eq!(edited.touched_file.as_deref(), Some("app/parser.py"));

        let discussed = m.iter().find(|x| x.touched_file.is_none()).unwrap();
        assert!(
            discussed.task_mentions,
            "a run that named the symbol without editing it is still history"
        );
    }

    #[test]
    fn a_symbol_resolves_through_the_file_that_defines_it() {
        // The failure the first wedge run exposed. The factual section records
        // files; the user holds a symbol. Matching only the literal query said
        // "no file matching `boolean_validator` was changed" about a run that
        // had changed exactly the file defining it — the trail written
        // faithfully and unable to answer the question it exists for.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-fix.json",
            &trail_json("fix the validator", "src/app/config.py", true),
        );
        let all = all(d.path());

        assert!(
            mentions_any(&all, &["boolean_validator".to_string()]).is_empty(),
            "the literal name is not in the path, which is the bug"
        );
        // The caller resolves the symbol to its file and passes both.
        let m = mentions_any(
            &all,
            &[
                "boolean_validator".to_string(),
                "src/app/config.py".to_string(),
            ],
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].touched_file.as_deref(), Some("src/app/config.py"));
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing_rather_than_everything() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-a.json",
            &trail_json("a", "a.py", true),
        );
        assert!(mentions_any(&all(d.path()), &["nosuchsymbol".to_string()]).is_empty());
    }

    /// `run_id` is what `trellis resume <run_id>` names, and it has to name
    /// something for every run — including trails written before the field
    /// existed, which fall back to the filename.
    ///
    /// Two mutants survived on it: returning an empty string and returning
    /// `"xyzzy"`. Nothing asserted either branch.
    #[test]
    fn a_run_id_comes_from_the_trail_or_from_the_filename() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260921-120000-000-with-id.json",
            r#"{"run_id":"20260921-120000-000-with-id","task":"t","factual":{"proposed":[]}}"#,
        );
        write(
            d.path(),
            "20260101-090000-000-no-id.json",
            r#"{"task":"older","factual":{"proposed":[]}}"#,
        );

        let entries = all(d.path());
        let ids: Vec<String> = entries.iter().map(Entry::run_id).collect();
        assert!(
            ids.contains(&"20260921-120000-000-with-id".to_string()),
            "the recorded run_id must be used when present: {ids:?}"
        );
        assert!(
            ids.contains(&"20260101-090000-000-no-id".to_string()),
            "a trail with no run_id must fall back to its filename stem: {ids:?}"
        );
        assert!(
            ids.iter().all(|s| !s.is_empty() && s != "unknown"),
            "every run must have a handle `resume` can name: {ids:?}"
        );
    }

    /// The integrity verdict a user actually sees. `cmd_log` and `cmd_why`
    /// read `Entry::integrity`, so this is the path the `ALTERED` line is
    /// printed from — `store::latest` only logs a warning alongside it.
    #[test]
    fn a_tampered_trail_surfaces_as_altered_in_the_listing() {
        let d = tempfile::tempdir().unwrap();

        // A trail whose digest matches its factual section, built the way a
        // run builds it.
        let factual = crate::Factual {
            proposed: vec!["app/core.py".into()],
            radius: crate::RadiusCounts::default(),
            checks: Vec::new(),
            check_outcomes: vec![crate::CheckOutcome {
                kind: "Tests".into(),
                passed: true,
            }],
            approved_by: crate::Approval::Human,
            mode_counts: crate::ModeCounts::default(),
            context: None,
            actions: Vec::new(),
            control: None,
            tokens: None,
            prompt_tokens: None,
            agent: None,
            label: None,
            inputs: Vec::new(),
            steering: Vec::new(),
            suspected_recall_gaps: Vec::new(),
        };
        let digest = crate::Trail::compute_factual_sha256(&factual);
        let factual_json = serde_json::to_string(&factual).unwrap();
        let good = format!(
            r#"{{"run_id":"good-run","task":"t","factual_sha256":"{digest}","factual":{factual_json}}}"#
        );
        write(d.path(), "20260921-120000-000-good.json", &good);

        // The same trail with one value changed: a failed check made to look
        // like a passed one, which is the edit that would matter.
        let bad = good
            .replace("\"passed\":true", "\"passed\":false")
            .replace("good-run", "bad-run");
        assert_ne!(bad, good, "the fixture must actually differ");
        write(d.path(), "20260921-130000-000-bad.json", &bad);

        let entries = all(d.path());
        let by_id = |id: &str| -> &Entry {
            entries
                .iter()
                .find(|e| e.run_id().contains(id))
                .unwrap_or_else(|| panic!("no entry matching {id}"))
        };
        assert_eq!(
            by_id("good").integrity,
            crate::Integrity::Intact,
            "an untouched trail must not be reported as altered"
        );
        assert_eq!(
            by_id("bad").integrity,
            crate::Integrity::Altered,
            "an edited check outcome must be visible in the listing"
        );
    }

    #[test]
    fn a_declined_edit_is_found_although_it_changed_nothing() {
        // ADR 0018: a declined edit never reaches `proposed`, and the task
        // may not name the file. The person's decision is the record.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260927-000000-000-declined.json",
            r#"{"task":"tidy things","factual":{"proposed":[],
               "actions":[
                 {"turn":1,"tool":"approval","target":"edit app/calc.py","outcome":"declined","wrote":false},
                 {"turn":1,"tool":"edit","target":"app/calc.py","outcome":"error: declined by the user","wrote":false}
               ]}}"#,
        );
        let entries = all(d.path());
        let found = mentions_any(&entries, &["app/calc.py".to_string()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].touched_file.is_none());
        assert_eq!(
            found[0].decisions,
            vec![("edit app/calc.py".to_string(), "declined".to_string())]
        );
    }

    fn with_actions(proposed: &str, actions: &[(&str, &str)]) -> String {
        let acts: Vec<String> = actions
            .iter()
            .map(|(tool, target)| {
                format!(
                    r#"{{"turn":1,"tool":"{tool}","target":"{target}","outcome":"ok","wrote":false}}"#
                )
            })
            .collect();
        format!(
            r#"{{"task":"t","factual":{{"proposed":[{proposed}],"actions":[{}]}}}}"#,
            acts.join(",")
        )
    }

    #[test]
    fn touching_finds_a_run_that_proposed_the_path_or_aimed_an_action_at_it() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-proposed.json",
            &with_actions(r#""app/a.py""#, &[]),
        );
        write(
            d.path(),
            "20260102-000000-000-checked.json",
            &with_actions(
                "",
                &[("run_checks", "run_checks Tests on app/b.py, app/a.py")],
            ),
        );
        let entries = all(d.path());
        let t = touching(&entries, "app/a.py");
        assert_eq!(t.len(), 2, "both runs name app/a.py");

        let proposed = t.iter().find(|x| x.proposed).unwrap();
        assert_eq!(proposed.entry.run_id(), "20260101-000000-000-proposed");
        assert!(proposed.actions.is_empty());

        let checked = t.iter().find(|x| !x.proposed).unwrap();
        assert_eq!(
            checked.actions.len(),
            1,
            "a path in a comma-separated target list is named by it"
        );
    }

    /// Pairing a changed file with the runs that touched it is exact. The
    /// substring match `mentions_any` uses would pair `a.py` with every run on
    /// `ba.py` or `src/a.py`, and the delta would cite history that is not
    /// about the file at all.
    #[test]
    fn touching_does_not_match_a_different_path_that_contains_this_one() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-other.json",
            &with_actions(
                r#""ba.py","src/a.py","a.pyc","/a.py""#,
                &[
                    ("edit", "edit src/a.py"),
                    ("read_file", "read_file a.py.bak"),
                ],
            ),
        );
        let entries = all(d.path());
        assert!(touching(&entries, "a.py").is_empty());
        assert!(
            touching(&entries, "A.py").is_empty(),
            "case is kept: A.py and a.py are two files on most filesystems"
        );
        assert!(
            touching(&entries, "").is_empty() && touching(&entries, "./").is_empty(),
            "an empty path names no file"
        );
    }

    /// A path recorded as `./app/a.py`, or with Windows separators, is the
    /// same repository path as `app/a.py`. A run written on Windows must
    /// pair with a delta computed anywhere.
    #[test]
    fn touching_treats_a_dot_slash_prefix_and_backslashes_as_the_same_path() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "20260101-000000-000-dot.json",
            &with_actions(r#""./app/a.py""#, &[]),
        );
        write(
            d.path(),
            "20260102-000000-000-win.json",
            &with_actions("", &[("edit", r"edit app\\a.py")]),
        );
        let entries = all(d.path());
        assert_eq!(touching(&entries, "app/a.py").len(), 2);
        assert_eq!(
            touching(&entries, r"app\a.py").len(),
            2,
            "the query is normalised as well as the record"
        );
        assert_eq!(touching(&entries, "./app//a.py").len(), 2);
    }
}
