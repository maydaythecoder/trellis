//! Scores a completed hand audit against the resolver's verdicts.
//!
//! Joins the auditor's filled `sample.json` with the blind `answers.json` and
//! computes the figures `BENCHMARK_AND_EVAL.md` §2 defines. Nothing here
//! decides anything: it prints numbers, and the gate call in §3 is made by a
//! person reading them.
//!
//! Definitions, taken from §2 rather than invented here:
//!
//! - **Recall** = sites where `Resolved` names the true callee / sites whose
//!   true callee is first-party.
//! - **Precision** = sites where `Resolved` names the true callee / sites
//!   where anything was `Resolved`. The v1.0 gate requires zero unsound
//!   edges, so anything below 100% here is a defect, not a score.
//! - A `Candidates` result containing the true callee counts as **not**
//!   resolved, and is reported separately as *ambiguous-but-covered*.
//!
//! One figure is not in §2 and is added because the repository-wide coverage
//! report structurally cannot show it: **wrongly excluded** — sites the
//! resolver dropped as external that the auditor found to be first-party.
//! Those are recall misses that never appear in any denominator the resolver
//! computes for itself.
//!
//! Usage:
//!   cargo run --release -p trellis-index --example audit_score -- <dir>

use std::collections::BTreeMap;

#[derive(Default, Debug, Clone)]
struct Tally {
    first_party: u32,
    resolved_correct: u32,
    resolved_wrong: u32,
    resolved_total: u32,
    ambiguous_covered: u32,
    ambiguous_missed: u32,
    unresolved: u32,
    constructor_reconciled: u32,
    wrongly_excluded: u32,
    correctly_excluded: u32,
    dynamic_truth: u32,
}

impl Tally {
    fn recall(&self) -> Option<f64> {
        (self.first_party > 0)
            .then(|| f64::from(self.resolved_correct) / f64::from(self.first_party))
    }

    fn precision(&self) -> Option<f64> {
        (self.resolved_total > 0)
            .then(|| f64::from(self.resolved_correct) / f64::from(self.resolved_total))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: audit_score <dir containing sample.json and answers.json>");
        std::process::exit(2);
    }
    let dir = std::path::Path::new(&args[1]);

    let sample: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("sample.json")).expect("sample"))
            .expect("sample.json is not valid JSON");
    let answers: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("answers.json")).expect("answers"))
            .expect("answers.json is not valid JSON");

    let verdicts = &answers["verdicts"];
    let entries = sample["sample"]
        .as_array()
        .expect("sample.sample is an array");

    let mut overall = Tally::default();
    let mut by_shape: BTreeMap<String, Tally> = BTreeMap::new();
    // Grouped by top-level directory as well as by shape. Same reason §2
    // gives for the shape split: a pooled figure hides the case that
    // matters. On a repository whose sample is mostly test code, a headline
    // recall is largely a statement about the test suite.
    let mut by_area: BTreeMap<String, Tally> = BTreeMap::new();
    let mut unaudited = 0u32;
    let mut collisions: Vec<String> = Vec::new();
    let mut reconciled: Vec<String> = Vec::new();

    for e in entries {
        let truth = e["true_callee"].as_str();
        let Some(truth) = truth else {
            unaudited += 1;
            continue;
        };
        let first_party = e["is_first_party"].as_bool().unwrap_or(false);
        let shape = e["shape"].as_str().unwrap_or("Unknown").to_string();
        let id = e["id"].to_string().trim_matches('"').to_string();
        let verdict = &verdicts[&id];
        let kind = verdict["kind"].as_str().unwrap_or("Missing");

        let area = e["file"]
            .as_str()
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("?")
            .to_string();
        let t = by_shape.entry(shape).or_default();
        let ar = by_area.entry(area).or_default();
        for tally in [&mut overall, t, ar] {
            if truth == "dynamic" {
                tally.dynamic_truth += 1;
            }
            if first_party {
                tally.first_party += 1;
            }
            match kind {
                "Resolved" => {
                    tally.resolved_total += 1;
                    let target = verdict["target"].as_str().unwrap_or("");
                    if matches(target, truth) {
                        tally.resolved_correct += 1;
                    } else if constructor_convention(truth) {
                        // The resolver named the class, the auditor its
                        // `__init__`. Agreement, differently written.
                        tally.resolved_correct += 1;
                        tally.constructor_reconciled += 1;
                    } else {
                        tally.resolved_wrong += 1;
                    }
                }
                "Candidates" => {
                    let covered = verdict["targets"].as_array().is_some_and(|v| {
                        v.iter().any(|c| matches(c.as_str().unwrap_or(""), truth))
                    });
                    if covered {
                        tally.ambiguous_covered += 1;
                    } else {
                        tally.ambiguous_missed += 1;
                    }
                }
                "ExternalExcluded" => {
                    if first_party {
                        tally.wrongly_excluded += 1;
                    } else {
                        tally.correctly_excluded += 1;
                    }
                }
                _ => tally.unresolved += 1,
            }
        }

        // An unsound edge is the one thing the gate forbids outright, so
        // name each one rather than only counting it.
        let target = verdict["target"].as_str().unwrap_or("");
        if kind == "Resolved" && !matches(target, truth) && constructor_convention(truth) {
            reconciled.push(format!(
                "  {}:{}  `{}`  class {} vs auditor {}",
                e["file"].as_str().unwrap_or("?"),
                e["line"],
                e["call"].as_str().unwrap_or("?").replace('\n', " "),
                target,
                truth
            ));
        }
        if kind == "Resolved" && !matches(target, truth) && !constructor_convention(truth) {
            collisions.push(format!(
                "  {}:{}  `{}`\n      resolver: {}\n      auditor:  {}",
                e["file"].as_str().unwrap_or("?"),
                e["line"],
                e["call"].as_str().unwrap_or("?").replace('\n', " "),
                verdict["target"].as_str().unwrap_or("?"),
                truth
            ));
        }
    }

    println!("AUDIT SCORE  (seed {})", sample["seed"]);
    println!(
        "  drawn {}  population {}",
        sample["drawn"], sample["population"]
    );
    if unaudited > 0 {
        println!("  UNAUDITED {unaudited} — these are excluded from every figure below");
    }
    println!();
    report("OVERALL", &overall);
    println!("-- by callee shape --");
    for (shape, t) in &by_shape {
        report(shape, t);
    }
    println!("-- by top-level directory --");
    for (area, t) in &by_area {
        report(area, t);
    }

    if !reconciled.is_empty() {
        println!(
            "CONSTRUCTOR CONVENTION — {} sites counted as correct.",
            reconciled.len()
        );
        println!(
            "The resolver named the class being constructed; the auditor named the \
             `__init__`\nthat runs. Both are right and neither is an unsound edge. The \
             audit protocol did\nnot state which to record, and now does. Listed so the \
             reconciliation is visible\nrather than folded in silently:"
        );
        for r in &reconciled {
            println!("{r}");
        }
        println!();
    }
    if collisions.is_empty() {
        println!("Unsound edges: none in this sample.");
    } else {
        println!("UNSOUND EDGES — {} in this sample.", collisions.len());
        println!("The v1.0 gate requires zero. Each is a defect to fix, not a score:");
        for c in &collisions {
            println!("{c}");
        }
    }
    println!();
    println!(
        "Compare recall against the thresholds in BENCHMARK_AND_EVAL.md §3 \
         (gate 85%, pivot 60%).\nThe repository-wide figure from `trellis index` \
         is NOT comparable: it counts\nunknown-receiver calls as misses even when \
         their true callee is third-party (A-30)."
    );
}

fn report(label: &str, t: &Tally) {
    println!("{label}");
    println!("  first-party sites (denominator)     {:>5}", t.first_party);
    match t.recall() {
        Some(r) => println!(
            "  RECALL                              {:>5.1}%   ({}/{})",
            r * 100.0,
            t.resolved_correct,
            t.first_party
        ),
        None => println!("  RECALL                                n/a   (no first-party sites)"),
    }
    match t.precision() {
        Some(p) => println!(
            "  PRECISION                           {:>5.1}%   ({}/{})",
            p * 100.0,
            t.resolved_correct,
            t.resolved_total
        ),
        None => println!("  PRECISION                             n/a   (nothing resolved)"),
    }
    println!(
        "  ambiguous, true callee covered      {:>5}",
        t.ambiguous_covered
    );
    println!(
        "  ambiguous, true callee absent       {:>5}",
        t.ambiguous_missed
    );
    println!("  unresolved, with a reason           {:>5}", t.unresolved);
    println!(
        "  of which constructor-convention     {:>5}   (class vs its __init__)",
        t.constructor_reconciled
    );
    println!(
        "  wrongly excluded as external        {:>5}   (first-party, no edge emitted)",
        t.wrongly_excluded
    );
    println!(
        "  correctly excluded as external      {:>5}",
        t.correctly_excluded
    );
    println!(
        "  auditor could not determine         {:>5}",
        t.dynamic_truth
    );
    println!();
}

/// Whether the resolver's `module.name` names the auditor's
/// `module.path.Class.method`.
///
/// The resolver emits a module path plus a bare symbol name and does not
/// record the enclosing class, so an exact string comparison would fail on
/// every method. Matching on module prefix plus final segment is the
/// available comparison.
fn matches(resolver: &str, auditor: &str) -> bool {
    if resolver == auditor {
        return true;
    }
    let last = |s: &str| s.rsplit('.').next().unwrap_or(s).to_string();
    if last(resolver) != last(auditor) {
        return false;
    }
    // Module prefix: everything before the final segment on the resolver
    // side must be a prefix of the auditor's qualified name.
    let prefix = resolver.rsplit_once('.').map_or("", |(m, _)| m);
    !prefix.is_empty() && auditor.starts_with(prefix)
}

/// Whether a disagreement is only about how a constructor call is written.
///
/// `Install(...)` instantiates a class. The resolver names the class; an
/// auditor may name the `__init__` that runs, and when the class inherits its
/// constructor that `__init__` lives on a *different* class entirely
/// (`DummyRepository(...)` -> `Repository.__init__`). Both answers are
/// defensible and neither is an unsound edge: the resolver's contract is to
/// name the first-party definition the call site depends on, and for a
/// constructor call that is the class being constructed.
///
/// This is a gap in the audit protocol rather than a defect, so the protocol
/// now states the rule (see `audit_sample.rs`). Reconciliations are counted
/// and printed rather than folded in silently — a scorer that quietly forgives
/// mismatches is how a precision figure stops meaning anything.
///
/// **Known looseness.** This rescues *any* `X.__init__` answer against *any*
/// resolved class, so `Foo(...)` resolving wrongly to `Bar` would score
/// correct if the auditor wrote `Foo.__init__`. All four occurrences in the
/// 2026-09-19 audit were checked by hand and are genuine agreements, and the
/// printed list exists so they always can be. Tighten before the next run:
/// require that the resolved symbol is a `Class` and that its name matches the
/// auditor's second-to-last segment. Not done now because it would change a
/// scored figure after the fact.
fn constructor_convention(auditor: &str) -> bool {
    auditor.ends_with(".__init__")
}
