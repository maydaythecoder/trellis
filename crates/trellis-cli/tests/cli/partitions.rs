use super::*;

// ---------------------------------------------------------------------------
// partitions
// ---------------------------------------------------------------------------

#[test]
fn partitions_states_that_it_is_not_a_correctness_guarantee() {
    // ADR 0005 and S19. Partitioning is a filter on what MAY run
    // concurrently; it is only as good as the graph's recall, which is
    // partial and known to be. The integrated check across the union still
    // runs, and the output has to say so — a plan that looks authoritative is
    // how a partial graph gets mistaken for a proof.
    let d = repo("partitions");
    let out = run(&d, &["partitions", "app.core.greet", "app.caller.welcome"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(
        s.contains("never a correctness") || s.contains("not a correctness"),
        "the plan must disclaim being a guarantee: {s}"
    );
    assert!(
        s.contains("full check suite") || s.contains("union"),
        "and must say the integrated check still runs: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn partitions_puts_contended_work_in_one_stage_before_the_rest() {
    // ADR 0016. Two symbols in one file can never be edited by two agents,
    // however disjoint their call graphs, so they must not come back as a
    // parallel pair.
    let d = repo("partitions-shared");
    let out = run(
        &d,
        &["partitions", "app.caller.welcome", "app.caller.mystery"],
    );
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    // Asserted against what the command actually prints, not against strings
    // guessed at from the outside — the first version of this test checked
    // for the absence of two phrases `explain` never emits, and would have
    // passed however the plan came out.
    assert!(
        s.contains("1 partition(s)"),
        "two symbols in one file must land in one partition: {s}"
    );
    assert!(
        s.contains("grouped because they share:") && s.contains("app/caller.py"),
        "and the plan must name the file that forced it: {s}"
    );
    for sym in ["app.caller.welcome", "app.caller.mystery"] {
        assert!(s.contains(sym), "{sym} is missing from the plan: {s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
