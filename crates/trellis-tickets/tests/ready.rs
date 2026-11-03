//! `trellis-tickets ready <n>` over issue JSON recorded from
//! `gh issue view <n> --json number,title,body,labels,state`
//! (`tests/fixtures/triage/`), each test changing one field of a recording
//! that passes every check. The fetch is a closure, so `gh` is never
//! spawned and no network is used.
//!
//! One `ready_fails_*` test per failure name. The G0 names are
//! `scripts/gates/ticket-ready.sh`'s strings: ADR 0040 decision 7 compares
//! the two over G0's checks, so a name drifting here would read as a
//! disagreement that is not one.

use std::cell::RefCell;
use std::path::PathBuf;

use trellis_tickets::{
    Abstention, IntakeError, ReadyReport, TriageError, gh_view_args, ready_with,
};

fn recorded(n: u64) -> Result<String, String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/triage")
        .join(format!("issue-{n}.json"));
    std::fs::read_to_string(&p).map_err(|e| format!("no recording for #{n}: {e}"))
}

/// #202 as recorded: open, one CLI file, a fenced `verify-tests.sh` line,
/// no blockers, no decision words. It passes every check, so a test that
/// changes one thing sees that thing's failure alone.
const BASE: u64 = 202;

/// #202's recording with `edit` applied to its parsed JSON.
fn base_with(edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&recorded(BASE).unwrap()).unwrap();
    edit(&mut v);
    v.to_string()
}

fn set_body(v: &mut serde_json::Value, f: impl FnOnce(&str) -> String) {
    let body = f(v["body"].as_str().unwrap());
    v["body"] = serde_json::Value::String(body);
}

fn add_label(v: &mut serde_json::Value, name: &str) {
    v["labels"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "name": name }));
}

/// `ready` of #202 changed by `edit`; every other issue is read from its
/// recording.
fn ready_of(edit: impl FnOnce(&mut serde_json::Value)) -> ReadyReport {
    let json = base_with(edit);
    ready_with(BASE, |n| {
        if n == BASE {
            Ok(json.clone())
        } else {
            recorded(n)
        }
    })
    .unwrap_or_else(|e| panic!("ready #{BASE}: {e}"))
}

/// Asserts `r` fails with exactly `name` and nothing else, and prints it.
fn fails_only(r: &ReadyReport, name: &str) {
    let all: Vec<&String> = r.g0.iter().chain(&r.extra).collect();
    assert_eq!(all, [name], "{r:?}");
    assert!(r.abstained.is_empty(), "{r:?}");
    assert!(!r.is_ready());
    assert!(
        r.text().lines().any(|l| l == format!("  - {name}")),
        "{name} not printed:\n{}",
        r.text()
    );
}

#[test]
fn ready_passes_the_recording_every_other_test_changes() {
    // If the base failed anything, each `fails_only` below would be
    // measuring that failure too.
    let r = ready_of(|_| {});
    assert!(r.is_ready(), "{r:?}");
    assert_eq!(r.text().lines().count(), 1);
    assert!(r.text().ends_with(": ready\n"), "{}", r.text());
}

// ---- G0's conditions, as scripts/gates/ticket-ready.sh names them ------

#[test]
fn ready_fails_a_closed_issue() {
    let r = ready_of(|v| v["state"] = "CLOSED".into());
    fails_only(&r, "issue is CLOSED");
}

#[test]
fn ready_fails_no_done_when_section() {
    let r = ready_of(|v| set_body(v, |b| b.replace("## Done when", "## Done")));
    fails_only(&r, "no '## Done when' section");
}

#[test]
fn ready_fails_no_fenced_command_under_verify() {
    // With no fence there is also no command, so the extra fails beside
    // it; G0's name is the one this test pins.
    let r = ready_of(|v| set_body(v, |b| b.replace("```", "")));
    assert_eq!(r.g0, ["no fenced command under '## Verify'"]);
    assert_eq!(r.extra, ["no verify command"]);
}

#[test]
fn ready_fails_triage_blocked_with_no_blocked_by() {
    // G0 fails `triage:blocked` only when nothing is named to wait on. A
    // gate that failed every `triage:blocked` issue would never agree with
    // G0 on one whose blockers have closed, and the parity check (ADR 0040
    // decision 7) could not pass.
    let r = ready_of(|v| add_label(v, "triage:blocked"));
    fails_only(&r, "labelled triage:blocked with no blocked_by");

    let waited = ready_of(|v| {
        add_label(v, "triage:blocked");
        set_body(v, |b| format!("{b}\n\nblocked_by: [75]"));
    });
    assert!(waited.is_ready(), "#75 is recorded CLOSED: {waited:?}");
}

#[test]
fn ready_fails_parked() {
    let r = ready_of(|v| add_label(v, "parked"));
    fails_only(&r, "parked: waits on the owner's answer");
}

#[test]
fn ready_fails_triage_needs_human() {
    let r = ready_of(|v| add_label(v, "triage:needs-human"));
    fails_only(&r, "labelled triage:needs-human: a person routes this one");
}

#[test]
fn ready_fails_split() {
    let r = ready_of(|v| add_label(v, "split"));
    fails_only(&r, "split: work happens in its child tickets");
}

#[test]
fn ready_fails_an_open_blocker_read_from_its_own_issue() {
    // #198 names #60, recorded OPEN. The state is the one `gh` gave for #60
    // itself; read from anywhere else it could unblock a waiting ticket.
    let asked = RefCell::new(Vec::new());
    let r = ready_with(198, |n| {
        asked.borrow_mut().push(n);
        recorded(n)
    })
    .unwrap();
    assert_eq!(asked.into_inner(), [198, 60]);
    fails_only(&r, "blocked_by #60 is OPEN");
}

#[test]
fn ready_fails_a_blocked_by_entry_that_is_not_an_issue_number() {
    // A dependency the gate cannot look up is not a satisfied one. The
    // numbered entry beside it is still looked up.
    let r = ready_of(|v| set_body(v, |b| format!("{b}\n\nblocked_by: [75, child-a]")));
    fails_only(&r, "blocked_by entry 'child-a' is not an issue number");
}

#[test]
fn ready_fails_a_blocker_that_cannot_be_read_as_unknown() {
    // G0 reports a lookup it could not make as UNKNOWN. So does this, with
    // the reason; `gh` answering for another issue is one such reason.
    let json = base_with(|v| set_body(v, |b| format!("{b}\n\nblocked_by: [75]")));
    let r = ready_with(BASE, |n| {
        if n == BASE {
            Ok(json.clone())
        } else {
            recorded(76)
        }
    })
    .unwrap();
    assert!(r.extra.is_empty() && r.abstained.is_empty(), "{r:?}");
    assert_eq!(r.g0.len(), 1, "{r:?}");
    assert!(
        r.g0[0].starts_with("blocked_by #75 is UNKNOWN (asked for #75 and got #76)"),
        "{r:?}"
    );
}

// ---- the four named extras ---------------------------------------------

#[test]
fn ready_fails_no_verify_command() {
    // G0 sees a fence and passes; the fence holds only a comment, so there
    // is nothing to run. This is the gap the extra exists for.
    let r = ready_of(|v| {
        set_body(v, |b| {
            b.replace(
                "scripts/tickets/verify-tests.sh trellis-cli tui_steer_for_the_next_task 1",
                "# run the tui tests",
            )
        })
    });
    fails_only(&r, "no verify command");
}

#[test]
fn ready_fails_decision_words() {
    let r = ready_of(|v| v["title"] = "Design the forward_steers boundary".into());
    fails_only(&r, "decision words: design");
}

#[test]
fn ready_fails_effort_l() {
    // Six files: split it (§5).
    let r = ready_of(|v| {
        set_body(v, |b| {
            b.replace(
                "**Files:** crates/trellis-cli/src/tui_run.rs",
                "**Files:** crates/trellis-cli/src/a.rs; crates/trellis-cli/src/b.rs; \
                 crates/trellis-cli/src/c.rs; crates/trellis-cli/src/d.rs; \
                 crates/trellis-cli/src/e.rs; crates/trellis-cli/src/f.rs",
            )
        })
    });
    fails_only(&r, "effort:l");
}

#[test]
fn ready_fails_needs_adr() {
    // `risk:high` (area:checks) on a ticket that is not a bug needs an ADR,
    // and no issue links one, so the gate fails it by name.
    let r = ready_of(|v| {
        set_body(v, |b| {
            b.replace(
                "**Files:** crates/trellis-cli/src/tui_run.rs",
                "**Files:** crates/trellis-checks/src/verdict.rs",
            )
        })
    });
    fails_only(&r, "needs ADR");
}

// ---- what the gate does around the failures ----------------------------

#[test]
fn ready_reports_an_abstention_by_name_and_is_not_ready() {
    // `fuzz/` is under no path in the area table (`scripts/` was, until
    // #194 mapped it to area:build), so risk cannot
    // be told and `needs ADR` is undecidable. Treating
    // that as a pass would call the ticket ready on a guess; treating it as
    // a failure would name a failure nobody found. It is neither.
    let r = ready_of(|v| {
        set_body(v, |b| {
            b.replace(
                "**Files:** crates/trellis-cli/src/tui_run.rs",
                "**Files:** fuzz/fuzz_targets/http_response.rs",
            )
        })
    });
    assert!(r.g0.is_empty() && r.extra.is_empty(), "{r:?}");
    assert_eq!(r.abstained.len(), 1, "{r:?}");
    let Abstention { check, reason } = &r.abstained[0];
    assert_eq!(*check, "needs ADR");
    assert!(
        reason.contains("`fuzz/fuzz_targets/http_response.rs` is under no path in the area table"),
        "{reason}"
    );
    assert!(!r.is_ready());
    assert!(
        r.text()
            .lines()
            .any(|l| l.starts_with("  - abstained: needs ADR (risk abstained: area abstained: ")),
        "{}",
        r.text()
    );
}

#[test]
fn ready_prints_every_failure_at_once_g0_first() {
    // "Not ready" with no reason is useless (§5), and so is one reason at a
    // time: each fix would reveal the next.
    let r = ready_of(|v| {
        v["state"] = "CLOSED".into();
        v["title"] = "Design the boundary".into();
        add_label(v, "parked");
        add_label(v, "split");
    });
    assert_eq!(
        r.g0,
        [
            "issue is CLOSED",
            "parked: waits on the owner's answer",
            "split: work happens in its child tickets",
        ]
    );
    assert_eq!(r.extra, ["decision words: design"]);
    assert_eq!(r.text().lines().count(), 5, "{}", r.text());
}

#[test]
fn ready_only_reads_the_issue_and_its_blockers_through_one_rest_read() {
    // The command is read-only: the one argv is a view, the same `triage`
    // builds, asked for #77 and the two issues it names, and nothing else.
    assert_eq!(gh_view_args(77), ["api", "repos/{owner}/{repo}/issues/77"]);
    let asked = RefCell::new(Vec::new());
    ready_with(77, |n| {
        asked.borrow_mut().push(n);
        recorded(n)
    })
    .unwrap();
    assert_eq!(asked.into_inner(), [77, 75, 76]);
}

#[test]
fn ready_refuses_gh_output_for_another_issue() {
    // The issue itself read wrong would gate some other ticket's text.
    let r = ready_with(BASE, |_| recorded(198));
    assert!(matches!(
        r,
        Err(TriageError::Intake(IntakeError::WrongIssue {
            asked: BASE,
            got: 198
        }))
    ));
}

#[test]
fn ready_escapes_control_characters_from_the_issue_in_its_text() {
    // An issue title is anyone's text; printed raw, an escape sequence in
    // it would recolour or rewrite the terminal the gate is read in. A
    // `blocked_by` word is quoted back in its failure, so it is escaped too.
    let r = ready_of(|v| {
        v["title"] = "\u{1b}[31mtui: it".into();
        set_body(v, |b| format!("{b}\n\nblocked_by: [x\u{1b}[2J]"));
    });
    assert_eq!(
        r.g0,
        ["blocked_by entry 'x\u{1b}[2J' is not an issue number"]
    );
    let text = r.text();
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(
        text.starts_with("#202 \\u{1b}[31mtui: it: not ready\n"),
        "{text}"
    );
    assert!(
        text.contains("  - blocked_by entry 'x\\u{1b}[2J' is not an issue number"),
        "{text}"
    );
}
