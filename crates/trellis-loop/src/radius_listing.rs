//! A blast radius rendered for the model as a byte-capped site listing
//! (ADR 0033).
//!
//! Its own module because it changes for its own reasons: the selector's
//! budget and line shapes, not the loop's control flow. The coverage header
//! is written first and never shortened, so the doubt sits beside the reach
//! as two counts and never a ratio (CLAUDE.md rule 3, ADR 0002, ADR 0014).

/// The byte ceiling on one rendered radius observation: coverage header,
/// entry listing and summary together.
///
/// Taken from the observation cap ([`crate::adapter::OBSERVATION_TAIL`]),
/// not converted from the 4,096-token minimum window (ADR 0033). The
/// adapter keeps an observation's last 2,000 characters and cuts from the
/// top, and the top of a radius is its coverage header, so a listing sized
/// past the cap would lose the doubt figure without saying so.
///
/// Revisited after #170 measured the window (#228) and kept: the prompt's
/// own budget, [`crate::adapter::LOCAL_CONTEXT_BYTES`], now elides older
/// observations to make room for the newest, and a radius is never cut to
/// fit it; a prompt that cannot hold one whole is not sent.
pub(crate) const RADIUS_OBSERVATION_BYTES: usize = crate::adapter::OBSERVATION_TAIL;

// `shown_observation` cuts when `len() > OBSERVATION_TAIL`, in bytes. A
// radius at or under the cap is shown whole.
const _: () = assert!(RADIUS_OBSERVATION_BYTES <= crate::adapter::OBSERVATION_TAIL);

/// One call site that reaches a radius's root, resolved to where a reader
/// can find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadiusSite {
    /// Repository-relative path, as the index recorded it.
    pub path: String,
    /// 1-based line of the call. `None` when the file could not be read or
    /// the offset lies past its end, which a model is shown as `?`.
    pub line: Option<usize>,
    /// Byte offset of the call, the last ordering key: two calls on one
    /// line must still come out in the same order every time.
    pub offset: usize,
    /// The enclosing symbol's name, `<module>` for top-level code.
    pub enclosing: String,
    /// Reached through a `Resolved` edge, rather than one of several
    /// `Candidates`.
    pub certain: bool,
}

/// The radius's call sites, one per entry in `report`, with paths, lines and
/// enclosing symbols filled in from the index and the files under `root`.
///
/// A file that cannot be read gives `?` lines rather than an error: the
/// radius itself came from the index and is still true, and refusing it
/// because a line number is unknown would hide the reach and its coverage.
pub(crate) fn radius_sites(
    index: &trellis_index::Index,
    root: &std::path::Path,
    report: &trellis_index::radius::RadiusReport,
) -> Vec<RadiusSite> {
    let mut locate = SiteLocator::new(index, root);
    let entries = report
        .certain
        .iter()
        .map(|e| (e, true))
        .chain(report.ambiguous.iter().map(|e| (e, false)));
    entries
        .map(|(e, certain)| {
            let Some(site) = index.sites.get(e.site.index()) else {
                // Not reachable from a report the same index produced; kept
                // as an entry so the listing still accounts for it.
                return RadiusSite {
                    path: "?".to_string(),
                    line: None,
                    offset: 0,
                    enclosing: MODULE_SCOPE.to_string(),
                    certain,
                };
            };
            let (path, line) = locate.path_and_line(site);
            RadiusSite {
                path,
                line,
                offset: site.span.0,
                enclosing: locate.enclosing(site),
                certain,
            }
        })
        .collect()
}

/// Turns a call site into what a reader can find: its path, its 1-based
/// line, and the name of the symbol whose body holds it. Each file is read
/// at most once, and a file that cannot be read gives a `None` line rather
/// than an error (see [`radius_sites`]).
pub(crate) struct SiteLocator<'a> {
    index: &'a trellis_index::Index,
    root: &'a std::path::Path,
    sources: std::collections::HashMap<usize, Option<Vec<u8>>>,
}

impl<'a> SiteLocator<'a> {
    pub(crate) fn new(index: &'a trellis_index::Index, root: &'a std::path::Path) -> Self {
        Self {
            index,
            root,
            sources: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn path_and_line(
        &mut self,
        site: &trellis_index::CallSiteEntry,
    ) -> (String, Option<usize>) {
        self.at(site.file.index(), site.span.0)
    }

    /// Where a definition is, for a reader who wants to open it.
    pub(crate) fn definition(
        &mut self,
        sym: &trellis_index::SymbolEntry,
    ) -> (String, Option<usize>) {
        self.at(sym.file.index(), sym.span.0)
    }

    /// The 1-based line of `offset` in the indexed file at `path`, for
    /// callers that hold a path rather than an id (`DoubtSite`). `None` when
    /// the index holds no such file or it cannot be read.
    pub(crate) fn line_in(&mut self, path: &str, offset: usize) -> Option<usize> {
        let file = self.index.files.iter().position(|f| f.path == path)?;
        self.at(file, offset).1
    }

    fn at(&mut self, file: usize, offset: usize) -> (String, Option<usize>) {
        let path = self
            .index
            .files
            .get(file)
            .map_or_else(|| "?".to_string(), |f| f.path.clone());
        let root = self.root;
        let bytes = self
            .sources
            .entry(file)
            .or_insert_with(|| std::fs::read(root.join(&path)).ok());
        let line = bytes
            .as_deref()
            .filter(|b| offset <= b.len())
            .map(|b| 1 + b[..offset].iter().filter(|&&c| c == b'\n').count());
        (path, line)
    }

    /// The enclosing symbol's name, `<module>` for top-level code.
    pub(crate) fn enclosing(&self, site: &trellis_index::CallSiteEntry) -> String {
        site.enclosing
            .and_then(|s| self.index.symbols.get(s.index()))
            .map_or_else(
                || MODULE_SCOPE.to_string(),
                |s| self.index.interner.resolve(s.name).to_string(),
            )
    }
}

/// The scope named for a call site outside any function or class. Shared
/// so the radius listing and the graph tools name it the same way.
pub(crate) const MODULE_SCOPE: &str = "<module>";

/// A 1-based line number for a listing, `?` when the source could not be
/// read to find it.
pub(crate) fn shown_line(line: Option<usize>) -> String {
    line.map_or_else(|| "?".to_string(), |n| n.to_string())
}

/// A rendered radius and what the selector did to fit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SiteListing {
    pub text: String,
    /// Call sites shown one per line.
    pub listed: u32,
    /// Call sites shown only as a count, per file or in the final line.
    /// `listed + summarised` is always the number of sites given.
    pub summarised: u32,
}

/// The coverage text every radius observation opens with, unchanged by the
/// selector.
fn radius_header(report: &trellis_index::radius::RadiusReport) -> String {
    // Two facts, never a ratio between them. The previous version of this
    // string handed the model "92 of 2474 first-party call sites resolved
    // (3.7%)" — a division across two different populations that reads as
    // the resolver being useless. A model given that has every reason to
    // disregard the radius entirely, which is the opposite of what S2's
    // honesty mechanism is for.
    let c = &report.coverage;
    let top: Vec<String> = c
        .reasons_by_frequency()
        .into_iter()
        .take(3)
        .map(|(r, n)| format!("{r} {n}"))
        .collect();
    format!(
        concat!(
            "{} call sites reach this symbol: {} resolved, {} ambiguous.\n",
            "Separately — a different population, not a fraction of the ",
            "above — {} call sites across the {} files this radius touches ",
            "could not be bound to any definition{}{}.\n",
            "Any of them may also reach this symbol. This is not a complete ",
            "list of callers; roughly three call edges in ten are statically ",
            "invisible in Python, so treat the resolved list as a floor, not ",
            "a boundary."
        ),
        report.site_count(),
        report.certain.len(),
        report.ambiguous.len(),
        c.unbound_in_touched_files,
        c.touched_files,
        if top.is_empty() { "" } else { ", mostly " },
        top.join(", "),
    )
}

fn confidence(certain: bool) -> &'static str {
    if certain { "certain" } else { "ambiguous" }
}

/// One entry of a sized call-site listing: where the call is, what the
/// middle column says about it, and whether its edge was `Resolved`.
///
/// The radius, `callers_of` and `callees_of` share the selector below, so
/// the one rule (nothing dropped without a count) is written once. Only the
/// middle column differs: the enclosing symbol for a caller, the callee for
/// a call a symbol makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedSite {
    pub path: String,
    pub line: Option<usize>,
    pub offset: usize,
    pub label: String,
    pub certain: bool,
}

impl From<&RadiusSite> for ListedSite {
    fn from(s: &RadiusSite) -> Self {
        Self {
            path: s.path.clone(),
            line: s.line,
            offset: s.offset,
            label: s.enclosing.clone(),
            certain: s.certain,
        }
    }
}

fn site_line(s: &ListedSite) -> String {
    let line = shown_line(s.line);
    format!(
        "\n{}:{} {} {}",
        s.path,
        line,
        s.label,
        confidence(s.certain)
    )
}

fn summary_total_line(listed: usize, summarised: usize, certain: u32, ambiguous: u32) -> String {
    format!(
        "\n{listed} listed; {summarised} more summarised by file \
         ({certain} certain, {ambiguous} ambiguous):"
    )
}

fn file_line(path: &str, (certain, ambiguous): (u32, u32)) -> String {
    format!(
        "\n{path}: {} more ({certain} certain, {ambiguous} ambiguous)",
        certain + ambiguous
    )
}

fn collapsed_line(sites: u32, files: usize) -> String {
    format!("\nand {sites} more call sites in {files} files")
}

/// The radius observation, sized to [`RADIUS_OBSERVATION_BYTES`].
pub(crate) fn render_radius(
    report: &trellis_index::radius::RadiusReport,
    sites: &[RadiusSite],
) -> SiteListing {
    let listed: Vec<ListedSite> = sites.iter().map(ListedSite::from).collect();
    render_listing(
        radius_header(report),
        "\nCall sites, certain first (path:line enclosing-symbol confidence):",
        &listed,
    )
}

/// `header`, then `heading` and the sites, sized to
/// [`RADIUS_OBSERVATION_BYTES`].
///
/// The header comes first and is never shortened: it is where every caller
/// puts the doubt. Then the call sites, one per line, certain before
/// ambiguous, then by path, line and offset. When they do not all fit, the
/// tail of that order is replaced by a count per file; the per-file counts
/// take precedence over individual entries, so fewer entries are listed
/// until every remaining file has its line. Only when the per-file lines
/// cannot all fit even with no entry listed do the files left over collapse
/// into one final count. Every call site is either listed or counted:
/// nothing is dropped without a number (CLAUDE.md rule 3).
pub(crate) fn render_listing(header: String, heading: &str, sites: &[ListedSite]) -> SiteListing {
    let mut sorted: Vec<&ListedSite> = sites.iter().collect();
    sorted.sort_by(|a, b| {
        b.certain
            .cmp(&a.certain)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.offset.cmp(&b.offset))
    });

    let mut text = header;
    if sorted.is_empty() {
        return SiteListing {
            text,
            listed: 0,
            summarised: 0,
        };
    }
    text.push_str(heading);

    let lines: Vec<String> = sorted.iter().map(|s| site_line(s)).collect();
    let listing: usize = lines.iter().map(String::len).sum();
    if text.len() + listing <= RADIUS_OBSERVATION_BYTES {
        for l in &lines {
            text.push_str(l);
        }
        return SiteListing {
            text,
            listed: sorted.len() as u32,
            summarised: 0,
        };
    }

    // Walk `listed` down from all-but-one, moving one entry at a time into
    // the per-file summary, and stop at the first split whose full summary
    // fits. The summary's length is kept up to date incrementally, so a
    // radius of thousands of sites costs one pass, not one pass per split.
    let n = sorted.len();
    let mut prefix = vec![0usize; n + 1];
    for (i, l) in lines.iter().enumerate() {
        prefix[i + 1] = prefix[i] + l.len();
    }
    let mut per_file: std::collections::BTreeMap<&str, (u32, u32)> =
        std::collections::BTreeMap::new();
    let mut file_lines_len = 0usize;
    let (mut rest_certain, mut rest_ambiguous) = (0u32, 0u32);
    let mut chosen = None;
    for listed in (0..n).rev() {
        let s = sorted[listed];
        let counts = per_file.entry(s.path.as_str()).or_insert((0, 0));
        if counts.0 + counts.1 > 0 {
            file_lines_len -= file_line(&s.path, *counts).len();
        }
        if s.certain {
            counts.0 += 1;
            rest_certain += 1;
        } else {
            counts.1 += 1;
            rest_ambiguous += 1;
        }
        file_lines_len += file_line(&s.path, *counts).len();
        let total = summary_total_line(listed, n - listed, rest_certain, rest_ambiguous);
        if text.len() + prefix[listed] + total.len() + file_lines_len <= RADIUS_OBSERVATION_BYTES {
            chosen = Some(listed);
            break;
        }
    }

    let listed = chosen.unwrap_or(0);
    for l in &lines[..listed] {
        text.push_str(l);
    }
    text.push_str(&summary_total_line(
        listed,
        n - listed,
        rest_certain,
        rest_ambiguous,
    ));
    if chosen.is_some() {
        for (path, counts) in &per_file {
            text.push_str(&file_line(path, *counts));
        }
    } else {
        // Even with nothing listed the per-file lines do not fit. Files are
        // given their line in path order while the line and a count of
        // everything after it still fit; the rest go into one final line,
        // whose room is checked before each file is added so it is never
        // the part that gets cut.
        let files: Vec<(&str, (u32, u32))> = per_file.into_iter().collect();
        let mut left_sites: u32 = files.iter().map(|(_, (c, a))| c + a).sum();
        let mut shown = 0;
        for (i, (path, counts)) in files.iter().enumerate() {
            let line = file_line(path, *counts);
            let after_sites = left_sites - (counts.0 + counts.1);
            let after_files = files.len() - i - 1;
            let reserve = if after_files == 0 {
                0
            } else {
                collapsed_line(after_sites, after_files).len()
            };
            if text.len() + line.len() + reserve > RADIUS_OBSERVATION_BYTES {
                break;
            }
            text.push_str(&line);
            left_sites = after_sites;
            shown += 1;
        }
        if shown < files.len() {
            text.push_str(&collapsed_line(left_sites, files.len() - shown));
        }
    }
    SiteListing {
        text,
        listed: listed as u32,
        summarised: (n - listed) as u32,
    }
}

/// The radius selector (ADR 0033). Every test here renders through
/// `render_radius` and holds the same two properties: the observation fits
/// `RADIUS_OBSERVATION_BYTES`, so the adapter never cuts its coverage header
/// off, and every call site is either listed or counted.
#[cfg(test)]
mod selector_tests {
    use super::*;
    use crate::agent::ToolCall;
    use trellis_core::{CallSiteId, SymbolId};
    use trellis_index::radius::{RadiusCoverage, RadiusEntry};
    use trellis_index::{RadiusReport, Reason};

    fn site(path: &str, line: usize, enclosing: &str, certain: bool) -> RadiusSite {
        RadiusSite {
            path: path.to_string(),
            line: Some(line),
            offset: line * 10,
            enclosing: enclosing.to_string(),
            certain,
        }
    }

    /// A report whose counts match `sites`, as the orchestrator's would.
    fn report_for(sites: &[RadiusSite], coverage: RadiusCoverage) -> RadiusReport {
        let entry = |i: usize, certain: bool| RadiusEntry {
            site: CallSiteId::from_index(i),
            certain,
        };
        RadiusReport {
            root: SymbolId::from_index(0),
            certain: (0..sites.iter().filter(|s| s.certain).count())
                .map(|i| entry(i, true))
                .collect(),
            ambiguous: (0..sites.iter().filter(|s| !s.certain).count())
                .map(|i| entry(i, false))
                .collect(),
            symbols: Vec::new(),
            coverage,
            partitionable: false,
        }
    }

    fn coverage() -> RadiusCoverage {
        let mut c = RadiusCoverage {
            unbound_in_touched_files: 41,
            touched_files: 7,
            ..RadiusCoverage::default()
        };
        c.by_reason.insert(Reason::MethodDispatchUntyped, 30);
        c.by_reason.insert(Reason::UnboundName, 11);
        c
    }

    /// What the observation accounts for, read back from its text: entry
    /// lines, the per-file counts, the collapsed count, and the stated
    /// summarised total. Parsed rather than taken from `SiteListing`,
    /// because the model reads the text and the text is what must add up.
    struct Accounted {
        listed: usize,
        per_file: usize,
        per_file_lines: usize,
        collapsed: usize,
        stated_summarised: Option<usize>,
    }

    fn account(text: &str) -> Accounted {
        let mut a = Accounted {
            listed: 0,
            per_file: 0,
            per_file_lines: 0,
            collapsed: 0,
            stated_summarised: None,
        };
        let mut in_listing = false;
        for line in text.lines() {
            if line.starts_with("Call sites, certain first") {
                in_listing = true;
            } else if let Some(rest) = line.strip_prefix("and ") {
                a.collapsed += rest.split(' ').next().unwrap().parse::<usize>().unwrap();
            } else if line.contains(" listed; ") {
                in_listing = false;
                let n = line.split(" listed; ").nth(1).unwrap();
                a.stated_summarised = Some(n.split(' ').next().unwrap().parse().unwrap());
            } else if in_listing {
                a.listed += 1;
            } else if let Some((_, rest)) = line.rsplit_once(": ")
                && rest.contains(" more (")
            {
                a.per_file += rest.split(' ').next().unwrap().parse::<usize>().unwrap();
                a.per_file_lines += 1;
            }
        }
        a
    }

    fn assert_shown_whole(text: &str) {
        assert!(
            text.len() <= RADIUS_OBSERVATION_BYTES,
            "{} bytes over a budget of {RADIUS_OBSERVATION_BYTES}",
            text.len()
        );
        let call = ToolCall::BlastRadius {
            symbol: "helper".into(),
        };
        let shown = crate::adapter::shown_observation(&call, text);
        assert_eq!(
            shown, text,
            "the adapter cut a radius, and the top it cuts is the coverage header"
        );
        assert!(!shown.contains("...(truncated)..."));
    }

    #[test]
    fn selector_budget_under_budget_lists_every_entry() {
        // Given out of order: the listing's order must not depend on the
        // order the index walked its edges in.
        let sites = vec![
            site("pkg/b.py", 9, "later", false),
            site("pkg/b.py", 3, "go", true),
            site("pkg/a.py", 12, "<module>", true),
        ];
        let report = report_for(&sites, coverage());
        let r = render_radius(&report, &sites);

        assert_shown_whole(&r.text);
        assert!(
            r.text.starts_with(&radius_header(&report)),
            "the coverage text comes first, unchanged: {}",
            r.text
        );
        assert!(
            r.text.contains("41 call sites across the 7 files"),
            "the doubt figure is in the observation, as a count: {}",
            r.text
        );
        let listing: Vec<&str> = r.text.lines().skip(4).collect();
        assert_eq!(
            listing,
            [
                "pkg/a.py:12 <module> certain",
                "pkg/b.py:3 go certain",
                "pkg/b.py:9 later ambiguous",
            ],
            "certain before ambiguous, then path, then line"
        );
        assert!(!r.text.contains("summarised"), "{}", r.text);
        assert_eq!((r.listed, r.summarised), (3, 0));
    }

    #[test]
    fn selector_budget_over_budget_summarises_with_counts() {
        // 240 call sites over 12 files: far past the budget one per line.
        let sites: Vec<RadiusSite> = (0..240)
            .map(|i| {
                site(
                    &format!("src/project/module_{:02}.py", i % 12),
                    i + 1,
                    "caller",
                    i % 5 != 0,
                )
            })
            .collect();
        let report = report_for(&sites, coverage());
        let r = render_radius(&report, &sites);

        assert_shown_whole(&r.text);
        assert!(
            r.text.starts_with(&radius_header(&report)),
            "the coverage header survives a summarised radius: {}",
            r.text
        );
        assert!(r.summarised > 0, "this radius does not fit unsummarised");
        assert!(
            r.text
                .contains("src/project/module_11.py: 20 more (16 certain, 4 ambiguous)"),
            "per-file counts: {}",
            r.text
        );

        let a = account(&r.text);
        assert_eq!(
            a.listed + a.per_file + a.collapsed,
            report.site_count(),
            "every call site listed or counted, none dropped: {}",
            r.text
        );
        assert_eq!(a.listed, r.listed as usize);
        assert_eq!(a.stated_summarised, Some(r.summarised as usize));
        assert_eq!(a.per_file + a.collapsed, r.summarised as usize);
        assert_eq!(a.collapsed, 0, "12 file lines fit; none collapse");
        assert_eq!(a.per_file_lines, 12);
    }

    #[test]
    fn selector_budget_collapses_files_that_do_not_fit_into_one_counted_line() {
        // One call site in each of 400 files with long paths: the per-file
        // lines alone are several times the budget, so the files left over
        // must end up in one final count rather than off the end.
        let sites: Vec<RadiusSite> = (0..400)
            .map(|i| {
                site(
                    &format!("src/a/fairly/deep/package/tree/module_{i:03}.py"),
                    1,
                    "f",
                    i % 2 == 0,
                )
            })
            .collect();
        let report = report_for(&sites, coverage());
        let r = render_radius(&report, &sites);

        assert_shown_whole(&r.text);
        let a = account(&r.text);
        assert_eq!(a.listed, 0, "no room for entries once file counts win");
        assert!(a.per_file_lines > 0, "some files still get their line");
        assert!(a.collapsed > 0, "{}", r.text);
        assert_eq!(a.listed + a.per_file + a.collapsed, report.site_count());
        assert!(
            r.text
                .lines()
                .last()
                .is_some_and(|l| l.starts_with("and ") && l.ends_with(" files")),
            "the final count is the last line, whole: {}",
            r.text
        );
        assert_eq!(r.summarised, 400);
    }

    #[test]
    fn selector_budget_worst_case_header_and_summary_fit() {
        // The summary is never cut, which is only true if the header, the
        // listing label, the summarised total and the collapsed line fit
        // together for any counts. The header's length is bounded: fixed
        // prose, three reason names and their counts, and five numbers.
        // Every reason at u32::MAX (three names chosen by the tie-break,
        // each within a few bytes of the longest, against a margin of well
        // over a thousand), and every count widened to 20 digits.
        let mut c = RadiusCoverage {
            unbound_in_touched_files: u32::MAX,
            touched_files: u32::MAX,
            ..RadiusCoverage::default()
        };
        for r in [
            Reason::MethodDispatchUntyped,
            Reason::DecoratorIndirection,
            Reason::StarImport,
            Reason::DynamicAttribute,
            Reason::ReexportUnfollowed,
            Reason::ShadowedAmbiguous,
            Reason::LocalBinding,
            Reason::UnboundName,
        ] {
            c.by_reason.insert(r, u32::MAX);
        }
        let report = report_for(&[], c);
        // The three site counts in the header are 1 digit here; widen each
        // to usize::MAX's 20.
        let header = radius_header(&report).len() + 3 * 19;
        let label = "\nCall sites, certain first (path:line enclosing-symbol confidence):".len();
        let total = summary_total_line(usize::MAX, usize::MAX, u32::MAX, u32::MAX).len();
        let collapsed = collapsed_line(u32::MAX, usize::MAX).len();
        assert!(
            header + label + total + collapsed <= RADIUS_OBSERVATION_BYTES,
            "{header} + {label} + {total} + {collapsed} bytes"
        );
    }

    #[test]
    fn radius_sites_name_the_path_line_and_enclosing_symbol() {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("app/main.py"),
            "from app.util import helper\n\ndef go():\n    return helper()\n\nhelper()\n",
        )
        .unwrap();
        let orch = crate::orchestrator::Orchestrator::new(
            d.path(),
            trellis_checks::CheckConfig::default(),
        );
        let index = orch.index();
        let root = index
            .symbols
            .iter()
            .position(|s| index.interner.resolve(s.name) == "helper")
            .map(SymbolId::from_index)
            .expect("helper is indexed");
        let report = trellis_index::radius::blast_radius(index, root);

        let mut got = radius_sites(index, d.path(), &report);
        got.sort_by_key(|s| s.line);
        let got: Vec<_> = got
            .iter()
            .map(|s| (s.path.as_str(), s.line, s.enclosing.as_str(), s.certain))
            .collect();
        assert_eq!(
            got,
            [
                ("app/main.py", Some(4), "go", true),
                ("app/main.py", Some(6), "<module>", true),
            ]
        );

        // A file that cannot be read gives `?`, not an error: the radius
        // came from the index and is still true.
        std::fs::remove_file(d.path().join("app/main.py")).unwrap();
        let unread = radius_sites(index, d.path(), &report);
        assert!(unread.iter().all(|s| s.line.is_none()));
        let r = render_radius(&report, &unread);
        assert!(r.text.contains("app/main.py:? go certain"), "{}", r.text);
    }
}
