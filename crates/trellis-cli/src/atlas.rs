//! The read-only rpc answers the desktop's Atlas section draws from, added
//! on the owner's request of 2026-10-11: "the current view is nice in the
//! trellis atlas, could you add that to the current view without removing
//! the other ones". The Atlas is a seventh section beside the six; every
//! answer here is data trellis or git already holds (ADR 0025 §5, ADR 0048
//! §10). The one write is `get_codebase`'s save of the worktree's index to
//! `.trellis/index.bin` (#654, the owner's decision of 2026-10-12); nothing
//! else here writes to the repository.

use crate::codebase_graph::Edges;
use serde_json::{Value, json};
use std::path::Path;
use trellis_vcs::history::{self, HistoryError};

/// `get_codebase`'s answer: the module graph of the worktree with HEAD's
/// commit and date beside it, or, given `rev`, the graph of that commit's
/// tree with the revision as asked, its commit and its date.
///
/// HEAD's commit and date are metadata beside a graph that needs no git,
/// so they never fail the answer: they are `null` when the repository has
/// no commit yet, and `null` with `head_error` (git's message) when git
/// could not read HEAD at all, as for a hand-made `.git` git refuses. The
/// worktree's graph is answered either way, and may have moved past HEAD.
/// A commit's tree is written to a directory outside the repository
/// (`history::tree_copy`, `.py` files only, since those are all the index
/// reads), indexed there exactly as a worktree is, and removed: its paths
/// are the repository-relative ones the commit holds. A `rev` needs git,
/// so one git cannot resolve is refused.
///
/// The worktree's index is saved to `.trellis/index.bin` through
/// `index_cmd::save_index`, the writer `trellis index` uses, so the
/// overview, which reads only that cache, describes the graph the desktop
/// just drew (#654, the owner's decision of 2026-10-12). `index_saved`
/// says whether it was, with `index_bytes` when it was and
/// `index_not_saved` (why) when it was not. A failed save does not fail
/// the answer: the cache is derived state and the graph is still right. A
/// commit's graph is never saved, since the cache describes the worktree.
pub(crate) fn codebase(
    root: &Path,
    limit: usize,
    edges: Edges,
    rev: Option<&str>,
) -> Result<Value, String> {
    let Some(rev) = rev else {
        let sources = trellis_index::read_sources(root);
        let index = trellis_index::build_from_sources(&sources);
        let mut data =
            crate::codebase_graph::graph_json(&trellis_index::module_graph(&index), limit, edges);
        match crate::index_cmd::save_index(root, &index) {
            Ok(bytes) => {
                data["index_saved"] = json!(true);
                data["index_bytes"] = json!(bytes);
            }
            Err(e) => {
                data["index_saved"] = json!(false);
                data["index_not_saved"] =
                    json!(format!("{} not written: {e}", crate::index_cmd::INDEX_BIN));
            }
        }
        match history::head(root) {
            Ok(head) => {
                data["commit"] = json!(head.as_ref().map(|h| &h.commit));
                data["date"] = json!(head.as_ref().map(|h| &h.date));
            }
            Err(e) => {
                data["commit"] = Value::Null;
                data["date"] = Value::Null;
                data["head_error"] = json!(git_message(&e));
            }
        }
        return Ok(data);
    };
    let at = history::resolve(root, rev).map_err(|e| refusal("get_codebase", root, e))?;
    let copy = history::tree_copy(root, &at.commit, |p| p.ends_with(".py"))
        .map_err(|e| refusal("get_codebase", root, e))?;
    let mut data = crate::codebase_graph::codebase_json(copy.path(), limit, edges);
    data["ref"] = json!(rev);
    data["index_saved"] = json!(false);
    data["index_not_saved"] = json!(format!(
        "a commit's graph is not the worktree's; {} describes only the worktree",
        crate::index_cmd::INDEX_BIN
    ));
    data["commit"] = json!(at.commit);
    data["date"] = json!(at.date);
    Ok(data)
}

/// What git said when it failed, trimmed; the error itself when git said
/// nothing or never started.
fn git_message(e: &trellis_vcs::VcsError) -> String {
    match e {
        trellis_vcs::VcsError::GitFailed { stderr, .. } if !stderr.trim().is_empty() => {
            stderr.trim().to_string()
        }
        other => other.to_string().trim().to_string(),
    }
}

/// What `command` answers for `e`: refused when the request named
/// something the repository cannot give, failed when git did.
fn refusal(command: &str, root: &Path, e: HistoryError) -> String {
    match e {
        HistoryError::OptionLike(_)
        | HistoryError::Unknown(_)
        | HistoryError::UnsafePath { .. } => {
            format!("{command} refused: {e} ({})", root.display())
        }
        HistoryError::Missing { .. } | HistoryError::Vcs(_) => format!("{command} failed: {e}"),
    }
}

/// A `ref` argument as a client sent it, checked before any thread or
/// git starts: a non-empty string with no control character that does
/// not start with `-`, which git could read as an option.
pub(crate) fn revision(v: &Value) -> Result<String, String> {
    let Some(r) = v.as_str() else {
        return Err(format!(
            "get_codebase refused: `ref` is a git revision, as a string, not {v}"
        ));
    };
    if r.is_empty() {
        return Err("get_codebase refused: `ref` is empty".into());
    }
    if r.chars().any(char::is_control) {
        return Err(format!(
            "get_codebase refused: `ref` holds a control character: {v}"
        ));
    }
    if r.starts_with('-') {
        return Err(format!(
            "get_codebase refused: {}",
            HistoryError::OptionLike(r.to_string())
        ));
    }
    Ok(r.to_string())
}

/// An `edges` argument: absent, or `"all"`.
pub(crate) fn edges(v: Option<&Value>) -> Result<Edges, String> {
    match v {
        None => Ok(Edges::Drawn),
        Some(Value::String(s)) if s == "all" => Ok(Edges::All),
        Some(v) => Err(format!(
            "get_codebase refused: `edges` is \"all\" or absent, not {v}"
        )),
    }
}

/// A `module` argument: absent, or a module name as a string.
pub(crate) fn module_arg(v: Option<&Value>) -> Result<Option<String>, String> {
    match v {
        None => Ok(None),
        Some(Value::String(m)) => Ok(Some(m.clone())),
        Some(v) => Err(format!(
            "get_symbols refused: `module` is a dotted module name, as a string, not {v}"
        )),
    }
}

/// `get_symbols`'s answer for the worktree at `root`, built as
/// `get_codebase` builds its index: in memory from `read_sources`, so
/// `.trellis/index.bin` is neither read nor written.
pub(crate) fn symbols(root: &Path, module: Option<&str>) -> Result<Value, String> {
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    symbols_json(&index, &sources, module)
}

/// Every definition in `index`, or `module`'s, in the index's order (files
/// as it walks them, definitions by position): its module, path, bare
/// name, kind, and its first and last line, 1-based, from the index's own
/// byte span over the bytes it parsed, so no second parser can disagree.
///
/// Three counts of call sites, each named for what it counts and none a
/// part or a share of another (CLAUDE.md rule 3):
/// - `calls_in`: sites bound to this definition by a `Resolved` edge;
/// - `calls_in_ambiguous`: sites whose `Candidates` include it, none chosen;
/// - `calls_out`: sites in its own body (not a nested definition's) bound
///   by a `Resolved` edge. An ambiguous or unresolved site there is not
///   bound to anything, so it is not counted.
///
/// A `module` the index holds no file for is refused, so a misspelt name is
/// not answered as a module with no definitions.
fn symbols_json(
    index: &trellis_index::Index,
    sources: &[(String, String)],
    module: Option<&str>,
) -> Result<Value, String> {
    use trellis_index::{SymbolKind, Target};
    if let Some(m) = module
        && !index.files.iter().any(|f| f.module == m)
    {
        return Err(format!("get_symbols refused: no module `{m}` was indexed"));
    }
    let n = index.symbols.len();
    let (mut calls_in, mut ambiguous_in, mut calls_out) =
        (vec![0u32; n], vec![0u32; n], vec![0u32; n]);
    for e in &index.edges {
        match &e.target {
            Target::Resolved(s) => {
                if let Some(c) = calls_in.get_mut(s.index()) {
                    *c += 1;
                }
                let enclosing = index.sites.get(e.from.index()).and_then(|s| s.enclosing);
                if let Some(c) = enclosing.and_then(|s| calls_out.get_mut(s.index())) {
                    *c += 1;
                }
            }
            Target::Candidates(set) => {
                // A set naming one definition twice is still one site.
                for (i, s) in set.iter().enumerate() {
                    if !set[..i].contains(s)
                        && let Some(c) = ambiguous_in.get_mut(s.index())
                    {
                        *c += 1;
                    }
                }
            }
            Target::Unresolved(_) => {}
        }
    }
    let text: std::collections::HashMap<&str, &str> = sources
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    let starts: Vec<Vec<usize>> = index
        .files
        .iter()
        .map(|f| line_starts(text.get(f.path.as_str()).copied().unwrap_or("")))
        .collect();
    let mut out = Vec::new();
    for (i, s) in index.symbols.iter().enumerate() {
        let Some(file) = index.files.get(s.file.index()) else {
            continue;
        };
        if module.is_some_and(|m| file.module != m) {
            continue;
        }
        let lines = &starts[s.file.index()];
        let (from, to) = s.span;
        out.push(json!({
            "module": file.module,
            "path": file.path,
            "name": index.interner.resolve(s.name),
            "kind": match s.kind {
                SymbolKind::Function => "function",
                SymbolKind::Method => "method",
                SymbolKind::Class => "class",
            },
            "line": line_at(lines, from),
            // The span's end is exclusive: its last byte is the one before.
            "end_line": line_at(lines, to.saturating_sub(1).max(from)),
            "calls_in": calls_in[i],
            "calls_in_ambiguous": ambiguous_in[i],
            "calls_out": calls_out[i],
        }));
    }
    Ok(json!({"modules_indexed": index.files.len(), "symbols": out}))
}

/// The windows `get_history` counts churn in: days back from HEAD's
/// committer date, never from this machine's clock, so one repository
/// gives one answer whenever it is asked.
const WINDOWS_DAYS: [i64; 3] = [30, 90, 365];

/// The most commits or tags `get_history` lists, however many are asked
/// for.
const HISTORY_CAP: u64 = 200;

/// A `commits` or `tags` argument: absent for `default`, or a count from 0,
/// held to [`HISTORY_CAP`].
pub(crate) fn count_arg(cmd: &Value, field: &str, default: u64) -> Result<usize, String> {
    let n = match cmd.get(field) {
        None => default,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("get_history refused: `{field}` is a count from 0, not {v}"))?,
    };
    Ok(usize::try_from(n.min(HISTORY_CAP)).unwrap_or(0))
}

/// `get_history`'s answer, all of it from git through `trellis_vcs`.
///
/// - `head`: HEAD's commit and committer date; `null`, with every list
///   empty, when the repository has no commit yet.
/// - `commits`: the newest `commits` on HEAD's first-parent line, each with
///   its subject (data its author wrote) and the paths it changed against
///   its first parent, so a merge carries what it brought in.
/// - `tags`: the newest `tags` tags HEAD can reach, by their commits' dates.
/// - `churn`: per path, how many first-parent commits changed it within
///   each of [`WINDOWS_DAYS`] (a window is the run of commits from HEAD up
///   to the first one committed before it opens, as `git log --since`
///   walks), and `commits`, how many after the oldest tag listed; `null`
///   when no tag is listed, as there is then nothing to count from. A path
///   is a row when a commit in either range changed it, and `last_commit`
///   and `last_date` name the newest such commit. Renames are a deletion
///   and an addition.
pub(crate) fn history(root: &Path, commits: usize, tags: usize) -> Result<Value, String> {
    const DAY: i64 = 86_400;
    let failed = |e: trellis_vcs::VcsError| format!("get_history failed: {e}");
    let Some(head) = history::head(root).map_err(failed)? else {
        return Ok(json!({
            "head": null, "windows_days": WINDOWS_DAYS,
            "churn": [], "commits": [], "tags": [],
        }));
    };
    let mut listed_tags = history::tags_in_head(root).map_err(failed)?;
    listed_tags.truncate(tags);
    let oldest = WINDOWS_DAYS[WINDOWS_DAYS.len() - 1];
    let year = history::first_parent_count_since(root, head.time - oldest * DAY).map_err(failed)?;
    let since_tag = listed_tags
        .last()
        .map(|t| history::first_parent_count_after(root, &t.stamp.commit))
        .transpose()
        .map_err(failed)?;
    let walked = history::first_parent_changes(root, commits.max(year).max(since_tag.unwrap_or(0)))
        .map_err(failed)?;
    let windows: Vec<usize> = WINDOWS_DAYS
        .iter()
        .map(|d| {
            walked
                .iter()
                .position(|c| c.stamp.time < head.time - d * DAY)
                .unwrap_or(walked.len())
        })
        .collect();
    let range = windows[windows.len() - 1]
        .max(since_tag.unwrap_or(0))
        .min(walked.len());
    struct Row<'a> {
        windows: [u32; WINDOWS_DAYS.len()],
        after_tag: u32,
        last: &'a history::Stamp,
    }
    let mut rows: std::collections::BTreeMap<&str, Row> = Default::default();
    for (i, c) in walked.iter().enumerate().take(range) {
        for p in &c.paths {
            let row = rows.entry(p.as_str()).or_insert(Row {
                windows: [0; WINDOWS_DAYS.len()],
                after_tag: 0,
                last: &c.stamp,
            });
            for (n, len) in row.windows.iter_mut().zip(&windows) {
                if i < *len {
                    *n += 1;
                }
            }
            if since_tag.is_some_and(|n| i < n) {
                row.after_tag += 1;
            }
        }
    }
    let churn: Vec<Value> = rows
        .into_iter()
        .map(|(path, r)| {
            json!({
                "path": path,
                "d30": r.windows[0],
                "d90": r.windows[1],
                "d365": r.windows[2],
                "commits": since_tag.map(|_| r.after_tag),
                "last_commit": r.last.commit,
                "last_date": r.last.date,
            })
        })
        .collect();
    let commits: Vec<Value> = walked
        .iter()
        .take(commits)
        .map(|c| {
            json!({
                "commit": c.stamp.commit,
                "date": c.stamp.date,
                "subject": c.subject,
                "paths": c.paths,
            })
        })
        .collect();
    let tags: Vec<Value> = listed_tags
        .iter()
        .map(|t| json!({"name": t.name, "commit": t.stamp.commit, "date": t.stamp.date}))
        .collect();
    Ok(json!({
        "head": {"commit": head.commit, "date": head.date},
        "windows_days": WINDOWS_DAYS,
        "churn": churn,
        "commits": commits,
        "tags": tags,
    }))
}

/// The byte offset each line of `src` starts at, the first being 0.
pub(crate) fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            src.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect()
}

/// The 1-based line holding byte `at`, given [`line_starts`].
pub(crate) fn line_at(starts: &[usize], at: usize) -> usize {
    starts.partition_point(|s| *s <= at).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_atlas_history_counts_default_cap_at_200_and_refuse_what_is_not_a_count() {
        // The risk: a client asking for every commit of a long history in
        // one answer, or a count it misspelt taken as the default.
        let cmd = json!({"commits": 500, "tags": 0});
        assert_eq!(count_arg(&cmd, "commits", 20).unwrap(), 200);
        assert_eq!(count_arg(&cmd, "tags", 12).unwrap(), 0);
        assert_eq!(count_arg(&json!({}), "commits", 20).unwrap(), 20);
        for bad in [json!("5"), json!(-1), json!(2.5), json!(null)] {
            let e = count_arg(&json!({"tags": bad}), "tags", 12).unwrap_err();
            assert!(e.contains("`tags`"), "{e}");
        }
    }

    #[test]
    fn rpc_atlas_lines_are_counted_from_one_and_a_newline_ends_its_own_line() {
        // The risk: an off-by-one that puts every definition a line late,
        // or a CRLF file counted twice per line.
        let src = "a\nbc\r\n\nd";
        let s = line_starts(src);
        assert_eq!(s, [0, 2, 6, 7]);
        assert_eq!(line_at(&s, 0), 1);
        assert_eq!(line_at(&s, 1), 1, "the newline is on its own line");
        assert_eq!(line_at(&s, 2), 2);
        assert_eq!(line_at(&s, 5), 2, "a CRLF's LF is on the line it ends");
        assert_eq!(line_at(&s, 6), 3);
        assert_eq!(line_at(&s, 7), 4);
        assert_eq!(line_at(&s, 99), 4, "past the end is the last line");
        assert_eq!(line_at(&line_starts(""), 0), 1);
    }

    #[test]
    fn rpc_atlas_symbols_count_a_candidate_set_once_per_site_and_out_only_bound_sites() {
        // The risks: a call through a declared base counted as bound to
        // one override; an ambiguous or unresolved site in a body counted
        // as a call out; a nested definition's calls charged to its parent.
        let sources: Vec<(String, String)> = vec![(
            "m.py".into(),
            concat!(
                "class B:\n",
                "    def r(self):\n",
                "        return 1\n",
                "class C(B):\n",
                "    def r(self):\n",
                "        return 2\n",
                "def h():\n",
                "    return 0\n",
                "def outer(b: B):\n",
                "    def inner():\n",
                "        return h()\n",
                "    return b.r() + h() + b.nothing() + inner()\n",
            )
            .into(),
        )];
        let index = trellis_index::build_from_sources(&sources);
        let data = symbols_json(&index, &sources, None).unwrap();
        let get = |name: &str, line: u64| {
            data["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == name && s["line"] == line)
                .unwrap_or_else(|| panic!("{name}:{line} in {data}"))
                .clone()
        };
        let counts = |v: Value| {
            (
                v["calls_in"].clone(),
                v["calls_in_ambiguous"].clone(),
                v["calls_out"].clone(),
            )
        };
        assert_eq!(counts(get("r", 2)), (json!(0), json!(1), json!(0)));
        assert_eq!(counts(get("r", 5)), (json!(0), json!(1), json!(0)));
        assert_eq!(counts(get("h", 7)), (json!(2), json!(0), json!(0)));
        // `h()` and `inner()` are bound; `b.r()` is ambiguous and
        // `b.nothing()` unresolved; `inner`'s own `h()` is inner's.
        assert_eq!(counts(get("outer", 9)), (json!(0), json!(0), json!(2)));
        assert_eq!(counts(get("inner", 10)), (json!(1), json!(0), json!(1)));
        assert_eq!(get("outer", 9)["end_line"], 12);
        let none = symbols_json(&index, &sources, Some("elsewhere")).unwrap_err();
        assert!(none.contains("`elsewhere`"), "{none}");
    }

    #[test]
    fn rpc_atlas_ref_and_edges_arguments_are_refused_before_git_runs() {
        // The risk: a `ref` that git reads as an option, or one carrying a
        // newline into a command line, reaching a spawn; and an `edges`
        // value nobody defined read as the default.
        for bad in [
            json!("-n"),
            json!("--output=x"),
            json!(""),
            json!("a\nb"),
            json!(1),
        ] {
            let e = revision(&bad).unwrap_err();
            assert!(e.starts_with("get_codebase refused: "), "{e}");
        }
        assert_eq!(
            revision(&json!("v1.2^{commit}~3")).unwrap(),
            "v1.2^{commit}~3"
        );
        assert_eq!(edges(None).unwrap(), Edges::Drawn);
        assert_eq!(edges(Some(&json!("all"))).unwrap(), Edges::All);
        for bad in [json!("All"), json!(true), json!(10)] {
            assert!(edges(Some(&bad)).unwrap_err().contains("`edges`"));
        }
    }
}
