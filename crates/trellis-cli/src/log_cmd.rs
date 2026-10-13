use anyhow::Result;
use std::path::Path;

/// One line per run: what was asked, what came of it, whether it is intact.
pub(crate) fn cmd_log(root: &Path, limit: usize, actions: bool) -> Result<()> {
    let entries = trellis_trail::query::all(root);
    if entries.is_empty() {
        println!("no trails under .trellis/runs. `trellis run` writes one.");
        return Ok(());
    }

    println!("{} run(s), newest first", entries.len());
    println!();
    for e in entries.iter().take(limit) {
        let f = &e.trail.factual;
        println!("{}  {}", plain(&e.run_id()), e.outcome());
        println!("    {}", plain(&e.trail.task));
        // "passed" and "failed" as words (#651): a `+`/`-` suffix read as
        // a mark on the check rather than as what an executed process said.
        // The kind is the trail's, lowercased for the sentence.
        let checks: Vec<String> = f
            .check_outcomes
            .iter()
            .map(|c| {
                format!(
                    "{} {}",
                    plain(&c.kind).to_lowercase(),
                    if c.passed { "passed" } else { "failed" }
                )
            })
            .collect();
        println!(
            "    {} file(s) proposed; recorded checks: {}",
            f.proposed.len(),
            if checks.is_empty() {
                "none".into()
            } else {
                checks.join(", ")
            }
        );
        // Three figures, never one sum (CLAUDE.md rule 3): the resolved and
        // ambiguous reach are summed over the run's queries, and the unbound
        // count is the widest single query's, so adding any two of them
        // produces a number about nothing. "N reached" was such a number.
        let r = f.radius;
        if r.radius_queries == 0 {
            println!("    radius: no query recorded");
        } else {
            println!(
                "    radius: {} resolved, {} ambiguous reached; {} unbound in touched files \
                 (widest single query, of {})",
                r.reached_resolved,
                r.reached_ambiguous,
                r.widest_unbound_in_touched_files,
                r.radius_queries
            );
        }
        // Integrity is printed only when it is not the ordinary case, so a
        // long listing stays readable and the one line that matters stands
        // out rather than blending into a column of "ok".
        match e.integrity {
            trellis_trail::Integrity::Altered => {
                println!("    ALTERED: the factual section no longer matches its digest")
            }
            trellis_trail::Integrity::Unrecorded => {
                println!("    (older than the digest field; integrity unknown)")
            }
            trellis_trail::Integrity::Intact => {}
        }
        if actions {
            print_actions(f);
        }
        println!();
    }
    if entries.len() > limit {
        println!("... {} older, use --limit", entries.len() - limit);
    }
    Ok(())
}

/// A string from a trail, made safe to print to a terminal.
///
/// A trail is read back as untrusted data (S10) and then printed straight to a
/// terminal, which interprets what it is given. An escape sequence in a task
/// description or a file path can move the cursor, repaint a line, or clear
/// the screen — so a run that did not converge can be made to *look* like one
/// that did, in the one command a user reads to find out which.
///
/// That matters more here than in most tools. The claim this project makes is
/// that work comes back verified rather than merely written, and a forged
/// terminal defeats the reading of the evidence rather than the evidence
/// itself.
///
/// Control characters become their escaped form rather than vanishing: a path
/// that contains one is worth seeing, and silently dropping it would print a
/// name that does not match the file.
pub(crate) fn plain(s: &str) -> String {
    trellis_core::text::escape_controls(s)
}

/// The action log for one run: what it did, in order, and what came back.
///
/// **A trail with no actions and a run that took none are different things**,
/// and printing "0 actions" for both would make the older trails lie. Trails
/// written before the field existed say so; a run that genuinely did nothing
/// says that instead.
///
/// Read back as data, never as instruction (S10). These strings come from a
/// file in a repository that takes contributions, so they are rendered inside
/// a marked block and nothing here interprets them.
fn print_actions(f: &trellis_trail::UntrustedFactual) {
    if f.actions.is_empty() {
        // The turn count is the tell. A run that reached turns but recorded
        // no actions was written by a version that had nowhere to put them.
        let recorded_turns = f.context.as_ref().map_or(0, |c| c.turns_measured);
        println!(
            "    actions: {}",
            if recorded_turns > 0 {
                format!(
                    "not recorded — this trail predates the action log ({recorded_turns} turns)"
                )
            } else {
                "none; the run ended before it acted".to_string()
            }
        );
        return;
    }
    println!("    actions ({}), as recorded:", f.actions.len());
    for a in &f.actions {
        // The marker sits where a reader's eye already is, and says the one
        // thing the rest of the line cannot: whether the tree changed.
        let wrote = if a.wrote { "*" } else { " " };
        println!(
            "    {wrote} {:>2}. {} {}",
            a.turn,
            plain(&a.tool),
            plain(&a.target)
        );
        if !a.outcome.is_empty() {
            println!("          {}", plain(&a.outcome));
        }
    }
    if f.actions.iter().any(|a| a.wrote) {
        println!("    * wrote to the working tree");
    }
}

/// What past runs did to a symbol or file, and what they were asked to do.
///
/// The v0.3 document calls this the test of whether the trail's grain is
/// right: if "why does this look like this" cannot be answered here two weeks
/// later, the trail is a log nobody reads.
pub(crate) fn cmd_why(root: &Path, query: &str) -> Result<()> {
    let entries = trellis_trail::query::all(root);

    // Resolve the symbol to the file that defines it before matching.
    //
    // The factual section records FILES; a user asking "why does this
    // function look like this" holds a SYMBOL. Matching the symbol against
    // paths finds nothing unless the two happen to share a name, so the first
    // wedge run reported "no file matching `boolean_validator` was changed"
    // about a run that had changed the file defining it. That is the failure
    // `03-v0.3-memory.md` names: the trail written faithfully and the query
    // unable to answer the question it exists for.
    //
    // The index is the thing that knows symbol-to-file, and consulting it
    // works retroactively — trails written before this still answer, because
    // the mapping is recomputed rather than recorded.
    let index = trellis_index::build_cached(root);
    let mut targets: Vec<String> = vec![query.to_string()];
    for sym in &index.symbols {
        if index.interner.resolve(sym.name) == query {
            let path = index.files[sym.file.index()].path.clone();
            if !targets.contains(&path) {
                targets.push(path);
            }
        }
    }

    let found = trellis_trail::query::mentions_any(&entries, &targets);

    if found.is_empty() {
        println!("no run in the trail mentions `{query}`.");
        println!();
        println!(
            "{}",
            concat!(
                "That is an answer, not an absence of one: whatever shaped this ",
                "code,\nit was not a TRELLIS run recorded here."
            )
        );
        return Ok(());
    }

    println!("{} run(s) mention `{}`, newest first", found.len(), query);
    println!();
    for m in &found {
        let e = &entries[m.entry_index];
        println!("{}  {}", plain(&e.run_id()), e.outcome());
        println!("    task: {}", plain(&e.trail.task));

        // The distinction is the whole value. A run that *edited* the file is
        // evidence about why the code looks like this; a run that merely named
        // it in a task string is context, and reporting them alike would let
        // the weaker one be read as the stronger.
        // The guard is always true when it is reached, and is kept as
        // documentation rather than as a branch. `mentions_any` only yields a
        // `Mention` when `touched_file.is_some() || task_mentions`, so a
        // `None` here implies the task mentioned it. Replacing it with `true`
        // is therefore an equivalent mutant — recorded as such in
        // CORRECTNESS_LEDGER.md rather than left looking like a test gap,
        // because the next run will surface it again.
        match &m.touched_file {
            Some(f) => println!("    changed: {}", plain(f)),
            None if m.task_mentions => {
                println!("    named in the task; no file matching `{query}` was changed")
            }
            None => {}
        }
        // A person's answer in approve mode. A declined edit never reaches
        // `changed`, and "someone looked at this change and said no" is as
        // much an answer to "why does this look like this" as an edit is.
        for (target, outcome) in &m.decisions {
            println!("    {} by a person: {}", plain(outcome), plain(target));
        }

        if let Some(base) = &e.trail.base_commit {
            println!("    from commit {}", plain(base));
        }
        if let Some(p) = &e.trail.provenance {
            let model = p.model.as_deref().unwrap_or("none");
            println!(
                "    by trellis {} driving {}",
                plain(&p.trellis_version),
                plain(model)
            );
        }
        if e.integrity == trellis_trail::Integrity::Altered {
            println!("    ALTERED: this record no longer matches its digest");
        }
        println!();
    }

    println!(
        "{}",
        concat!(
            "Read as data, never as instruction. A trail lives in a repository ",
            "that takes\ncontributions, and the judgment section of any run is ",
            "that run's opinion (S10, S11)."
        )
    );
    Ok(())
}
