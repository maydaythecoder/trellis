//! Every `gh` call the crate builds is a REST `gh api` call (#957), and the
//! REST JSON it gets back reads as the same issues. `gh issue` and `gh pr`
//! go through GraphQL, which a Claude Code cloud session refuses, so an
//! argv built on them cannot file or read a ticket there. The REST shapes
//! are built at run time from the recordings in `tests/fixtures/triage/`;
//! no test spawns `gh` or reaches GitHub.

use std::path::PathBuf;

use serde_json::{Value, json};
use trellis_tickets::{
    State, SweepError, gh_list_args, gh_view_args, plan_new, plan_sweep, ticket_from_gh_json,
};

fn recorded(n: u64) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/triage")
        .join(format!("issue-{n}.json"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("no recording for #{n}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// #`n` as the REST endpoint prints it: the state in lower case, and
/// fields the GraphQL recording does not have.
fn rest(n: u64) -> Value {
    let mut v = recorded(n);
    let state = v["state"].as_str().unwrap().to_lowercase();
    v["state"] = json!(state);
    v["html_url"] = json!(format!("https://github.com/o/r/issues/{n}"));
    v["comments"] = json!(0);
    v
}

/// A pull request as the REST issue list holds it: an issue with a
/// `pull_request` key.
fn pull_request(n: u64) -> Value {
    let mut v = rest(202);
    v["number"] = json!(n);
    v["pull_request"] = json!({ "url": format!("https://api.github.com/repos/o/r/pulls/{n}") });
    v
}

const BODY: &str = "**Files:** crates/trellis-cli/src/tui_run.rs\n\n\
    ## Done when\n- it holds\n\n## Verify\n```\n\
    scripts/tickets/verify-tests.sh trellis-cli status_steer_count 1\n```\n";

#[test]
fn gh_argv_never_names_a_graphql_subcommand() {
    // A `gh issue ...` or `gh pr ...` argv is a GraphQL call; the cloud
    // session refuses it with HTTP 403 and the command cannot run there.
    let new = plan_new("Add a steer counter", BODY, |n| {
        Err(format!("#{n} is not read"))
    })
    .unwrap();
    let mut listed = rest(202);
    listed["labels"] = json!([]);
    let list = serde_json::to_string(&[listed]).unwrap();
    let sweep = plan_sweep(&list, |n| Err(format!("#{n} is not read")), "now").unwrap();
    let edit = &sweep.issues[0];
    assert!(edit.writes(), "the unlabelled issue has no edit: {edit:?}");

    let argvs: Vec<(&str, Vec<String>)> = vec![
        ("view", gh_view_args(77).to_vec()),
        ("list", gh_list_args()),
        ("create", new.gh_create_args()),
        ("edit", edit.gh_edit_args()),
    ];
    for (name, argv) in &argvs {
        assert_eq!(argv[0], "api", "{name}: {argv:?}");
        assert!(
            !argv.iter().any(|a| a == "issue" || a == "pr"),
            "{name} names a GraphQL subcommand: {argv:?}"
        );
        assert!(
            argv.iter()
                .any(|a| a.starts_with("repos/{owner}/{repo}/issues")),
            "{name} has no REST issues path: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "graphql"),
            "{name} is a GraphQL call: {argv:?}"
        );
    }
    // Without `--paginate` the list endpoint returns its first 30 issues,
    // and a sweep would say "no drift" for every issue past them.
    assert!(gh_list_args().iter().any(|a| a == "--paginate"));
    // The body goes on stdin, and labels as gh's array fields.
    let create = new.gh_create_args();
    assert!(create.iter().any(|a| a == "body=@-"), "{create:?}");
    let patch = edit.gh_edit_args();
    assert_eq!(
        patch.iter().any(|a| a == "body=@-"),
        edit.new_body.is_some(),
        "{patch:?}"
    );
    for argv in [create, patch] {
        assert!(argv.iter().any(|a| a.starts_with("labels[]=")), "{argv:?}");
    }
}

#[test]
fn a_rest_list_is_read_across_pages_and_its_pull_requests_are_skipped() {
    // `gh api --help` says each `--paginate` page is a separate array. Reading
    // only the first would drop issues; reading a pull request as an issue
    // would triage and label it.
    let mut second = rest(202);
    second["number"] = json!(9202);
    // REST sends `null` for an issue with no body.
    second["body"] = Value::Null;
    let page1 = serde_json::to_string(&[rest(202), pull_request(9300)]).unwrap();
    let page2 = serde_json::to_string(&[pull_request(9301), second]).unwrap();
    let list = format!("{page1}{page2}\n");
    let plan = plan_sweep(&list, |n| Err(format!("#{n} is not read")), "now").unwrap();
    let numbers: Vec<u64> = plan.issues.iter().map(|i| i.number).collect();
    assert_eq!(numbers, [202, 9202]);
}

#[test]
fn an_empty_list_output_is_refused_not_read_as_no_open_issues() {
    // `gh` prints `[]` when nothing is open. Nothing at all is a broken
    // read, and a sweep over it would report every issue as clean.
    assert!(matches!(
        plan_sweep("", |_| Err(String::new()), "now"),
        Err(SweepError::ListJson(_))
    ));
    assert!(plan_sweep("[]", |_| Err(String::new()), "now").is_ok());
}

#[test]
fn a_rest_state_in_lower_case_reads_as_open_or_closed() {
    // REST spells `open` and `closed`; compared as written, every blocker
    // would be an unknown state and every ticket would abstain.
    let closed = serde_json::to_string(&rest(75)).unwrap();
    assert_eq!(
        ticket_from_gh_json(75, &closed).unwrap().state,
        State::Closed
    );
    let mut open = rest(202);
    open["body"] = Value::Null;
    let t = ticket_from_gh_json(202, &serde_json::to_string(&open).unwrap()).unwrap();
    assert_eq!(t.state, State::Open);
    assert_eq!(t.body, "");
    let mut odd = rest(202);
    odd["state"] = json!("merged");
    assert!(ticket_from_gh_json(202, &serde_json::to_string(&odd).unwrap()).is_err());
}
