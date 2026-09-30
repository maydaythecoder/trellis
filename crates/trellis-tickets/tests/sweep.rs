//! `trellis-tickets sweep` with `gh` stubbed. The open-issue list is built
//! from issue JSON recorded from `gh issue view <n> --json
//! number,title,body,labels,state` (`tests/fixtures/triage/`), each test
//! changing what it needs; reads are a closure over those, and the edit
//! step is a closure that records what it was given. No test spawns `gh`
//! or reaches GitHub.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use trellis_tickets::{SWEEP_LIST_LIMIT, SweepError, SweepOutcome, sweep_with};

fn recorded(n: u64) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/triage")
        .join(format!("issue-{n}.json"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("no recording for #{n}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn set_labels(v: &mut Value, labels: &[&str]) {
    v["labels"] = Value::Array(labels.iter().map(|l| json!({ "name": l })).collect());
}

fn body(v: &Value) -> &str {
    v["body"].as_str().unwrap()
}

/// #202 as recorded (one CLI file, no triage block, no blockers), labelled
/// so that every rule decides and the labels already match: no drift.
fn quiet_202() -> Value {
    let mut v = recorded(202);
    set_labels(
        &mut v,
        &[
            "area:cli",
            "kind:feature",
            "risk:medium",
            "surface:cli",
            "effort:s",
            "origin:manual",
            "triage:auto",
        ],
    );
    v
}

/// #77 as recorded, with a `**Files:**` line so every rule decides, and
/// `effort:l` where the rules compute `effort:m`: one label of drift, on a
/// body whose lines end `\r\r\n` and which carries a triage block.
fn drifted_77() -> Value {
    let mut v = recorded(77);
    let b = body(&v).replacen(
        "\r\r\n",
        "\r\r\n**Files:** crates/trellis-cli/src/main.rs\r\r\n",
        1,
    );
    v["body"] = Value::String(b);
    set_labels(
        &mut v,
        &[
            "P2",
            "area:cli",
            "kind:feature",
            "risk:medium",
            "surface:cli",
            "effort:l",
            "origin:manual",
            "!agent-ok",
            "triage:auto",
            "type:ticket",
        ],
    );
    v
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Shown(String),
    Edited {
        args: Vec<String>,
        stdin: Option<String>,
    },
}

/// Runs sweep over `open`, with reads served from `open` (as `gh issue
/// view` would serve the same issues), then from `reads`, then from the
/// recordings. Returns the outcome and every event in order.
fn run(
    apply: bool,
    open: &[Value],
    reads: BTreeMap<u64, Value>,
) -> (Result<SweepOutcome, SweepError>, Vec<Event>) {
    let events = RefCell::new(Vec::new());
    let list = Value::Array(open.to_vec()).to_string();
    let fetch = |n: u64| -> Result<String, String> {
        if let Some(v) = reads.get(&n) {
            return Ok(v.to_string());
        }
        if let Some(v) = open.iter().find(|v| v["number"] == n) {
            return Ok(v.to_string());
        }
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/triage")
            .join(format!("issue-{n}.json"));
        std::fs::read_to_string(&p).map_err(|e| format!("no recording for #{n}: {e}"))
    };
    let out = sweep_with(
        apply,
        "2026-09-30T12:00:00Z",
        || Ok(list),
        fetch,
        |s| events.borrow_mut().push(Event::Shown(s.to_string())),
        |args, stdin| {
            events.borrow_mut().push(Event::Edited {
                args: args.to_vec(),
                stdin: stdin.map(str::to_string),
            });
            Ok(())
        },
    );
    (out, events.into_inner())
}

fn shown(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Shown(s) => Some(s.as_str()),
            Event::Edited { .. } => None,
        })
        .collect()
}

fn edits(events: &[Event]) -> Vec<(&[String], Option<&str>)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Edited { args, stdin } => Some((args.as_slice(), stdin.as_deref())),
            Event::Shown(_) => None,
        })
        .collect()
}

#[test]
fn sweep_dry_run_prints_drift_as_minus_plus_with_its_rule_and_calls_no_edit() {
    // The default must not write: a sweep someone runs to look would
    // otherwise relabel every open issue.
    let (out, events) = run(false, &[drifted_77(), quiet_202()], BTreeMap::new());
    let out = out.unwrap();
    let text = shown(&events);
    assert!(edits(&events).is_empty(), "a dry run edited:\n{events:?}");
    assert!(out.written.is_empty());
    assert!(
        text.contains("#77: -effort:l +effort:m  (rule: §5 effort: 3-5 files or 2+ new tests)"),
        "{text}"
    );
    assert!(
        text.lines().all(|l| !l.starts_with("#202:")),
        "an issue with no drift printed a line:\n{text}"
    );
    assert!(text.contains("dry run: nothing written"), "{text}");
}

#[test]
fn sweep_apply_never_touches_a_parked_or_split_issue() {
    // ADR 0040: a parked issue waits on the owner and a split one's work is
    // in its children; relabelling either would put it back in the queue.
    let mut split = drifted_77();
    split["labels"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "split" }));
    let mut parked = recorded(202);
    parked["number"] = json!(9202);
    set_labels(&mut parked, &["parked", "effort:l"]);
    let (out, events) = run(true, &[split, parked], BTreeMap::new());
    let out = out.unwrap();
    let text = shown(&events);
    assert!(
        edits(&events).is_empty(),
        "wrote a skipped issue:\n{events:?}"
    );
    assert!(out.written.is_empty());
    assert!(text.contains("#77: skipped (split)"), "{text}");
    assert!(text.contains("#9202: skipped (parked)"), "{text}");
    assert!(
        !text.contains("effort:m"),
        "rules ran on a skipped issue:\n{text}"
    );
}

#[test]
fn sweep_apply_changes_only_the_triage_block_lines_and_keeps_every_other_byte() {
    // `--body-file` replaces the whole body. A rewrite that normalised the
    // `\r\r\n` endings, or touched `ready:` (sync-ready.py's), would be a
    // change to every issue nobody asked for.
    let before = drifted_77();
    let (out, events) = run(true, std::slice::from_ref(&before), BTreeMap::new());
    assert_eq!(out.unwrap().written, [77]);
    let first_edit = events
        .iter()
        .position(|e| matches!(e, Event::Edited { .. }))
        .unwrap();
    assert!(
        shown(&events[..first_edit]).contains("#77: -effort:l +effort:m"),
        "the plan was not shown before the write"
    );
    let e = edits(&events);
    assert_eq!(e.len(), 1);
    let (args, stdin) = e[0];
    assert_eq!(
        args,
        [
            "issue",
            "edit",
            "77",
            "--add-label",
            "effort:m",
            "--remove-label",
            "effort:l",
            "--body-file",
            "-"
        ]
    );
    let old = body(&before);
    let new = stdin.expect("the triage block changed, so a body is sent");
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    assert_eq!(old_lines.len(), new_lines.len());
    let block_start = old_lines
        .iter()
        .position(|l| l.starts_with("<!-- triage:start"))
        .unwrap();
    let mut changed = Vec::new();
    for (i, (o, n)) in old_lines.iter().zip(&new_lines).enumerate() {
        if o == n {
            continue;
        }
        assert!(i > block_start, "line {i} outside the block changed: {o:?}");
        assert!(
            o.ends_with("\r\r\n") && n.ends_with("\r\r\n"),
            "{o:?} -> {n:?}"
        );
        changed.push(n.trim_end().to_string());
    }
    // The block recorded `blocked` on #75; #75 and #76 are closed in the
    // recordings, so the route applied, and recorded, is `auto`. The
    // `labels:` and `flags:` lines already said what the labels now say.
    assert_eq!(
        changed,
        [
            "triage: auto",
            "triage_reasons: [\"classified confidently and ready\"]",
            "triaged_at: 2026-09-30T12:00:00Z"
        ]
    );
    for keep in ["ready: false", "ready_failures:", "blocked_by: [75, 76]"] {
        assert!(new.contains(keep), "`{keep}` lost");
    }
}

#[test]
fn sweep_leaves_a_dimension_whose_rule_abstained_as_labelled_and_says_abstained() {
    // #194: crates/trellis-loop maps to no area. Removing `area:loop` or
    // moving the issue to needs-human on that would be a guess.
    let mut v = recorded(201);
    set_labels(&mut v, &["area:loop", "triage:auto", "origin:manual"]);
    let (out, events) = run(true, &[v], BTreeMap::new());
    let text = shown(&events);
    assert!(
        text.contains("#201: area abstained: `crates/trellis-loop/src/trust.rs` is under no path in the area table"),
        "{text}"
    );
    assert!(text.contains("#201: triage abstained: the route"), "{text}");
    for (args, _) in edits(&events) {
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("area:") || a.starts_with("triage:")),
            "an abstained dimension was written: {args:?}"
        );
    }
    let out = out.unwrap();
    let plan = &out.plan.issues[0];
    assert!(plan.changes.iter().all(|c| c.dimension != "area"));
}

#[test]
fn sweep_never_writes_the_ready_label_or_agent_ok_and_never_lowers_risk() {
    // `ready` is sync-ready.py's (two writers would undo each other);
    // `!agent-ok` has no rule (#195); a computed lower risk may be the
    // rule's blind spot (#196).
    let mut v = drifted_77();
    set_labels(
        &mut v,
        &[
            "area:cli",
            "kind:feature",
            "risk:high",
            "surface:cli",
            "effort:l",
            "origin:manual",
            "!agent-ok",
            "ready",
            "triage:needs-human",
        ],
    );
    let (_, events) = run(true, &[v], BTreeMap::new());
    let text = shown(&events);
    assert!(text.contains("#77: risk abstained:"), "{text}");
    assert!(text.contains("(#196)"), "{text}");
    assert!(text.contains("#77: !agent-ok abstained:"), "{text}");
    // Risk was held, so the route that follows from it is too.
    assert!(text.contains("#77: triage abstained:"), "{text}");
    let e = edits(&events);
    assert_eq!(e.len(), 1, "{events:?}");
    let (args, stdin) = e[0];
    for forbidden in ["ready", "!agent-ok", "risk:high", "risk:medium"] {
        assert!(
            !args.iter().any(|a| a == forbidden),
            "{forbidden} in {args:?}"
        );
    }
    let new = stdin.unwrap();
    assert!(new.contains("ready: false\r\r\n"));
}

#[test]
fn sweep_does_not_write_an_issue_that_changed_after_it_was_read() {
    // `--body-file` replaces the whole body, so writing a plan made from an
    // older body would undo whatever changed it (a sync-ready.py run).
    let listed = drifted_77();
    let mut now = listed.clone();
    let b = body(&now).replace("ready: false", "ready: true");
    now["body"] = Value::String(b);
    let (out, events) = run(true, &[listed], BTreeMap::from([(77, now)]));
    let out = out.unwrap();
    assert!(edits(&events).is_empty(), "{events:?}");
    assert_eq!(
        out.not_written,
        [(77, "changed since it was read".to_string())]
    );
    assert!(shown(&events).contains("#77: changed since it was read; not written"));
}

#[test]
fn sweep_refuses_a_list_that_may_have_been_cut_short() {
    // A list at `gh`'s limit may be missing issues, and a sweep over it
    // would report "no drift" for ones it never read.
    let many: Vec<Value> = (1..=SWEEP_LIST_LIMIT as u64)
        .map(|n| {
            let mut v = quiet_202();
            v["number"] = json!(n);
            v
        })
        .collect();
    let (out, events) = run(true, &many, BTreeMap::new());
    assert!(matches!(out, Err(SweepError::Truncated { .. })), "{out:?}");
    assert!(events.is_empty());
}
