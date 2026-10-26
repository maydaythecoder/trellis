//! The Atlas view: a map of the repository's modules over its history.
//!
//! The owner asked for it on 2026-10-11 ("the current view is nice in the
//! trellis atlas, could you add that to the current view without removing
//! the other ones"), from a prototype drawn on exported data. Here it is
//! drawn from `trellis rpc` answers, and the Codebase view beside it is
//! unchanged.
//!
//! Four read-only answers feed it, each asked by `ui/atlas.js` with an id
//! that starts with [`ID_PREFIX`]: `get_history` (git's churn, commits and
//! tags, read by trellis through `trellis_vcs`), `get_codebase` with
//! `edges: "all"` at HEAD and with `ref` at up to [`SNAPSHOTS_MAX`] earlier
//! commits, `get_symbols` at HEAD, and `get_radius` for a symbol the user
//! selects. Each is answered on its own thread, so an earlier question's
//! answer can come after a later one's; only the answer to the last
//! question of each kind is taken (one per snapshot for `get_codebase`), as
//! the vault and queue views do. The reducer (`state.rs`) keeps the result
//! and the `view` event carries it under `atlas`; a new child clears it
//! with the rest of the view.
//!
//! What is decided here, where `cargo test` holds it, and not in the page
//! (ADR 0048 section 5):
//!
//! - **Which snapshots.** From the history answer, up to [`SNAPSHOTS_MAX`]
//!   of the tags dated in the two years before HEAD's date, spread across
//!   them (the newest, the oldest and evenly between), or with none the
//!   commits returned, then HEAD. Each is asked by its commit, never by a
//!   name the repository chose.
//! - **The layout.** An ordered binary treemap over the real path tree
//!   ([`layout`]): directories, then module files, each by name; a chain of
//!   directories holding one directory each is one region; the top-level
//!   regions sit apart by a wider gutter, so source and tests read as two
//!   blocks. A module's area follows its `defines` at the newest loaded
//!   snapshot holding it, with a floor of [`FLOOR`] so a module that
//!   defines nothing is still a target. Region headers, padding and gaps
//!   take area no module gets, so each of [`PASSES`] passes charges that
//!   area back to its directory as extra weight, and a module's area
//!   follows its own weight rather than how deep its package sits. The
//!   layout is computed over HEAD as soon as HEAD arrives, then once more
//!   over the union of every snapshot loaded when the last one has
//!   settled, and is then fixed for this root: a module keeps its
//!   rectangle in every snapshot, and one absent from a snapshot keeps its
//!   slot as a ghost. Rectangles are normalised to 0..1 on each axis of a
//!   world [`WORLD_W`] by [`WORLD_H`] wide.
//! - **Symbols.** HEAD's definitions from `get_symbols`, nested by their
//!   line ranges (a method inside its class) and laid out inside their
//!   module's rectangle the same way, by their length in lines.
//! - **The figures.** Per snapshot and per module: the three call-site
//!   counts as trellis gave them, and the call sites bound into and out of
//!   it from other modules with how many modules, summed from the answer's
//!   edges and labelled derived on the page. Per region, the same summed
//!   over its modules. Bound, ambiguous and unresolved stay three counts,
//!   and reach and doubt two blocks: nothing here divides one by another
//!   (CLAUDE.md rule 3, ADR 0014), and a figure an answer did not carry is
//!   `null`, never 0.
//!
//! The page owns only what is the page's: zoom and pan, the lens, which
//! snapshot and baseline are shown, the selection, which labels fit at the
//! zoom drawn, and edge widths summed for drawing at that zoom.
//!
//! Module names, paths, tag names, commit subjects and symbol names are
//! repository or git text: the page writes them through `textContent` only
//! (ADR 0007, S10).

use crate::asked::{self, Asked};
use crate::frames::Response;
use crate::graph::round;
use crate::json;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Every id the Atlas asks with starts with this, so its answers are told
/// from the Codebase view's and the radius pane's.
pub const ID_PREFIX: &str = "desktop-atlas-";

/// The most snapshots loaded besides HEAD: the prototype's six, which keep
/// the scrubber's ticks readable at the window's minimum width.
pub const SNAPSHOTS_MAX: usize = 6;

/// The least weight a module is laid out with, in definitions: a module
/// that defines nothing (an `__init__.py`, a script) still gets a
/// rectangle a pointer can hit.
pub const FLOOR: u64 = 4;

/// The world the layout is computed in, before it is normalised. Its
/// aspect, 1.4, is the prototype's, near a desktop pane's.
pub const WORLD_W: f64 = 1400.0;
pub const WORLD_H: f64 = 1000.0;

/// A region's header height by depth, in world units; the root has none.
const HEADER: [f64; 6] = [0.0, 30.0, 20.0, 15.0, 12.0, 10.0];
const PAD: f64 = 3.0;
const GAP: f64 = 3.0;
/// The gap between top-level regions: wider than any other, so source and
/// tests sit apart.
const GUTTER: f64 = 36.0;
/// Charge-back passes. Six settle the poetry fixture's density to well
/// inside the tolerance its test states.
pub const PASSES: usize = 6;

/// Whether `id` is one the Atlas asked with.
pub fn is_atlas_id(id: Option<&str>) -> bool {
    id.is_some_and(|i| i.starts_with(ID_PREFIX))
}

/// A path as trellis gives one: relative to the root, `/`-separated, with
/// no empty, `.` or `..` segment. Anything else is not drawn, since the
/// layout's tree is built from these segments.
fn repository_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\\')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// A commit id as git prints one: hex, abbreviated or full (SHA-1 or
/// SHA-256). Only such an id is sent back to trellis as a `ref`.
fn is_commit(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// --- answers, as read -----------------------------------------------------

/// One module of a `get_codebase` answer.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    module: String,
    path: String,
    defines: Option<u64>,
    ambiguous: Option<u64>,
    unresolved: Option<u64>,
    calls_not_drawn: Option<u64>,
}

/// A `get_codebase` answer, typed: its modules, and the edges between them
/// as `(caller, callee, call sites)` over the modules' indices.
#[derive(Debug, Clone, PartialEq)]
struct Codebase {
    rows: Vec<Row>,
    edges: Vec<(usize, usize, u64)>,
    modules_indexed: Option<u64>,
    bound_call_sites: Option<u64>,
    ambiguous: Option<u64>,
    unresolved: Option<u64>,
    modules_not_drawn: Option<u64>,
    commit: Option<String>,
    date: Option<String>,
    /// Modules the answer listed that are not drawn: no module name, or a
    /// path that is not a repository path.
    rows_skipped: usize,
}

fn read_codebase(data: &Value) -> Result<Codebase, String> {
    let o = data.as_object().ok_or("it is not an object")?;
    let list = json::array(o, "modules").ok_or("it has no list of modules")?;
    let mut rows: Vec<Row> = Vec::new();
    let mut calls: Vec<Vec<(String, u64)>> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut rows_skipped = 0;
    for m in list {
        let Some(mo) = m.as_object() else {
            rows_skipped += 1;
            continue;
        };
        let (Some(module), Some(path)) = (json::string(mo, "module"), json::string(mo, "path"))
        else {
            rows_skipped += 1;
            continue;
        };
        if module.is_empty() || !repository_path(&path) || !seen.insert(path.clone()) {
            rows_skipped += 1;
            continue;
        }
        calls.push(
            json::array(mo, "calls")
                .map(|cs| {
                    cs.iter()
                        .filter_map(|c| {
                            let co = c.as_object()?;
                            Some((json::string(co, "module")?, json::u64(co, "call_sites")?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
        rows.push(Row {
            defines: json::u64(mo, "defines"),
            ambiguous: json::u64(mo, "ambiguous"),
            unresolved: json::u64(mo, "unresolved"),
            calls_not_drawn: json::u64(mo, "calls_not_drawn"),
            module,
            path,
        });
    }
    // Edges by module name, to another module the answer lists; a call to
    // itself is not an edge between modules.
    let by_name: HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.module.as_str(), i))
        .collect();
    let mut summed: BTreeMap<(usize, usize), u64> = BTreeMap::new();
    for (a, cs) in calls.iter().enumerate() {
        for (callee, n) in cs {
            if let Some(&b) = by_name.get(callee.as_str())
                && a != b
            {
                *summed.entry((a, b)).or_default() += n;
            }
        }
    }
    let mut edges: Vec<(usize, usize, u64)> =
        summed.into_iter().map(|((a, b), n)| (a, b, n)).collect();
    // Heaviest first, then by index, so the same answer lists the same way.
    edges.sort_by(|x, y| y.2.cmp(&x.2).then(x.0.cmp(&y.0)).then(x.1.cmp(&y.1)));
    Ok(Codebase {
        rows,
        edges,
        modules_indexed: json::u64(o, "modules_indexed"),
        bound_call_sites: json::u64(o, "bound_call_sites"),
        ambiguous: json::u64(o, "ambiguous"),
        unresolved: json::u64(o, "unresolved"),
        modules_not_drawn: json::u64(o, "modules_not_drawn"),
        commit: json::string(o, "commit").filter(|c| is_commit(c)),
        date: json::string(o, "date"),
        rows_skipped,
    })
}

/// One path's commits, as `get_history` counts them.
#[derive(Debug, Clone, PartialEq)]
struct Churn {
    path: String,
    d30: Option<u64>,
    d90: Option<u64>,
    d365: Option<u64>,
    commits: Option<u64>,
    last_commit: Option<String>,
    last_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Commit {
    commit: String,
    date: Option<String>,
    /// Git text someone else wrote: shown, never parsed.
    subject: Option<String>,
    paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Tag {
    name: String,
    commit: String,
    date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct History {
    /// HEAD's commit and committer date; `None` in a repository with no
    /// commit yet.
    head: Option<(String, Option<String>)>,
    windows_days: Vec<u64>,
    churn: Vec<Churn>,
    commits: Vec<Commit>,
    tags: Vec<Tag>,
}

fn read_history(data: &Value) -> Result<History, String> {
    let o = data.as_object().ok_or("it is not an object")?;
    let list = |k: &str| json::array(o, k).ok_or_else(|| format!("its {k} is not a list"));
    let head = match o.get("head") {
        Some(Value::Null) => None,
        Some(Value::Object(h)) => {
            let commit = json::string(h, "commit")
                .filter(|c| is_commit(c))
                .ok_or("its head has no commit id")?;
            Some((commit, json::string(h, "date")))
        }
        _ => return Err("it has no head".into()),
    };
    let windows_days = list("windows_days")?
        .iter()
        .map(|w| {
            w.as_u64()
                .ok_or("an entry of its windows_days is not a count")
        })
        .collect::<Result<Vec<u64>, _>>()?;
    let churn = list("churn")?
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|c| {
            let path = json::string(c, "path").filter(|p| repository_path(p))?;
            Some(Churn {
                path,
                d30: json::u64(c, "d30"),
                d90: json::u64(c, "d90"),
                d365: json::u64(c, "d365"),
                commits: json::u64(c, "commits"),
                last_commit: json::string(c, "last_commit").filter(|s| is_commit(s)),
                last_date: json::string(c, "last_date"),
            })
        })
        .collect();
    let commits = list("commits")?
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|c| {
            Some(Commit {
                commit: json::string(c, "commit").filter(|s| is_commit(s))?,
                date: json::string(c, "date"),
                subject: json::string(c, "subject"),
                paths: json::array(c, "paths")
                    .map(|ps| {
                        ps.iter()
                            .filter_map(Value::as_str)
                            .filter(|p| repository_path(p))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect();
    let tags = list("tags")?
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|t| {
            Some(Tag {
                name: json::string(t, "name").filter(|n| !n.is_empty())?,
                commit: json::string(t, "commit").filter(|s| is_commit(s))?,
                date: json::string(t, "date"),
            })
        })
        .collect();
    Ok(History {
        head,
        windows_days,
        churn,
        commits,
        tags,
    })
}

/// One definition of a `get_symbols` answer.
#[derive(Debug, Clone, PartialEq)]
struct Symbol {
    module: String,
    path: String,
    name: String,
    kind: String,
    line: u64,
    end_line: u64,
    calls_in: Option<u64>,
    calls_in_ambiguous: Option<u64>,
    calls_out: Option<u64>,
}

const KINDS: [&str; 3] = ["function", "method", "class"];

/// A `get_symbols` answer, typed.
#[derive(Debug, Clone, PartialEq)]
struct Symbols {
    modules_indexed: Option<u64>,
    /// How many definitions the answer listed, drawn or not.
    listed: usize,
    list: Vec<Symbol>,
    /// Definitions the answer listed that are not drawn: a field missing,
    /// a kind not one of [`KINDS`], or a path that is not a repository path.
    skipped: usize,
}

fn read_symbols(data: &Value) -> Result<Symbols, String> {
    let o = data.as_object().ok_or("it is not an object")?;
    let list = json::array(o, "symbols").ok_or("it has no list of symbols")?;
    let mut skipped = 0;
    let symbols = list
        .iter()
        .filter_map(|s| {
            let read = || {
                let so = s.as_object()?;
                let kind = json::string(so, "kind").filter(|k| KINDS.contains(&k.as_str()))?;
                let line = json::u64(so, "line").filter(|&l| l > 0)?;
                Some(Symbol {
                    module: json::string(so, "module")?,
                    path: json::string(so, "path").filter(|p| repository_path(p))?,
                    name: json::string(so, "name").filter(|n| !n.is_empty())?,
                    kind,
                    line,
                    end_line: json::u64(so, "end_line").filter(|&e| e >= line)?,
                    calls_in: json::u64(so, "calls_in"),
                    calls_in_ambiguous: json::u64(so, "calls_in_ambiguous"),
                    calls_out: json::u64(so, "calls_out"),
                })
            };
            let r = read();
            if r.is_none() {
                skipped += 1;
            }
            r
        })
        .collect();
    Ok(Symbols {
        modules_indexed: json::u64(o, "modules_indexed"),
        listed: list.len(),
        list: symbols,
        skipped,
    })
}

/// A symbol a radius reached, as `get_radius` lists it.
#[derive(Debug, Clone, PartialEq)]
struct Reached {
    symbol: Option<String>,
    module: Option<String>,
    path: Option<String>,
    kind: Option<String>,
    line: Option<u64>,
}

/// A call site in the radius: `certain` when it is bound to the symbol,
/// not when the symbol is one of several candidates.
#[derive(Debug, Clone, PartialEq)]
struct Site {
    path: String,
    line: Option<u64>,
    certain: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
struct RadiusAnswer {
    symbol: Option<String>,
    file: Option<String>,
    resolved: Option<u64>,
    ambiguous: Option<u64>,
    symbols_reached: Option<u64>,
    unbound_in_touched_files: Option<u64>,
    touched_files: Option<u64>,
    reasons: Vec<(String, Option<String>, Option<u64>)>,
    files: Vec<(String, Option<u64>)>,
    reached: Vec<Reached>,
    reached_not_listed: Option<u64>,
    sites: Vec<Site>,
    sites_not_listed: Option<u64>,
}

fn read_radius(data: &Value) -> Result<RadiusAnswer, String> {
    let o = data
        .as_object()
        .ok_or("get_radius answered with no radius")?;
    // The same refusal as the radius pane's: a later trellis that added a
    // ratio would put the 3.7% line back on screen (radius.rs).
    if let Some(k) = crate::radius::ratio_key(data) {
        return Err(format!(
            "get_radius answered with a key shaped like a ratio, `{k}`, so the answer is not \
             shown: reach and doubt count different populations and are never divided \
             (CLAUDE.md rule 3, ADR 0014)"
        ));
    }
    let empty = Map::new();
    let reach = o.get("reach").and_then(Value::as_object).unwrap_or(&empty);
    let doubt = o.get("doubt").and_then(Value::as_object).unwrap_or(&empty);
    let objects = |m: &Map<String, Value>, k: &str| -> Vec<Map<String, Value>> {
        json::array(m, k)
            .map(|a| a.iter().filter_map(Value::as_object).cloned().collect())
            .unwrap_or_default()
    };
    Ok(RadiusAnswer {
        symbol: json::string(o, "symbol"),
        file: json::string(o, "file"),
        resolved: json::u64(reach, "resolved"),
        ambiguous: json::u64(reach, "ambiguous"),
        symbols_reached: json::u64(reach, "symbols_reached"),
        unbound_in_touched_files: json::u64(doubt, "unbound_in_touched_files"),
        touched_files: json::u64(doubt, "touched_files"),
        reasons: objects(doubt, "reasons")
            .iter()
            .filter_map(|r| {
                Some((
                    json::string(r, "reason")?,
                    json::string(r, "explain"),
                    json::u64(r, "call_sites"),
                ))
            })
            .collect(),
        files: objects(doubt, "files")
            .iter()
            .filter_map(|f| Some((json::string(f, "path")?, json::u64(f, "call_sites"))))
            .collect(),
        reached: objects(o, "reached")
            .iter()
            .map(|r| Reached {
                symbol: json::string(r, "symbol"),
                module: json::string(r, "module"),
                path: json::string(r, "path"),
                kind: json::string(r, "kind"),
                line: json::u64(r, "line"),
            })
            .collect(),
        reached_not_listed: json::u64(o, "reached_not_listed"),
        sites: objects(o, "sites")
            .iter()
            .filter_map(|s| {
                Some(Site {
                    path: json::string(s, "path")?,
                    line: json::u64(s, "line"),
                    certain: json::bool(s, "certain"),
                })
            })
            .collect(),
        sites_not_listed: json::u64(o, "sites_not_listed"),
    })
}

// --- which snapshots ------------------------------------------------------

/// `max` indices spread over `0..n`: the first, the last and evenly
/// between, each once.
fn spread(n: usize, max: usize) -> Vec<usize> {
    if n <= max {
        return (0..n).collect();
    }
    let mut out: Vec<usize> = (0..max)
        .map(|i| ((i as f64) * ((n - 1) as f64) / ((max - 1) as f64)).round() as usize)
        .collect();
    out.dedup();
    out
}

/// One snapshot: HEAD (no `reference`), or an earlier commit.
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    label: String,
    reference: Option<String>,
    tag: Option<String>,
    date: Option<String>,
    subject: Option<String>,
    /// The last `get_codebase` asked for it.
    asked: Asked,
    answer: Option<Result<Codebase, String>>,
}

impl Slot {
    fn head() -> Self {
        Self {
            label: "HEAD".into(),
            reference: None,
            tag: None,
            date: None,
            subject: None,
            asked: Asked::default(),
            answer: None,
        }
    }

    fn loaded(&self) -> Option<&Codebase> {
        self.answer.as_ref().and_then(|a| a.as_ref().ok())
    }

    fn state(&self) -> &'static str {
        match &self.answer {
            Some(Ok(_)) => "loaded",
            Some(Err(_)) => "failed",
            None if self.asked.is_asked() => "asked",
            None => "planned",
        }
    }
}

/// How far back from HEAD's date the snapshots' tags are taken: two years
/// of 365.2425 days, in seconds.
pub const WINDOW_SECONDS: i64 = 63_113_904;

/// Seconds since 1970-01-01T00:00:00Z for a date as git writes it with
/// `%cI`: `2026-10-06T17:57:49+02:00`, or with `Z`. `None` for anything
/// else, a date that does not exist (`02-30`) among it.
fn epoch_seconds(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = |from: usize, n: usize| -> Option<i64> {
        let part = b.get(from..from + n)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| part.iter().fold(0, |v, d| v * 10 + i64::from(d - b'0')))
    };
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(at(4, b'-') && at(7, b'-') && at(10, b'T') && at(13, b':') && at(16, b':')) {
        return None;
    }
    let (y, mo, d) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (h, mi, sec) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    let offset = match b.get(19..) {
        Some(b"Z") => 0,
        Some([sign @ (b'+' | b'-'), _, _, b':', _, _]) => {
            let o = digits(20, 2)? * 3600 + digits(23, 2)? * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&mo) || d < 1 || d > month_days[(mo - 1) as usize] {
        return None;
    }
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// The snapshots to load before HEAD, oldest first: up to
/// [`SNAPSHOTS_MAX`] of the history's tags dated in the [`WINDOW_SECONDS`]
/// before HEAD's date, spread from the newest to the oldest of them, so the
/// oldest is always taken; with no HEAD date to measure from, spread over
/// every tag; with no tag to take, the history's commits. HEAD's own commit
/// is HEAD's.
fn plan(h: &History) -> Vec<Slot> {
    let head = h.head.as_ref().map(|(c, _)| c.as_str());
    let head_at = h
        .head
        .as_ref()
        .and_then(|(_, d)| d.as_deref())
        .and_then(epoch_seconds);
    // Newest first, as get_history lists them; inside the window when HEAD
    // has a date, each by its own.
    let mut tags: Vec<(&Tag, i64)> = h
        .tags
        .iter()
        .filter_map(|t| match head_at {
            None => Some((t, 0)),
            Some(at) => {
                let when = epoch_seconds(t.date.as_deref()?)?;
                (when <= at && at - when <= WINDOW_SECONDS).then_some((t, when))
            }
        })
        .collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1));
    let tags: Vec<&Tag> = tags.into_iter().map(|(t, _)| t).collect();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let subject = |commit: &str| {
        h.commits
            .iter()
            .find(|c| c.commit == commit)
            .and_then(|c| c.subject.clone())
    };
    // Newest first, as get_history lists both.
    let points: Vec<Slot> = if tags.iter().any(|t| Some(t.commit.as_str()) != head) {
        tags.iter()
            .filter(|t| Some(t.commit.as_str()) != head && seen.insert(&t.commit))
            .map(|t| Slot {
                label: t.name.clone(),
                reference: Some(t.commit.clone()),
                tag: Some(t.name.clone()),
                date: t.date.clone(),
                subject: subject(&t.commit),
                asked: Asked::default(),
                answer: None,
            })
            .collect()
    } else {
        h.commits
            .iter()
            .filter(|c| Some(c.commit.as_str()) != head && seen.insert(&c.commit))
            .map(|c| Slot {
                label: c.commit.chars().take(7).collect(),
                reference: Some(c.commit.clone()),
                tag: None,
                date: c.date.clone(),
                subject: c.subject.clone(),
                asked: Asked::default(),
                answer: None,
            })
            .collect()
    };
    let mut picked: Vec<Slot> = spread(points.len(), SNAPSHOTS_MAX)
        .into_iter()
        .map(|i| points[i].clone())
        .collect();
    picked.reverse();
    picked
}

// --- the layout -----------------------------------------------------------

/// An axis-aligned rectangle: `[x0, y0, x1, y1]`.
pub type Rect = [f64; 4];

/// `values` laid out in `r` in order, as d3's `treemapBinary` does: the
/// list is cut where its running sum is nearest half, along the longer
/// side, and each half recursed into, with `gap` (at most three tenths of
/// the side) between the halves. Keeping the order keeps a sorted list in
/// reading order on the map.
pub fn binary_split(values: &[f64], r: Rect, gap: f64) -> Vec<Rect> {
    let n = values.len();
    let mut out = vec![[0.0; 4]; n];
    if n == 0 {
        return out;
    }
    let mut sums = vec![0.0; n + 1];
    for (i, v) in values.iter().enumerate() {
        sums[i + 1] = sums[i] + v;
    }
    part(&sums, &mut out, 0, n, sums[n], r, gap);
    out
}

fn part(sums: &[f64], out: &mut [Rect], i: usize, j: usize, val: f64, r: Rect, gap: f64) {
    let [x0, y0, x1, y1] = r;
    if i + 1 >= j {
        out[i] = r;
        return;
    }
    let off = sums[i];
    let target = val / 2.0 + off;
    let (mut k, mut hi) = (i + 1, j - 1);
    while k < hi {
        let mid = (k + hi) / 2;
        if sums[mid] < target {
            k = mid + 1;
        } else {
            hi = mid;
        }
    }
    if target - sums[k - 1] < sums[k] - target && i + 1 < k {
        k -= 1;
    }
    let vl = sums[k] - off;
    let vr = val - vl;
    let share = |len: f64| if val > 0.0 { len * vl / val } else { len / 2.0 };
    if x1 - x0 > y1 - y0 {
        let g = gap.min((x1 - x0) * 0.3);
        let xk = x0 + share(x1 - x0 - g);
        part(sums, out, i, k, vl, [x0, y0, xk, y1], gap);
        part(sums, out, k, j, vr, [xk + g, y0, x1, y1], gap);
    } else {
        let g = gap.min((y1 - y0) * 0.3);
        let yk = y0 + share(y1 - y0 - g);
        part(sums, out, i, k, vl, [x0, y0, x1, yk], gap);
        part(sums, out, k, j, vr, [x0, yk + g, x1, y1], gap);
    }
}

fn min_side(r: &Rect) -> f64 {
    (r[2] - r[0]).min(r[3] - r[1])
}

fn area(r: &Rect) -> f64 {
    (r[2] - r[0]) * (r[3] - r[1])
}

/// A node of the path tree, in the world's units.
#[derive(Debug, Clone)]
struct Node {
    /// A directory's name, `a/b` once folded; a module's file name.
    name: String,
    /// A directory's path; a module's file path.
    path: String,
    kids: Vec<usize>,
    /// The module's index into the entries, for a module.
    module: Option<usize>,
    depth: usize,
    value: f64,
    /// Area its header, padding and gaps took, as weight.
    over: f64,
    rect: Rect,
    header: f64,
}

/// One region of the map: a directory, or the root (depth 0).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub test: bool,
    pub rect: Rect,
    /// Its header's height, in the same normalised units as `rect`'s y.
    pub header: f64,
}

/// One module placed on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub module: String,
    pub path: String,
    pub label: String,
    pub region: usize,
    pub test: bool,
    pub weight: u64,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub regions: Vec<Region>,
    pub modules: Vec<Placed>,
    /// Whether this is the layout over every snapshot, kept for this root.
    pub fixed: bool,
    /// The snapshots it was computed over, by label.
    pub over: Vec<String>,
}

/// Whether a directory holds test code: it, or a directory above it, is
/// named `tests` or `test`. The prototype marked `tests/` alone; a package's
/// own `tests` directory is test code too.
fn test_dir(dir: &str) -> bool {
    dir.split('/').any(|s| s == "tests" || s == "test")
}

/// The layout of `entries`, each `(path, module name, weight)`, in the
/// world's units normalised to 0..1. A pure function of the set of
/// entries: their order does not matter.
pub fn layout(entries: &[(String, String, u64)]) -> (Vec<Region>, Vec<Placed>) {
    let mut sorted: Vec<&(String, String, u64)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted.dedup_by(|a, b| a.0 == b.0);

    let mut nodes: Vec<Node> = vec![Node {
        name: String::new(),
        path: String::new(),
        kids: Vec::new(),
        module: None,
        depth: 0,
        value: 0.0,
        over: 0.0,
        rect: [0.0; 4],
        header: 0.0,
    }];
    for (i, (path, _, _)) in sorted.iter().enumerate() {
        let parts: Vec<&str> = path.split('/').collect();
        let mut n = 0;
        for (j, seg) in parts[..parts.len() - 1].iter().enumerate() {
            let found = nodes[n]
                .kids
                .iter()
                .copied()
                .find(|&k| nodes[k].module.is_none() && nodes[k].name == *seg);
            n = match found {
                Some(k) => k,
                None => {
                    nodes.push(Node {
                        name: seg.to_string(),
                        path: parts[..=j].join("/"),
                        kids: Vec::new(),
                        module: None,
                        depth: 0,
                        value: 0.0,
                        over: 0.0,
                        rect: [0.0; 4],
                        header: 0.0,
                    });
                    let k = nodes.len() - 1;
                    nodes[n].kids.push(k);
                    k
                }
            };
        }
        nodes.push(Node {
            name: parts[parts.len() - 1].to_string(),
            path: path.to_string(),
            kids: Vec::new(),
            module: Some(i),
            depth: 0,
            value: 0.0,
            over: 0.0,
            rect: [0.0; 4],
            header: 0.0,
        });
        let k = nodes.len() - 1;
        nodes[n].kids.push(k);
    }
    fold(&mut nodes, 0);
    order(&mut nodes, 0, 0);

    let weight = |m: usize| sorted[m].2.max(FLOOR) as f64;
    for _ in 0..PASSES {
        value(&mut nodes, 0, &weight);
        nodes[0].rect = [0.0, 0.0, WORLD_W, WORLD_H];
        place(&mut nodes, 0);
        // The area each module got, per unit of weight, over the map.
        let (mut la, mut lv) = (0.0, 0.0);
        walk(&nodes, 0, &mut |n: &Node| {
            if let Some(m) = n.module {
                la += area(&n.rect);
                lv += weight(m);
            }
        });
        let density = la / lv;
        let dirs: Vec<usize> = (1..nodes.len())
            .filter(|&i| nodes[i].module.is_none())
            .collect();
        for d in dirs {
            let kids: f64 = nodes[d].kids.iter().map(|&k| area(&nodes[k].rect)).sum();
            nodes[d].over = ((area(&nodes[d].rect) - kids) / density).max(0.0);
        }
    }

    // Pre-order: a region before what it holds, modules in reading order.
    let mut regions: Vec<Region> = Vec::new();
    let mut modules: Vec<Placed> = Vec::new();
    emit(&nodes, 0, None, &sorted, &mut regions, &mut modules);
    (regions, modules)
}

/// Folds each chain of directories that hold one directory each into one
/// region (`src/poetry`), so every region on the map holds more than a
/// label. The root is never folded.
fn fold(nodes: &mut [Node], n: usize) {
    for k in nodes[n].kids.clone() {
        fold(nodes, k);
    }
    if n == 0 {
        return;
    }
    while nodes[n].kids.len() == 1 && nodes[nodes[n].kids[0]].module.is_none() {
        let c = nodes[n].kids[0];
        nodes[n].name = format!("{}/{}", nodes[n].name, nodes[c].name);
        nodes[n].path = nodes[c].path.clone();
        nodes[n].kids = nodes[c].kids.clone();
    }
}

/// Directories before modules, each by name, the order a file tree shows;
/// and each node's depth.
fn order(nodes: &mut [Node], n: usize, depth: usize) {
    nodes[n].depth = depth;
    let mut kids = nodes[n].kids.clone();
    kids.sort_by(|&a, &b| {
        nodes[a]
            .module
            .is_some()
            .cmp(&nodes[b].module.is_some())
            .then_with(|| nodes[a].name.cmp(&nodes[b].name))
    });
    nodes[n].kids = kids.clone();
    for k in kids {
        order(nodes, k, depth + 1);
    }
}

fn value(nodes: &mut [Node], n: usize, weight: &impl Fn(usize) -> f64) -> f64 {
    let v = match nodes[n].module {
        Some(m) => weight(m),
        None => {
            let mut s = nodes[n].over;
            for k in nodes[n].kids.clone() {
                s += value(nodes, k, weight);
            }
            s
        }
    };
    nodes[n].value = v;
    v
}

fn place(nodes: &mut [Node], n: usize) {
    if nodes[n].module.is_some() {
        return;
    }
    let r = nodes[n].rect;
    let d = nodes[n].depth;
    let (h, p, g) = if d == 0 {
        (0.0, 0.0, GUTTER)
    } else {
        let s = min_side(&r);
        (
            HEADER[d.min(HEADER.len() - 1)].min(0.2 * s),
            PAD.min(0.05 * s),
            GAP.min(0.04 * s),
        )
    };
    nodes[n].header = h;
    let kids = nodes[n].kids.clone();
    let values: Vec<f64> = kids.iter().map(|&k| nodes[k].value).collect();
    let inner = [
        r[0] + p,
        r[1] + h,
        (r[0] + p).max(r[2] - p),
        (r[1] + h).max(r[3] - p),
    ];
    for (&k, rect) in kids.iter().zip(binary_split(&values, inner, g)) {
        nodes[k].rect = rect;
        place(nodes, k);
    }
}

fn walk(nodes: &[Node], n: usize, f: &mut impl FnMut(&Node)) {
    f(&nodes[n]);
    for &k in &nodes[n].kids {
        walk(nodes, k, f);
    }
}

fn normalised(r: &Rect) -> Rect {
    [
        round(r[0] / WORLD_W, 6),
        round(r[1] / WORLD_H, 6),
        round(r[2] / WORLD_W, 6),
        round(r[3] / WORLD_H, 6),
    ]
}

fn emit(
    nodes: &[Node],
    n: usize,
    parent: Option<usize>,
    sorted: &[&(String, String, u64)],
    regions: &mut Vec<Region>,
    modules: &mut Vec<Placed>,
) {
    let node = &nodes[n];
    match node.module {
        Some(m) => {
            let (path, module, w) = sorted[m];
            modules.push(Placed {
                module: module.clone(),
                path: path.clone(),
                label: node
                    .name
                    .strip_suffix(".py")
                    .unwrap_or(&node.name)
                    .to_string(),
                region: parent.unwrap_or(0),
                test: path.rsplit_once('/').is_some_and(|(dir, _)| test_dir(dir)),
                weight: (*w).max(FLOOR),
                rect: normalised(&node.rect),
            });
        }
        None => {
            regions.push(Region {
                path: node.path.clone(),
                label: node.name.clone(),
                depth: node.depth,
                parent,
                test: node.depth > 0 && test_dir(&node.path),
                rect: normalised(&node.rect),
                header: round(node.header / WORLD_H, 6),
            });
            let me = regions.len() - 1;
            for &k in &node.kids {
                emit(nodes, k, Some(me), sorted, regions, modules);
            }
        }
    }
}

// --- symbols in their module ----------------------------------------------

/// A definition placed inside its module's rectangle.
#[derive(Debug, Clone, PartialEq)]
struct PlacedSymbol {
    /// Index into the answer's symbols.
    symbol: usize,
    /// The module's index in the layout.
    module: usize,
    /// The enclosing definition's index in the placed list.
    parent: Option<usize>,
    depth: usize,
    /// Whether it holds other definitions: drawn as a box with a header.
    holds: bool,
    rect: Rect,
    header: f64,
}

/// HEAD's symbols laid out inside their modules, parents before what they
/// hold, and how many could not be placed (a module the layout lacks).
fn place_symbols(layout: &Layout, symbols: &[Symbol]) -> (Vec<PlacedSymbol>, usize) {
    let by_path: HashMap<&str, usize> = layout
        .modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.path.as_str(), i))
        .collect();
    let mut per_module: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut not_placed = 0;
    for (i, s) in symbols.iter().enumerate() {
        match by_path.get(s.path.as_str()) {
            Some(&m) => per_module.entry(m).or_default().push(i),
            None => not_placed += 1,
        }
    }
    let mut out: Vec<PlacedSymbol> = Vec::new();
    for (m, mut ids) in per_module {
        // By line, the outer of two that start together first, so a
        // definition's parent is the innermost one open around it.
        ids.sort_by(|&a, &b| {
            let (x, y) = (&symbols[a], &symbols[b]);
            x.line
                .cmp(&y.line)
                .then(y.end_line.cmp(&x.end_line))
                .then(x.name.cmp(&y.name))
                .then(a.cmp(&b))
        });
        let mut parent: Vec<Option<usize>> = vec![None; ids.len()];
        let mut open: Vec<usize> = Vec::new();
        for (k, &i) in ids.iter().enumerate() {
            let s = &symbols[i];
            while let Some(&top) = open.last() {
                if symbols[ids[top]].end_line >= s.end_line && symbols[ids[top]].line <= s.line {
                    break;
                }
                open.pop();
            }
            parent[k] = open.last().copied();
            open.push(k);
        }
        let kids: Vec<Vec<usize>> = (0..ids.len())
            .map(|k| (0..ids.len()).filter(|&c| parent[c] == Some(k)).collect())
            .collect();
        let mut weight = vec![0.0; ids.len()];
        fn weigh(
            k: usize,
            ids: &[usize],
            symbols: &[Symbol],
            kids: &[Vec<usize>],
            w: &mut [f64],
        ) -> f64 {
            let s = &symbols[ids[k]];
            let lines = ((s.end_line - s.line + 1) as f64).max(2.0);
            let v = if kids[k].is_empty() {
                lines
            } else {
                let sum: f64 = kids[k]
                    .iter()
                    .map(|&c| weigh(c, ids, symbols, kids, w))
                    .sum();
                lines.max(sum * 1.2 + 3.0)
            };
            w[k] = v;
            v
        }
        let tops: Vec<usize> = (0..ids.len()).filter(|&k| parent[k].is_none()).collect();
        for &t in &tops {
            weigh(t, &ids, symbols, &kids, &mut weight);
        }
        // In world units: the module's rectangle as the layout placed it.
        let r = layout.modules[m].rect;
        let l = [
            r[0] * WORLD_W,
            r[1] * WORLD_H,
            r[2] * WORLD_W,
            r[3] * WORLD_H,
        ];
        let (w, h) = (l[2] - l[0], l[3] - l[1]);
        let hdr = (h * 0.1).min(w * 0.1);
        let pad = w.min(h) * 0.012;
        let inner = [
            l[0] + pad,
            l[1] + hdr,
            (l[0] + pad).max(l[2] - pad),
            (l[1] + hdr).max(l[3] - pad),
        ];
        let mut rects: Vec<Rect> = vec![[0.0; 4]; ids.len()];
        let mut headers: Vec<f64> = vec![0.0; ids.len()];
        let top_values: Vec<f64> = tops.iter().map(|&t| weight[t]).collect();
        for (&t, rect) in tops.iter().zip(binary_split(&top_values, inner, pad)) {
            rects[t] = rect;
        }
        // Parents come before children in `ids`' order, so each box is
        // placed before what it holds.
        for k in 0..ids.len() {
            if kids[k].is_empty() {
                continue;
            }
            let b = rects[k];
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            let p = bw.min(bh) * 0.03;
            let hd = (bh * 0.25).min((bh * 0.08).max(bw.min(bh) * 0.14));
            headers[k] = hd;
            let inner = [
                b[0] + p,
                b[1] + hd,
                (b[0] + p).max(b[2] - p),
                (b[1] + hd).max(b[3] - p),
            ];
            let values: Vec<f64> = kids[k].iter().map(|&c| weight[c]).collect();
            for (&c, rect) in kids[k].iter().zip(binary_split(&values, inner, p)) {
                rects[c] = rect;
            }
        }
        let base = out.len();
        let mut depth = vec![0usize; ids.len()];
        for k in 0..ids.len() {
            depth[k] = parent[k].map_or(0, |p| depth[p] + 1);
            out.push(PlacedSymbol {
                symbol: ids[k],
                module: m,
                parent: parent[k].map(|p| base + p),
                depth: depth[k],
                holds: !kids[k].is_empty(),
                rect: normalised(&rects[k]),
                header: round(headers[k] / WORLD_H, 6),
            });
        }
    }
    (out, not_placed)
}

// --- the view -------------------------------------------------------------

/// The radius asked for last, and where it is.
#[derive(Debug, Clone, PartialEq)]
enum Radius {
    Asked(String),
    Refused(String, String),
    Answered(String, Box<RadiusAnswer>),
}

/// The Atlas's part of the reducer.
#[derive(Debug)]
pub struct Atlas {
    asked_history: Asked,
    history: Option<Result<History, String>>,
    /// Whether the history's snapshots have been planned: once per root.
    planned: bool,
    /// Scrubber order: oldest first, HEAD last.
    slots: Vec<Slot>,
    asked_symbols: Asked,
    symbols: Option<Result<Symbols, String>>,
    asked_radius: Asked,
    radius: Option<Radius>,
    layout: Option<Layout>,
    placed_symbols: Vec<PlacedSymbol>,
    symbols_not_placed: usize,
}

impl Default for Atlas {
    fn default() -> Self {
        Self {
            asked_history: Asked::default(),
            history: None,
            planned: false,
            slots: vec![Slot::head()],
            asked_symbols: Asked::default(),
            symbols: None,
            asked_radius: Asked::default(),
            radius: None,
            layout: None,
            placed_symbols: Vec::new(),
            symbols_not_placed: 0,
        }
    }
}

impl Atlas {
    /// The page sent `line` (a `command` it forwards): from now on only
    /// its answer is taken for that kind, or for that snapshot. A line
    /// with no Atlas id, or a `get_codebase` for a snapshot not planned,
    /// is not the Atlas's.
    pub fn asked(&mut self, command: &str, line: &Value) {
        let id = line.get("id").and_then(Value::as_str);
        if !is_atlas_id(id) {
            return;
        }
        match command {
            "get_history" => self.asked_history.ask(id),
            // One module's symbols would replace every module's.
            "get_symbols" if line.get("module").is_none() => self.asked_symbols.ask(id),
            "get_radius" => {
                let symbol = line
                    .get("symbol")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.asked_radius.ask(id);
                self.radius = Some(Radius::Asked(symbol));
            }
            "get_codebase" => {
                let reference = line.get("ref").and_then(Value::as_str);
                if let Some(slot) = self
                    .slots
                    .iter_mut()
                    .find(|s| s.reference.as_deref() == reference)
                {
                    slot.asked.ask(id);
                }
            }
            _ => {}
        }
    }

    /// Takes an answer to the last question the Atlas asked of its kind,
    /// and says whether the view changed.
    pub fn answer(&mut self, r: &Response) -> bool {
        let id = r.id.as_deref();
        if !is_atlas_id(id) {
            return false;
        }
        let read = |what: &str| -> Result<&Value, String> {
            match (r.success, &r.data) {
                (Some(true), Some(d)) => Ok(d),
                (Some(true), None) => Err(format!("{what} answered with no data")),
                _ => Err(asked::refusal(what, r)),
            }
        };
        let bad = |what: &str, e: String| {
            format!("{what} answered with bad data, so nothing from it is shown: {e}")
        };
        match r.command.as_deref() {
            Some("get_history") if self.asked_history.is(id) => {
                self.history = Some(
                    read("get_history")
                        .and_then(|d| read_history(d).map_err(|e| bad("get_history", e))),
                );
                if !self.planned {
                    self.planned = true;
                    if let Some(Ok(h)) = &self.history {
                        let mut slots = plan(h);
                        let mut head = self.slots.pop().unwrap_or_else(Slot::head);
                        if let Some((commit, date)) = &h.head {
                            head.date = head.date.take().or_else(|| date.clone());
                            head.subject = h
                                .commits
                                .iter()
                                .find(|c| &c.commit == commit)
                                .and_then(|c| c.subject.clone());
                        }
                        slots.push(head);
                        self.slots = slots;
                    }
                }
                self.relayout();
                true
            }
            Some("get_codebase") => {
                let Some(slot) = self
                    .slots
                    .iter_mut()
                    .find(|s| s.asked.is_asked() && s.asked.is(id))
                else {
                    return false;
                };
                slot.answer = Some(
                    read("get_codebase")
                        .and_then(|d| read_codebase(d).map_err(|e| bad("get_codebase", e))),
                );
                if let Some(Ok(c)) = &slot.answer
                    && slot.reference.is_none()
                    && slot.date.is_none()
                {
                    slot.date = c.date.clone();
                }
                self.relayout();
                true
            }
            Some("get_symbols") if self.asked_symbols.is(id) => {
                self.symbols = Some(
                    read("get_symbols")
                        .and_then(|d| read_symbols(d).map_err(|e| bad("get_symbols", e))),
                );
                self.place();
                true
            }
            Some("get_radius") if self.asked_radius.is(id) => {
                let symbol = match &self.radius {
                    Some(Radius::Asked(s) | Radius::Refused(s, _) | Radius::Answered(s, _)) => {
                        s.clone()
                    }
                    None => String::new(),
                };
                self.radius = Some(match read("get_radius").and_then(read_radius) {
                    Ok(a) => Radius::Answered(symbol, Box::new(a)),
                    Err(e) => Radius::Refused(symbol, e),
                });
                true
            }
            _ => false,
        }
    }

    /// Lays the map out over HEAD as soon as HEAD has loaded, and once
    /// more over every snapshot loaded when the last planned one has
    /// settled; from then on the layout is fixed.
    fn relayout(&mut self) {
        if self.layout.as_ref().is_some_and(|l| l.fixed) {
            return;
        }
        let settled = self.planned && self.slots.iter().all(|s| s.answer.is_some());
        let head_loaded = self.slots.last().is_some_and(|s| s.loaded().is_some());
        if !(settled || (self.layout.is_none() && head_loaded)) {
            return;
        }
        // Oldest first, so the newest snapshot holding a module sets its
        // weight.
        let mut weights: BTreeMap<String, (String, u64)> = BTreeMap::new();
        let mut over = Vec::new();
        for s in &self.slots {
            if let Some(c) = s.loaded() {
                over.push(s.label.clone());
                for r in &c.rows {
                    weights.insert(r.path.clone(), (r.module.clone(), r.defines.unwrap_or(0)));
                }
            }
        }
        if weights.is_empty() {
            return;
        }
        let entries: Vec<(String, String, u64)> =
            weights.into_iter().map(|(p, (m, w))| (p, m, w)).collect();
        let (regions, modules) = layout(&entries);
        self.layout = Some(Layout {
            regions,
            modules,
            fixed: settled,
            over,
        });
        self.place();
    }

    fn place(&mut self) {
        (self.placed_symbols, self.symbols_not_placed) = match (&self.layout, &self.symbols) {
            (Some(l), Some(Ok(s))) => place_symbols(l, &s.list),
            _ => (Vec::new(), 0),
        };
    }

    /// The view's `atlas` key.
    pub fn to_json(&self) -> Value {
        let index: HashMap<&str, usize> = self
            .layout
            .as_ref()
            .map(|l| {
                l.modules
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (m.path.as_str(), i))
                    .collect()
            })
            .unwrap_or_default();
        let m_of = |path: &str| index.get(path).copied();
        json!({
            "history": self.history_json(&m_of),
            "snapshots": self.slots.iter().map(|s| self.slot_json(s, &m_of)).collect::<Vec<_>>(),
            "progress": self.progress_json(),
            "layout": self.layout.as_ref().map(layout_json),
            "symbols": self.symbols_json(),
            "radius": self.radius_json(&m_of),
        })
    }

    fn history_json(&self, m_of: &impl Fn(&str) -> Option<usize>) -> Value {
        let h = match &self.history {
            None => return Value::Null,
            Some(Err(e)) => return json!({"error": e}),
            Some(Ok(h)) => h,
        };
        // The counts in `commits` run from the oldest tag returned.
        let since = h
            .tags
            .last()
            .map(|t| json!({"name": t.name, "commit": t.commit, "date": t.date}));
        json!({
            "error": null,
            "head": h.head.as_ref().map(|(c, d)| json!({"commit": c, "date": d})),
            "windows_days": h.windows_days,
            "since": since,
            "churn": h.churn.iter().map(|c| json!({
                "path": c.path, "m": m_of(&c.path),
                "d30": c.d30, "d90": c.d90, "d365": c.d365, "commits": c.commits,
                "last_commit": c.last_commit, "last_date": c.last_date,
            })).collect::<Vec<_>>(),
            "commits": h.commits.iter().map(|c| {
                let (on, off): (Vec<&String>, Vec<&String>) =
                    c.paths.iter().partition(|p| m_of(p).is_some());
                json!({
                    "commit": c.commit, "date": c.date, "subject": c.subject,
                    "modules": on.iter().filter_map(|p| m_of(p)).collect::<Vec<_>>(),
                    "other_paths": off,
                })
            }).collect::<Vec<_>>(),
            "tags": h.tags.iter()
                .map(|t| json!({"name": t.name, "commit": t.commit, "date": t.date}))
                .collect::<Vec<_>>(),
        })
    }

    fn slot_json(&self, s: &Slot, m_of: &impl Fn(&str) -> Option<usize>) -> Value {
        let mut v = json!({
            "label": s.label,
            "ref": s.reference,
            "tag": s.tag,
            "date": s.date,
            "subject": s.subject,
            "state": s.state(),
            "error": s.answer.as_ref().and_then(|a| a.as_ref().err()),
            // A planned snapshot is asked by its commit; HEAD's is the
            // history's, or the answer's own.
            "commit": s.reference.clone()
                .or_else(|| match &self.history {
                    Some(Ok(h)) => h.head.as_ref().map(|(c, _)| c.clone()),
                    _ => None,
                })
                .or_else(|| s.loaded().and_then(|c| c.commit.clone())),
            "modules_indexed": null, "definitions": null, "bound_call_sites": null, "ambiguous": null,
            "unresolved": null, "modules_not_drawn": null, "module_edges": null,
            "rows": [], "edges": [], "regions": [], "not_placed": [], "rows_skipped": 0,
        });
        let Some(c) = s.loaded() else {
            return v;
        };
        let at: Vec<Option<usize>> = c.rows.iter().map(|r| m_of(&r.path)).collect();
        // Into and out of each module, from the other modules' edges.
        let n = c.rows.len();
        let (mut bound_in, mut callers, mut bound_out, mut callees) =
            (vec![0u64; n], vec![0u64; n], vec![0u64; n], vec![0u64; n]);
        for &(a, b, w) in &c.edges {
            bound_out[a] += w;
            callees[a] += 1;
            bound_in[b] += w;
            callers[b] += 1;
        }
        let rows: Vec<Value> = c
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                Some(json!([
                    at[i]?,
                    r.defines,
                    r.ambiguous,
                    r.unresolved,
                    r.calls_not_drawn,
                    bound_in[i],
                    callers[i],
                    bound_out[i],
                    callees[i]
                ]))
            })
            .collect();
        let edges: Vec<Value> = c
            .edges
            .iter()
            .filter_map(|&(a, b, w)| Some(json!([at[a]?, at[b]?, w])))
            .collect();
        // Each region's modules at this snapshot, and their three counts
        // and definitions summed; `null` where a module left one out.
        let regions: Vec<Value> = match &self.layout {
            None => Vec::new(),
            Some(l) => {
                let mut held = vec![0u64; l.regions.len()];
                // Definitions, ambiguous and unresolved, in that order.
                let mut sums: Vec<[Option<u64>; 3]> = vec![[Some(0); 3]; l.regions.len()];
                for (i, r) in c.rows.iter().enumerate() {
                    let Some(m) = at[i] else { continue };
                    let mut g = Some(l.modules[m].region);
                    while let Some(k) = g {
                        held[k] += 1;
                        for (e, x) in sums[k]
                            .iter_mut()
                            .zip([r.defines, r.ambiguous, r.unresolved])
                        {
                            *e = e.zip(x).map(|(a, b)| a + b);
                        }
                        g = l.regions[k].parent;
                    }
                }
                held.iter()
                    .zip(&sums)
                    .map(|(n, s)| json!([n, s[0], s[1], s[2]]))
                    .collect()
            }
        };
        let not_placed: Vec<&String> = c
            .rows
            .iter()
            .enumerate()
            .filter(|(i, _)| at[*i].is_none())
            .map(|(_, r)| &r.path)
            .collect();
        v["modules_indexed"] = json!(c.modules_indexed);
        // Its modules' defines, summed; `null` when one was not a count.
        v["definitions"] = json!(c.rows.iter().map(|r| r.defines).sum::<Option<u64>>());
        v["bound_call_sites"] = json!(c.bound_call_sites);
        v["ambiguous"] = json!(c.ambiguous);
        v["unresolved"] = json!(c.unresolved);
        v["modules_not_drawn"] = json!(c.modules_not_drawn);
        v["module_edges"] = json!(c.edges.len());
        v["rows"] = json!(rows);
        v["edges"] = json!(edges);
        v["regions"] = json!(regions);
        v["not_placed"] = json!(not_placed);
        v["rows_skipped"] = json!(c.rows_skipped);
        v
    }

    /// Which snapshot is loading and how many of the planned have settled,
    /// for the page's "loading 2.0.0 … 3 of 6".
    fn progress_json(&self) -> Value {
        let planned: Vec<&Slot> = self
            .slots
            .iter()
            .filter(|s| s.reference.is_some())
            .collect();
        json!({
            "planned": self.planned,
            "loading": self.slots.iter().filter(|s| s.state() == "asked").map(|s| &s.label)
                .collect::<Vec<_>>(),
            "settled": planned.iter().filter(|s| s.answer.is_some()).count(),
            "of": planned.len(),
            "failed": self.slots.iter().filter(|s| s.state() == "failed").map(|s| &s.label)
                .collect::<Vec<_>>(),
        })
    }

    fn symbols_json(&self) -> Value {
        let s = match &self.symbols {
            None => return Value::Null,
            Some(Err(e)) => return json!({"error": e}),
            Some(Ok(s)) => s,
        };
        let symbols = &s.list;
        json!({
            "error": null,
            "modules_indexed": s.modules_indexed,
            "skipped": s.skipped,
            // The definitions the answer listed, and the call sites bound
            // to them by a Resolved edge: each one's calls_in, summed, so
            // `null` when one was not read or not a count.
            "definitions": s.listed,
            "resolved_call_sites": if s.skipped == 0 {
                symbols.iter().map(|x| x.calls_in).sum::<Option<u64>>()
            } else {
                None
            },
            "not_placed": self.symbols_not_placed,
            "list": self.placed_symbols.iter().map(|p| {
                let s = &symbols[p.symbol];
                json!({
                    "m": p.module, "name": s.name, "kind": s.kind,
                    "line": s.line, "end_line": s.end_line,
                    "calls_in": s.calls_in, "calls_in_ambiguous": s.calls_in_ambiguous,
                    "calls_out": s.calls_out,
                    "parent": p.parent, "depth": p.depth, "holds": p.holds,
                    "x0": p.rect[0], "y0": p.rect[1], "x1": p.rect[2], "y1": p.rect[3],
                    "header": p.header,
                })
            }).collect::<Vec<_>>(),
        })
    }

    fn radius_json(&self, m_of: &impl Fn(&str) -> Option<usize>) -> Value {
        let (symbol, a) = match &self.radius {
            None => return Value::Null,
            Some(Radius::Asked(s)) => {
                return json!({"symbol": s, "state": "asked", "error": null, "answer": null});
            }
            Some(Radius::Refused(s, e)) => {
                return json!({"symbol": s, "state": "refused", "error": e, "answer": null});
            }
            Some(Radius::Answered(s, a)) => (s, a),
        };
        // A reached symbol by its module and first line, which `get_symbols`
        // and `get_radius` both give.
        let mut sym_at: HashMap<(usize, u64), usize> = HashMap::new();
        if let Some(Ok(symbols)) = &self.symbols {
            for (k, p) in self.placed_symbols.iter().enumerate() {
                sym_at.insert((p.module, symbols.list[p.symbol].line), k);
            }
        }
        let reached: Vec<Value> = a
            .reached
            .iter()
            .map(|r| {
                let m = r.path.as_deref().and_then(m_of);
                let s = m.zip(r.line).and_then(|k| sym_at.get(&k).copied());
                json!({"symbol": r.symbol, "module": r.module, "path": r.path, "kind": r.kind,
                       "line": r.line, "m": m, "s": s})
            })
            .collect();
        let reached_modules: BTreeSet<usize> = a
            .reached
            .iter()
            .filter_map(|r| r.path.as_deref().and_then(m_of))
            .collect();
        let touched_modules: BTreeSet<usize> =
            a.files.iter().filter_map(|(p, _)| m_of(p)).collect();
        json!({
            "symbol": symbol,
            "state": "answered",
            "error": null,
            "answer": {
                "symbol": a.symbol,
                "file": a.file,
                "reach": {"resolved": a.resolved, "ambiguous": a.ambiguous,
                          "symbols_reached": a.symbols_reached},
                "doubt": {
                    "unbound_in_touched_files": a.unbound_in_touched_files,
                    "touched_files": a.touched_files,
                    "reasons": a.reasons.iter().map(|(r, e, n)| json!({"reason": r, "explain": e,
                        "call_sites": n})).collect::<Vec<_>>(),
                    "files": a.files.iter().map(|(p, n)| json!({"path": p, "call_sites": n,
                        "m": m_of(p)})).collect::<Vec<_>>(),
                },
                "reached": reached,
                "reached_not_listed": a.reached_not_listed,
                "reached_modules": reached_modules,
                "touched_modules": touched_modules,
                "sites": a.sites.iter().map(|s| json!({"path": s.path, "line": s.line,
                    "certain": s.certain, "m": m_of(&s.path)})).collect::<Vec<_>>(),
                "sites_not_listed": a.sites_not_listed,
            },
        })
    }
}

fn layout_json(l: &Layout) -> Value {
    json!({
        "aspect": WORLD_W / WORLD_H,
        "floor": FLOOR,
        "fixed": l.fixed,
        "over": l.over,
        "regions": l.regions.iter().map(|r| json!({
            "path": r.path, "label": r.label, "depth": r.depth, "parent": r.parent,
            "test": r.test, "x0": r.rect[0], "y0": r.rect[1], "x1": r.rect[2], "y1": r.rect[3],
            "header": r.header,
        })).collect::<Vec<_>>(),
        "modules": l.modules.iter().map(|m| json!({
            "module": m.module, "path": m.path, "label": m.label, "region": m.region,
            "test": m.test, "weight": m.weight,
            "x0": m.rect[0], "y0": m.rect[1], "x1": m.rect[2], "y1": m.rect[3],
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cut of python-poetry/poetry as trellis indexed it at seven commits
    /// (the prototype's data): paths, module names, per-module counts, two
    /// snapshots' edges and two modules' symbols. See its `about`.
    const FIXTURE: &str = include_str!("atlas_poetry.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("the fixture is JSON")
    }

    fn ok(command: &str, id: &str, data: Value) -> Response {
        Response {
            command: Some(command.into()),
            success: Some(true),
            id: Some(id.into()),
            data: Some(data),
            error: None,
        }
    }

    fn refused(command: &str, id: &str, error: &str) -> Response {
        Response {
            command: Some(command.into()),
            success: Some(false),
            id: Some(id.into()),
            data: None,
            error: Some(error.into()),
        }
    }

    /// Snapshot `k` of the fixture as `get_codebase` with `edges: "all"`
    /// answers it (Part A's shape): every module, every call to another
    /// module, `ref`, `commit` and `date`.
    fn codebase_data(fx: &Value, k: usize) -> Value {
        let s = &fx["snapshots"][k];
        let (paths, names) = (&fx["paths"], &fx["names"]);
        let mut calls: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
        for e in s["edges"].as_array().into_iter().flatten() {
            calls.entry(e[0].as_u64().unwrap()).or_default().push(
                json!({"module": names[e[1].as_u64().unwrap() as usize], "call_sites": e[2]}),
            );
        }
        let modules: Vec<Value> = s["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let p = r[0].as_u64().unwrap();
                json!({
                    "module": names[p as usize], "path": paths[p as usize], "defines": r[1],
                    "calls": calls.get(&p).cloned().unwrap_or_default(),
                    "calls_not_drawn": 0, "ambiguous": r[2], "unresolved": r[3],
                })
            })
            .collect();
        let head = s["label"] == "HEAD";
        json!({
            "modules_indexed": s["modules_indexed"], "bound_call_sites": s["bound_call_sites"],
            "ambiguous": s["ambiguous"], "unresolved": s["unresolved"],
            "modules": modules, "modules_not_drawn": 0,
            "ref": if head { Value::Null } else { s["commit"].clone() },
            "commit": s["commit"], "date": s["date"],
        })
    }

    /// A `get_history` answer whose tags are the fixture's six earlier
    /// snapshots, newest first; the first, a commit, is given a tag here.
    /// HEAD's date is left out, so the plan spreads over every tag and the
    /// layout tests load all seven commits: the first is a week more than
    /// two years before HEAD, outside the window a dated HEAD gets
    /// (`atlas_plan_takes_tags_from_the_two_years_before_head`).
    fn history_data(fx: &Value) -> Value {
        let snaps = fx["snapshots"].as_array().unwrap();
        let head = &snaps[snaps.len() - 1];
        let tags: Vec<Value> = snaps[..snaps.len() - 1]
            .iter()
            .rev()
            .map(|s| {
                json!({"name": s["tag"].as_str().unwrap_or("first"), "commit": s["commit"],
                       "date": s["date"]})
            })
            .collect();
        json!({
            "head": {"commit": head["commit"], "date": null},
            "windows_days": [30, 90, 365],
            "churn": [{"path": "src/poetry/factory.py", "d30": 0, "d90": 0, "d365": 3,
                       "commits": 10, "last_commit": head["commit"], "last_date": head["date"]}],
            "commits": [{"commit": head["commit"], "date": head["date"],
                         "subject": "tests: <b>fix</b> a conditional assertion",
                         "paths": ["tests/puzzle/test_solver.py", "docs/cli.md"]}],
            "tags": tags,
        })
    }

    /// An Atlas that asked for the history and HEAD and was answered, then
    /// asked for and was answered each earlier snapshot in `order` (the
    /// fixture's indices).
    fn loaded(order: &[usize]) -> Atlas {
        let fx = fixture();
        let mut a = Atlas::default();
        a.asked(
            "get_history",
            &json!({"id": "desktop-atlas-history-1", "type": "get_history"}),
        );
        assert!(a.answer(&ok(
            "get_history",
            "desktop-atlas-history-1",
            history_data(&fx)
        )));
        a.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-codebase-0", "type": "get_codebase", "edges": "all"}),
        );
        assert!(a.answer(&ok(
            "get_codebase",
            "desktop-atlas-codebase-0",
            codebase_data(&fx, 6)
        )));
        for &k in order {
            let id = format!("desktop-atlas-codebase-{}", k + 1);
            let commit = fx["snapshots"][k]["commit"].clone();
            a.asked(
                "get_codebase",
                &json!({"id": id, "type": "get_codebase", "edges": "all", "ref": commit}),
            );
            assert!(
                a.answer(&ok("get_codebase", &id, codebase_data(&fx, k))),
                "{k}"
            );
        }
        a
    }

    /// A rectangle as the view carries it.
    fn rect(v: &Value) -> Rect {
        ["x0", "y0", "x1", "y1"].map(|k| v[k].as_f64().unwrap_or_else(|| panic!("{k}: {v}")))
    }

    /// Whether `a` and `b` share any area. Rectangles that only touch do not.
    fn overlap(a: &Rect, b: &Rect) -> bool {
        a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
    }

    /// Whether `inner` lies inside `outer`, to the view's precision: each
    /// coordinate is rounded to a millionth on its own, so an edge and a
    /// header computed apart may differ by one rounding step.
    fn inside(inner: &Rect, outer: &Rect) -> bool {
        const STEP: f64 = 1.5e-6;
        inner[0] >= outer[0] - STEP
            && inner[1] >= outer[1] - STEP
            && inner[2] <= outer[2] + STEP
            && inner[3] <= outer[3] + STEP
    }

    #[test]
    fn atlas_every_module_keeps_one_rectangle_in_every_snapshot() {
        // The risk (the owner's request, 2026-10-11): a map laid out per
        // snapshot, so scrubbing moves every module and the eye loses its
        // place; or a layout that moves again after it was shown fixed.
        let fx = fixture();
        let a = loaded(&[]);
        let v = a.to_json();
        // HEAD alone: shown at once, not yet fixed.
        assert_eq!(v["layout"]["fixed"], false, "{}", v["progress"]);
        assert_eq!(v["layout"]["over"], json!(["HEAD"]));
        assert_eq!(v["progress"]["of"], 6);
        assert_eq!(v["progress"]["settled"], 0);
        let a = loaded(&[5, 4, 3, 2, 1, 0]);
        let v = a.to_json();
        assert_eq!(v["layout"]["fixed"], true);
        let labels: Vec<&str> = v["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            ["first", "2.0.0", "2.1.3", "2.2.0", "2.3.0", "2.4.0", "HEAD"],
            "oldest first, HEAD last"
        );
        assert_eq!(v["layout"]["over"].as_array().unwrap().len(), 7);
        let modules = v["layout"]["modules"].as_array().unwrap();
        assert_eq!(
            modules.len(),
            fx["paths"].as_array().unwrap().len(),
            "the union"
        );
        // Every snapshot names its modules by their one place in the
        // layout, and holds each module it names: the view's snapshots are
        // the fixture's, in the same order.
        for (k, s) in v["snapshots"].as_array().unwrap().iter().enumerate() {
            assert_eq!(s["state"], "loaded", "{}", s["label"]);
            assert_eq!(s["not_placed"], json!([]), "{}", s["label"]);
            let held: BTreeSet<&str> = fx["snapshots"][k]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    fx["paths"][r[0].as_u64().unwrap() as usize]
                        .as_str()
                        .unwrap()
                })
                .collect();
            let named: BTreeSet<&str> = s["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    modules[r[0].as_u64().unwrap() as usize]["path"]
                        .as_str()
                        .unwrap()
                })
                .collect();
            assert_eq!(named, held, "{}", s["label"]);
        }
        // The order the snapshots arrive in changes nothing.
        let b = loaded(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(b.to_json()["layout"], v["layout"]);
        // Once fixed, a later answer moves nothing: HEAD asked again, with
        // a module no snapshot had, keeps every rectangle, and the new
        // module is said to be unplaced rather than squeezed in.
        let mut a = a;
        let mut head = codebase_data(&fx, 6);
        head["modules"].as_array_mut().unwrap().push(json!({
            "module": "poetry.brand_new", "path": "src/poetry/brand_new.py", "defines": 40,
            "calls": [], "calls_not_drawn": 0, "ambiguous": 0, "unresolved": 0}));
        a.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-codebase-9", "type": "get_codebase", "edges": "all"}),
        );
        assert!(a.answer(&ok("get_codebase", "desktop-atlas-codebase-9", head)));
        let w = a.to_json();
        assert_eq!(w["layout"], v["layout"]);
        assert_eq!(
            w["snapshots"][6]["not_placed"],
            json!(["src/poetry/brand_new.py"])
        );
    }

    #[test]
    fn atlas_layout_no_two_modules_overlap_and_each_sits_in_its_region() {
        // The risk: a module drawn over another, so a click selects the
        // wrong one; or a module outside its package's box, or under its
        // header, so the map says it is somewhere it is not.
        let v = loaded(&[0, 1, 2, 3, 4, 5]).to_json();
        let l = &v["layout"];
        let regions = l["regions"].as_array().unwrap();
        let modules: Vec<(Rect, usize)> = l["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (rect(m), m["region"].as_u64().unwrap() as usize))
            .collect();
        let mut overlaps = 0;
        for (i, (a, _)) in modules.iter().enumerate() {
            assert!(a[2] > a[0] && a[3] > a[1], "an empty rectangle: {a:?}");
            assert!(inside(a, &[0.0, 0.0, 1.0, 1.0]), "{a:?}");
            for (b, _) in &modules[i + 1..] {
                overlaps += usize::from(overlap(a, b));
            }
        }
        assert_eq!(overlaps, 0, "module rectangles that overlap");
        for (m, r) in &modules {
            let g = &regions[*r];
            let mut below = rect(g);
            below[1] += g["header"].as_f64().unwrap();
            assert!(
                inside(m, &below),
                "{m:?} is not in {} below its header",
                g["path"]
            );
        }
        // Regions too: each inside its parent, below its header, and apart
        // from its siblings.
        for (i, g) in regions.iter().enumerate() {
            let Some(p) = g["parent"].as_u64() else {
                assert_eq!(i, 0, "only the root has no parent");
                continue;
            };
            let mut below = rect(&regions[p as usize]);
            below[1] += regions[p as usize]["header"].as_f64().unwrap();
            assert!(inside(&rect(g), &below), "{}", g["path"]);
            for h in &regions[i + 1..] {
                if h["parent"] == g["parent"] {
                    assert!(
                        !overlap(&rect(g), &rect(h)),
                        "{} and {}",
                        g["path"],
                        h["path"]
                    );
                }
            }
        }
    }

    #[test]
    fn atlas_layout_area_tracks_defines_within_the_stated_tolerance() {
        // The risk: a module's area set by how deep its package sits, or by
        // the headers around it, so a big module reads as small. The
        // tolerance, measured on the poetry fixture: every module's area is
        // within 0.82x to 1.38x of its share (its weight, defines with the
        // floor of 4, times the map's area per weight), and within 0.93x to
        // 1.16x for the 76 modules of 16 definitions or more, where the
        // fixed gaps between small rectangles weigh least.
        let v = loaded(&[0, 1, 2, 3, 4, 5]).to_json();
        let modules = v["layout"]["modules"].as_array().unwrap();
        let world = |r: Rect| (r[2] - r[0]) * WORLD_W * (r[3] - r[1]) * WORLD_H;
        let (mut la, mut lw) = (0.0, 0.0);
        for m in modules {
            la += world(rect(m));
            lw += m["weight"].as_f64().unwrap();
        }
        let density = la / lw;
        let (mut lo, mut hi, mut lo16, mut hi16, mut n16) = (f64::MAX, 0.0f64, f64::MAX, 0.0f64, 0);
        for m in modules {
            let w = m["weight"].as_f64().unwrap();
            assert!(w >= FLOOR as f64, "{m}");
            let share = world(rect(m)) / (w * density);
            lo = lo.min(share);
            hi = hi.max(share);
            if w >= 16.0 {
                n16 += 1;
                lo16 = lo16.min(share);
                hi16 = hi16.max(share);
            }
        }
        assert!(
            lo >= 0.82 && hi <= 1.38,
            "every module: {lo:.3}x to {hi:.3}x"
        );
        assert!(n16 >= 70, "{n16} modules of 16 definitions or more");
        assert!(
            lo16 >= 0.93 && hi16 <= 1.16,
            "16 or more: {lo16:.3}x to {hi16:.3}x"
        );
        // A module's weight is its definitions at the newest snapshot
        // holding it, and the floor for one that defines nothing.
        let factory = modules
            .iter()
            .find(|m| m["module"] == "poetry.factory")
            .unwrap();
        assert_eq!(
            factory["weight"], 7,
            "HEAD's 7 definitions, not 2024-09-29's"
        );
        let main = modules
            .iter()
            .find(|m| m["path"] == "src/poetry/__main__.py")
            .unwrap();
        assert_eq!(main["weight"], FLOOR);
    }

    #[test]
    fn atlas_layout_is_deterministic_and_ignores_input_order() {
        // The risk: a layout that depends on the order an answer listed its
        // modules, or on anything but the paths and weights, so the same
        // repository draws two maps.
        let fx = fixture();
        let entries: Vec<(String, String, u64)> = fx["snapshots"][6]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let p = r[0].as_u64().unwrap() as usize;
                (
                    fx["paths"][p].as_str().unwrap().to_string(),
                    fx["names"][p].as_str().unwrap().to_string(),
                    r[1].as_u64().unwrap(),
                )
            })
            .collect();
        let first = layout(&entries);
        assert_eq!(layout(&entries), first);
        let mut reversed = entries.clone();
        reversed.reverse();
        assert_eq!(layout(&reversed), first);
    }

    #[test]
    fn atlas_layout_folds_chains_sorts_directories_first_and_keeps_tests_apart() {
        // The risk: a region per directory level (`src` holding only
        // `src/pkg`), modules interleaved with packages, or source and tests
        // packed edge to edge so the two read as one block.
        let e = |p: &str, w: u64| (p.to_string(), p.replace('/', "."), w);
        let (regions, modules) = layout(&[
            e("tests/test_a.py", 10),
            e("src/pkg/z.py", 10),
            e("src/pkg/a.py", 10),
            e("src/pkg/sub/b.py", 10),
            e("src/pkg/sub/c.py", 0),
        ]);
        let labels: Vec<(&str, usize, bool)> = regions
            .iter()
            .map(|r| (r.label.as_str(), r.depth, r.test))
            .collect();
        assert_eq!(
            labels,
            [
                ("", 0, false),
                ("src/pkg", 1, false),
                ("sub", 2, false),
                ("tests", 1, true)
            ]
        );
        let order: Vec<&str> = modules.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            order,
            [
                "src/pkg/sub/b.py",
                "src/pkg/sub/c.py",
                "src/pkg/a.py",
                "src/pkg/z.py",
                "tests/test_a.py"
            ],
            "directories first, then modules, each by name"
        );
        assert_eq!(modules[0].label, "b");
        assert!(modules[4].test && !modules[0].test);
        assert_eq!(modules[1].weight, FLOOR, "the floor");
        // The top-level regions sit a gutter apart, wider than any gap
        // inside them.
        let (s, t) = (regions[1].rect, regions[3].rect);
        let apart = (t[0] - s[2]) * WORLD_W;
        let apart = apart.max((t[1] - s[3]) * WORLD_H);
        assert!((apart - GUTTER).abs() < 1e-3, "{apart}");
    }

    #[test]
    fn atlas_a_ghost_keeps_its_slot() {
        // The risk: a module that a snapshot lacks dropped from the map, so
        // its neighbours shift into its place as the scrubber moves.
        let fx = fixture();
        let v = loaded(&[0, 1, 2, 3, 4, 5]).to_json();
        let modules = v["layout"]["modules"].as_array().unwrap();
        // In 2024-09-29 and not at HEAD: removed since.
        let rows = |k: usize| -> BTreeSet<u64> {
            v["snapshots"][k]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r[0].as_u64().unwrap())
                .collect()
        };
        let (first, head) = (rows(0), rows(6));
        let gone: Vec<&u64> = first.difference(&head).collect();
        assert!(!gone.is_empty(), "the fixture has modules removed since");
        let added: Vec<&u64> = head.difference(&first).collect();
        assert!(!added.is_empty(), "and modules added since");
        for m in gone.iter().chain(added.iter()) {
            let r = rect(&modules[**m as usize]);
            assert!(
                r[2] > r[0] && r[3] > r[1],
                "{} has no slot",
                modules[**m as usize]["path"]
            );
        }
        assert_eq!(modules.len(), fx["paths"].as_array().unwrap().len());
    }

    #[test]
    fn atlas_plan_spreads_up_to_six_tags_and_falls_back_to_commits() {
        // The risk: snapshots bunched at the newest tags, so the scrubber
        // shows a week of history; a tag's name, which the repository
        // chose, sent to git as a revision; or HEAD loaded twice.
        let c = |n: usize| format!("{n:040x}");
        let h = |tags: usize, commits: usize| History {
            head: Some((c(999), None)),
            windows_days: vec![30, 90, 365],
            churn: Vec::new(),
            commits: (0..commits)
                .map(|i| Commit {
                    commit: if i == 0 { c(999) } else { c(500 + i) },
                    date: None,
                    subject: Some(format!("commit {i}")),
                    paths: Vec::new(),
                })
                .collect(),
            tags: (0..tags)
                .map(|i| Tag {
                    name: format!("v{}", tags - i),
                    commit: c(i),
                    date: None,
                })
                .collect(),
        };
        let labels = |p: Vec<Slot>| p.into_iter().map(|s| s.label).collect::<Vec<_>>();
        assert_eq!(
            labels(plan(&h(12, 3))),
            ["v1", "v3", "v5", "v8", "v10", "v12"],
            "the oldest, the newest and evenly between, oldest first"
        );
        assert_eq!(labels(plan(&h(3, 3))), ["v1", "v2", "v3"]);
        for s in plan(&h(12, 3)) {
            assert!(is_commit(s.reference.as_deref().unwrap()), "{s:?}");
        }
        // No tag: the commits, without HEAD's own.
        let p = plan(&h(0, 3));
        assert_eq!(labels(p.clone()).len(), 2);
        assert!(
            p.iter()
                .all(|s| s.reference.as_deref() != Some(c(999).as_str()))
        );
        assert_eq!(p[0].subject.as_deref(), Some("commit 2"), "oldest first");
        // A tag on HEAD is HEAD.
        let mut on_head = h(1, 1);
        on_head.tags[0].commit = c(999);
        assert!(plan(&on_head).is_empty());
        assert!(plan(&h(0, 1)).is_empty(), "only HEAD");
    }

    #[test]
    fn atlas_plan_takes_tags_from_the_two_years_before_head() {
        // The risk (the end-to-end run on poetry, 2026-10-11): the scrubber
        // spanned only the twelve newest tags, eight months of history,
        // because snapshots were spread over whatever tags came back. Now
        // get_history is asked for up to 200, and the snapshots are spread
        // over the tags dated in the two years before HEAD, the oldest of
        // them always among them.
        let c = |n: usize| format!("{n:040x}");
        let tag = |i: usize, name: &str, date: &str| Tag {
            name: name.to_string(),
            commit: c(i),
            date: Some(date.to_string()),
        };
        let h = |tags: Vec<Tag>| History {
            head: Some((c(999), Some("2026-10-06T17:57:49+02:00".into()))),
            windows_days: vec![30, 90, 365],
            churn: Vec::new(),
            commits: Vec::new(),
            tags,
        };
        // Newest first, as get_history lists them: twelve inside the window,
        // then two older, one just past its edge.
        let mut tags: Vec<Tag> = (0..12)
            .map(|i| {
                tag(
                    i,
                    &format!("2.{}", 11 - i),
                    &format!("2026-{:02}-01T12:00:00+00:00", 10 - (i % 9)),
                )
            })
            .collect();
        tags[9].date = Some("2025-06-01T12:00:00+00:00".into());
        tags[10].date = Some("2025-01-05T12:41:13+01:00".into());
        tags[11].date = Some("2024-10-07T12:00:00+02:00".into());
        tags.push(tag(12, "1.9", "2024-10-05T12:00:00+02:00"));
        tags.push(tag(13, "1.0", "2018-01-01T00:00:00Z"));
        let p = plan(&h(tags.clone()));
        let labels: Vec<&str> = p.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels.len(), SNAPSHOTS_MAX, "{labels:?}");
        assert_eq!(
            labels[0], "2.0",
            "the oldest tag in the window, first: {labels:?}"
        );
        assert_eq!(labels[SNAPSHOTS_MAX - 1], "2.11", "the newest: {labels:?}");
        assert!(
            !labels.contains(&"1.9") && !labels.contains(&"1.0"),
            "a tag from before the window: {labels:?}"
        );
        // Fewer in the window, fewer snapshots: all of them.
        let few = vec![
            tag(0, "b", "2026-01-01T00:00:00Z"),
            tag(1, "a", "2025-01-01T00:00:00Z"),
            tag(2, "old", "2020-01-01T00:00:00Z"),
        ];
        let labels: Vec<String> = plan(&h(few)).into_iter().map(|s| s.label).collect();
        assert_eq!(labels, ["a", "b"]);
        // A tag whose date does not read as one is left out of the window.
        let odd = vec![
            tag(0, "x", "yesterday"),
            tag(1, "y", "2026-02-30T00:00:00Z"),
        ];
        assert!(plan(&h(odd)).is_empty());
        // With no HEAD date to measure from, the tags are spread as before.
        let mut undated = h(tags);
        undated.head = Some((c(999), None));
        let p = plan(&undated);
        assert_eq!(p.first().map(|s| s.label.as_str()), Some("1.0"));
    }

    #[test]
    fn atlas_dates_read_as_git_writes_them() {
        // The risk: a window measured from a misread date, so a tag in it is
        // left out or one before it taken in.
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            epoch_seconds("2026-10-06T17:57:49+02:00"),
            Some(1_791_302_269)
        );
        assert_eq!(
            epoch_seconds("2000-02-29T12:00:00-05:30"),
            Some(951_845_400)
        );
        for bad in [
            "",
            "2026-10-06",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-10-06T24:00:00Z",
            "2026-10-06T17:57:49",
            "2026-10-06 17:57:49+02:00",
            "+026-10-06T17:57:49+02:00",
        ] {
            assert_eq!(epoch_seconds(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn atlas_head_figures_name_definitions_and_resolved_sites_apart_from_the_edges() {
        // The risk (the end-to-end run on poetry): the HEAD panel showed no
        // definitions, and gave call sites bound across modules where the
        // resolved sites belong, so its counts did not describe the
        // first-party sites. get_symbols gives both: the definitions it
        // lists, and its calls_in, which sum to the sites bound by a
        // Resolved edge. A sum that would leave a definition out is null.
        let fx = fixture();
        let mut a = loaded(&[]);
        let sym = |name: &str, calls_in: Value| {
            json!({"module": "poetry.factory", "path": "src/poetry/factory.py", "name": name,
                   "kind": "function", "line": 1, "end_line": 2, "calls_in": calls_in,
                   "calls_in_ambiguous": 0, "calls_out": 0})
        };
        a.asked(
            "get_symbols",
            &json!({"id": "desktop-atlas-s1", "type": "get_symbols"}),
        );
        a.answer(&ok(
            "get_symbols",
            "desktop-atlas-s1",
            json!({"modules_indexed": 451, "symbols": [sym("f", json!(3)), sym("g", json!(4))]}),
        ));
        let v = a.to_json();
        assert_eq!(v["symbols"]["definitions"], 2);
        assert_eq!(v["symbols"]["resolved_call_sites"], 7);
        // At every snapshot, its modules' definitions summed from
        // get_codebase, which at HEAD are the ones get_symbols lists.
        let defines: u64 = fx["snapshots"][6]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[1].as_u64().unwrap())
            .sum();
        assert_eq!(v["snapshots"][6]["definitions"], defines);
        // HEAD's own figures stay as trellis sent them, apart.
        assert_eq!(
            v["snapshots"][6]["bound_call_sites"],
            fx["snapshots"][6]["bound_call_sites"]
        );
        a.asked(
            "get_symbols",
            &json!({"id": "desktop-atlas-s2", "type": "get_symbols"}),
        );
        a.answer(&ok(
            "get_symbols",
            "desktop-atlas-s2",
            json!({"symbols": [sym("f", json!(3)), sym("g", Value::Null)]}),
        ));
        let v = a.to_json();
        assert_eq!(v["symbols"]["definitions"], 2);
        assert_eq!(v["symbols"]["resolved_call_sites"], Value::Null);
        a.asked(
            "get_symbols",
            &json!({"id": "desktop-atlas-s3", "type": "get_symbols"}),
        );
        a.answer(&ok(
            "get_symbols",
            "desktop-atlas-s3",
            json!({"symbols": [sym("f", json!(3)), {"name": "broken"}]}),
        ));
        let v = a.to_json();
        assert_eq!(v["symbols"]["definitions"], 2, "every definition listed");
        assert_eq!(
            v["symbols"]["resolved_call_sites"],
            Value::Null,
            "one unread"
        );
    }

    #[test]
    fn atlas_takes_only_the_last_question_s_answer_of_each_kind() {
        // The risk: each read is answered on its own thread, so an earlier
        // question answered late replaced a later one's answer: a radius for
        // the symbol clicked before last, or a snapshot from an old ask;
        // or an answer to another view taken as the Atlas's.
        let fx = fixture();
        let mut a = Atlas::default();
        for n in 1..=2 {
            a.asked(
                "get_history",
                &json!({"id": format!("desktop-atlas-history-{n}"), "type": "get_history"}),
            );
        }
        assert!(!a.answer(&ok(
            "get_history",
            "desktop-atlas-history-1",
            history_data(&fx)
        )));
        assert_eq!(a.to_json()["history"], Value::Null);
        assert!(a.answer(&refused("get_history", "desktop-atlas-history-2", "no")));
        assert_eq!(a.to_json()["history"]["error"], "no");
        // Another view's answer, or one never asked, is not the Atlas's.
        assert!(!a.answer(&ok(
            "get_codebase",
            "desktop-codebase",
            codebase_data(&fx, 6)
        )));
        assert!(!a.answer(&ok(
            "get_codebase",
            "desktop-atlas-codebase-7",
            codebase_data(&fx, 6)
        )));
        a.asked(
            "get_radius",
            &json!({"id": "desktop-atlas-radius-1", "type": "get_radius", "symbol": "a.f"}),
        );
        a.asked(
            "get_radius",
            &json!({"id": "desktop-atlas-radius-2", "type": "get_radius", "symbol": "a.g"}),
        );
        assert_eq!(a.to_json()["radius"]["state"], "asked");
        assert_eq!(a.to_json()["radius"]["symbol"], "a.g");
        assert!(!a.answer(&refused("get_radius", "desktop-atlas-radius-1", "late")));
        assert!(!a.answer(&refused("get_radius", "desktop-radius-2", "the pane's")));
        assert!(a.answer(&refused(
            "get_radius",
            "desktop-atlas-radius-2",
            "no such symbol"
        )));
        assert_eq!(
            a.to_json()["radius"],
            json!({"symbol": "a.g", "state": "refused", "error": "no such symbol", "answer": null})
        );
        // A line with no Atlas id records nothing.
        a.asked(
            "get_symbols",
            &json!({"id": "desktop-x", "type": "get_symbols"}),
        );
        assert!(!a.answer(&ok("get_symbols", "desktop-x", json!({"symbols": []}))));
    }

    #[test]
    fn atlas_a_refused_history_or_snapshot_is_said_and_loading_carries_on() {
        // The risk: one bad ref (a tag on a commit trellis cannot index)
        // stalls the Atlas at "loading", or a history trellis refused (an
        // older trellis without get_history) leaves no map at all.
        let fx = fixture();
        let mut a = Atlas::default();
        a.asked(
            "get_history",
            &json!({"id": "desktop-atlas-h", "type": "get_history"}),
        );
        a.answer(&ok("get_history", "desktop-atlas-h", history_data(&fx)));
        let snaps = fx["snapshots"].as_array().unwrap();
        for (k, s) in snaps.iter().enumerate().take(6) {
            let id = format!("desktop-atlas-c{k}");
            a.asked(
                "get_codebase",
                &json!({"id": id, "type": "get_codebase", "ref": s["commit"]}),
            );
            if k == 2 {
                a.answer(&refused("get_codebase", &id, "unknown revision"));
            } else {
                a.answer(&ok("get_codebase", &id, codebase_data(&fx, k)));
            }
        }
        let v = a.to_json();
        assert_eq!(v["layout"], Value::Null, "HEAD has not settled");
        assert_eq!(v["progress"]["failed"], json!(["2.1.3"]));
        assert_eq!(v["snapshots"][2]["state"], "failed");
        assert_eq!(v["snapshots"][2]["error"], "unknown revision");
        assert_eq!(v["snapshots"][2]["rows"], json!([]));
        a.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-head", "type": "get_codebase"}),
        );
        a.answer(&ok(
            "get_codebase",
            "desktop-atlas-head",
            codebase_data(&fx, 6),
        ));
        let v = a.to_json();
        assert_eq!(v["layout"]["fixed"], true, "every snapshot settled");
        assert_eq!(v["layout"]["over"].as_array().unwrap().len(), 6);

        // A refused history: HEAD alone, laid out and fixed.
        let mut a = Atlas::default();
        a.asked(
            "get_history",
            &json!({"id": "desktop-atlas-h", "type": "get_history"}),
        );
        a.answer(&refused(
            "get_history",
            "desktop-atlas-h",
            "unknown command",
        ));
        a.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-head", "type": "get_codebase"}),
        );
        a.answer(&ok(
            "get_codebase",
            "desktop-atlas-head",
            codebase_data(&fx, 6),
        ));
        let v = a.to_json();
        assert_eq!(v["history"]["error"], "unknown command");
        assert_eq!(v["snapshots"].as_array().unwrap().len(), 1);
        assert_eq!(v["layout"]["fixed"], true);
        // HEAD's commit and date come from its own answer.
        assert_eq!(v["snapshots"][0]["commit"], snaps[6]["commit"]);
        assert_eq!(v["snapshots"][0]["date"], snaps[6]["date"]);
    }

    /// Every key in `v`, at any depth.
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn atlas_figures_stay_counts_and_a_missing_one_is_null() {
        // The risk (CLAUDE.md rule 3, ADR 0014): a share of bound,
        // ambiguous and unresolved, or of reach and doubt, put on the map;
        // or a count trellis did not send drawn as 0.
        let fx = fixture();
        let mut a = loaded(&[0, 1, 2, 3, 4, 5]);
        a.asked(
            "get_radius",
            &json!({"id": "desktop-atlas-radius-1", "type": "get_radius",
                    "symbol": "poetry.factory.create_pool"}),
        );
        a.answer(&ok(
            "get_radius",
            "desktop-atlas-radius-1",
            json!({"symbol": "poetry.factory.create_pool", "file": "src/poetry/factory.py",
                   "reach": {"resolved": 19, "ambiguous": 0, "symbols_reached": 3},
                   "doubt": {"unbound_in_touched_files": 120, "touched_files": 2,
                             "reasons": [{"reason": "MethodDispatchUntyped", "call_sites": 100,
                                          "explain": "x"}],
                             "files": [{"path": "tests/test_factory.py", "call_sites": 90},
                                       {"path": "src/poetry/factory.py", "call_sites": 30}]},
                   "reached": [{"symbol": "poetry.factory.create_pool",
                                "module": "poetry.factory", "path": "src/poetry/factory.py",
                                "kind": "method", "line": 190}],
                   "reached_not_listed": 0,
                   "sites": [{"path": "tests/test_factory.py", "line": 12, "certain": true}],
                   "sites_not_listed": 0}),
        ));
        let v = a.to_json();
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                ["ratio", "percent", "fraction", "total", "share"]
                    .iter()
                    .any(|w| k.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "the view derives a figure: {bad:?}");
        let r = &v["radius"]["answer"];
        assert_eq!(r["reach"]["resolved"], 19);
        assert_eq!(r["doubt"]["touched_files"], 2);
        let m = |p: &str| {
            v["layout"]["modules"]
                .as_array()
                .unwrap()
                .iter()
                .position(|x| x["path"] == p)
                .unwrap()
        };
        assert_eq!(r["reached_modules"], json!([m("src/poetry/factory.py")]));
        let mut touched = vec![m("tests/test_factory.py"), m("src/poetry/factory.py")];
        touched.sort();
        assert_eq!(r["touched_modules"], json!(touched));
        // A ratio-shaped key refuses the whole answer, as the pane does.
        a.asked(
            "get_radius",
            &json!({"id": "desktop-atlas-radius-2", "type": "get_radius", "symbol": "f"}),
        );
        a.answer(&ok(
            "get_radius",
            "desktop-atlas-radius-2",
            json!({"reach": {"resolved": 1}, "doubt": {"resolved_ratio": 0.5}}),
        ));
        let v = a.to_json();
        assert_eq!(v["radius"]["state"], "refused");
        assert!(
            v["radius"]["error"]
                .as_str()
                .unwrap()
                .contains("resolved_ratio")
        );

        // Per module: the three counts as sent, the edges summed and
        // labelled by what they count; a count the answer left out is null.
        let head = &v["snapshots"][6];
        let solver = m("src/poetry/puzzle/solver.py") as u64;
        let row = head["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r[0] == solver)
            .unwrap();
        assert_eq!(row[1], 25, "defines");
        assert_eq!(row[5], 221, "call sites bound in from other modules");
        assert_eq!(row[6], 5, "modules calling it");
        let mut data = codebase_data(&fx, 6);
        data["modules"][0]
            .as_object_mut()
            .unwrap()
            .remove("ambiguous");
        let mut b = Atlas::default();
        b.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-x", "type": "get_codebase"}),
        );
        b.answer(&ok("get_codebase", "desktop-atlas-x", data));
        let w = b.to_json();
        let first = &fx["paths"][fx["snapshots"][6]["rows"][0][0].as_u64().unwrap() as usize];
        let fm = w["layout"]["modules"]
            .as_array()
            .unwrap()
            .iter()
            .position(|x| &x["path"] == first)
            .unwrap() as u64;
        let row = w["snapshots"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r[0] == fm)
            .unwrap();
        assert_eq!(row[2], Value::Null, "{row}");
        // Its regions' ambiguous sums are not reported either, while the
        // root's module count still is.
        assert_eq!(w["snapshots"][0]["regions"][0][2], Value::Null);
        assert_eq!(w["snapshots"][0]["regions"][0][0], 451);
    }

    #[test]
    fn atlas_symbols_nest_by_line_and_sit_inside_their_module() {
        // The risk: a method drawn outside its class, a symbol outside its
        // module's rectangle, or two symbols over each other, so zooming in
        // shows a picture of the module that is not its structure.
        let fx = fixture();
        let mut a = loaded(&[0, 1, 2, 3, 4, 5]);
        let symbols: Vec<Value> = fx["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let p = s["p"].as_u64().unwrap() as usize;
                let mut s = s.clone();
                let o = s.as_object_mut().unwrap();
                o.remove("p");
                o.insert("module".into(), fx["names"][p].clone());
                o.insert("path".into(), fx["paths"][p].clone());
                s
            })
            .collect();
        let n = symbols.len();
        a.asked(
            "get_symbols",
            &json!({"id": "desktop-atlas-symbols-1", "type": "get_symbols"}),
        );
        assert!(a.answer(&ok(
            "get_symbols",
            "desktop-atlas-symbols-1",
            json!({"modules_indexed": 451, "symbols": symbols}),
        )));
        let v = a.to_json();
        let list = v["symbols"]["list"].as_array().unwrap();
        assert_eq!(list.len(), n);
        assert_eq!(v["symbols"]["not_placed"], 0);
        let modules = v["layout"]["modules"].as_array().unwrap();
        for (i, s) in list.iter().enumerate() {
            let r = rect(s);
            assert!(r[2] > r[0] && r[3] > r[1], "{s}");
            let m = &modules[s["m"].as_u64().unwrap() as usize];
            assert!(inside(&r, &rect(m)), "{} outside {}", s["name"], m["path"]);
            match s["parent"].as_u64() {
                Some(p) => {
                    let p = &list[p as usize];
                    let mut below = rect(p);
                    below[1] += p["header"].as_f64().unwrap();
                    assert!(inside(&r, &below), "{} outside {}", s["name"], p["name"]);
                    assert!(p["line"].as_u64() <= s["line"].as_u64());
                    assert!(p["end_line"].as_u64() >= s["end_line"].as_u64());
                    assert_eq!(p["holds"], true);
                }
                None => assert_eq!(s["depth"], 0),
            }
            for t in &list[i + 1..] {
                if t["parent"] == s["parent"] && t["m"] == s["m"] {
                    assert!(!overlap(&r, &rect(t)), "{} and {}", s["name"], t["name"]);
                }
            }
        }
        let solver = list
            .iter()
            .position(|s| s["name"] == "Solver" && s["kind"] == "class")
            .unwrap();
        let solve = list.iter().find(|s| s["name"] == "solve").unwrap();
        assert_eq!(solve["parent"], solver, "Solver.solve is inside Solver");
        // A symbol whose module is not on the map is counted, not drawn.
        let mut b = Atlas::default();
        b.asked(
            "get_codebase",
            &json!({"id": "desktop-atlas-x", "type": "get_codebase"}),
        );
        b.answer(&ok(
            "get_codebase",
            "desktop-atlas-x",
            codebase_data(&fx, 6),
        ));
        b.asked(
            "get_symbols",
            &json!({"id": "desktop-atlas-s", "type": "get_symbols"}),
        );
        b.answer(&ok(
            "get_symbols",
            "desktop-atlas-s",
            json!({"symbols": [{"module": "x", "path": "x.py", "name": "f", "kind": "function",
                                "line": 1, "end_line": 2, "calls_in": 0, "calls_in_ambiguous": 0,
                                "calls_out": 0},
                               {"module": "x", "path": "../x.py", "name": "g",
                                "kind": "function", "line": 1, "end_line": 2}]}),
        ));
        let w = b.to_json();
        assert_eq!(w["symbols"]["not_placed"], 1);
        assert_eq!(w["symbols"]["skipped"], 1, "a path outside the repository");
    }

    #[test]
    fn atlas_history_maps_paths_to_modules_and_keeps_text_as_text() {
        // The risk: a commit's subject, written by someone else, read as
        // anything but text; a path outside the repository drawn; or churn
        // and commits not tied to the module they touch.
        let v = loaded(&[0, 1, 2, 3, 4, 5]).to_json();
        let h = &v["history"];
        assert_eq!(h["windows_days"], json!([30, 90, 365]));
        assert_eq!(h["since"]["name"], "first", "the oldest tag returned");
        let c = &h["commits"][0];
        assert_eq!(c["subject"], "tests: <b>fix</b> a conditional assertion");
        assert_eq!(c["other_paths"], json!(["docs/cli.md"]));
        let m = c["modules"][0].as_u64().unwrap() as usize;
        assert_eq!(
            v["layout"]["modules"][m]["path"],
            "tests/puzzle/test_solver.py"
        );
        let f = h["churn"][0]["m"].as_u64().unwrap() as usize;
        assert_eq!(v["layout"]["modules"][f]["module"], "poetry.factory");
        // A path that is not a repository path, or a commit id that is not
        // one, is dropped.
        let mut a = Atlas::default();
        a.asked(
            "get_history",
            &json!({"id": "desktop-atlas-h", "type": "get_history"}),
        );
        a.answer(&ok(
            "get_history",
            "desktop-atlas-h",
            json!({"head": null, "windows_days": [30, 90, 365],
                   "churn": [{"path": "../etc/passwd", "d30": 1}, {"path": "a.py", "d30": "x"}],
                   "commits": [{"commit": "--upload-pack=x", "subject": "s", "paths": []}],
                   "tags": [{"name": "v1", "commit": "-x"}]}),
        ));
        let v = a.to_json();
        assert_eq!(v["history"]["churn"].as_array().unwrap().len(), 1);
        assert_eq!(v["history"]["churn"][0]["d30"], Value::Null);
        assert_eq!(v["history"]["commits"], json!([]));
        assert_eq!(v["history"]["tags"], json!([]));
        assert_eq!(
            v["snapshots"].as_array().unwrap().len(),
            1,
            "nothing to load but HEAD"
        );
        assert_eq!(
            v["history"]["head"],
            Value::Null,
            "a repository with no commit"
        );
    }
}
