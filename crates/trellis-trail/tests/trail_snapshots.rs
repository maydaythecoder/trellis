//! Snapshots of the JSON a trail is written as.
//!
//! The trail is the on-disk record (ADR 0007): it is committed, later runs
//! read it back as context, and its factual section carries a digest that a
//! reader checks. The unit tests in `lib.rs` assert individual properties of
//! that JSON. These pin the whole document, so a renamed field, a reordered
//! key, a field that starts or stops being omitted, or a change to how the
//! digest is computed shows up as a reviewed diff instead of passing because
//! no single assertion looked there.
//!
//! Every volatile field (`run_id`, `started_at`, `base_commit`, the trellis
//! version) is a fixed literal in the fixture rather than redacted, so the
//! snapshot needs no insta `redactions` feature and the digest in it is the
//! digest of exactly what is shown.
//!
//! `Factual::checks` is always empty here: a `Verdict` cannot be constructed
//! outside `trellis-checks` (ADR 0004), which is why the trail also records
//! `check_outcomes` as inert data.

use trellis_trail::{
    Action, Approval, CheckOutcome, ContextFill, Factual, Input, Integrity, Judgment, ModeCounts,
    Provenance, RadiusCounts, SCHEMA, Steer, SteerKind, SuspectedRecallGap, Trail, UntrustedTrail,
};

/// Builds a trail around `factual` with fixed identity fields and the digest
/// computed the way `cmd_run` computes it, then serializes it.
///
/// Before returning, the JSON is read back as untrusted data and its
/// integrity checked, so a snapshot can never be accepted for a document
/// whose recorded digest a reader would call `Altered`.
fn trail_json(task: &str, factual: Factual, judgment: Judgment) -> String {
    let t = Trail {
        schema: SCHEMA,
        run_id: "20260918-051200-000-fixture".into(),
        task: task.into(),
        base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        started_at: "2026-09-18T05:12:00Z".into(),
        provenance: Provenance {
            trellis_version: "0.0.0-fixture".into(),
            model: Some("scripted".into()),
            tier: Some("tier1".into()),
        },
        factual_sha256: Trail::compute_factual_sha256(&factual),
        factual,
        judgment,
    };
    let json = t.to_json().expect("a trail serializes");
    let back = UntrustedTrail::from_json(&json).expect("a written trail reads back");
    assert_eq!(
        back.integrity(&json),
        Integrity::Intact,
        "the digest a writer records must match what a reader recomputes"
    );
    json
}

#[test]
fn trail_snapshot_full_run_with_every_optional_field_present() {
    // A run that did everything a trail can record: radius queries, an edit,
    // checks, context fill, the control dial, tokens, an agent profile and
    // user inputs. The risk defended: a field of the factual section changing
    // name, shape or presence, or the digest over it changing, without a
    // reviewed diff. Both are what later runs trust (S10, S11).
    let factual = Factual {
        proposed: vec!["src/pkg/installer.py".into()],
        radius: RadiusCounts {
            reached_resolved: 47,
            reached_ambiguous: 3,
            widest_unbound_in_touched_files: 2,
            radius_queries: 1,
            summarised: 1,
        },
        checks: vec![],
        check_outcomes: vec![
            CheckOutcome {
                kind: "Typecheck".into(),
                passed: true,
            },
            CheckOutcome {
                kind: "Tests".into(),
                passed: true,
            },
        ],
        approved_by: Approval::Human,
        mode_counts: ModeCounts {
            verified: 45,
            reviewed: 7,
        },
        context: Some(ContextFill {
            first_prompt_bytes: 1_200,
            peak_prompt_bytes: 9_400,
            turns_measured: 3,
        }),
        actions: vec![
            Action {
                turn: 1,
                tool: "blast_radius".into(),
                target: "Installer.run".into(),
                outcome: "47 resolved, 3 candidates, 2 unresolved".into(),
                wrote: false,
                changed: Vec::new(),
                changed_count: 0,
            },
            Action {
                turn: 2,
                tool: "approval".into(),
                target: "src/pkg/installer.py".into(),
                outcome: "approved".into(),
                wrote: false,
                changed: Vec::new(),
                changed_count: 0,
            },
            Action {
                turn: 2,
                tool: "edit".into(),
                target: "src/pkg/installer.py".into(),
                outcome: "applied".into(),
                wrote: true,
                changed: Vec::new(),
                changed_count: 0,
            },
            Action {
                turn: 2,
                tool: "bash".into(),
                target: "python scripts/regen.py".into(),
                outcome: "exit 0".into(),
                wrote: false,
                changed: Vec::new(),
                changed_count: 0,
            }
            .with_changed(&["src/pkg/installer_gen.py".into()]),
            Action {
                turn: 3,
                tool: "run_checks".into(),
                target: String::new(),
                outcome: "Typecheck: passed; Tests: passed".into(),
                wrote: false,
                changed: Vec::new(),
                changed_count: 0,
            },
        ],
        control: Some("approve (edit)".into()),
        tokens: Some(812),
        prompt_tokens: Some(9_400),
        agent: Some("builder".into()),
        inputs: vec![
            Input {
                kind: "context_file".into(),
                name: "CLAUDE.md".into(),
                bytes: 2_048,
                sha256: None,
            },
            Input {
                kind: "forced_skill".into(),
                name: "refactor".into(),
                bytes: 512,
                sha256: None,
            },
            Input::hashed("image", "docs/before.png", b"fixture image bytes"),
        ],
        steering: vec![Steer::new(
            2,
            SteerKind::Steer,
            "keep the old name as an alias",
        )],
        label: Some("installer rename".into()),
        // Present for its shape, though the final outcomes above passed: a
        // real run records a gap only for a failing integrated check.
        suspected_recall_gaps: vec![SuspectedRecallGap {
            partitions: vec![0, 1],
            files: vec!["src/pkg/installer.py".into(), "src/pkg/cli.py".into()],
            check: "Typecheck".into(),
        }],
    };
    let judgment = Judgment {
        tech_debt_projection: Some("the alias will need removing".into()),
        rationale: Some("kept the old name as an alias".into()),
    };
    insta::assert_snapshot!(trail_json("rename Installer.run", factual, judgment));
}

#[test]
fn trail_snapshot_rolled_back_run_omits_absent_optional_fields() {
    // A run that never reached approval: nothing proposed, one failed check,
    // and every optional field absent. `control`, `tokens`, `agent` and
    // `inputs` are skipped when absent so that trails written before those
    // fields existed keep their digest; `context` is not skipped and shows as
    // null. The risk defended: an omitted field starting to serialize as null
    // or empty, which changes the digest of every older-shaped trail.
    let factual = Factual {
        proposed: vec![],
        radius: RadiusCounts::default(),
        checks: vec![],
        check_outcomes: vec![CheckOutcome {
            kind: "Tests".into(),
            passed: false,
        }],
        approved_by: Approval::None,
        mode_counts: ModeCounts::default(),
        context: None,
        actions: vec![],
        control: None,
        tokens: None,
        prompt_tokens: None,
        agent: None,
        inputs: vec![],
        steering: vec![],
        label: None,
        suspected_recall_gaps: vec![],
    };
    insta::assert_snapshot!(trail_json("fix flaky test", factual, Judgment::default()));
}
