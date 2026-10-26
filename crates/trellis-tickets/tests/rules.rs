//! The rules against `docs/TICKETING.md`, one ticket body at a time.
//!
//! Every `abstains_*` test names the input that makes its dimension abstain.
//! The rest pin what a rule must decide, and the risk if it did not.

use std::collections::BTreeMap;

use trellis_tickets::{
    Effort, Flag, Kind, Origin, Outcome, Risk, Route, State, Surface, TRIAGE_VERSION, Ticket, area,
    effort, flags, kind, origin, ready, risk, surface, triage,
};

fn ticket(title: &str, body: &str, labels: &[&str]) -> Ticket {
    Ticket {
        title: title.to_string(),
        body: body.to_string(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        state: State::Open,
    }
}

/// A body G0 would accept, with the given `**Files:**` line.
fn body(files: &str) -> String {
    format!(
        "**Source:** test\n\n{files}\n\n## Done when\n- the thing holds\n\n## Verify\n```\nscripts/tickets/verify-tests.sh trellis-x thing_ 1\n```\n"
    )
}

fn no_blockers() -> BTreeMap<u64, State> {
    BTreeMap::new()
}

fn reason<T: std::fmt::Debug>(o: &Outcome<T>) -> &str {
    match o {
        Outcome::Abstained { reason, .. } => reason,
        Outcome::Decided { .. } => panic!("expected an abstention, got {o:?}"),
    }
}

fn value<T: std::fmt::Debug>(o: &Outcome<T>) -> &T {
    o.decided()
        .unwrap_or_else(|| panic!("expected a decision, got {o:?}"))
}

// ---- one abstention per dimension -------------------------------------

#[test]
fn abstains_area_when_the_body_has_no_files_line() {
    let t = ticket("Add a thing", "No files named here.", &["origin:manual"]);
    assert!(reason(&area(&t)).contains("no `**Files:**` line"));
}

#[test]
fn abstains_area_when_files_span_three_areas() {
    // Spanning areas usually means the ticket is really a story (§5).
    let t = ticket(
        "Add a thing",
        &body(
            "**Files:** crates/trellis-model/src/a.rs; crates/trellis-cli/src/main.rs; docs/X.md",
        ),
        &[],
    );
    assert!(reason(&area(&t)).contains("3 areas"));
}

#[test]
fn abstains_area_when_a_file_is_under_no_path_in_the_table() {
    // fuzz/ has no row in the table (trellis-index did until #194 mapped
    // it); mapping an unlisted path would be a guess.
    let t = ticket(
        "Add a thing",
        &body("**Files:** fuzz/fuzz_targets/http_response.rs"),
        &[],
    );
    assert!(reason(&area(&t)).contains("fuzz/fuzz_targets/http_response.rs"));
}

#[test]
fn the_owners_mapping_rows_map_onto_the_twelve_areas_194() {
    // #194: the repository's other crates and directories map onto the
    // existing area values, one row each, rather than adding new ones.
    // A row that went missing would send every ticket touching it back to
    // a person, silently.
    for (file, want) in [
        ("crates/trellis-index/src/resolve.rs", "area:graph"),
        ("crates/trellis-loop/src/agent.rs", "area:runtime"),
        ("crates/trellis-vcs/src/lib.rs", "area:runtime"),
        ("crates/trellis-vault/src/lib.rs", "area:context"),
        ("crates/trellis-queue/src/lib.rs", "area:agents"),
        ("crates/trellis-trail/src/lib.rs", "area:protocol"),
        ("crates/trellis-core/src/lib.rs", "area:build"),
        ("scripts/gates/run-all.sh", "area:build"),
        (".github/workflows/ci.yml", "area:build"),
    ] {
        let t = ticket("Add a thing", &body(&format!("**Files:** {file}")), &[]);
        assert_eq!(value(&area(&t)).label(), want, "{file}");
    }
}

#[test]
fn abstains_kind_when_the_title_is_empty() {
    let t = ticket("  ", &body("**Files:** docs/X.md"), &[]);
    assert!(reason(&kind(&t)).contains("title is empty"));
}

#[test]
fn abstains_kind_when_the_files_line_is_unreadable_and_the_title_names_no_bug() {
    // Without the files, "only *.md touched" cannot be ruled out, so
    // `feature` would be a guess.
    let t = ticket("Add a thing", "**Files:** no crate files", &[]);
    assert!(matches!(kind(&t), Outcome::Abstained { .. }));
}

#[test]
fn abstains_surface_when_a_library_crate_src_file_needs_the_diff() {
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-model/src/tier.rs"),
        &[],
    );
    let o = surface(&t);
    assert!(reason(&o).contains("needs the diff"), "{o:?}");
}

#[test]
fn abstains_risk_when_surface_is_unknown_and_nothing_raises_it() {
    // Defaulting to low here would under-rate a change to a pub signature.
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-model/src/tier.rs"),
        &[],
    );
    assert!(reason(&risk(&t)).contains("surface abstained"));
}

#[test]
fn abstains_risk_when_a_manifest_could_lift_the_unsafe_forbid() {
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-cli/Cargo.toml"),
        &[],
    );
    assert!(reason(&risk(&t)).contains("unsafe"));
}

#[test]
fn abstains_effort_when_a_files_entry_is_a_directory() {
    let t = ticket(
        "Add a thing",
        &body("**Files:** docs/bench/ (new run)"),
        &[],
    );
    assert!(reason(&effort(&t)).contains("docs/bench"));
}

#[test]
fn abstains_origin_when_no_origin_label_is_recorded() {
    let t = ticket("Add a thing", &body("**Files:** docs/X.md"), &[]);
    assert!(reason(&origin(&t)).contains("no `origin:` label"));
}

#[test]
fn abstains_origin_when_two_origin_labels_are_recorded() {
    let t = ticket(
        "Add a thing",
        &body("**Files:** docs/X.md"),
        &["origin:manual", "origin:qa"],
    );
    assert!(matches!(origin(&t), Outcome::Abstained { .. }));
}

#[test]
fn abstains_flags_when_a_lockfile_may_carry_a_dependency_change() {
    // `!needs-adr` is set by a dependency change (§5), and the files name
    // the lockfile without saying whether one happens. Six files make risk
    // medium on its own, so the abstention is the dependency's and not an
    // upstream one passing through.
    let t = ticket(
        "Add a thing",
        &body("**Files:** docs/a.md; docs/b.md; docs/c.md; docs/d.md; docs/e.md; Cargo.lock"),
        &["origin:manual"],
    );
    assert_eq!(*value(&risk(&t)), Risk::Medium);
    let o = flags(&t);
    assert!(reason(&o).contains("dependency"), "{o:?}");
}

#[test]
fn abstains_ready_when_a_blockers_state_was_not_supplied() {
    // An unknown dependency is not a satisfied one.
    let t = ticket(
        "Add a thing",
        &format!("{}\nblocked_by: [75]\n", body("**Files:** docs/X.md")),
        &["origin:manual"],
    );
    assert!(reason(&ready(&t, &no_blockers())).contains("#75"));
}

#[test]
fn abstains_triage_when_a_blocked_by_entry_is_not_an_issue_number() {
    // Malformed intake is a hard fail, not a guess (§4).
    let t = ticket(
        "Add a thing",
        &format!(
            "{}\nblocked_by: [75, child-a]\n",
            body("**Files:** docs/X.md")
        ),
        &["origin:manual"],
    );
    let o = triage(&t, &BTreeMap::from([(75, State::Closed)]));
    assert!(reason(&o).contains("child-a"), "{o:?}");
}

// ---- what the rules must decide ----------------------------------------

#[test]
fn triage_version_is_the_one_on_the_issues() {
    assert_eq!(TRIAGE_VERSION, 2);
}

#[test]
fn a_clean_docs_ticket_reaches_auto() {
    // If `auto` were unreachable the pipeline would route everything to a
    // person and carry no information.
    let t = ticket(
        "Document the thing",
        &body("**Files:** docs/X.md"),
        &["origin:manual"],
    );
    let o = triage(&t, &no_blockers());
    assert_eq!(value(&o).route, Route::Auto, "{o:?}");
    assert_eq!(value(&area(&t)).label(), "area:docs");
    assert_eq!(*value(&kind(&t)), Kind::Docs);
    assert_eq!(*value(&surface(&t)), Surface::Internal);
    assert_eq!(*value(&risk(&t)), Risk::Low);
    assert_eq!(*value(&effort(&t)), Effort::S);
    assert_eq!(*value(&origin(&t)), Origin::Manual);
}

#[test]
fn agent_ok_is_decided_absent_because_rule_2_has_no_check() {
    // The flag releases unsupervised execution. It asserts all six of §5's
    // conditions were shown, and rule 2 cannot be; absence is decided, and
    // the rule string says why rather than leaving it to look accidental.
    let t = ticket(
        "Document the thing",
        &body("**Files:** docs/X.md"),
        &["origin:manual", "!agent-ok"],
    );
    let o = flags(&t);
    assert!(!value(&o).contains(&Flag::AgentOk));
    assert!(o.rule().contains("rule 2"));
}

#[test]
fn a_checks_file_is_risk_high_even_when_area_abstains() {
    // The Verdict seal is high risk "always" (§5). An unmapped second file
    // makes area abstain; that must not hide the checks file.
    let t = ticket(
        "Add a thing",
        &body("**Files:** fuzz/fuzz_targets/a.rs; crates/trellis-checks/src/lib.rs"),
        &["origin:manual"],
    );
    assert!(matches!(area(&t), Outcome::Abstained { .. }));
    assert_eq!(*value(&risk(&t)), Risk::High);
    assert_eq!(value(&triage(&t, &no_blockers())).route, Route::NeedsHuman);
}

#[test]
fn a_security_label_makes_risk_high_without_any_files() {
    let t = ticket("Add a thing", "no files", &["!security"]);
    assert_eq!(*value(&risk(&t)), Risk::High);
}

#[test]
fn high_risk_that_is_not_a_bug_needs_an_adr() {
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-checks/tests/x_test.rs"),
        &["origin:manual"],
    );
    assert_eq!(*value(&kind(&t)), Kind::Test);
    assert!(value(&flags(&t)).contains(&Flag::NeedsAdr));
    let r = ready(&t, &no_blockers());
    assert!(value(&r).failures.contains(&"needs ADR".to_string()));
}

#[test]
fn kind_follows_the_table_order() {
    let fix = ticket("Fix the thing", &body("**Files:** docs/X.md"), &[]);
    assert_eq!(*value(&kind(&fix)), Kind::Bug);
    let rename = ticket(
        "Rename the thing",
        &body("**Files:** crates/trellis-cli/src/main.rs"),
        &[],
    );
    assert_eq!(*value(&kind(&rename)), Kind::Refactor);
    let ci = ticket(
        "Run it in CI",
        &body("**Files:** .github/workflows/ci.yml"),
        &[],
    );
    assert_eq!(*value(&kind(&ci)), Kind::Chore);
}

#[test]
fn a_command_definition_is_surface_cli_and_risk_medium() {
    let t = ticket(
        "Add a flag",
        &body("**Files:** crates/trellis-cli/src/main.rs"),
        &["origin:manual"],
    );
    assert_eq!(*value(&surface(&t)), Surface::Cli);
    assert_eq!(*value(&risk(&t)), Risk::Medium);
}

#[test]
fn effort_counts_new_tests_from_the_verify_block() {
    let one_file_many_tests = ticket(
        "Add a thing",
        "**Files:** docs/X.md\n\n## Verify\n```\nscripts/tickets/verify-tests.sh trellis-x a_ 9\n```\n",
        &[],
    );
    assert_eq!(*value(&effort(&one_file_many_tests)), Effort::M);
    let six = ticket(
        "Add a thing",
        &body("**Files:** docs/a.md; docs/b.md; docs/c.md; docs/d.md; docs/e.md; docs/f.md"),
        &[],
    );
    assert_eq!(*value(&effort(&six)), Effort::L);
}

#[test]
fn ready_names_every_failure() {
    // "Not ready" with no reason is useless (§5).
    let mut t = ticket(
        "Design the thing",
        "**Files:** docs/a.md; docs/b.md; docs/c.md; docs/d.md; docs/e.md; docs/f.md\n\nblocked_by: [75]\n",
        &["origin:manual"],
    );
    t.state = State::Closed;
    let r = ready(&t, &BTreeMap::from([(75, State::Open)]));
    assert_eq!(
        value(&r).failures,
        [
            "issue is CLOSED",
            "no '## Done when' section",
            "no verify command",
            "decision words: design",
            "effort:l",
            "blocked by #75 (OPEN)",
        ]
    );
}

#[test]
fn decision_words_inside_the_decision_section_do_not_fail_ready() {
    // Every released issue records "Decided ... by the owner" under
    // `## Decision`; reading that as a pending decision would fail them all.
    let t = ticket(
        "Document the thing",
        &format!(
            "## Decision\nYes. Decided 2026-09-28 by the owner.\n\n{}",
            body("**Files:** docs/X.md")
        ),
        &["origin:manual"],
    );
    assert!(value(&ready(&t, &no_blockers())).is_ready());
}

#[test]
fn an_open_blocker_routes_blocked_and_a_closed_one_does_not() {
    let t = ticket(
        "Document the thing",
        &format!("{}\nblocked_by: [75]\n", body("**Files:** docs/X.md")),
        &["origin:manual"],
    );
    let open = triage(&t, &BTreeMap::from([(75, State::Open)]));
    assert_eq!(value(&open).route, Route::Blocked);
    let closed = triage(&t, &BTreeMap::from([(75, State::Closed)]));
    assert_eq!(value(&closed).route, Route::Auto);
}

#[test]
fn an_agents_ticket_goes_to_a_person() {
    // An agent adding work to the queue unsupervised fills it with
    // plausible noise (§2).
    let t = ticket(
        "Document the thing",
        &body("**Files:** docs/X.md"),
        &["origin:agent"],
    );
    assert_eq!(value(&triage(&t, &no_blockers())).route, Route::NeedsHuman);
}

#[test]
fn a_ticket_that_is_not_ready_and_not_blocked_goes_to_a_person_with_its_failures() {
    let t = ticket(
        "Document the thing",
        "**Files:** docs/X.md\n\n## Done when\n- it holds\n",
        &["origin:manual"],
    );
    let o = triage(&t, &no_blockers());
    assert_eq!(value(&o).route, Route::NeedsHuman);
    assert_eq!(value(&o).reasons, ["not ready: no verify command"]);
}

#[test]
fn rollback_worktree_teardown_and_session_persistence_are_risk_high_196() {
    // The owner's path table (#196). Each entry would otherwise compute
    // low or medium, and a change to rollback is the one the clean-tree
    // guarantee (ADR 0009) rests on.
    for file in [
        "crates/trellis-vcs/src/lib.rs",
        "crates/trellis-trail/src/lib.rs",
        "crates/trellis-loop/src/integrate.rs",
        "crates/trellis-loop/src/partition.rs",
        "crates/trellis-loop/src/supervise.rs",
        "crates/trellis-cli/src/session.rs",
        "crates/trellis-cli/src/session_extra.rs",
    ] {
        let t = ticket(
            "Add a thing",
            &body(&format!("**Files:** {file}")),
            &["origin:manual"],
        );
        assert_eq!(*value(&risk(&t)), Risk::High, "{file}");
    }
}

#[test]
fn a_directory_that_contains_a_persistence_path_is_risk_high_196() {
    // A directory entry names an unknown set of files; the safe reading is
    // that it includes the persistence file inside it.
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-loop/src/"),
        &["origin:manual"],
    );
    assert_eq!(*value(&risk(&t)), Risk::High);
}

#[test]
fn a_loop_file_outside_the_table_is_not_raised_by_it_196() {
    // The table is a list, not the whole crate: agent.rs is not rollback.
    let t = ticket(
        "Add a thing",
        &body("**Files:** crates/trellis-loop/src/agent.rs"),
        &["origin:manual"],
    );
    assert_ne!(risk(&t).decided().copied(), Some(Risk::High));
}

#[test]
fn the_desktop_workspace_is_an_interface_area() {
    // #249: every desktop ticket names files under desktop/, and an
    // unmapped path would route each of them to a person.
    let t = ticket(
        "Add a thing",
        &body("**Files:** desktop/src/main.rs"),
        &["origin:manual"],
    );
    assert_eq!(value(&area(&t)).label(), "area:tui");
}

#[test]
fn area_of_root_claude_md_is_docs() {
    // #282: tickets name the root CLAUDE.md, and an unmapped path routes
    // each to a person. The row matches the root file only: desktop's own
    // CLAUDE.md is under `desktop` and must stay an interface ticket.
    for (file, want) in [
        ("CLAUDE.md", "area:docs"),
        ("desktop/CLAUDE.md", "area:tui"),
    ] {
        let t = ticket("Add a thing", &body(&format!("**Files:** {file}")), &[]);
        assert_eq!(value(&area(&t)).label(), want, "{file}");
    }
}

#[test]
fn area_of_the_tickets_crate_is_build_and_the_go_tui_is_tui() {
    // #282: #194's mapping left the tickets crate out, so every ticket
    // touching it went to a person. It is repository tooling, as scripts/
    // is; tui/ is an interface, as the desktop is (#777: the ratatui crate
    // this once asserted is gone, #516).
    for (file, want) in [
        ("crates/trellis-tickets/src/rules.rs", "area:build"),
        ("tui/internal/model/model.go", "area:tui"),
    ] {
        let t = ticket("Add a thing", &body(&format!("**Files:** {file}")), &[]);
        assert_eq!(value(&area(&t)).label(), want, "{file}");
    }
}

#[test]
fn a_decision_word_that_is_a_path_component_does_not_fail_ready() {
    // docs/design/ is a directory of this repository, and DESIGN.md a file
    // in it; a ticket that edits them names them on its Files line and in
    // prose. Reading the path as "design" would route every such ticket to
    // a person, so a word that touches a `/` or carries a file extension is
    // a path, not a decision.
    let t = ticket(
        "Record the token meanings",
        &body(
            "**Files:** docs/design/DESIGN.md; docs/design/events.md\n\n\
             Add the table to `docs/design/DESIGN.md` and DESIGN.md's index line.",
        ),
        &["origin:manual"],
    );
    assert!(value(&ready(&t, &no_blockers())).is_ready());
}

#[test]
fn a_decision_word_in_prose_beside_a_path_still_fails_ready() {
    // The exemption is for the path itself, not the sentence around it.
    let t = ticket(
        "Record the token meanings",
        &body("**Files:** docs/design/DESIGN.md\n\nDesign the table, then add it."),
        &["origin:manual"],
    );
    assert!(!value(&ready(&t, &no_blockers())).is_ready());
}

#[test]
fn a_decision_word_joined_to_prose_by_a_slash_still_fails_ready() {
    // A bare `/` is not a path: "Decide/consider" leaves two decisions open,
    // and reading either as a path component would mark the ticket ready
    // when §1 says to fail to a person. A path needs two separators or a
    // file extension beside its one.
    let t = ticket(
        "Cache scope",
        &body(
            "**Files:** docs/plans/cache.md\n\nDecide/consider whether the cache is per-run or global.",
        ),
        &["origin:manual"],
    );
    assert!(!value(&ready(&t, &no_blockers())).is_ready());
}

#[test]
fn a_decision_word_that_ends_a_sentence_still_fails_ready() {
    // The file-extension rule looks for `.` followed by a letter or digit,
    // so a full stop at the end of a sentence or line is not an extension.
    for tail in [
        "We must decide.",
        "We must decide.\nThen build it.",
        "We must decide...",
    ] {
        let t = ticket(
            "Cache scope",
            &body(&format!("**Files:** docs/plans/cache.md\n\n{tail}")),
            &["origin:manual"],
        );
        assert!(!value(&ready(&t, &no_blockers())).is_ready(), "{tail:?}");
    }
}

#[test]
fn a_decision_word_inside_a_hyphenated_file_name_does_not_fail_ready() {
    // `cache-design-notes.md` is one file name; the word between its hyphens
    // is not a decision the work has to make. The path is judged as a whole
    // token, so the hyphens do not expose it.
    let t = ticket(
        "Move the cache notes",
        &body("**Files:** docs/plans/cache-design-notes.md\n\nMove the file beside its ADR."),
        &["origin:manual"],
    );
    assert!(value(&ready(&t, &no_blockers())).is_ready());
}
