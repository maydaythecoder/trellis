use super::*;

// ---------------------------------------------------------------------------
// resume, log, why
// ---------------------------------------------------------------------------

#[test]
fn resume_with_no_trail_says_so_and_names_what_writes_one() {
    let d = repo("resume-empty");
    let out = run(&d, &["resume"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("Nothing to resume"), "{s}");
    assert!(
        s.contains("trellis run"),
        "it must name what writes one: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Writes a minimal trail as `<run_id>.json`. Ids begin with a sortable
/// timestamp, so the order they are written in does not matter: `log` and
/// `resume` both order by name.
fn plant_trail(root: &Path, run_id: &str, task: &str) {
    let dir = root.join(".trellis/runs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{run_id}.json")),
        format!(r#"{{"run_id":"{run_id}","task":"{task}","factual":{{"proposed":[]}}}}"#),
    )
    .unwrap();
}

#[test]
fn resume_run_id_known_loads_that_run_rather_than_the_newest() {
    // The risk: an id that is accepted and then ignored, so the user reads
    // the newest run's facts believing they are the named run's.
    let d = repo("resume-id-known");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = run(&d, &["resume", "20260101-000000-000-older"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("previous task: the older task"), "{s}");
    assert!(
        !s.contains("the newer task"),
        "the newest run leaked in: {s}"
    );
    // `run --resume` carries the newest trail only, so offering it here
    // would name a different run than the one just shown.
    assert!(
        !s.contains("pass --resume"),
        "an older run must not be offered to `run --resume`: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_unknown_fails_naming_the_ten_most_recent_ids() {
    // The risk: an unknown id falling back to some other run. Resuming from
    // the wrong run is worse than refusing, so it fails, and names what
    // exists so the user need not run a second command to find out.
    let d = repo("resume-id-unknown");
    for i in 0..11 {
        plant_trail(&d, &format!("202601{:02}-000000-000-run", i + 1), "t");
    }
    // A prefix of a real id is still unknown: a partial match could pick
    // either of two runs sharing it.
    let out = run(&d, &["resume", "20260111"]);
    assert!(!out.status.success(), "{}", combined(&out));
    let s = combined(&out);
    assert!(s.contains("no run `20260111`"), "{s}");
    for i in 2..=11 {
        let id = format!("202601{i:02}-000000-000-run");
        assert!(s.contains(&id), "{id} is one of the ten newest: {s}");
    }
    assert!(
        !s.contains("20260101-000000-000-run"),
        "the eleventh-newest id must not be listed: {s}"
    );
    assert!(!s.contains("previous task"), "nothing may be loaded: {s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_bare_resume_still_loads_the_newest() {
    // The risk: making the id optional changed what bare `resume` does.
    let d = repo("resume-id-bare");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = run(&d, &["resume"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("previous task: the newer task"), "{s}");
    assert!(!s.contains("the older task"), "{s}");
    assert!(s.contains("pass --resume"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn resume_run_id_session_passes_the_id_through() {
    // `/resume <run_id>` is the session spelling of the same command; an id
    // dropped between the prompt and `cmd_resume` would show the newest run.
    let d = repo("resume-id-session");
    plant_trail(&d, "20260101-000000-000-older", "the older task");
    plant_trail(&d, "20260102-000000-000-newer", "the newer task");
    let out = session_with(&d, "/resume 20260101-000000-000-older\n/exit\n");
    let s = combined(&out);
    assert!(s.contains("previous task: the older task"), "{s}");
    assert!(!s.contains("the newer task"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_with_no_trails_is_not_an_error() {
    // A fresh repository has no runs. Exiting non-zero here would break any
    // script that checks history before deciding to write more.
    let d = repo("log-empty");
    let out = run(&d, &["log"]);
    assert!(out.status.success(), "{}", combined(&out));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_actions_lists_each_recorded_action_and_only_the_outcomes_there_are() {
    // The risk (#248): `--actions` printing nothing, or a blank outcome row
    // under every action that has none, so the one that wrote is lost
    // among them.
    let d = repo("log-actions");
    let runs = d.join(".trellis/runs");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(
        runs.join("r1.json"),
        r#"{"run_id":"r1","task":"t","factual":{"proposed":[],"actions":[
            {"turn":1,"tool":"read_file","target":"a.py","outcome":"","wrote":false},
            {"turn":2,"tool":"edit","target":"a.py","outcome":"applied","wrote":true}]}}"#,
    )
    .unwrap();
    let s = stdout(&run(&d, &["log", "--actions"]));
    assert!(s.contains("actions (2), as recorded:"), "{s}");
    assert!(
        s.contains("    *  2. edit a.py\n          applied\n"),
        "{s}"
    );
    assert!(s.contains("       1. read_file a.py\n    *  2."), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_prints_ended_at_and_each_duration_a_run_recorded_and_nothing_for_an_older_trail() {
    // The risk (#622): the timings written to the trail unread by the one
    // command a person uses to see where a run's time went; or a trail from
    // before the fields shown with an end of "" or a duration of 0, which
    // reads as a measurement.
    let d = repo_that_converges("log-durations", "def greet(name):\n    return \"hello\"\n");
    let out = run(&d, &["run", "--model", "scripted:script.json", "tweak"]);
    assert!(combined(&out).contains("CONVERGED"), "{}", combined(&out));
    let runs = d.join(".trellis/runs");
    let trail = std::fs::read_dir(&runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .expect("the run wrote a trail");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&trail).unwrap()).unwrap();
    assert!(v["ended_at"].is_string(), "{v}");
    assert!(
        v["factual"].get("ended_at").is_none(),
        "outside the digest: {v}"
    );
    let actions = v["factual"]["actions"].as_array().unwrap();
    assert!(!actions.is_empty());
    assert!(
        actions.iter().all(|a| a["duration_ms"].is_u64()),
        "every loop action is timed: {actions:?}"
    );
    let checks = v["factual"]["check_outcomes"].as_array().unwrap();
    assert!(!checks.is_empty());
    assert!(
        checks.iter().all(|c| c["duration_ms"].is_u64()),
        "{checks:?}"
    );

    let s = stdout(&run(&d, &["log", "--actions"]));
    let started = v["started_at"].as_str().unwrap();
    let ended = v["ended_at"].as_str().unwrap();
    assert!(
        s.contains(&format!("started {started}; ended {ended}")),
        "{s}"
    );
    assert!(
        s.contains(" ms\n") || s.contains(" ms,"),
        "a check's time: {s}"
    );
    assert!(s.contains(" ms)"), "an action's time: {s}");

    // A trail written before the fields, beside it.
    plant_log_fixture(
        &d,
        r#"[{"kind":"Tests","passed":true}]"#,
        r#"{"reached_resolved":0,"reached_ambiguous":0,"widest_unbound_in_touched_files":0,"radius_queries":0}"#,
    );
    let s = stdout(&run(&d, &["log"]));
    assert!(
        s.contains("recorded checks: tests passed\n"),
        "no time shown: {s}"
    );
    assert!(!s.contains("in 0 ms"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A trail whose factual section holds the given check outcomes and radius
/// counts, planted under `.trellis/runs`.
fn plant_log_fixture(d: &Path, checks: &str, radius: &str) {
    let runs = d.join(".trellis/runs");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(
        runs.join("r1.json"),
        format!(
            r#"{{"run_id":"r1","task":"t","factual":{{"proposed":["a.py"],
                "check_outcomes":{checks},"radius":{radius},"actions":[]}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn log_words_checks_are_passed_or_failed_not_a_suffix() {
    // #651: "checks: Tests+ Typecheck-" read as marks on the checks, and the
    // one line of terminal history a person scans for a run's result
    // carried its verdicts in a glyph. The word is what an executed
    // process said; nothing else here is a verdict (ADR 0004).
    let d = repo("log-words-checks");
    plant_log_fixture(
        &d,
        r#"[{"kind":"Tests","passed":true},{"kind":"Typecheck","passed":false}]"#,
        r#"{"reached_resolved":0,"reached_ambiguous":0,"widest_unbound_in_touched_files":0,"radius_queries":0}"#,
    );
    let s = stdout(&run(&d, &["log"]));
    assert!(
        s.contains("recorded checks: tests passed, typecheck failed"),
        "{s}"
    );
    assert!(!s.contains("Tests+") && !s.contains("Typecheck-"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_words_radius_is_three_figures_and_never_one_sum() {
    // #651: "15 reached" added the resolved and ambiguous reach, which rule
    // 3 forbids as much as a ratio: the two reach counts are sums over the
    // run's queries and the unbound count is one query's, so no arithmetic
    // across them is a figure about anything. Each is printed by itself,
    // and the doubt figure is printed beside the reach it qualifies.
    let d = repo("log-words-radius");
    plant_log_fixture(
        &d,
        "[]",
        r#"{"reached_resolved":12,"reached_ambiguous":3,"widest_unbound_in_touched_files":4,"radius_queries":2}"#,
    );
    let s = stdout(&run(&d, &["log"]));
    assert!(
        s.contains(
            "radius: 12 resolved, 3 ambiguous reached; 4 unbound in touched files \
             (widest single query, of 2)"
        ),
        "{s}"
    );
    assert!(!s.contains("15 reached"), "{s}");
    assert!(s.contains("recorded checks: none"), "{s}");

    // A run that asked no radius has no counts to show: zeros here would
    // read as a radius that found nothing rather than one never queried.
    let d2 = repo("log-words-radius-none");
    plant_log_fixture(
        &d2,
        "[]",
        r#"{"reached_resolved":0,"reached_ambiguous":0,"widest_unbound_in_touched_files":0,"radius_queries":0}"#,
    );
    let s = stdout(&run(&d2, &["log"]));
    assert!(s.contains("radius: no query recorded"), "{s}");
    assert!(!s.contains("0 resolved"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn an_inferred_root_is_named_and_an_explicit_one_is_not() {
    // The risk (#248): acting on a tree found several levels up without
    // saying which, so a task runs against the wrong repository.
    let d = repo("root-named");
    let inferred = combined(&run(&d.join("app"), &["log"]));
    assert!(inferred.contains("(repository root)"), "{inferred}");
    let elsewhere = scratch("root-named-elsewhere");
    let root = d.to_str().unwrap();
    let explicit = combined(&run(&elsewhere, &["--root", root, "log"]));
    assert!(!explicit.contains("(repository root)"), "{explicit}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

#[test]
fn run_approve_overrides_a_repository_in_auto_mode() {
    // The risk (#248): `--approve` dropped, so a person who asked to be
    // asked before each edit gets a run that asks nothing (ADR 0018).
    let d = repo("approve-over-auto");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[control]\nmode = \"auto\"\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "auto mode"]);
    let spec = script_that_stops(&scratch("approve-over-auto-script"));
    let out = run(&d, &["run", "--approve", "--model", &spec, "t"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(
        combined(&out).contains("approve mode needs an interactive terminal"),
        "{}",
        combined(&out)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn run_refuses_a_skill_it_cannot_find_rather_than_running_without_it() {
    // The risk (#248): `--skill` dropped on the way to the run, so the run
    // goes ahead without the instructions the person named.
    let d = repo("skill-missing");
    let spec = script_that_stops(&scratch("skill-missing-script"));
    let out = run(&d, &["run", "--skill", "nope", "--model", &spec, "t"]);
    assert!(!out.status.success(), "{}", combined(&out));
    assert!(combined(&out).contains("nope"), "{}", combined(&out));
    assert!(!d.join(".trellis/runs").exists(), "no trail");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_reports_absence_rather_than_inventing_a_reason() {
    // `why` answers from the trail. With no trail there is no answer, and the
    // honest output is that there is none — the alternative is a plausible
    // reconstruction, which is the one thing a decision trail exists to stop.
    let d = repo("why-empty");
    let out = run(&d, &["why", "app.core.greet"]);
    let s = combined(&out);
    assert!(out.status.success(), "{s}");
    assert!(
        s.contains("no run in the trail mentions `app.core.greet`"),
        "it must report having nothing to say: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// the trail's radius counts — which of them sum, and which deliberately do not
// ---------------------------------------------------------------------------

/// A repository with one symbol reached by resolved edges and one reached
/// only ambiguously, plus doubt in both radii.
///
/// The override lives in its own module on purpose. A method's qualified name
/// does not carry its class, so a base and an override in one file are both
/// `app.core.execute` and `radius` refuses them as ambiguous — there is no
/// way to ask about either. Split across modules they are `app.core.execute`
/// and `app.sub.execute`, the base is nameable, and a call on a `Base`-typed
/// receiver resolves to `Candidates` over both (ADR 0012: a declared type is
/// an upper bound, not an identity).
fn repo_with_resolved_and_ambiguous_reach(name: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        concat!(
            "class Base:\n    def execute(self):\n        return 1\n\n",
            "def greet(name):\n    return f\"hello {name}\"\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/sub.py"),
        concat!(
            "from app.core import Base\n\n",
            "class Sub(Base):\n    def execute(self):\n        return 2\n\n",
            "def murky(thing):\n    return thing.spin()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        concat!(
            "from app.core import Base, greet\n\n",
            "def go(b: Base):\n    return b.execute()\n\n",
            "def also(b: Base):\n    return b.execute()\n\n",
            "def hi(w):\n    return greet(w)\n\n",
            "def hey(w):\n    return greet(w)\n\n",
            "def vague(thing):\n    return thing.run()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("script.json"),
        concat!(
            r#"{"steps":["#,
            r#"{"tool":"blast_radius","symbol":"app.core.execute"},"#,
            r#"{"tool":"blast_radius","symbol":"app.core.greet"},"#,
            r#"{"tool":"done","summary":"looked"}]}"#,
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// The `N` from one of the two doubt headlines `radius` can print.
fn unbound_reported_by_radius(root: &Path, symbol: &str) -> u32 {
    let s = stdout(&run(root, &["radius", symbol]));
    s.lines()
        .find(|l| l.contains("bound to nothing") || l.contains("was bound"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("no doubt headline for {symbol}:\n{s}"))
}

#[test]
fn the_trail_sums_reach_across_queries_and_takes_the_widest_doubt() {
    // Two facts in one run, because they are only interesting together.
    //
    // Reach *is* additive: each query asks about a different symbol, so "how
    // many call sites did this run examine" is the sum. Doubt is not. Two
    // radii in one repository touch overlapping files, so adding their
    // unbound counts counts the same call sites once per query — a figure
    // that grows with how many questions were asked rather than with how
    // much is unknown. The widest single query is a true statement about a
    // real radius; the sum is a statement about nothing.
    //
    // The fixture is built so the two differ: each radius reports the same
    // doubt, so a sum would be double the max and the two rules cannot be
    // confused for one another by accident.
    let d = repo_with_resolved_and_ambiguous_reach("trail-radius-counts");

    let per_query = [
        unbound_reported_by_radius(&d, "app.core.execute"),
        unbound_reported_by_radius(&d, "app.core.greet"),
    ];
    let widest = per_query.iter().copied().max().unwrap();
    let summed: u32 = per_query.iter().sum();
    assert_ne!(
        widest, summed,
        "this fixture is only meaningful when a sum and a max differ; got {per_query:?}"
    );

    let out = run(
        &d,
        &["run", "--model", "scripted:script.json", "look around"],
    );
    // A `done` with no passing check does not converge, and must not: only an
    // executed check accepts a change. The trail is written either way, and
    // is written *after* the rollback so it survives it.
    assert!(
        !out.status.success(),
        "a run that only looked around must not report convergence:\n{}",
        combined(&out)
    );

    // Read back the way the product reads it: `UntrustedTrail`, never
    // `Trail`. A trail lives in a repository that may take contributions, so
    // reading one is reading attacker-influenceable text (S10).
    let (_, trail) = trellis_trail::latest(&d)
        .expect("the trail directory must be readable")
        .expect("a run that reached its first turn writes a trail");
    let r = &trail.factual.radius;

    assert_eq!(r.radius_queries, 2, "two queries were made: {r:?}");
    assert_eq!(
        r.reached_resolved, 2,
        "`greet` has two resolved callers and `execute` none, so the sum is two: {r:?}"
    );
    assert_eq!(
        r.reached_ambiguous, 2,
        "`execute` is reached twice through Candidates and `greet` never: {r:?}"
    );
    assert_eq!(
        r.widest_unbound_in_touched_files, widest,
        "the widest single radius, not a sum — a sum here would be {summed}: {r:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// why / log — reading the trail back
// ---------------------------------------------------------------------------

/// A repository plus a script that edits the file defining `greet`.
///
/// The run does not converge and is rolled back — a `done` with no passing
/// check never converges, because only an executed check accepts a change —
/// and that is fine here: the trail is written after the rollback and
/// survives it, which is the property `why` depends on.
fn repo_with_an_edit_script(name: &str) -> PathBuf {
    let d = empty_repo(name);

    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(name):\n    return \"hello\"\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        "from app.core import greet\n\ndef w(x):\n    return greet(x)\n",
    )
    .unwrap();
    std::fs::write(
        d.join("script.json"),
        concat!(
            r#"{"steps":[{"tool":"edit","path":"app/core.py","#,
            r#""old":"return \"hello\"","new":"return \"hi\""},"#,
            r#"{"tool":"done","summary":"changed the greeting"}]}"#,
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "fixture"]);
    d
}

/// Runs the scripted edit once, leaving one more trail behind.
fn leave_a_trail(root: &Path, task: &str) {
    let out = run(root, &["run", "--model", "scripted:script.json", task]);
    assert!(
        !out.status.success(),
        "a run with no passing check must not converge:\n{}",
        combined(&out)
    );
}

#[test]
fn why_finds_a_run_by_the_symbol_whose_file_it_changed() {
    // The failure `03-v0.3-memory.md` names, and the one the first wedge run
    // actually hit. The factual section records FILES; a user asking "why
    // does this function look like this" holds a SYMBOL. Matching the symbol
    // against paths finds nothing unless the two happen to share a name, so
    // `why` reported "no file matching `boolean_validator` was changed"
    // about a run that had changed the file defining it — the trail written
    // faithfully and the query unable to answer the question it exists for.
    //
    // The mapping is recomputed from the index rather than recorded, which
    // is also why it works on trails written before it existed.
    let d = repo_with_an_edit_script("why-symbol");
    leave_a_trail(&d, "tweak the greeting");

    let s = stdout(&run(&d, &["why", "greet"]));
    assert!(
        s.contains("changed: app/core.py"),
        "`why` on a symbol must reach the file that defines it:\n{s}"
    );
    assert!(
        !s.contains("no run in the trail mentions"),
        "the run is there and it changed that file:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_separates_a_file_that_changed_from_a_word_in_the_task() {
    // The distinction is the whole value of this command. A run that *edited*
    // the file is evidence about why the code looks like this; a run that
    // merely named it in a task string is context. Reported alike, the
    // weaker one reads as the stronger — and the stronger one loses the fact
    // that a file actually moved.
    let d = repo_with_an_edit_script("why-mentions");
    leave_a_trail(&d, "tweak the greeting");

    let changed = stdout(&run(&d, &["why", "greet"]));
    assert!(
        !changed.contains("named in the task"),
        "a run that changed the file must not be reported as merely naming it:\n{changed}"
    );

    // "greeting" is a word in the task and not a symbol anywhere, so this is
    // the other half of the same branch.
    let mentioned = stdout(&run(&d, &["why", "greeting"]));
    assert!(
        mentioned.contains("named in the task"),
        "a run that only named it must say so rather than staying silent:\n{mentioned}"
    );
    assert!(
        !mentioned.contains("changed:"),
        "nothing matching `greeting` was changed:\n{mentioned}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn why_says_when_a_trail_no_longer_matches_its_digest() {
    // A trail lives in a repository that takes contributions, so it is read
    // back as untrusted data (S10). Tamper-evidence is only worth having if
    // the surface that reads it says so: a silently altered record is worse
    // than no record, because it is believed.
    let d = repo_with_an_edit_script("why-altered");
    leave_a_trail(&d, "tweak the greeting");

    let clean = stdout(&run(&d, &["why", "greet"]));
    assert!(
        !clean.contains("ALTERED"),
        "an untouched trail must not be reported as altered:\n{clean}"
    );

    // Edit inside the factual section, which is what the digest covers, and
    // deliberately not `proposed` — so the `changed:` line stays identical
    // and the only difference is the integrity verdict.
    let p = newest_trail(&d);
    let text = std::fs::read_to_string(&p).unwrap();
    let tampered = text.replace("\"radius_queries\": 0", "\"radius_queries\": 9");
    assert_ne!(text, tampered, "the tamper must actually change the file");
    std::fs::write(&p, tampered).unwrap();

    let s = stdout(&run(&d, &["why", "greet"]));
    assert!(s.contains("ALTERED"), "an edited trail must say so:\n{s}");
    assert!(
        s.contains("changed: app/core.py"),
        "and must still report what it records, flagged rather than hidden:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_lists_the_runs_it_found() {
    let d = repo_with_an_edit_script("log-lists");
    leave_a_trail(&d, "first pass");

    let s = stdout(&run(&d, &["log"]));
    assert!(s.contains("run(s), newest first"), "got:\n{s}");
    assert!(s.contains("first pass"), "the task must appear:\n{s}");
    assert!(
        !s.contains("no trails"),
        "there is a trail, so the empty message is wrong:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_says_how_many_runs_it_did_not_show() {
    // Same residue rule as the radius report. A listing that silently stops
    // at the limit tells the reader they have seen everything, and the one
    // thing a decision trail must not do is imply there is no more history.
    let d = repo_with_an_edit_script("log-residue");
    leave_a_trail(&d, "first pass");
    leave_a_trail(&d, "second pass");

    let s = stdout(&run(&d, &["log", "--limit", "1"]));
    assert!(
        s.contains("1 older"),
        "two runs shown one at a time must account for the other:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn log_does_not_offer_more_when_it_showed_everything() {
    // The boundary, and the only input that separates `>` from `>=`.
    let d = repo_with_an_edit_script("log-boundary");
    leave_a_trail(&d, "first pass");
    leave_a_trail(&d, "second pass");

    let s = stdout(&run(&d, &["log", "--limit", "2"]));
    assert!(
        !s.contains("older, use --limit"),
        "everything was shown, so there is nothing older to offer:\n{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
