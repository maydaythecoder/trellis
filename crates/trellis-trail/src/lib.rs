//! The decision trail: one JSON document per run, committed to the repo.
//!
//! Future agents read this as context, which is what makes the split
//! between `factual` and `judgment` structural rather than cosmetic. An
//! errant judgment entry filed next to facts propagates indefinitely (S11).
//!
//! It also makes the trail an injection surface: it lives in a repository
//! that may take contributions, so anything read back from it is untrusted
//! input (S10).

pub mod query;
mod store;
mod untrusted;

pub use store::{iso_now, latest, now, slugify, timestamp_now, write};
pub use untrusted::{Integrity, RecordedCheck, UntrustedFactual, UntrustedTrail};

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
    /// Which checks ran, in order, and whether each passed.
    ///
    /// `checks` above holds `Verdict`s, which cannot be constructed outside
    /// `trellis-checks` — that seal is what makes Constraint 2 structural, and
    /// it also means a trail written by the CLI cannot carry them. This field
    /// records the same sequence as inert data so the factual section says
    /// what actually ran, rather than being silently empty.
    #[serde(default)]
    pub check_outcomes: Vec<CheckOutcome>,
    pub approved_by: Approval,
    pub mode_counts: ModeCounts,
    /// Prompt size across the run, in **bytes** (v0.2 gate: context fill).
    ///
    /// Bytes rather than tokens because TRELLIS does not tokenize — the
    /// runtime does — and `bytes / 4` under a token-shaped name would be an
    /// invented figure. Absent for a scripted run, which has no prompt.
    #[serde(default)]
    pub context: Option<ContextFill>,
    /// What the agent did, in the order it did it.
    ///
    /// The trail recorded what a run *proposed* and what the checks *said*,
    /// and nothing about what it **did**. So the first question a reader asks
    /// — what happened, in order — could only be answered from a terminal
    /// scrollback that no longer exists, and a run that ended having touched
    /// nothing was indistinguishable from one that never started.
    ///
    /// `#[serde(default)]` so a trail written before this field reads back as
    /// a run with no actions recorded, which is what it is. That is not the
    /// same claim as "a run that took no actions", and `trellis log` says
    /// which of the two it is looking at rather than printing "0 actions" for
    /// both.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Where the control dial stood for this run (ADR 0018): `auto`, or
    /// `approve (<tools>)`.
    ///
    /// Hands-off and fine-grained runs go through the same loop, checks and
    /// trail, and the only difference is which gates were installed — so
    /// without this a reader cannot tell which kind of run they hold. Omitted
    /// when unknown, so a trail written before the dial existed keeps its
    /// shape and its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
}

/// One thing the agent did, and what came back.
///
/// **Not a verdict, and shaped so it cannot be read as one** (ADR 0004). There
/// is no `passed` here: an edit that applied is not an edit that was correct,
/// and the only field that carries a correctness claim is `check_outcomes`,
/// which comes from a `Verdict` that only an executed process can make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// 1-based, matching the turn numbering a run prints.
    pub turn: u32,
    /// The tool, as the model named it: `blast_radius`, `read_file`, `edit`,
    /// `run_checks`, `read_skill`.
    ///
    /// Or `approval`: a person's answer in approve mode (ADR 0018), recorded
    /// just before the call it decided, under the same turn, with `outcome`
    /// `approved` or `declined`. A human decision is a fact about the run
    /// like any other, and a declined edit must be findable afterwards.
    pub tool: String,

    /// What it was aimed at — a path, a symbol, the checks requested. Empty
    /// when the call named nothing, which `run_checks` with no paths does.
    pub target: String,
    /// What came back, in one line.
    ///
    /// A rendering for a reader, not the observation the model saw: that runs
    /// to thousands of lines for a check and is truncated before it ever
    /// reaches the model. A trail that stored it would be mostly type-checker
    /// output.
    pub outcome: String,
    /// Whether the working tree changed.
    ///
    /// The one bit a reader most wants and cannot infer: `proposed` says which
    /// files a run ended up touching, not which of its actions did the
    /// touching. A run whose every edit was refused has an empty `proposed`
    /// and a full list of actions, and those are different stories.
    pub wrote: bool,
}

/// How much prompt a run consumed, and how fast it grew.
///
/// The growth is the interesting half. A loop that re-sends its whole
/// transcript every turn grows superlinearly in a wide change, and S5 asks
/// what that costs before a user finds the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFill {
    pub first_prompt_bytes: usize,
    pub peak_prompt_bytes: usize,
    pub turns_measured: u32,
}

/// What the run's blast-radius queries found, and what they could not see.
///
/// The three-field `resolved / candidates / unresolved` shape this replaces
/// invited exactly one arithmetic operation — divide the first by the sum —
/// and that division is meaningless here, because the reached counts and the
/// unbound count come from different populations. See `RadiusCoverage` and
/// `docs/adr/0014-radius-coverage.md`.
///
/// It matters more in a trail than anywhere else. Later agents read trails as
/// context, so a figure that reads as "this repository is 3.7% analysable"
/// gains authority with every run that quotes it — the S11 propagation
/// problem, applied to a number rather than an opinion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusCounts {
    /// Summed over every radius the run queried. Additive: each query asks
    /// about a different symbol, and "how many call sites did this run
    /// examine" is the sum of them.
    #[serde(default, alias = "resolved")]
    pub reached_resolved: u32,
    #[serde(default, alias = "candidates")]
    pub reached_ambiguous: u32,
    /// The **largest single** radius's unbound count, not a sum.
    ///
    /// Two radii whose touched files overlap count the same unbound call sites
    /// twice, and most radii in a repository touch overlapping files, so a sum
    /// grows without bound as a run asks more questions. The widest single
    /// query is a real figure about a real radius; a sum is not a figure about
    /// anything.
    ///
    /// Not aliased from the old `unresolved` field: that value *was* a sum, so
    /// migrating it would carry the over-count forward under a name promising
    /// it had been fixed.
    #[serde(default)]
    pub widest_unbound_in_touched_files: u32,
    /// How many radius queries the above summarises, so a reader can tell the
    /// difference between one query and twenty.
    #[serde(default)]
    pub radius_queries: u32,
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
    /// percentage. There is no `total()` here on purpose — ADR 0003.
    pub fn summary(&self) -> String {
        format!(
            "{} verified by an executed check; {} reviewed only \
             (no test covers them — a weak signal)",
            self.verified, self.reviewed
        )
    }
}

/// One executed check, as the trail records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub kind: String,
    pub passed: bool,
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
    /// Format version. Present from the first trail ever written.
    ///
    /// This format will change — v1.x will want fields nobody has thought of
    /// yet — and a reader that cannot tell which version it is holding has to
    /// guess from shape. Every trail written carries it so no trail is ever
    /// orphaned by a change.
    pub schema: u32,
    /// Stable identity: `<timestamp>-<slug>`, matching the filename.
    ///
    /// `trellis resume <run_id>` needs something to name, and a filename is
    /// not it — a path is where a thing lives, not what it is.
    pub run_id: String,
    pub task: String,
    /// The commit the run started from.
    ///
    /// Without it the factual section says what changed and not what it
    /// changed *from*, which makes it uncheckable against the repository a
    /// year later.
    pub base_commit: String,
    /// ISO-8601, so a trail file sorts by name and by content.
    pub started_at: String,
    pub provenance: Provenance,
    pub factual: Factual,
    /// SHA-256 over the canonical JSON of `factual`.
    ///
    /// The v0.3 gate asks for the factual section to be tamper-evident. It
    /// lives in a repository that takes contributions, and an edited figure
    /// there propagates into every future session that reads it (S10). This
    /// does not make editing impossible — anyone who changes the section can
    /// recompute the digest — it makes *silent* editing impossible, which is
    /// the difference between a record and a rumour.
    pub factual_sha256: String,
    pub judgment: Judgment,
}

/// Who produced this trail.
///
/// The gate asks for provenance per entry: which run, which model, which
/// version of TRELLIS. A trail read back a year later is only as useful as
/// what a reader can tell about where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub trellis_version: String,
    /// The `--model` spec, or `None` where nothing drove the loop.
    pub model: Option<String>,
    /// Adapter tier in force, when one was known. Tier 4 output is materially
    /// less reliable and a reader judging an old run should know which it was.
    pub tier: Option<String>,
}

/// The current trail format.
pub const SCHEMA: u32 = 1;

impl Trail {
    /// SHA-256 over `factual`, serialized canonically.
    ///
    /// **Through `Value`, so the key order matches the reader's.** The check
    /// that reads this back — [`UntrustedTrail::integrity`] — parses the file
    /// and hashes `v["factual"]`, and `serde_json::Map` is a `BTreeMap`, so
    /// its keys come back sorted. Hashing the struct directly emits fields in
    /// *declaration* order, which is a different byte sequence and therefore
    /// a different digest.
    ///
    /// Measured, for one `Factual`:
    ///
    /// ```text
    /// to_vec(&factual)                 dedc5e7e…   {"proposed":…
    /// to_vec(&to_value(&factual))      6a2530b3…   {"approved_by":…
    /// to_vec(parse(to_string(factual)))6a2530b3…   {"approved_by":…
    /// ```
    ///
    /// **Why it matters that both sides go through `Value`.** Hashing the
    /// struct directly agrees with the reader only while
    /// `serde_json::Map` is an `IndexMap` — that is, while
    /// `serde_json/preserve_order` is enabled. It is, but by `schemars`,
    /// which this crate does not depend on and which enables it to derive
    /// tool-call schemas in `trellis-loop`. Tamper-evidence resting on a
    /// sibling crate's transitive feature is not a property anyone declared
    /// or tested. Going through `Value` here means both sides share whatever
    /// `Map` is and agree either way. C-016.
    ///
    /// Written by `cmd_run` when a trail is recorded. The *check* against it
    /// lives in [`UntrustedTrail::integrity`], which reads the raw JSON as it
    /// came off disk — a trail is read back as untrusted data (S10), so the
    /// thing that verifies it takes bytes rather than a parsed `Trail`.
    ///
    /// `serde_json` orders struct fields by declaration and `BTreeMap` keys by
    /// sort order, so the same `Factual` always serializes identically. That
    /// determinism is what makes the digest checkable rather than decorative.
    pub fn compute_factual_sha256(factual: &Factual) -> String {
        use sha2::{Digest, Sha256};
        let json = serde_json::to_value(factual)
            .and_then(|v| serde_json::to_vec(&v))
            .unwrap_or_default();
        let digest = Sha256::digest(&json);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
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
            schema: SCHEMA,
            run_id: "20260920-000000-000-test".into(),
            base_commit: "0000000".into(),
            provenance: Provenance {
                trellis_version: "0.0.1".into(),
                model: None,
                tier: None,
            },
            factual: Factual {
                proposed: vec!["src/poetry/installation/installer.py".into()],
                radius: RadiusCounts {
                    reached_resolved: 47,
                    reached_ambiguous: 3,
                    widest_unbound_in_touched_files: 2,
                    radius_queries: 1,
                },
                checks: vec![],
                check_outcomes: vec![CheckOutcome {
                    kind: "Tests".into(),
                    passed: true,
                }],
                approved_by: Approval::Human,
                mode_counts: ModeCounts {
                    verified: 45,
                    reviewed: 7,
                },
                context: Some(ContextFill {
                    first_prompt_bytes: 1_200,
                    peak_prompt_bytes: 9_400,
                    turns_measured: 6,
                }),
                actions: vec![
                    Action {
                        turn: 1,
                        tool: "blast_radius".into(),
                        target: "Installer.run".into(),
                        outcome: "47 resolved, 3 candidates, 2 unresolved".into(),
                        wrote: false,
                    },
                    Action {
                        turn: 2,
                        tool: "edit".into(),
                        target: "src/poetry/installation/installer.py".into(),
                        outcome: "applied".into(),
                        wrote: true,
                    },
                    Action {
                        turn: 3,
                        tool: "run_checks".into(),
                        target: "Tests".into(),
                        outcome: "Tests: passed".into(),
                        wrote: false,
                    },
                ],
                control: None,
            },

            factual_sha256: String::new(),
            judgment: Judgment {
                tech_debt_projection: Some("the adapter shim will need removing".into()),
                rationale: Some("kept the old name as an alias".into()),
            },
        };

        let v: serde_json::Value = serde_json::from_str(&t.to_json().unwrap()).unwrap();
        assert!(v["factual"]["radius"]["widest_unbound_in_touched_files"] == 2);
        // The field name is load-bearing. `unresolved` beside `resolved` reads
        // as two parts of one whole and invites a division that is meaningless
        // here (ADR 0014), and a trail is the document later agents quote.
        assert!(
            v["factual"]["radius"]["unresolved"].is_null(),
            "the old name must not reappear, under any serde alias"
        );
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
