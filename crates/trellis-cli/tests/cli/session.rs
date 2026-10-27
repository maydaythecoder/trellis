use super::*;

// ---------------------------------------------------------------------------
// session — the interactive surface, driven the way a user drives it
// ---------------------------------------------------------------------------

#[test]
fn session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists() {
    // The risks: `!cmd` printing "exit 0" after a command that worked, or
    // nothing after one that failed; `/prompts` or `/models` printing
    // nothing; a template's description left out of `/prompts`.
    let d = repo("session-extra");
    std::fs::create_dir_all(d.join(".trellis/prompts")).unwrap();
    std::fs::write(
        d.join(".trellis/prompts/review.md"),
        "---\ndescription: Review one module\n---\nReview $1\n",
    )
    .unwrap();
    let out = session_with(
        &d,
        "!git --version\n!git no-such-command-248\n/prompts\n/models\n/exit\n",
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("exit 1"), "{s}");
    assert!(!s.contains("exit 0"), "{s}");
    assert!(s.contains("  /review\n      Review one module\n"), "{s}");
    assert!(s.contains("configured specs\n"), "{s}");
    assert!(
        s.contains("catalog (weights `trellis model pull` can fetch)"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn every_command_help_lists_is_routed_somewhere() {
    // Each of these is one match arm, and deleting any of them leaves the
    // command falling through to "unknown command" — a command the tool
    // documents in its own `/help` and then refuses. Five of the arms had no
    // test at all.
    //
    // Asserted on the *error text* rather than on success, because several
    // of these legitimately fail in a fixture with no checkers and no trail.
    // Failing for the right reason is the property; succeeding is not.
    let d = repo("session-routing");
    for cmd in ["/help", "/?", "/index", "/check", "/doctor", "/resume"] {
        let out = session_with(&d, &format!("{cmd}\n/exit\n"));
        let s = combined(&out);
        assert!(
            !s.contains("unknown command"),
            "`{cmd}` is in /help and was not routed:\n{s}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn help_lists_the_commands_rather_than_printing_nothing() {
    // `help` is a bare `fn` whose whole body is printing, so replacing it
    // with `()` leaves `/help` returning Ok and saying nothing. A help
    // command that silently succeeds is worse than a missing one: the user
    // concludes there are no commands.
    let d = repo("session-help");
    let s = stdout(&session_with(&d, "/help\n/exit\n"));

    for expected in ["/radius", "/check", "/doctor", "/accept", "/model", "/exit"] {
        assert!(
            s.contains(expected),
            "`{expected}` missing from /help:\n{s}"
        );
    }
    assert!(
        s.contains("trellis tui"),
        "the full-screen view is a command the user cannot discover any \
         other way, so /help has to name it:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_banner_reports_the_index_and_carries_its_caveat() {
    // The coverage figure is the product's honesty mechanism and does not
    // get to be absent because the surface changed (S2). The banner is also
    // a bare printing function, so `()` leaves a session that opens to a
    // blank prompt and says nothing about what it indexed.
    let d = repo("session-banner");
    let s = stdout(&session_with(&d, "/exit\n"));

    assert!(s.contains("TRELLIS"), "got:\n{s}");
    assert!(s.contains("definitions"), "it must say what it found:\n{s}");
    assert!(
        s.contains("a lower bound on recall, not recall"),
        "the figure must carry its caveat here as it does everywhere else, \
         or it reads as recall to everyone who sees it once:\n{s}"
    );
    assert!(
        s.contains("/help"),
        "a prompt with no way in is a dead end:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn status_reports_the_state_the_session_is_holding() {
    // The point of a session is the state it keeps between tasks — the
    // index, the model, the tier. `/status` is how a user sees it, and as
    // `()` it prints nothing while still returning Ok.
    let d = repo("session-status");
    let s = stdout(&session_with(&d, "/status\n/exit\n"));

    assert!(s.contains("root"), "got:\n{s}");
    assert!(s.contains("index"), "got:\n{s}");
    assert!(
        s.contains("model    none"),
        "no model is set in this fixture and status must say so rather than \
         omit the line:\n{s}"
    );
    assert!(
        s.contains("tier"),
        "the tier decides whether a run can fail on format alone:\n{s}"
    );
    // The banner and `/status` both count the population `trellis index`
    // calls "non-excluded call sites" (A-30); "first-party" there named a
    // smaller set than the number printed beside it.
    assert_eq!(
        s.matches("non-excluded call sites").count(),
        2,
        "banner and status must name the count as the index does:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unknown_command_is_refused_rather_than_run_as_a_task() {
    // The other side of the routing table. Anything beginning with `/` that
    // is not a command must be refused, not handed to the agent loop as a
    // task — `/radiuss describe` typed at a prompt would otherwise start a
    // run against a repository.
    let d = repo("session-unknown");
    let s = combined(&session_with(&d, "/nosuchcommand\n/exit\n"));
    assert!(s.contains("unknown command"), "got:\n{s}");
    assert!(
        s.contains("/help"),
        "and it must say where the list is:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn accept_reads_its_answer_from_the_next_line_of_input() {
    // #248. The risk: the approval question reading end of input, or a
    // key it never got, so `a` typed after `/accept` cancels instead.
    let d = repo("session-accept-stdin");
    let caller = d.join("app/caller.py");
    let mut body = std::fs::read_to_string(&caller).unwrap();
    body.push_str("# edited\n");
    std::fs::write(&caller, body).unwrap();

    let s = stdout(&session_with(&d, "/accept kept\na\n/exit\n"));
    assert!(s.contains("accept> "), "{s}");
    assert!(s.contains("committed 1 path(s)"), "{s}");
    assert_eq!(git(&d, &["log", "-1", "--pretty=%s"]).trim(), "kept");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_session_keeps_its_history_outside_the_repository() {
    // #248. The risk: no history file at all, which the unit tests allow
    // for a machine with no HOME or LOCALAPPDATA.
    let d = repo("session-history");
    let data = scratch("session-history-data");
    let out = session_with_env(
        &d,
        "/status\n/exit\n",
        &[("LOCALAPPDATA", &data), ("XDG_DATA_HOME", &data)],
    );
    assert!(out.status.success(), "{}", combined(&out));
    let saved: Vec<_> = std::fs::read_dir(data.join("trellis"))
        .expect("the history directory was made")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(
        saved[0].to_string_lossy().ends_with(".history"),
        "{saved:?}"
    );
    assert!(
        std::fs::read_to_string(&saved[0])
            .unwrap()
            .contains("/status"),
        "the typed line was kept"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_runtime_that_is_not_listening_is_said_at_open_and_at_model() {
    // #248. The risk: the banner, and `/model`, silent about a runtime
    // nothing is listening on, so the first task is where the user finds
    // out. Port 1 refuses at once.
    let d = repo("session-unreachable");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[model]\nspec = \"llamacpp:http://127.0.0.1:1\"\n",
    )
    .unwrap();
    let s = stdout(&session_with(
        &d,
        "/model llamacpp:http://127.0.0.1:1\n/exit\n",
    ));
    assert!(s.contains("NOT REACHABLE"), "the banner:\n{s}");
    assert!(s.contains("not reachable yet"), "`/model`:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// How many diff lines `/diff` shows before collapsing the rest.
///
/// Mirrored rather than imported — `tests/cli.rs` is a separate crate and
/// `DIFF_LINES` is private — and asserted against the shipped behaviour by
/// the boundary test, which checks both `DIFF_LINES` and `DIFF_LINES + 1`. If
/// it moves either way, that test fails rather than silently testing nothing.
const DIFF_LINES: usize = 60;

#[test]
fn index_rebuilds_rather_than_reporting_a_cached_answer() {
    // `/index` is the command whose whole job is to refresh, which is why it
    // calls `build` and not `build_cached`. As `Ok(())` it returns success
    // having done nothing, and the session then holds a stale graph while
    // reporting that it reindexed.
    let d = repo("session-reindex");
    let s = stdout(&session_with(&d, "/index\n/exit\n"));
    assert!(
        s.contains("indexed") && s.contains("files"),
        "`/index` must say what it rebuilt:\n{s}"
    );
    assert!(
        s.matches("call site").count() >= 2,
        "the coverage summary travels with it, as it does in the banner:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_shows_the_change_and_not_only_its_filename() {
    // "Acceptance is yours" is not a real offer if taking it up means
    // leaving for another terminal. The filename says a file moved; only the
    // body says whether to accept it.
    let d = repo("session-diff");
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"HELLO {name}\"\n",
    )
    .unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(s.contains("app/core.py"), "the file must be named:\n{s}");
    assert!(
        s.contains("+    return f\"HELLO {name}\""),
        "and the change itself shown:\n{s}"
    );
    assert!(
        s.contains("/accept") && s.contains("/revert"),
        "both ways out must be offered where the change is shown:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_says_how_much_of_a_long_change_it_did_not_show() {
    // The same residue rule as the radius report and the log listing. A diff
    // that stops at sixty lines without saying so shows a user part of a
    // change and lets them accept it believing they saw all of it.
    let d = repo("session-diff-long");
    let mut body = String::from("def greet(name):\n");
    for i in 0..(DIFF_LINES * 2) {
        body.push_str(&format!("    x{i} = {i}\n"));
    }
    body.push_str("    return name\n");
    std::fs::write(d.join("app/core.py"), body).unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("more lines"),
        "a change far longer than the limit must account for the rest:\n{s}"
    );
    assert!(s.contains("git diff"), "and name how to see it:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_does_not_offer_more_when_it_showed_the_whole_change() {
    // The boundary. A two-line edit followed by "... 0 more lines" is a
    // listing claiming a remainder that does not exist.
    let d = repo("session-diff-short");
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hi {name}\"\n",
    )
    .unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        !s.contains("more lines"),
        "the whole change fitted, so there is nothing more to offer:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_on_a_clean_worktree_says_so_rather_than_printing_nothing() {
    // The other branch. Silence here is indistinguishable from a command
    // that failed, and the user's next move — accept, or run again — differs
    // between those two.
    let d = repo("session-diff-clean");
    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("nothing pending") && s.contains("clean"),
        "got:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_opens_no_diff_section_for_a_file_git_has_never_seen() {
    // An untracked file is pending and has no diff: git cannot show what it
    // has never seen, so `diff` comes back empty while `pending` names the
    // file. The guard that notices this survived as `true`, and its only
    // effect is a blank line — but the rule is real, and the case is
    // reachable by the most ordinary thing a run does, which is write a new
    // file.
    //
    // Asserted as "no section was opened" rather than on exact whitespace.
    // The window starts mid-line, so the remainder of the listing line
    // counts as one: two is the clean shape, three means an empty diff block
    // was started.
    let d = repo("session-untracked");
    std::fs::write(d.join("app/fresh.py"), "def brand_new():\n    return 1\n").unwrap();

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(s.contains("app/fresh.py"), "the file must be listed:\n{s}");

    let window = s
        .split_once("app/fresh.py")
        .and_then(|(_, rest)| rest.split_once("/accept"))
        .map(|(mid, _)| mid)
        .unwrap_or_else(|| panic!("the listing and the offer must both appear:\n{s}"));
    let blanks = window.lines().filter(|l| l.trim().is_empty()).count();
    assert_eq!(
        blanks, 2,
        "an empty diff section was opened for an untracked file; between the \
         listing and the offer there should be nothing but the line break \
         and one blank line, found {blanks}:\n{window:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn diff_offers_no_remainder_at_exactly_the_number_of_lines_it_shows() {
    // The boundary, and the only input that separates `>` from `>=` on the
    // diff listing. `>=` at exactly DIFF_LINES prints "... 0 more lines —
    // `git diff` for the rest" under a diff that already showed everything.
    //
    // The fixture converges on the length rather than hardcoding a body that
    // happens to produce it: a diff's line count includes git's own headers
    // and hunk markers, so a hand-counted fixture would be pinned to one
    // git's formatting and would quietly stop testing the boundary under
    // another. Each added source line adds exactly one line to the same
    // hunk, so one correction is enough.
    let d = repo("session-diff-boundary");
    let path = d.join("app/core.py");

    let write_body = |n: usize| {
        let mut b = String::from("def greet(name):\n");
        for i in 0..n {
            b.push_str(&format!("    x{i} = {i}\n"));
        }
        b.push_str("    return name\n");
        std::fs::write(&path, b).unwrap();
    };
    let diff_lines = || trellis_vcs::diff(&d).unwrap().lines().count();

    let probe = 40;
    write_body(probe);
    let measured = diff_lines();
    let target = probe as isize + (DIFF_LINES as isize - measured as isize);
    assert!(target >= 0, "the boundary is below an empty change");
    write_body(target as usize);

    assert_eq!(
        diff_lines(),
        DIFF_LINES,
        "this test is only meaningful at exactly the limit"
    );

    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        !s.contains("more lines"),
        "the diff was exactly the length shown, so there is no remainder:\n{s}"
    );

    // One line past it must offer a remainder, or a raised limit would leave
    // the assertion above passing at a length that is no longer the boundary.
    write_body(target as usize + 1);
    assert_eq!(diff_lines(), DIFF_LINES + 1);
    let s = stdout(&session_with(&d, "/diff\n/exit\n"));
    assert!(
        s.contains("more lines"),
        "one line past the limit leaves one unshown:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_index_percentages_partition_the_call_sites_they_describe() {
    // resolved, ambiguous and unresolved are the three outcomes a call site
    // can have — ADR 0002 keeps them three precisely so none is collapsed
    // into another — so their percentages are a partition and must come to
    // a hundred. All four mutants of the arithmetic that produces them
    // survived, and each breaks that in its own way: one makes every figure
    // a multiple of the total, one adds a hundred to each, one divides them
    // to nothing.
    //
    // Asserted as the invariant rather than against fixed numbers, so the
    // test says what the figures mean rather than what this fixture happens
    // to produce.
    let d = repo("index-percentages");
    let out = run(&d, &["index"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let pct_on = |label: &str| -> f64 {
        let line = s
            .lines()
            .find(|l| l.trim_start().starts_with(label) && l.contains('%'))
            .unwrap_or_else(|| panic!("no `{label}` line with a percentage:\n{s}"));
        let open = line.find('(').unwrap_or_else(|| panic!("got {line:?}"));
        let close = line[open..]
            .find('%')
            .map(|i| open + i)
            .unwrap_or_else(|| panic!("got {line:?}"));
        line[open + 1..close]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("`{}` is not a percentage: {e}", &line[open + 1..close]))
    };

    let parts = [
        pct_on("resolved"),
        pct_on("ambiguous"),
        pct_on("unresolved"),
    ];
    let sum: f64 = parts.iter().sum();
    assert!(
        (sum - 100.0).abs() < 0.3,
        "the three outcomes must account for every first-party call site \
         and nothing else; {parts:?} came to {sum}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
