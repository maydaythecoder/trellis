//! `callers_of` and `callees_of`: the depth-1 graph queries (ADR 0027),
//! and `coverage_report`, the per-file doubt behind the approval prompt.
//!
//! `blast_radius` answers "what reaches this symbol", transitively. These two
//! answer one step of it in each direction. Both return reach, so CLAUDE.md
//! rule 3 applies to them as it does to the radius: each observation carries
//! its doubt beside it, as counts, never as a ratio (ADR 0002, ADR 0014).
//!
//! Rendered here rather than in the loop, and returned as
//! `ToolResult::Output`: the line numbers need the files, and the files are
//! on the orchestrator's side. The listing goes through the radius's own
//! selector ([`crate::agent::render_listing`]), so the doubt sits in the
//! header the selector never shortens and every call site is listed or
//! counted within `RADIUS_OBSERVATION_BYTES`.

use crate::agent::{ListedSite, MODULE_SCOPE, SiteLocator, render_listing, shown_line};
use crate::tools::ToolResult;
use std::collections::BTreeMap;
use std::path::Path;
use trellis_core::SymbolId;
use trellis_index::{Index, Reason, Target};

/// How many candidates a `callees_of` line names before it counts the rest.
/// A `Candidates` edge can name every override in a hierarchy, and one line
/// per call site is the unit the selector budgets in.
const NAMED_CANDIDATES: usize = 3;

/// How many unbound call sites a `coverage_report` lists before it counts
/// the rest. The counts above the list are the answer; the list is where to
/// start reading.
pub(crate) const COVERAGE_REPORT_SITES: usize = 50;

/// `module.name`, the form `blast_radius` matches against.
pub(crate) fn qualified(index: &Index, id: SymbolId) -> String {
    let s = &index.symbols[id.index()];
    format!(
        "{}.{}",
        index.files[s.file.index()].module,
        index.interner.resolve(s.name)
    )
}

/// The one symbol `symbol` names, or the error a model is shown.
///
/// Shared by `blast_radius`, `callers_of` and `callees_of`, so an unknown or
/// ambiguous name gives the same observation whichever tool was asked.
pub(crate) fn find_symbol(index: &Index, symbol: &str) -> Result<SymbolId, String> {
    let suffix = format!(".{symbol}");
    let matches: Vec<SymbolId> = (0..index.symbols.len())
        .map(SymbolId::from_index)
        .filter(|&id| {
            let q = qualified(index, id);
            q == symbol
                || q.ends_with(&suffix)
                || index.interner.resolve(index.symbols[id.index()].name) == symbol
        })
        .collect();
    match matches.as_slice() {
        [] => Err(format!(
            "no first-party definition matches `{symbol}`; only definitions \
             inside the repository root are indexed"
        )),
        [one] => Ok(*one),
        many => Err(format!(
            "`{symbol}` matches {} definitions; qualify it further",
            many.len()
        )),
    }
}

/// Reasons, most frequent first and then in declaration order, as
/// `reason n` pairs.
fn by_reason(counts: &BTreeMap<Reason, u32>) -> String {
    let mut v: Vec<(Reason, u32)> = counts.iter().map(|(r, n)| (*r, *n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.iter()
        .map(|(r, n)| format!("{r} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The direct callers of `sym`, and the coverage its blast radius carries.
pub(crate) fn callers_of(index: &Index, root: &Path, sym: SymbolId) -> ToolResult {
    let mut locate = SiteLocator::new(index, root);
    let mut sites = Vec::new();
    for edge in index.incoming(sym) {
        let certain = edge.target.is_certain();
        // Every edge in `incoming` comes from a site the same index holds; a
        // missing one is still listed, as `radius_sites` lists it, so the
        // count and the listing cannot disagree.
        let site = match index.sites.get(edge.from.index()) {
            Some(site) => {
                let (path, line) = locate.path_and_line(site);
                ListedSite {
                    path,
                    line,
                    offset: site.span.0,
                    label: locate.enclosing(site),
                    certain,
                }
            }
            None => ListedSite {
                path: "?".to_string(),
                line: None,
                offset: 0,
                label: MODULE_SCOPE.to_string(),
                certain,
            },
        };
        sites.push(site);
    }
    let certain = sites.iter().filter(|s| s.certain).count();
    let ambiguous = sites.len() - certain;

    // The coverage `blast_radius` attaches for this symbol, taken from the
    // same function so the two tools cannot report different doubt for one
    // symbol. Its reach counts are transitive, which is why the header says
    // so: without it the two counts read as a contradiction.
    let c = trellis_index::radius::blast_radius(index, sym).coverage;
    let reasons = if c.by_reason.is_empty() {
        String::new()
    } else {
        format!(", by reason: {}", by_reason(&c.by_reason))
    };
    let header = format!(
        concat!(
            "{} call sites call {} directly: {} certain, {} ambiguous ",
            "(one of several candidates, none chosen).\n\n",
            "Coverage of this symbol's blast radius, as blast_radius reports it. ",
            "It counts the transitive reach, callers of callers included, so it ",
            "is a larger population than the direct callers: {} call sites reach ",
            "this symbol, {} resolved, {} ambiguous.\n",
            "Separately — a different population, not a fraction of the above — ",
            "{} call sites across the {} files that radius touches could not be ",
            "bound to any definition{}. Any of them may also call this symbol, so ",
            "the callers listed here are a floor, not a boundary."
        ),
        sites.len(),
        qualified(index, sym),
        certain,
        ambiguous,
        c.reached(),
        c.reached_resolved,
        c.reached_ambiguous,
        c.unbound_in_touched_files,
        c.touched_files,
        reasons,
    );
    let rendered = render_listing(
        header,
        "\nDirect callers, certain first (path:line enclosing-symbol confidence):",
        &sites,
    );
    ToolResult::Output(rendered.text)
}

/// A callee as `module.name at path:line`. `module.name` alone does not tell
/// two methods called `run` apart; where each is defined does.
fn callee(index: &Index, locate: &mut SiteLocator<'_>, t: SymbolId) -> String {
    let (path, line) = locate.definition(&index.symbols[t.index()]);
    let line = shown_line(line);
    format!("{} at {path}:{line}", qualified(index, t))
}

/// What `sym`'s own call sites bound to, and how many bound to nothing.
///
/// The unbound count is the `DoubtSite` population restricted to this
/// symbol: every call site whose enclosing symbol is `sym` and whose edge is
/// `Unresolved`. Taken from `outgoing`, which holds every edge from such a
/// site, unresolved ones included (`Index::push_edge`), so it is matched by
/// id. Matching `DoubtSite::enclosing` would match by bare name and merge two
/// methods called `run` in different classes.
pub(crate) fn callees_of(index: &Index, root: &Path, sym: SymbolId) -> ToolResult {
    let mut locate = SiteLocator::new(index, root);
    let mut sites = Vec::new();
    let mut unbound: BTreeMap<Reason, u32> = BTreeMap::new();
    for edge in index.outgoing(sym) {
        let (label, certain) = match &edge.target {
            Target::Resolved(t) => (callee(index, &mut locate, *t), true),
            Target::Candidates(ts) => {
                let named: Vec<String> = ts
                    .iter()
                    .take(NAMED_CANDIDATES)
                    .map(|t| callee(index, &mut locate, *t))
                    .collect();
                let rest = ts.len().saturating_sub(NAMED_CANDIDATES);
                let more = if rest > 0 {
                    format!(" and {rest} more")
                } else {
                    String::new()
                };
                (format!("one of {}{more}", named.join(", ")), false)
            }
            Target::Unresolved(r) => {
                *unbound.entry(*r).or_insert(0) += 1;
                continue;
            }
        };
        let (path, line, offset) = match index.sites.get(edge.from.index()) {
            Some(site) => {
                let (path, line) = locate.path_and_line(site);
                (path, line, site.span.0)
            }
            None => ("?".to_string(), None, 0),
        };
        sites.push(ListedSite {
            path,
            line,
            offset,
            label,
            certain,
        });
    }
    let certain = sites.iter().filter(|s| s.certain).count();
    let ambiguous = sites.len() - certain;
    let unbound_total: u32 = unbound.values().sum();
    let reasons = if unbound.is_empty() {
        String::new()
    } else {
        format!(", by reason: {}", by_reason(&unbound))
    };
    let header = format!(
        concat!(
            "{} of {}'s own call sites bound to a first-party definition: ",
            "{} certain, {} ambiguous (one of several candidates, none chosen).\n\n",
            "Separately, {} of its own call sites bound to no definition{}. ",
            "Each of those may call anything, first-party or not, so the calls ",
            "listed here are not everything this symbol calls."
        ),
        sites.len(),
        qualified(index, sym),
        certain,
        ambiguous,
        unbound_total,
        reasons,
    );
    let rendered = render_listing(
        header,
        "\nCalls that bound, certain first (path:line callee confidence):",
        &sites,
    );
    ToolResult::Output(rendered.text)
}

/// The call sites in `paths` that bound to no definition: the population
/// `doubt_in_files` counts for the approval prompt (ADR 0016), as text.
///
/// Counts only. It never prints the index-wide `resolved` figure or any
/// percentage: that figure is a lower bound on recall that reads as recall
/// (.claude/rules/index-soundness.md, "The number that is easy to misread"), and a fraction of it
/// would divide two populations (rule 3). Files named and files examined are
/// both given, so a named file the index never parsed shows as a difference
/// rather than as a file with nothing in doubt.
///
/// The caller has already refused any path outside the repository or into
/// `.git`.
pub(crate) fn coverage_report(index: &Index, root: &Path, paths: &[String]) -> ToolResult {
    let d = trellis_index::doubt_in_files(index, paths);
    let mut text = format!(
        "Python files named: {}. In the index and examined: {}.",
        d.files_named, d.files_examined
    );
    if d.files_examined < d.files_named {
        text.push_str(&format!(
            " Named but not in the index, so nothing is known about their calls: {}.",
            d.files_named - d.files_examined
        ));
    }
    // `doubt_in_files` drops these before it counts, so without this line
    // a named `setup.cfg` would vanish from both counts.
    let skipped = paths.iter().filter(|p| !p.ends_with(".py")).count();
    if skipped > 0 {
        text.push_str(&format!(
            " Paths named that do not end in .py, not counted: {skipped}."
        ));
    }
    let reasons = if d.by_reason.is_empty() {
        String::new()
    } else {
        format!(", by reason: {}", by_reason(&d.by_reason))
    };
    text.push_str(&format!(
        concat!(
            "

{} call sites in the examined files bound to no definition{}. ",
            "Each may call anything, first-party or not. This counts what the ",
            "graph could not follow; it says nothing about how much of the rest ",
            "it followed."
        ),
        d.unbound, reasons
    ));
    if d.sites.is_empty() {
        return ToolResult::Output(text);
    }
    text.push_str(
        "
Call sites bound to no definition (path:line enclosing-symbol reason):",
    );
    let mut locate = SiteLocator::new(index, root);
    for s in d.sites.iter().take(COVERAGE_REPORT_SITES) {
        let line = shown_line(locate.line_in(&s.path, s.offset));
        let enclosing = s.enclosing.as_deref().unwrap_or(MODULE_SCOPE);
        text.push_str(&format!(
            "
{}:{line} {enclosing} {}",
            s.path, s.reason
        ));
    }
    let left_out = d.sites.len().saturating_sub(COVERAGE_REPORT_SITES);
    if left_out > 0 {
        text.push_str(&format!(
            "
{left_out} more not listed; name fewer files to see them."
        ));
    }
    ToolResult::Output(text)
}

#[cfg(test)]
mod tests {
    use crate::agent::{NotConverged, Outcome, RADIUS_OBSERVATION_BYTES, ToolCall, run};
    use crate::budget::Budget;
    use crate::orchestrator::{Handle, Orchestrator};
    use crate::registry::{ToolSelection, ToolSet};
    use crate::scripted::ScriptedModel;
    use crate::tools::ToolResult;
    use trellis_checks::CheckConfig;

    /// `helper` has two direct callers in `go` and one at module level, and
    /// is reached again through `outer -> go`. `go` makes one certain call
    /// twice, one call through a declared base type (`Candidates` over the
    /// base and its override, ADR 0012) and one on an untyped parameter,
    /// which binds to nothing.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            concat!(
                "class Base:\n",
                "    def run(self):\n",
                "        return 1\n\n",
                "class Child(Base):\n",
                "    def run(self):\n",
                "        return 2\n\n",
                "def helper():\n",
                "    return 1\n",
            ),
        )
        .unwrap();
        std::fs::write(
            d.path().join("app/main.py"),
            concat!(
                "from app.util import helper, Base\n\n",
                "def go(thing, b: Base):\n",
                "    helper()\n",
                "    b.run()\n",
                "    thing.run()\n",
                "    return helper()\n\n",
                "def outer():\n",
                "    return go(1, None)\n\n",
                "helper()\n",
            ),
        )
        .unwrap();
        d
    }

    fn enabled() -> ToolSet {
        ToolSet::build(ToolSelection {
            enable: vec!["callers_of".into(), "callees_of".into()],
            ..Default::default()
        })
        .expect("both are opt-in built-ins")
    }

    fn spawn(root: &std::path::Path, tools: ToolSet) -> Handle {
        let (h, _j) = Orchestrator::new(root, CheckConfig::default())
            .with_tools(tools)
            .spawn();
        h
    }

    async fn output(h: &Handle, call: ToolCall) -> String {
        match h.call(&call).await.result {
            ToolResult::Output(s) => s,
            other => panic!("{call:?} gave {other:?}"),
        }
    }

    #[tokio::test]
    async fn callers_of_lists_depth_one_callers_beside_the_radius_coverage_counts() {
        // Reach without its doubt is the failure CLAUDE.md rule 3 names. The
        // coverage block must carry the same counts blast_radius attaches for
        // the symbol, and as counts: a `%` here is a ratio across two
        // populations, the "3.7% resolved" defect again.
        let d = repo();
        let h = spawn(d.path(), enabled());
        let got = output(
            &h,
            ToolCall::CallersOf {
                symbol: "helper".into(),
            },
        )
        .await;
        assert!(!got.contains('%'), "{got}");

        assert!(
            got.starts_with("3 call sites call app.util.helper directly: 3 certain, 0 ambiguous"),
            "{got}"
        );
        assert!(got.contains("\napp/main.py:4 go certain"), "{got}");
        assert!(got.contains("\napp/main.py:7 go certain"), "{got}");
        assert!(got.contains("\napp/main.py:12 <module> certain"), "{got}");
        assert!(
            !got.contains(" outer "),
            "outer reaches helper only through go, which is depth 2: {got}"
        );

        // The doubt block, figure for figure from blast_radius's coverage.
        let c = match h.blast_radius("helper").await {
            ToolResult::Radius(r) => r.coverage,
            other => panic!("{other:?}"),
        };
        assert!(c.unbound_in_touched_files > 0, "thing.run() is unbound");
        let block = format!(
            "{} call sites reach this symbol, {} resolved, {} ambiguous.\n",
            c.reached(),
            c.reached_resolved,
            c.reached_ambiguous
        );
        assert!(got.contains(&block), "{block:?} missing from {got}");
        let unbound = format!(
            "{} call sites across the {} files that radius touches could not be bound",
            c.unbound_in_touched_files, c.touched_files
        );
        assert!(got.contains(&unbound), "{unbound:?} missing from {got}");
        assert!(got.contains("MethodDispatchUntyped"), "{got}");
        assert!(
            got.find("Coverage of this symbol's blast radius") < got.find("\nDirect callers"),
            "the doubt comes before the listing, where the selector never cuts: {got}"
        );
    }

    #[tokio::test]
    async fn callees_of_marks_certain_and_candidate_calls_and_counts_the_unbound_by_reason() {
        // The calls a symbol makes that bound are a floor. The ones that
        // bound to nothing are the DoubtSite entries enclosed by the symbol,
        // and the observation must say how many, by reason, and never as a
        // fraction of anything.
        let d = repo();
        let h = spawn(d.path(), enabled());
        let got = output(
            &h,
            ToolCall::CalleesOf {
                symbol: "app.main.go".into(),
            },
        )
        .await;
        assert!(!got.contains('%'), "{got}");
        assert!(
            got.starts_with(
                "3 of app.main.go's own call sites bound to a first-party definition: \
                 2 certain, 1 ambiguous"
            ),
            "{got}"
        );
        assert!(
            got.contains(
                "\n\nSeparately, 1 of its own call sites bound to no definition, by reason: \
                 MethodDispatchUntyped 1."
            ),
            "{got}"
        );
        assert!(
            got.contains("\napp/main.py:4 app.util.helper at app/util.py:9 certain"),
            "{got}"
        );
        assert!(
            got.contains(
                "\napp/main.py:5 one of app.util.run at app/util.py:2, \
                 app.util.run at app/util.py:6 ambiguous"
            ),
            "{got}"
        );

        // The same population as the approval view's DoubtSite list,
        // restricted to what `go` encloses.
        let orch = Orchestrator::new(d.path(), CheckConfig::default());
        let doubt = trellis_index::radius::doubt_in_files(orch.index(), &["app/main.py".into()]);
        let enclosed = doubt
            .sites
            .iter()
            .filter(|s| s.enclosing.as_deref() == Some("go"))
            .count();
        assert_eq!(enclosed, 1, "{doubt:?}");
    }

    #[tokio::test]
    async fn callers_of_and_callees_of_resolve_a_name_exactly_as_blast_radius_does() {
        // One resolver, so an unknown or ambiguous name reads the same
        // whichever graph tool the model asked.
        let d = repo();
        let h = spawn(d.path(), enabled());
        for symbol in ["nowhere", "run"] {
            let want = match h.blast_radius(symbol).await {
                ToolResult::Error(e) => e,
                other => panic!("{symbol}: {other:?}"),
            };
            for call in [
                ToolCall::CallersOf {
                    symbol: symbol.into(),
                },
                ToolCall::CalleesOf {
                    symbol: symbol.into(),
                },
            ] {
                match h.call(&call).await.result {
                    ToolResult::Error(e) => assert_eq!(e, want, "{call:?}"),
                    other => panic!("{call:?} gave {other:?}"),
                }
            }
        }
    }

    #[tokio::test]
    async fn callers_of_and_callees_of_are_refused_unless_enabled() {
        // Opt-in: a repository with no `[tools]` section is not offered them,
        // and the orchestrator's refusal is the one that binds.
        let d = repo();
        let h = spawn(d.path(), ToolSet::default());
        for call in [
            ToolCall::CallersOf {
                symbol: "helper".into(),
            },
            ToolCall::CalleesOf {
                symbol: "go".into(),
            },
        ] {
            assert!(
                matches!(h.call(&call).await.result, ToolResult::Error(_)),
                "{call:?}"
            );
        }
    }

    #[tokio::test]
    async fn callers_of_many_sites_fits_the_radius_budget_and_counts_what_it_does_not_list() {
        // The listing shares the radius selector: past the budget, entries
        // become per-file counts and nothing is dropped without a number.
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        for f in 0..6 {
            let mut body = String::from("from app.util import helper\n\ndef caller():\n");
            for _ in 0..40 {
                body.push_str("    helper()\n");
            }
            std::fs::write(d.path().join(format!("app/m{f}.py")), body).unwrap();
        }
        let h = spawn(d.path(), enabled());
        let got = output(
            &h,
            ToolCall::CallersOf {
                symbol: "helper".into(),
            },
        )
        .await;
        assert!(got.len() <= RADIUS_OBSERVATION_BYTES, "{} bytes", got.len());
        assert!(!got.contains('%'), "{got}");
        assert!(
            got.starts_with("240 call sites call app.util.helper directly"),
            "{got}"
        );
        assert!(
            got.contains("Coverage of this symbol's blast radius"),
            "{got}"
        );
        let listed = got
            .lines()
            .filter(|l| l.starts_with("app/m") && l.ends_with(" caller certain"))
            .count();
        let summarised: usize = got
            .lines()
            .find_map(|l| {
                l.split_once(" listed; ")
                    .and_then(|(_, rest)| rest.split_once(' '))
                    .and_then(|(n, _)| n.parse().ok())
            })
            .expect("a summary line, since 240 entries cannot fit");
        assert_eq!(listed + summarised, 240, "{got}");
    }

    #[tokio::test]
    async fn a_run_of_only_callers_of_and_callees_of_does_not_converge() {
        // Both return Output or Error, never a Verdict: reading the graph is
        // not evidence the code is right (ADR 0004).
        let d = repo();
        let tools = enabled();
        let h = spawn(d.path(), tools.clone());
        let mut m = ScriptedModel::from_json_with(
            r#"{"steps":[
                {"tool":"callers_of","symbol":"helper"},
                {"tool":"callees_of","symbol":"go"},
                {"tool":"done","summary":"the graph looks fine"}
            ]}"#,
            &tools,
        )
        .expect("valid script");
        let budget = Budget {
            max_iterations: 10,
            ..Budget::default()
        };
        let (outcome, t) = run("look", &mut m, &h, &budget).await;
        assert!(
            matches!(
                outcome,
                Outcome::NotConverged {
                    reason: NotConverged::StoppedWithoutPassingChecks { .. },
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(t.turns.len(), 2);
        assert!(t.turns.iter().all(|t| t.verdict_passed.is_none()));
        assert!(t.turns.iter().all(|t| !t.may_have_changed_tree));
        assert!(
            t.turns[0]
                .observation
                .contains("Coverage of this symbol's blast radius")
        );
        assert!(t.turns[1].observation.contains("bound to no definition"));
    }

    fn with_report() -> ToolSet {
        ToolSet::build(ToolSelection {
            enable: vec!["coverage_report".into()],
            ..Default::default()
        })
        .expect("coverage_report is an opt-in built-in")
    }

    fn report(paths: &[&str]) -> ToolCall {
        ToolCall::CoverageReport {
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn coverage_report_gives_files_named_and_examined_and_the_unbound_count_by_reason() {
        // The two file counts are what make a zero legible: a named file the
        // index never parsed must show as a difference, not as a file with
        // nothing in doubt (the default-FileDoubt defect ADR 0016 records).
        let d = repo();
        let h = spawn(d.path(), with_report());
        let got = output(
            &h,
            report(&["app/main.py", "app/util.py", "app/missing.py", "README.md"]),
        )
        .await;
        assert!(
            got.starts_with(
                "Python files named: 3. In the index and examined: 2. \
                 Named but not in the index, so nothing is known about their calls: 1. \
                 Paths named that do not end in .py, not counted: 1."
            ),
            "{got}"
        );
        assert!(
            got.contains(
                "\n\n1 call sites in the examined files bound to no definition, by reason: \
                 MethodDispatchUntyped 1."
            ),
            "{got}"
        );
        assert!(
            got.contains("\napp/main.py:6 go MethodDispatchUntyped"),
            "{got}"
        );
        assert!(!got.contains("more not listed"), "{got}");

        // The same population the approval prompt counts, not a second one.
        let orch = Orchestrator::new(d.path(), CheckConfig::default());
        let doubt = trellis_index::doubt_in_files(
            orch.index(),
            &["app/main.py".into(), "app/util.py".into()],
        );
        assert_eq!(doubt.unbound, 1, "{doubt:?}");
    }

    #[tokio::test]
    async fn coverage_report_prints_no_percentage_and_no_index_wide_resolved_count() {
        // `trellis index` prints how many call sites the resolver bound. That
        // is a lower bound on recall that reads as recall (CLAUDE.md), and a
        // per-file doubt count beside it invites the ratio rule 3 forbids.
        // The repository is built so the index-wide figure is a number no
        // per-file count or line in the report can be.
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        let mut body = String::from("from app.util import helper\n\ndef caller():\n");
        for _ in 0..37 {
            body.push_str("    helper()\n");
        }
        std::fs::write(d.path().join("app/busy.py"), body).unwrap();
        std::fs::write(
            d.path().join("app/edge.py"),
            "def mystery(thing):\n    return thing.run()\n",
        )
        .unwrap();

        let orch = Orchestrator::new(d.path(), CheckConfig::default());
        let c = &orch.index().coverage;
        assert!(c.resolved >= 37, "{c:?}");
        let (resolved, total) = (c.resolved.to_string(), c.total().to_string());

        let h = spawn(d.path(), with_report());
        let got = output(&h, report(&["app/edge.py"])).await;
        assert!(
            got.contains("\napp/edge.py:2 mystery MethodDispatchUntyped"),
            "{got}"
        );
        assert!(!got.contains('%'), "{got}");
        assert!(!got.to_lowercase().contains("resolved"), "{got}");
        assert!(!got.contains("complete"), "{got}");
        for figure in [&resolved, &total] {
            assert!(
                !got.split(|ch: char| !ch.is_ascii_digit())
                    .any(|n| n == figure.as_str()),
                "index-wide figure {figure} in {got}"
            );
        }
    }

    #[tokio::test]
    async fn coverage_report_refuses_a_path_outside_the_repository_or_into_git_as_read_file_does() {
        // The report names files and lines. A path read_file would refuse
        // must be refused here too, with the same reason, and never answered
        // with a zero that reads as "nothing in doubt there".
        let d = repo();
        let h = spawn(d.path(), with_report());
        let absolute = d.path().join("app/main.py").to_string_lossy().into_owned();
        for bad in [
            "../elsewhere.py",
            ".git/config",
            ".git/hooks/x.py",
            absolute.as_str(),
        ] {
            let want = match h
                .call(&ToolCall::ReadFile { path: bad.into() })
                .await
                .result
            {
                ToolResult::Error(e) => {
                    e.replacen("read_file refused", "coverage_report refused", 1)
                }
                other => panic!("read_file {bad} gave {other:?}"),
            };
            assert!(want.starts_with("coverage_report refused: "), "{want}");
            match h.call(&report(&["app/main.py", bad])).await.result {
                ToolResult::Error(e) => assert_eq!(e, want, "{bad}"),
                other => panic!("coverage_report {bad} gave {other:?}"),
            }
        }
        assert!(matches!(
            h.call(&report(&[])).await.result,
            ToolResult::Error(e) if e.contains("at least one path")
        ));
    }

    #[tokio::test]
    async fn coverage_report_lists_at_most_fifty_sites_counts_the_rest_and_is_shown_whole() {
        // Fifty listed lines overrun the tail window every other observation
        // is cut to. Cut from the top, the model would keep the sites and
        // lose the counts, which is the doubt figure dropped without a word.
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        let mut body = String::from("def a_function_with_a_long_descriptive_name(thing):\n");
        for _ in 0..61 {
            body.push_str("    thing.run()\n");
        }
        std::fs::write(
            d.path()
                .join("app/a_module_with_a_long_descriptive_name.py"),
            body,
        )
        .unwrap();
        let h = spawn(d.path(), with_report());
        let call = report(&["app/a_module_with_a_long_descriptive_name.py"]);
        let got = output(&h, call.clone()).await;

        let listed = got
            .lines()
            .filter(|l| {
                l.ends_with(" a_function_with_a_long_descriptive_name MethodDispatchUntyped")
            })
            .count();
        assert_eq!(listed, crate::graph_tools::COVERAGE_REPORT_SITES, "{got}");
        assert!(
            got.ends_with("\n11 more not listed; name fewer files to see them."),
            "{got}"
        );
        assert!(
            got.contains("\n\n61 call sites in the examined files bound to no definition"),
            "{got}"
        );

        assert!(
            got.len() > crate::adapter::OBSERVATION_TAIL,
            "{} bytes",
            got.len()
        );
        assert_eq!(crate::adapter::shown_observation(&call, &got), got);
    }

    #[tokio::test]
    async fn a_run_of_only_coverage_report_does_not_converge() {
        // Counting doubt is not evidence the code is right (ADR 0004): the
        // tool returns Output or Error, never a Verdict.
        let d = repo();
        let tools = with_report();
        let h = spawn(d.path(), tools.clone());
        let mut m = ScriptedModel::from_json_with(
            r#"{"steps":[
                {"tool":"coverage_report","paths":["app/main.py"]},
                {"tool":"done","summary":"one unbound call, looks fine"}
            ]}"#,
            &tools,
        )
        .expect("valid script");
        let budget = Budget {
            max_iterations: 10,
            ..Budget::default()
        };
        let (outcome, t) = run("look", &mut m, &h, &budget).await;
        assert!(
            matches!(
                outcome,
                Outcome::NotConverged {
                    reason: NotConverged::StoppedWithoutPassingChecks { .. },
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(t.turns.len(), 1);
        assert_eq!(t.turns[0].verdict_passed, None);
        assert!(!t.turns[0].may_have_changed_tree);
        assert!(t.turns[0].observation.contains("bound to no definition"));
    }
}
