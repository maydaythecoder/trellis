//! `trellis-tickets new` with `gh` stubbed: blockers are read from issue
//! JSON recorded from `gh issue view <n> --json number,title,body,labels,state`
//! (`tests/fixtures/triage/`, where #76 is CLOSED and #77 OPEN), and the
//! filing step is a closure that records what it was given, so a test sees
//! whether it ran, with what, and after what was shown. No test spawns `gh`
//! or reaches GitHub.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use trellis_tickets::{
    Line, NewError, NewOutcome, State, Ticket, new_with, ready_gate, report, ticket_from_gh_json,
};

fn recorded(n: u64) -> Result<String, String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/triage")
        .join(format!("issue-{n}.json"));
    std::fs::read_to_string(&p).map_err(|e| format!("no recording for #{n}: {e}"))
}

const TITLE: &str = "Add a steer counter to the status line";

/// A body every rule decides: one CLI file, a fenced `verify-tests.sh`
/// line, no decision words. The triage it gets is `auto`. `extra` is
/// appended, for a `blocked_by` line.
fn body(files: &str, extra: &str) -> String {
    format!(
        "**Source:** a test of `trellis-tickets new`\n\n\
         **Files:** {files}\n\n\
         The status line shows how many steers are queued.\n\n\
         ## Done when\n\
         - The status line prints the queued steer count.\n\n\
         ## Verify\n\
         ```\n\
         scripts/tickets/verify-tests.sh trellis-cli status_steer_count 1\n\
         ```\n{extra}"
    )
}

const CLI_FILE: &str = "crates/trellis-cli/src/tui_run.rs";

/// What happened, in order, across `show` and `create`.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    Shown(String),
    Created { args: Vec<String>, stdin: String },
}

/// `new` over the recordings, with a create step that records its call and
/// returns a URL. Returns the outcome and every event.
fn run(title: &str, body: &str, dry_run: bool) -> (Result<NewOutcome, NewError>, Vec<Event>) {
    let events = RefCell::new(Vec::new());
    let out = new_with(
        title,
        body,
        dry_run,
        recorded,
        |s| events.borrow_mut().push(Event::Shown(s.to_string())),
        |args, stdin| {
            events.borrow_mut().push(Event::Created {
                args: args.to_vec(),
                stdin: stdin.to_string(),
            });
            Ok("https://github.com/o/r/issues/1\n".to_string())
        },
    );
    (out, events.into_inner())
}

fn refusals(r: Result<NewOutcome, NewError>) -> Vec<String> {
    match r {
        Err(NewError::Refused { failures }) => failures,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn shown(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Shown(s) => Some(s.as_str()),
            Event::Created { .. } => None,
        })
        .collect()
}

#[test]
fn new_refuses_a_body_without_done_when_or_a_fenced_verify_and_files_nothing() {
    // G0 would refuse the filed issue forever; filing it anyway puts a
    // ticket in the queue no agent may start.
    let no_done = body(CLI_FILE, "").replace("## Done when", "## Outcome");
    let (r, events) = run(TITLE, &no_done, false);
    assert_eq!(refusals(r), ["no '## Done when' section"]);
    assert!(events.is_empty(), "{events:?}");

    let unfenced = body(CLI_FILE, "").replace("```\n", "");
    let (r, events) = run(TITLE, &unfenced, false);
    assert_eq!(refusals(r), ["no fenced command under '## Verify'"]);
    assert!(events.is_empty(), "{events:?}");

    // G6's `[ -n "$cmd" ]`: a fence with nothing in it is no command.
    let empty = body(CLI_FILE, "").replace(
        "scripts/tickets/verify-tests.sh trellis-cli status_steer_count 1\n",
        "",
    );
    let (r, _) = run(TITLE, &empty, false);
    assert_eq!(refusals(r), ["no fenced command under '## Verify'"]);

    let (r, events) = run("  ", &body(CLI_FILE, ""), false);
    assert_eq!(refusals(r), ["the title is empty"]);
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn new_refuses_every_verify_line_g6_would_not_run_and_files_nothing() {
    // G6 runs these lines with `bash -c`; a line it refuses makes the
    // ticket's own Verify unrunnable, and one it would not refuse must not
    // be refused here either.
    let verify = "scripts/tickets/verify-tests.sh trellis-cli status_steer_count 1\n";
    let lines = "# a comment G6 skips\n\
                 bash scripts/tickets/verify-tests.sh x y\n\
                 cargo test -p x | tail -5\n\
                 scripts/../target/evil\n\
                 git status\n";
    let (r, events) = run(TITLE, &body(CLI_FILE, "").replace(verify, lines), false);
    assert_eq!(
        refusals(r),
        [
            "Verify line not allowed (cargo, git, scripts/, py -3 qa/, python3 qa/ only): \
             bash scripts/tickets/verify-tests.sh x y",
            "Verify line uses shell composition; put it in a script under scripts/: \
             cargo test -p x | tail -5",
            "Verify line contains '..': scripts/../target/evil",
        ]
    );
    assert!(events.is_empty(), "{events:?}");

    // G6 does not strip `\r`, so a CRLF Verify line would run with it on
    // its last argument.
    let crlf = body(CLI_FILE, "").replace('\n', "\r\n");
    let (r, events) = run(TITLE, &crlf, false);
    let failures = refusals(r);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("carriage returns"), "{failures:?}");
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn new_applies_exactly_the_labels_triage_computes() {
    // The labels are the rule engine's, not a template's: they must equal
    // what `triage` would show for the same issue, recorded as `new`'s
    // origin.
    let b = body(CLI_FILE, "");
    let (r, _) = run(TITLE, &b, false);
    let plan = r.unwrap().plan;
    assert_eq!(
        plan.labels,
        [
            "area:cli",
            "kind:feature",
            "risk:medium",
            "surface:cli",
            "effort:s",
            "origin:manual",
            "triage:auto",
        ]
    );
    let t = Ticket {
        title: TITLE.to_string(),
        body: b,
        labels: vec!["origin:manual".to_string()],
        state: State::Open,
    };
    let triaged = report(0, &t, &BTreeMap::new());
    let lines: Vec<(&str, Line)> = triaged
        .lines()
        .into_iter()
        .map(|(n, l)| (n, l.clone()))
        .collect();
    assert_eq!(plan.lines, lines);
    assert!(plan.abstained().is_empty());
}

#[test]
fn new_reports_each_abstention_and_applies_no_label_for_it() {
    // crates/trellis-loop is in no area table (#194) and its src/ could be
    // a public API: area and surface abstain. A label chosen anyway would
    // be the authoritative-looking guess the standard forbids (§1).
    let (r, events) = run(TITLE, &body("crates/trellis-loop/src/run.rs", ""), true);
    let plan = r.unwrap().plan;
    assert!(
        !plan.labels.iter().any(|l| l.starts_with("area:")),
        "{:?}",
        plan.labels
    );
    assert!(!plan.labels.iter().any(|l| l.starts_with("surface:")));
    assert!(plan.abstained().contains(&"area"));
    assert!(plan.abstained().contains(&"surface"));
    // What follows from an abstention is applied as computed.
    assert!(plan.labels.contains(&"triage:needs-human"));
    let text = shown(&events);
    assert!(
        text.lines().any(|l| l.starts_with(
            "area    abstained: `crates/trellis-loop/src/run.rs` is under no path in the area table"
        )),
        "{text}"
    );
    assert!(text.contains("\nabstained: area, "), "{text}");
}

#[test]
fn new_dry_run_prints_the_gh_create_argv_and_never_files() {
    let (r, events) = run(TITLE, &body(CLI_FILE, ""), true);
    let outcome = r.unwrap();
    assert_eq!(outcome.created, None);
    assert!(
        events.iter().all(|e| matches!(e, Event::Shown(_))),
        "a dry run called create: {events:?}"
    );
    assert_eq!(
        outcome.plan.gh_create_args(),
        [
            "issue",
            "create",
            "--title",
            TITLE,
            "--body-file",
            "-",
            "--label",
            "area:cli",
            "--label",
            "kind:feature",
            "--label",
            "risk:medium",
            "--label",
            "surface:cli",
            "--label",
            "effort:s",
            "--label",
            "origin:manual",
            "--label",
            "triage:auto",
        ]
    );
    let text = shown(&events);
    assert!(
        text.contains(
            "gh issue create --title 'Add a steer counter to the status line' --body-file - \
             --label area:cli --label kind:feature --label risk:medium --label surface:cli \
             --label effort:s --label origin:manual --label triage:auto\n"
        ),
        "{text}"
    );
    assert!(text.contains("  | ## Done when\n"), "{text}");
    assert!(text.ends_with("dry run: nothing filed\n"), "{text}");
}

#[test]
fn new_prints_the_plan_before_it_files_and_files_the_checked_body_once() {
    // The plan is shown before anything is written to GitHub, and the body
    // filed is the one checked, on stdin, not a path re-read later.
    let b = body(CLI_FILE, "");
    let (r, events) = run(TITLE, &b, false);
    let outcome = r.unwrap();
    assert_eq!(
        outcome.created.as_deref(),
        Some("https://github.com/o/r/issues/1\n")
    );
    assert_eq!(events.len(), 2, "{events:?}");
    let Event::Shown(plan_text) = &events[0] else {
        panic!("create ran before the plan was shown: {events:?}");
    };
    assert_eq!(plan_text, &outcome.plan.text());
    assert_eq!(
        events[1],
        Event::Created {
            args: outcome.plan.gh_create_args(),
            stdin: b,
        }
    );
}

#[test]
fn new_reads_blockers_through_the_fetch_and_labels_an_open_one_blocked() {
    let (r, _) = run(TITLE, &body(CLI_FILE, "\nblocked_by: [77]\n"), true);
    let labels = r.unwrap().plan.labels;
    assert_eq!(labels.last(), Some(&"triage:blocked"), "{labels:?}");

    let (r, _) = run(TITLE, &body(CLI_FILE, "\nblocked_by: [76]\n"), true);
    let labels = r.unwrap().plan.labels;
    assert_eq!(labels.last(), Some(&"triage:auto"), "{labels:?}");

    // A blocker that cannot be read stops `new` before anything is filed:
    // an unknown dependency is not a satisfied one.
    let (r, events) = run(TITLE, &body(CLI_FILE, "\nblocked_by: [99999]\n"), false);
    assert!(matches!(r, Err(NewError::Triage(_))), "{r:?}");
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn new_files_a_body_and_labels_g0_accepts() {
    // What `new` files as `triage:auto` must be workable by G0's reading
    // (`ready` mirrors `scripts/gates/ticket-ready.sh`), or the auto route
    // is a label on a ticket no agent may start.
    let b = body(CLI_FILE, "\nblocked_by: [76]\n");
    let (r, _) = run(TITLE, &b, true);
    let plan = r.unwrap().plan;
    let filed = Ticket {
        title: plan.title.clone(),
        body: plan.body.clone(),
        labels: plan.labels.iter().map(|l| l.to_string()).collect(),
        state: State::Open,
    };
    let blocker = ticket_from_gh_json(76, &recorded(76).unwrap()).unwrap();
    let blockers = BTreeMap::from([(76, Ok(blocker.state))]);
    let gate = ready_gate(1, &filed, &blockers);
    assert!(gate.g0.is_empty(), "{:?}", gate.g0);
    assert!(gate.is_ready(), "{gate:?}");
}
