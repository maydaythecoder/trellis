//! `trellis-tickets triage <n>` over issue JSON recorded from
//! `gh issue view <n> --json number,title,body,labels,state`
//! (`tests/fixtures/triage/`). The fetch is a closure over those files, so
//! `gh` is never spawned and no network is used.

use std::cell::RefCell;
use std::path::PathBuf;

use trellis_tickets::{
    GH_VIEW_FIELDS, IntakeError, Line, Report, TriageError, Value, gh_view_args, triage_with,
};

fn recorded(n: u64) -> Result<String, String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/triage")
        .join(format!("issue-{n}.json"));
    std::fs::read_to_string(&p).map_err(|e| format!("no recording for #{n}: {e}"))
}

/// Triage of `n` over the recordings, and every issue the command fetched.
fn triage_recorded(n: u64) -> (Report, Vec<u64>) {
    let asked = RefCell::new(Vec::new());
    let r = triage_with(n, |m| {
        asked.borrow_mut().push(m);
        recorded(m)
    })
    .unwrap_or_else(|e| panic!("triage #{n}: {e}"));
    (r, asked.into_inner())
}

#[test]
fn triage_prints_one_line_per_dimension_with_the_rule_or_the_abstention() {
    // The point of the command (§10): a label nobody can trace to a rule is
    // one nobody can check. #202 decides eight lines and abstains on one.
    let (r, _) = triage_recorded(202);
    let text = r.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 10, "a heading and nine lines:\n{text}");
    assert!(lines[0].starts_with("#202 tui: forward_steers"));
    let names = [
        "area", "kind", "risk", "surface", "effort", "origin", "flags", "ready", "triage",
    ];
    for (line, name) in lines[1..].iter().zip(names) {
        assert!(line.starts_with(name), "{name} out of order:\n{text}");
        assert!(line.contains("  (rule: §"), "{name} has no rule:\n{text}");
    }
    assert!(lines[1].contains("area:cli  (rule: §5 area: from file paths)"));
    assert!(lines[4].contains("surface:cli"));
    // No `origin:` label is recorded on #202: the line says so rather than
    // printing a default origin.
    assert!(
        lines[6].contains("abstained: no `origin:` label is recorded"),
        "{text}"
    );
    assert!(lines[9].contains("triage:needs-human: origin abstained"));
}

#[test]
fn triage_abstains_on_a_crate_the_area_table_does_not_name() {
    // #201 names only crates/trellis-loop, which §5's table leaves out
    // (the owner's open question #194). A guessed `area:` there would be
    // the "wrong label that looks authoritative" §1 forbids, in both forms.
    let (r, _) = triage_recorded(201);
    let Line::Abstained { reason, .. } = &r.area else {
        panic!("area decided on an unmapped crate: {:?}", r.area);
    };
    assert!(
        reason.contains("`crates/trellis-loop/src/trust.rs` is under no path in the area table")
    );
    assert!(
        r.text()
            .lines()
            .any(|l| l.starts_with("area    abstained: `crates/trellis-loop"))
    );
    let json: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
    assert_eq!(json["area"]["outcome"], "abstained");
    assert_eq!(json["area"]["reason"].as_str(), Some(reason.as_str()));
    assert_eq!(json["triage"]["value"]["route"], "triage:needs-human");
}

#[test]
fn triage_json_is_one_object_and_blocker_state_comes_from_the_same_read() {
    // #198 is blocked by #60, recorded OPEN. Reading the blocker's state
    // wrong would report a waiting ticket as ready.
    let (r, asked) = triage_recorded(198);
    assert_eq!(asked, [198, 60], "fetched only the issue and its blocker");
    let out = r.json();
    // `from_str` rejects anything after the first value, so this is "one
    // object", not "starts with one".
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let obj = json.as_object().expect("an object");
    for key in [
        "number",
        "title",
        "triage_version",
        "area",
        "kind",
        "risk",
        "surface",
        "effort",
        "origin",
        "flags",
        "ready",
        "triage",
    ] {
        assert!(obj.contains_key(key), "no `{key}` in {out}");
    }
    assert_eq!(json["number"], 198);
    assert_eq!(json["triage_version"], 2);
    assert_eq!(json["ready"]["outcome"], "decided");
    assert_eq!(json["ready"]["value"]["ready"], false);
    assert_eq!(
        json["ready"]["value"]["failures"],
        serde_json::json!(["blocked by #60 (OPEN)"])
    );
    assert_eq!(json["ready"]["rule"], "§5 ready");
}

#[test]
fn triage_only_reads_the_issue_and_its_blockers_through_gh_issue_view() {
    // Read-only is the ticket's contract: `sweep --apply` is the only
    // command that changes an issue. The one argv is a view, and it is
    // asked for #77 and the two issues its triage block names, nothing else.
    assert_eq!(
        gh_view_args(77),
        ["issue", "view", "77", "--json", GH_VIEW_FIELDS]
    );
    let (r, asked) = triage_recorded(77);
    assert_eq!(asked, [77, 75, 76]);
    // #77's body has no `**Files:**` line, so every file-derived dimension
    // abstains and says why, and triage routes it to a person.
    assert!(
        matches!(&r.area, Line::Abstained { reason, .. } if reason.contains("no `**Files:**` line"))
    );
    assert!(matches!(
        &r.triage,
        Line::Decided {
            value: Value::Route {
                route: "triage:needs-human",
                ..
            },
            ..
        }
    ));
}

#[test]
fn triage_refuses_gh_output_for_another_issue_or_an_unknown_state() {
    // `gh`'s output crosses a trust boundary. Output for the wrong issue
    // would give #198 some other issue's blocker state; a state the rules
    // do not know would be read as open or closed by accident.
    let wrong = triage_with(198, |m| if m == 60 { recorded(202) } else { recorded(m) });
    assert!(matches!(
        wrong,
        Err(TriageError::Intake(IntakeError::WrongIssue {
            asked: 60,
            got: 202
        }))
    ));

    let merged = recorded(202)
        .unwrap()
        .replace("\"state\":\"OPEN\"", "\"state\":\"MERGED\"");
    assert!(
        merged.contains("MERGED"),
        "the recording's state field moved"
    );
    let unknown = triage_with(202, |_| Ok(merged.clone()));
    assert!(matches!(
        unknown,
        Err(TriageError::Intake(IntakeError::UnknownState {
            number: 202,
            ..
        }))
    ));

    let failed = triage_with(202, |_| Err("gh: not logged in".to_string()));
    assert!(matches!(
        failed,
        Err(TriageError::Fetch { number: 202, .. })
    ));
}

#[test]
fn triage_escapes_control_characters_from_the_issue_in_its_text() {
    // An issue title is anyone's text; printed raw, an escape sequence in it
    // would recolour or rewrite the terminal the triage is read in.
    let json = recorded(202)
        .unwrap()
        .replace("\"title\":\"tui:", "\"title\":\"\\u001b[31mtui:");
    let r = triage_with(202, |_| Ok(json.clone())).unwrap();
    assert!(
        r.title.contains('\u{1b}'),
        "the edit did not reach the title"
    );
    let text = r.text();
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(text.starts_with("#202 \\u{1b}[31mtui:"), "{text}");
}
