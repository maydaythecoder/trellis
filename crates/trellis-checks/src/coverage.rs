//! Which lines a test run actually executed.
//!
//! This is what turns `Verified` from a type that cannot be wrong into a
//! number that can be right. S3's distinction — covered by a test that ran,
//! versus read by a model that did not object — needs evidence of execution,
//! and a type checker cannot supply it.
//!
//! **TRELLIS does not run coverage.** It reads `coverage.json` if the
//! repository produced one. That is deliberate: the alternative is owning the
//! invocation, which means guessing at `pytest --cov=<what?>`, a plugin the
//! user may not have, and a flag set that differs per project. The contract
//! here is a file format — coverage.py's JSON report, which `pytest-cov` and
//! bare `coverage` both emit — and a repository that wants `Verified`
//! populated adds `--cov-report=json` to the test command it already
//! configured.
//!
//! No data is not an error. It means the run reports no `Verified` figure and
//! says why, which is what it did before this module existed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Executed lines, per file.
///
/// Only `executed_lines` is read. `missing_lines` is its complement over
/// statements and carries no information this needs, and the summary
/// percentages are exactly the kind of single figure S3 forbids.
#[derive(Debug, Clone, Default)]
pub struct LineCoverage {
    files: HashMap<String, HashSet<u32>>,
}

impl LineCoverage {
    /// Reads `<root>/coverage.json`.
    ///
    /// `None` when there is none, which is the common case and not a failure.
    /// A malformed one is also `None` rather than an error: coverage is
    /// evidence that strengthens a claim, so its absence weakens the claim
    /// instead of failing the run — the opposite of a governance rule, which
    /// must be an error precisely because it cannot be allowed to go quiet.
    pub fn load(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(root.join("coverage.json")).ok()?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let files = v.get("files")?.as_object()?;

        let mut out: HashMap<String, HashSet<u32>> = HashMap::new();
        for (path, entry) in files {
            let lines: HashSet<u32> = entry
                .get("executed_lines")
                .and_then(|l| l.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(serde_json::Value::as_u64)
                        .map(|n| n as u32)
                        .collect()
                })
                .unwrap_or_default();
            out.insert(normalise(path), lines);
        }
        Some(Self { files: out })
    }

    /// Whether a test run executed this line.
    ///
    /// Paths are compared after separator normalisation: coverage.py writes
    /// the platform's own separator, so a report generated on Windows reads
    /// `app\util.py` while the index holds `app/util.py`.
    pub fn covers(&self, path: &str, line: u32) -> bool {
        self.files
            .get(&normalise(path))
            .is_some_and(|l| l.contains(&line))
    }

    /// Whether anything is known about this file at all.
    ///
    /// Distinct from "no line is covered". A file absent from the report was
    /// never imported by the tests; a file present with the line missing was
    /// imported and that line did not run. Test-only until a caller that
    /// reports the difference exists.
    #[cfg(test)]
    pub fn knows(&self, path: &str) -> bool {
        self.files.contains_key(&normalise(path))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn normalise(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from coverage.py 7.16.1, not written by hand.
    ///
    /// The point of the fixture is the shape of a real report: `def` lines
    /// appear as executed because importing a module runs them, and the body
    /// of a function nobody called does not.
    const REAL: &str = r#"{
      "meta": {"version": "7.16.1"},
      "files": {
        "app\\util.py": {
          "executed_lines": [1, 2, 5, 6, 9],
          "missing_lines": [10],
          "summary": {"percent_covered": 83.3}
        },
        "run_tests.py": {
          "executed_lines": [1, 2, 3],
          "missing_lines": []
        }
      },
      "totals": {"percent_covered": 90.0}
    }"#;

    #[test]
    fn a_real_report_parses_and_separators_do_not_matter() {
        let c = LineCoverage::parse(REAL).expect("parses");
        // Written by Windows coverage.py as `app\util.py`; the index holds
        // `app/util.py`. Both must answer the same question.
        assert!(c.covers("app/util.py", 6));
        assert!(c.covers("app\\util.py", 6));
        assert!(c.knows("app/util.py"));
    }

    #[test]
    fn a_line_in_a_function_nobody_called_is_not_covered() {
        // The discrimination the whole module exists for. Line 9 is the
        // `def never_called` line, executed at import; line 10 is its body,
        // which never ran. Treating the `def` as evidence the function ran
        // would make `Verified` meaningless.
        let c = LineCoverage::parse(REAL).unwrap();
        assert!(c.covers("app/util.py", 9), "the def line runs on import");
        assert!(
            !c.covers("app/util.py", 10),
            "its body did not, and that is the difference Verified turns on"
        );
    }

    #[test]
    fn a_file_the_tests_never_imported_is_distinguishable_from_an_uncovered_line() {
        let c = LineCoverage::parse(REAL).unwrap();
        assert!(!c.knows("app/untouched.py"));
        assert!(!c.covers("app/untouched.py", 1));
        assert!(
            c.knows("app/util.py") && !c.covers("app/util.py", 10),
            "present-but-uncovered and absent are different facts"
        );
    }

    #[test]
    fn absent_or_malformed_data_is_none_rather_than_an_error() {
        // Coverage strengthens a claim, so its absence weakens the claim
        // rather than failing the run. A governance rule is the opposite and
        // must be an error, precisely because it cannot go quiet.
        assert!(LineCoverage::parse("not json").is_none());
        assert!(LineCoverage::parse(r#"{"no_files_key": 1}"#).is_none());
        assert!(LineCoverage::load(Path::new("/definitely/not/here")).is_none());
    }

    #[test]
    fn a_report_with_no_files_is_empty_rather_than_absent() {
        let c = LineCoverage::parse(r#"{"files": {}}"#).expect("still a valid report");
        assert!(c.is_empty());
        assert!(!c.covers("anything.py", 1));
    }

    /// Two mutants survived here, and between them they are the whole of
    /// `audit_modes`'s gate: `load` returning `None` and `is_empty`
    /// returning `true` both make every call site report as **Reviewed**
    /// rather than **Verified**.
    ///
    /// That is the S3 distinction — "the callers still work" meaning a test
    /// executed them, or meaning a model read them and did not object — and
    /// ADR 0003 keeps the two counts apart precisely because collapsing them
    /// is what makes a user feel lied to the first time something breaks
    /// after a clean run.
    ///
    /// The existing tests cover `parse`, which takes a string. Neither
    /// reached `load`, which takes a directory, nor asserted that a report
    /// with files in it is *not* empty.
    #[test]
    fn load_reads_a_report_from_disk_and_a_populated_one_is_not_empty() {
        let dir =
            std::env::temp_dir().join(format!("trellis-coverage-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // No report yet: absence, not failure.
        assert!(
            LineCoverage::load(&dir).is_none(),
            "a directory with no coverage.json has no coverage"
        );

        std::fs::write(
            dir.join("coverage.json"),
            r#"{"files": {"app/core.py": {"executed_lines": [1, 2, 5]}}}"#,
        )
        .unwrap();

        let c = LineCoverage::load(&dir).expect("a report on disk must load");
        assert!(
            !c.is_empty(),
            "a report naming a file is not empty — this is what decides \
             Verified from Reviewed"
        );
        assert!(c.knows("app/core.py"));
        assert!(c.covers("app/core.py", 2));
        assert!(!c.covers("app/core.py", 3), "line 3 was not executed");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
