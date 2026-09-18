//! The decision trail: one JSON document per run, committed to the repo.
//!
//! Future agents read this as context, which is what makes the split
//! between `factual` and `judgment` structural rather than cosmetic. An
//! errant judgment entry filed next to facts propagates indefinitely (S11).
//!
//! It also makes the trail an injection surface: it lives in a repository
//! that may take contributions, so anything read back from it is untrusted
//! input (S10).

pub mod untrusted;

pub use untrusted::{RecordedCheck, UntrustedTrail};

use serde::{Deserialize, Serialize};
use trellis_checks::{AuditMode, Verdict};

/// What happened. Machine-checkable, every entry produced by something that
/// executed.
#[derive(Debug, Clone, Serialize)]
pub struct Factual {
    /// Paths the run proposed to change.
    pub proposed: Vec<String>,
    pub radius: RadiusCounts,
    /// Verdicts, in the order the checks ran. `Verdict` serializes but does
    /// not deserialize, so reading a trail back yields inert
    /// `RecordedCheck` data rather than a live verdict.
    pub checks: Vec<Verdict>,
    pub approved_by: Approval,
    pub mode_counts: ModeCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusCounts {
    pub resolved: u32,
    pub candidates: u32,
    pub unresolved: u32,
}

/// Counts of the two audit modes, never collapsed into one number.
///
/// S3: "the callers still work" means two different things. Covered by a
/// test that ran and passed, or read by a model that did not object. The
/// first time something breaks after a clean run, this distinction decides
/// whether the user feels informed or lied to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeCounts {
    pub verified: u32,
    pub reviewed: u32,
}

impl ModeCounts {
    pub fn record(&mut self, mode: AuditMode) {
        match mode {
            AuditMode::Verified => self.verified += 1,
            AuditMode::Reviewed => self.reviewed += 1,
        }
    }

    /// Deliberately returns both numbers and a label, never a single
    /// percentage. There is no total() here on purpose.
    pub fn summary(&self) -> String {
        format!(
            "{} verified by an executed check; {} reviewed only \
             (no test covers them — a weak signal)",
            self.verified, self.reviewed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Approval {
    Human,
    /// The run did not reach approval. Rollback reason recorded alongside.
    None,
}

/// The model's opinion. Speculation, labelled as such, structurally
/// separate from anything that executed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Judgment {
    pub tech_debt_projection: Option<String>,
    pub rationale: Option<String>,
}

/// One run.
#[derive(Debug, Clone, Serialize)]
pub struct Trail {
    pub task: String,
    /// ISO-8601, so a trail file sorts by name and by content.
    pub started_at: String,
    pub trellis_version: String,
    pub factual: Factual,
    pub judgment: Judgment,
}

impl Trail {
    /// Path the trail is committed at, relative to the repository root.
    pub fn path(timestamp: &str, slug: &str) -> String {
        format!(".trellis/runs/{timestamp}-{slug}.json")
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_counts_never_collapse_into_one_number() {
        let mut m = ModeCounts::default();
        m.record(AuditMode::Verified);
        m.record(AuditMode::Verified);
        m.record(AuditMode::Reviewed);
        assert_eq!(m.verified, 2);
        assert_eq!(m.reviewed, 1);

        let s = m.summary();
        assert!(s.contains("2 verified"));
        assert!(s.contains("1 reviewed"));
        assert!(
            s.contains("weak signal"),
            "the reviewed count must be labelled wherever it appears"
        );
    }

    #[test]
    fn trail_serializes_with_factual_and_judgment_separated() {
        let t = Trail {
            task: "rename Installer.run".into(),
            started_at: "2026-09-18T05:12:00Z".into(),
            trellis_version: "0.0.1".into(),
            factual: Factual {
                proposed: vec!["src/poetry/installation/installer.py".into()],
                radius: RadiusCounts {
                    resolved: 47,
                    candidates: 3,
                    unresolved: 2,
                },
                checks: vec![],
                approved_by: Approval::Human,
                mode_counts: ModeCounts {
                    verified: 45,
                    reviewed: 7,
                },
            },
            judgment: Judgment {
                tech_debt_projection: Some("the adapter shim will need removing".into()),
                rationale: Some("kept the old name as an alias".into()),
            },
        };

        let v: serde_json::Value = serde_json::from_str(&t.to_json().unwrap()).unwrap();
        assert!(v["factual"]["radius"]["unresolved"] == 2);
        assert!(
            v["judgment"]["tech_debt_projection"].is_string(),
            "speculation lives under judgment, never under factual"
        );
        assert!(
            v["factual"]["tech_debt_projection"].is_null(),
            "nothing speculative may appear in the factual section"
        );
    }
}
