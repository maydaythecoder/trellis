use super::*;

// ---------------------------------------------------------------------------
// radius
// ---------------------------------------------------------------------------

#[test]
fn radius_ships_reach_and_doubt_as_two_facts() {
    // ADR 0002 and ADR 0014: every blast radius ships with its coverage, as
    // two facts and never a ratio. `RadiusCoverage` has no `total()` and no
    // fraction because the reach counts and the doubt count come from
    // different populations — dividing them once printed "3.7% resolved" for
    // a resolver that hand-audits at 72%.
    let d = repo("radius");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(s.contains("greet"), "it must name the symbol: {s}");
    assert!(
        s.contains("call site") || s.contains("caller"),
        "it must report reach: {s}"
    );
    // `repo()`'s `thing.run()` is the doubt.
    assert!(s.contains("bound to nothing"), "it must report doubt: {s}");
    assert!(!s.contains('%'), "and never a ratio of the two: {s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_on_an_unknown_symbol_fails_and_says_what_it_searched() {
    // A silent empty radius is indistinguishable from a symbol with no
    // callers, and the two mean opposite things for whether an edit is safe.
    let d = repo("radius-missing");
    let out = run(&d, &["radius", "app.core.no_such_function"]);
    assert!(!out.status.success(), "it must fail: {}", combined(&out));
    let s = combined(&out);
    assert!(
        s.contains("no_such_function"),
        "the error must name what was asked for: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// radius — how much of the doubt reaches the screen
// ---------------------------------------------------------------------------

/// MethodDispatchUntyped, LocalBinding, UnboundName, DynamicAttribute —
/// four, so that three shown leaves a residue of exactly one.
const CALLER_FOUR_REASONS: &str = concat!(
    "from app.core import greet\n\n",
    "def welcome(who):\n    return greet(who)\n\n",
    "def mystery(thing):\n    return thing.run()\n\n",
    "def indirect(cb):\n    return cb()\n\n",
    "def missing():\n    return no_such_name()\n\n",
    "def dynamic(o):\n    return getattr(o, \"go\")()\n",
);

/// The same, minus the `getattr` call: **exactly** `SHOWN`, which is the only
/// input that tells `>` and `>=` apart on the residue row.
const CALLER_THREE_REASONS: &str = concat!(
    "from app.core import greet\n\n",
    "def welcome(who):\n    return greet(who)\n\n",
    "def mystery(thing):\n    return thing.run()\n\n",
    "def indirect(cb):\n    return cb()\n\n",
    "def missing():\n    return no_such_name()\n",
);

const OTHER_WITH_DOUBT: &str = concat!(
    "from app.core import greet\n\n",
    "def again(who):\n    return greet(who)\n\n",
    "def second_mystery(thing):\n    return thing.launch()\n\n",
    "def second_indirect(cb):\n    return cb()\n",
);

/// Four reasons across two files: past `SHOWN` and past the one-file guard,
/// so the residue row and the concentration block both render.
fn repo_with_doubt_spread(name: &str) -> Scratch {
    repo_with_doubt(name, CALLER_FOUR_REASONS, Some(OTHER_WITH_DOUBT))
}

/// `n` files that each call `greet` and each carry one unbound call.
///
/// The concentration block shows five files and collapses the rest, so `n`
/// is what puts a run either side of that boundary. One reason throughout,
/// deliberately: this fixture is about the file list, and a second reason
/// would move the table above it as well.
fn repo_with_doubt_in_n_files(name: &str, n: usize) -> Scratch {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    for i in 0..n {
        std::fs::write(
            d.join(format!("app/m{i}.py")),
            format!(
                "from app.core import greet\n\n\
                 def call{i}(who):\n    return greet(who)\n\n\
                 def doubt{i}(thing):\n    return thing.run()\n"
            ),
        )
        .unwrap();
    }
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// A repository with no doubt at all in the files a radius touches.
///
/// Every fixture above carries an unbound call on purpose, so the honest
/// all-clear — the branch that says every call site *was* bound — has never
/// been printed under test. Nothing here calls a method on an untyped value,
/// names an unbound name, or passes a callable, so there is nothing for the
/// resolver to miss.
fn repo_without_doubt(name: &str) -> Scratch {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return f\"hello {name}\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app/caller.py"),
        "from app.core import greet\n\ndef welcome(who):\n    return greet(who)\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// The `<count> <path>` rows under "concentrated in:", residue row included.
fn file_rows(s: &str) -> Vec<u32> {
    s.lines()
        .skip_while(|l| !l.contains("concentrated in"))
        .skip(1)
        .map_while(|l| l.split_whitespace().next()?.parse::<u32>().ok())
        .collect()
}

/// The `<Name> <count>` rows of the doubt table, residue row included.
///
/// Shape-matched rather than pattern-matched, and deliberately strict about
/// what counts: the file-concentration rows below the table lead with their
/// count instead of a name, so they cannot be mistaken for reasons.
fn reason_rows(s: &str) -> Vec<(String, u32)> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?;
            let n: u32 = it.next()?.parse().ok()?;
            let looks_like_a_reason =
                name == "(other)" || name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            looks_like_a_reason.then(|| (name.to_string(), n))
        })
        .collect()
}

#[test]
fn radius_does_not_claim_every_call_site_was_bound_when_one_was_not() {
    // Constraint 1, at the surface: the graph must never claim completeness.
    // `repo()` contains `thing.run()` on an untyped parameter, so there is
    // genuinely one call site bound to nothing, and the report has a branch
    // that says "every call site ... was bound" for the case where there is
    // not. Picking that branch here would print a false all-clear over real
    // doubt, which is the one output this project cannot ship.
    //
    // The older radius test greps for "call site", which appears in *both*
    // branches and so passes either way. That is why this needed its own.
    let d = repo("radius-completeness");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(
        !s.contains("was bound"),
        "a radius with real doubt claimed every call site was bound:\n{s}"
    );
    assert!(
        s.contains("bound to nothing"),
        "and it has to say so positively, not merely omit the claim:\n{s}"
    );
    assert!(
        s.contains("MethodDispatchUntyped"),
        "with the reason, which is what tells a reader whose fault it is:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_doubt_reasons_account_for_the_whole_count() {
    // ADR 0014's residue rule. The table shows three reasons and collapses
    // the rest into "(other)", and the point of that row is that the
    // headline figure stays fully accounted for on screen. Drop it and the
    // rows silently sum to less than the number printed above them, which is
    // the same class of error as dividing two populations: every line true,
    // the screen false.
    let d = repo_with_doubt_spread("radius-residue");
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let headline: u32 = s
        .lines()
        .find(|l| l.contains("bound to nothing"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline to check against:\n{s}"));

    let rows = reason_rows(&s);
    assert!(
        rows.iter().any(|(n, _)| n == "(other)"),
        "this fixture has more reasons than the table shows, so there must be a residue row:\n{s}"
    );
    let shown: u32 = rows.iter().map(|(_, n)| n).sum();
    assert_eq!(
        shown, headline,
        "the reasons on screen must account for the headline figure; rows were \
         {rows:?} against a headline of {headline}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_omits_the_residue_row_when_there_is_no_residue() {
    // The other direction, and the one a `>` -> `>=` slip breaks: with no
    // reasons beyond the three shown, "(other) 0 across 0 further reasons"
    // is a row claiming a residue that does not exist. `repo()` has exactly
    // one reason, so any residue row here is spurious.
    let d = repo("radius-no-residue");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);
    assert!(
        !s.contains("(other)"),
        "a residue row appeared with nothing to collapse into it:\n{s}"
    );
    // Same principle one block down. "concentrated in:" above a list of one
    // file distinguishes nothing; the doubt line already named the count.
    assert!(
        !s.contains("concentrated in"),
        "doubt in a single file was reported as a concentration:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_omits_the_residue_row_at_exactly_the_number_it_shows() {
    // The boundary itself, and the only input that separates `>` from `>=`.
    // With four reasons both behave alike and with one both stay silent, so
    // a fixture at exactly `SHOWN` is the whole test: `>=` here prints
    // "(other) 0 across 0 further reasons", a row whose two figures are both
    // zero, sitting under a table that already accounted for everything.
    let d = repo_with_doubt("radius-residue-boundary", CALLER_THREE_REASONS, None);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let rows = reason_rows(&s);
    assert_eq!(
        rows.len(),
        3,
        "this fixture is only meaningful at exactly three reasons; got {rows:?}\n{s}"
    );
    assert!(
        !s.contains("(other)"),
        "a residue row at exactly the number shown, with nothing left over:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_names_the_files_the_doubt_sits_in_when_it_is_spread() {
    // A count with no location is a number a reader cannot act on. The
    // concentration block is only worth printing when there is more than one
    // file to distinguish between, which is the condition being defended.
    let d = repo_with_doubt_spread("radius-concentration");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);
    assert!(s.contains("concentrated in"), "got:\n{s}");
    assert!(s.contains("app/caller.py"), "got:\n{s}");
    assert!(s.contains("app/other.py"), "got:\n{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_offers_the_way_to_shrink_doubt_only_when_there_is_doubt_to_shrink() {
    // The closing advice — "add a test that exercises this symbol" — is
    // conditional, and both directions are wrong in their own way. Printed
    // over a clean radius it invents work and quietly contradicts the line
    // above it; withheld where there *is* doubt it leaves the reader the
    // number and not the move. Asserted in both directions, because a guard
    // tested one way is a guard that can be inverted for free.
    let dirty = repo("radius-advice-doubt");
    let s = stdout(&run(&dirty, &["radius", "app.core.greet"]));
    assert!(
        s.contains("add a test that exercises this symbol"),
        "doubt was reported with no way out of it offered:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&dirty);

    let clean = repo_without_doubt("radius-advice-clean");
    let s = stdout(&run(&clean, &["radius", "app.core.greet"]));
    assert!(
        s.contains("was bound"),
        "this fixture is only meaningful with no doubt in the touched files:\n{s}"
    );
    assert!(
        !s.contains("To shrink the doubt"),
        "a clean radius was told to go shrink doubt it does not have:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&clean);
}

#[test]
fn radius_file_list_accounts_for_the_whole_count_when_it_is_truncated() {
    // The residue rule again, one block down. Five files are listed and the
    // rest collapse into "across N further files", and the reason that row
    // exists is that the listed counts otherwise sum to less than the
    // headline — a screen that quietly stops adding up.
    let d = repo_with_doubt_in_n_files("radius-files-residue", 7);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    let headline: u32 = s
        .lines()
        .find(|l| l.contains("bound to nothing"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline to check against:\n{s}"));

    assert!(
        s.contains("further files"),
        "seven files is past the five shown, so there must be a residue row:\n{s}"
    );
    let rows = file_rows(&s);
    assert_eq!(
        rows.iter().sum::<u32>(),
        headline,
        "the files on screen must account for the headline figure; rows were \
         {rows:?} against a headline of {headline}:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_file_list_omits_the_residue_row_at_exactly_the_number_it_shows() {
    // Five files, the boundary — the only input that separates `>` from
    // `>=` on the file residue, where `>=` prints "0 across 0 further
    // files" under a list that already named every one of them.
    let d = repo_with_doubt_in_n_files("radius-files-boundary", 5);
    let out = run(&d, &["radius", "app.core.greet"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert_eq!(
        file_rows(&s).len(),
        5,
        "this fixture is only meaningful at exactly five files:\n{s}"
    );
    assert!(
        !s.contains("further files"),
        "a residue row at exactly the number shown, with nothing left over:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn radius_reason_table_lines_up() {
    // End to end, because the defect lived in the seam: `radius_of` asks for
    // `{reason:<22}` and `Reason`'s Display used `write_str`, which ignores
    // the width the caller asked for. Every reason row ran ragged while the
    // "(other)" row — a plain &str, which does pad — lined up, so the column
    // broke in exactly the place a reader compares counts. Neither side is
    // wrong alone; only running them together shows it.
    let d = repo_with_doubt_spread("radius-columns");
    let out = run(&d, &["radius", "app.core.greet"]);
    let s = stdout(&out);

    // Where each row's *count* starts — not where its name ends, which
    // differs per row by construction and is what the padding exists to
    // absorb. The first version of this test measured the name and failed
    // against correct output.
    let counts_at: Vec<usize> = s
        .lines()
        .filter(|l| {
            let mut it = l.split_whitespace();
            let name = it.next().unwrap_or("");
            let is_reason =
                name == "(other)" || name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            is_reason && it.next().is_some_and(|t| t.parse::<u32>().is_ok())
        })
        .filter_map(|l| {
            let start = l.find(|c: char| !c.is_whitespace())?;
            let rest = &l[start..];
            let name_len = rest.find(char::is_whitespace)?;
            let gap = rest[name_len..].len() - rest[name_len..].trim_start().len();
            Some(start + name_len + gap)
        })
        .collect();

    assert!(counts_at.len() >= 2, "not enough rows to compare:\n{s}");
    assert!(
        counts_at.windows(2).all(|w| w[0] == w[1]),
        "the count column does not line up across rows (counts start at {counts_at:?}):\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
