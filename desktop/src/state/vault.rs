//! The vault notes view, from `vault_search` and `vault_graph` answers
//! (#309).
//!
//! The vault is the repository's note tree (ADR 0022), and a run shows its
//! notes to the model as data (ADR 0017); the desktop shows them the same
//! way, as text. DESK-16 made `trellis vault search` and `trellis vault
//! graph` answer through rpc as data, and this types those answers into the
//! view the reducer (`state.rs`) keeps and the next `view` event carries
//! under `vault`. `ui/vault.js` only draws it (ADR 0048 section 5).
//!
//! Each figure is written as the CLI prints it (`vault_cmd.rs`): a score to
//! four decimals, the precision DESK-16's tests compare; the CLI's own line
//! for a reduced search; and the CLI's text for a note with no depth and for
//! a parent that does not resolve. The relations stay the separate lists
//! `graph` prints. An answer of any other shape is bad data: nothing from it
//! is shown, and the view says so.
//!
//! The view only reads. No rpc command writes the vault: #70 keeps every
//! vault write a `trellis vault` command the person runs (ADR 0021, ADR
//! 0049). Titles, paths and issue lines are the notes' own text, which the
//! page writes through `textContent` only (ADR 0007, S10).

use crate::frames::Response;
use serde_json::{Map, Value, json};

pub const SEARCH: &str = "vault_search";
pub const GRAPH: &str = "vault_graph";

/// What `trellis vault search` prints first when the index holds titles
/// and paths only.
pub const TITLES_ONLY: &str =
    "search reduced to titles and paths: the vault is over Apex's body-index limits";
/// What it prints when the index has bodies but no trigrams.
pub const FUZZY_OFF: &str = "fuzzy search reduced: the vault is over Apex's trigram limit";
/// What `trellis vault graph` prints for a note with no depth.
pub const NO_DEPTH: &str = "none (under a missing parent, or on a cycle)";

/// One search hit, its score as the CLI prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub score: String,
}

/// A note as `trellis vault graph` names one: its path and title.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub path: String,
    pub title: String,
}

/// A link to or from the note, with how many times it is made.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub note: Named,
    pub weight: u64,
}

/// What `trellis vault graph` shows about one note.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteGraph {
    pub note: Named,
    /// The depth as the CLI prints it, or [`NO_DEPTH`].
    pub depth: String,
    pub lineage: Vec<String>,
    /// `[[p]] does not resolve`, or `None` when nothing is unresolved.
    pub unresolved_parent: Option<String>,
    pub children: Vec<Named>,
    pub refs_out: Vec<Reference>,
    pub refs_in: Vec<Reference>,
    pub issues: Vec<String>,
}

/// A well-formed answer, by the command it answers.
enum Read {
    Search(Option<String>, Vec<Hit>),
    Graph(NoteGraph),
}

/// The vault view.
#[derive(Debug, Default)]
pub struct Vault {
    /// `None` until a search is answered, and after an answer that is
    /// refused or bad data; empty for a search that matched nothing.
    hits: Option<Vec<Hit>>,
    reduced: Option<String>,
    graph: Option<NoteGraph>,
    /// trellis's refusal, or the reason the last answer was bad data.
    error: Option<String>,
    /// The ids of the last `vault_search` and `vault_graph` the page sent.
    /// Each is answered from its own thread, so an earlier question's
    /// answer can come after a later one's; only the last one asked is
    /// taken. `None` takes any answer: nothing was asked with an id.
    asked_search: Option<String>,
    asked_graph: Option<String>,
    /// Set by a search answer when a graph was asked before it: that
    /// question named a note from the last search's hits, so its answer,
    /// however late, is not shown beside the new ones. The next
    /// `vault_graph` asked clears it.
    graph_superseded: bool,
}

fn text(o: &Map<String, Value>, k: &str, what: &str) -> Result<String, String> {
    match o.get(k) {
        Some(Value::String(s)) => Ok(s.clone()),
        None | Some(Value::Null) => Err(format!("{what} has no {k}")),
        Some(_) => Err(format!("{what}'s {k} is not text")),
    }
}

fn list<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a Vec<Value>, String> {
    o.get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("its {k} is not a list"))
}

fn named(v: &Value, what: &str) -> Result<Named, String> {
    let o = v
        .as_object()
        .ok_or_else(|| format!("{what} is not a note"))?;
    Ok(Named {
        path: text(o, "path", what)?,
        title: text(o, "title", what)?,
    })
}

fn references(o: &Map<String, Value>, k: &str) -> Result<Vec<Reference>, String> {
    list(o, k)?
        .iter()
        .map(|r| {
            let what = format!("an entry of {k}");
            let weight = r
                .get("weight")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("{what} has no weight that is a count"))?;
            Ok(Reference {
                note: named(r, &what)?,
                weight,
            })
        })
        .collect()
}

/// A `vault_search` answer's `data`: the reduced-search line, if any, and
/// the hits in the answer's order, which is the order the CLI prints.
pub fn search(data: &Value) -> Result<(Option<String>, Vec<Hit>), String> {
    let o = data.as_object().ok_or("it is not an object")?;
    let mode = o
        .get("mode")
        .and_then(Value::as_object)
        .ok_or("it has no mode")?;
    let flag = |k: &str| {
        mode.get(k)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("its mode.{k} is not true or false"))
    };
    let (body, trigrams) = (flag("body")?, flag("trigrams")?);
    // The CLI's order of the two lines: titles-only says all there is to
    // say, since it drops trigrams too.
    let reduced = if !body {
        Some(TITLES_ONLY.to_string())
    } else if !trigrams {
        Some(FUZZY_OFF.to_string())
    } else {
        None
    };
    let hits = list(o, "hits")?
        .iter()
        .map(|h| {
            let n = named(h, "a hit")?;
            let score = h
                .get("score")
                .and_then(Value::as_f64)
                .ok_or("a hit's score is not a number")?;
            Ok(Hit {
                path: n.path,
                title: n.title,
                score: format!("{score:.4}"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((reduced, hits))
}

/// A `vault_graph` answer's `data`, each relation its own list.
pub fn graph(data: &Value) -> Result<NoteGraph, String> {
    let o = data.as_object().ok_or("it is not an object")?;
    let note = named(o.get("note").ok_or("it has no note")?, "its note")?;
    // Absent is not null: a missing depth is not a claim that the note is
    // under a missing parent or on a cycle.
    let depth = match o.get("depth") {
        Some(Value::Null) => NO_DEPTH.to_string(),
        Some(v) => v
            .as_u64()
            .map(|d| d.to_string())
            .ok_or("its depth is not a count")?,
        None => return Err("it has no depth".into()),
    };
    let unresolved_parent = match o.get("unresolved_parent") {
        Some(Value::Null) => None,
        Some(Value::String(p)) => Some(format!("[[{p}]] does not resolve")),
        Some(_) => return Err("its unresolved_parent is not text".into()),
        None => return Err("it has no unresolved_parent".into()),
    };
    let strings = |k: &str| -> Result<Vec<String>, String> {
        list(o, k)?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("an entry of {k} is not text"))
            })
            .collect()
    };
    Ok(NoteGraph {
        note,
        depth,
        lineage: strings("lineage")?,
        unresolved_parent,
        children: list(o, "children")?
            .iter()
            .map(|c| named(c, "a child"))
            .collect::<Result<_, _>>()?,
        refs_out: references(o, "refs_out")?,
        refs_in: references(o, "refs_in")?,
        issues: strings("issues")?,
    })
}

fn named_json(n: &Named) -> Value {
    json!({"path": n.path, "title": n.title})
}

fn reference_json(r: &Reference) -> Value {
    json!({"path": r.note.path, "title": r.note.title, "weight": r.weight})
}

impl NoteGraph {
    fn to_json(&self) -> Value {
        json!({
            "note": named_json(&self.note),
            "depth": self.depth,
            "lineage": self.lineage,
            "unresolved_parent": self.unresolved_parent,
            "children": self.children.iter().map(named_json).collect::<Vec<_>>(),
            "refs_out": self.refs_out.iter().map(reference_json).collect::<Vec<_>>(),
            "refs_in": self.refs_in.iter().map(reference_json).collect::<Vec<_>>(),
            "issues": self.issues,
        })
    }
}

impl Vault {
    /// The page sent `command` with `id`: from now on only that answer is
    /// taken for it.
    pub fn asked(&mut self, command: &str, id: Option<&str>) {
        let id = id.map(str::to_string);
        match command {
            SEARCH => self.asked_search = id,
            GRAPH => {
                self.asked_graph = id;
                self.graph_superseded = false;
            }
            _ => {}
        }
    }

    /// Takes a `vault_search` or `vault_graph` response into the view, and
    /// says whether it changed. Another command's response, or an answer to
    /// a question asked before the last one, changes nothing.
    pub fn answer(&mut self, r: &Response) -> bool {
        let command = match r.command.as_deref() {
            Some(c @ (SEARCH | GRAPH)) => c,
            _ => return false,
        };
        let asked = if command == SEARCH {
            &self.asked_search
        } else {
            &self.asked_graph
        };
        if asked.is_some() && r.id != *asked {
            return false;
        }
        if command == GRAPH && self.graph_superseded {
            return false;
        }
        // A search answer, taken or refused, starts the view over, so a
        // graph asked before it is no longer the one the view waits on.
        let superseded = command == SEARCH && self.asked_graph.is_some();
        let read = match (r.success, &r.data) {
            (Some(true), Some(data)) => {
                let bad = |why: String| {
                    format!("{command} answered with bad data, so nothing from it is shown: {why}")
                };
                if command == SEARCH {
                    search(data).map(|(reduced, hits)| Read::Search(reduced, hits))
                } else {
                    graph(data).map(Read::Graph)
                }
                .map_err(bad)
            }
            (Some(true), None) => Err(format!(
                "{command} answered with bad data, so nothing from it is shown: it has no data"
            )),
            _ => Err(r
                .error
                .clone()
                .unwrap_or_else(|| format!("{command} was refused, and trellis gave no reason"))),
        };
        match read {
            // A search starts over: the note shown was picked from the
            // last search's hits.
            Ok(Read::Search(reduced, hits)) => {
                *self = Self {
                    hits: Some(hits),
                    reduced,
                    asked_search: self.asked_search.take(),
                    asked_graph: self.asked_graph.take(),
                    graph_superseded: superseded,
                    ..Self::default()
                };
            }
            // A note's relations are shown beside the hits it came from.
            Ok(Read::Graph(g)) => {
                self.graph = Some(g);
                self.error = None;
            }
            // A refusal or bad data leaves nothing of either shown, so no
            // hit or relation is read as the answer to this question.
            Err(why) => {
                *self = Self {
                    error: Some(why),
                    asked_search: self.asked_search.take(),
                    asked_graph: self.asked_graph.take(),
                    graph_superseded: superseded || self.graph_superseded,
                    ..Self::default()
                };
            }
        }
        true
    }

    pub fn to_json(&self) -> Value {
        json!({
            "error": self.error,
            "reduced": self.reduced,
            "hits": self.hits.as_ref().map(|hs| {
                hs.iter()
                    .map(|h| json!({"path": h.path, "title": h.title, "score": h.score}))
                    .collect::<Vec<_>>()
            }),
            "graph": self.graph.as_ref().map(NoteGraph::to_json),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::parse_stdout;

    fn response(command: &str, id: &str, data: Value) -> Response {
        Response {
            command: Some(command.into()),
            success: Some(true),
            id: Some(id.into()),
            data: Some(data),
            error: None,
        }
    }

    fn refused(command: &str, error: &str) -> Response {
        Response {
            command: Some(command.into()),
            success: Some(false),
            id: Some("x".into()),
            data: None,
            error: Some(error.into()),
        }
    }

    /// A `vault_search` answer as `search_json` writes one.
    fn hits(mode: (bool, bool), hits: Value) -> Value {
        json!({"hits": hits, "mode": {"body": mode.0, "trigrams": mode.1}})
    }

    /// A `vault_graph` answer as `graph_json` writes one.
    fn a_graph() -> Value {
        json!({
            "note": {"path": "child.md", "title": "Child"},
            "depth": 1,
            "lineage": ["root.md", "child.md"],
            "unresolved_parent": null,
            "children": [{"path": "child/a.md", "title": "A"},
                         {"path": "child/b.md", "title": "B"}],
            "refs_out": [{"path": "orphan.md", "title": "Orphan", "weight": 2},
                         {"path": "root.md", "title": "Root", "weight": 1}],
            "refs_in": [{"path": "orphan.md", "title": "Orphan", "weight": 1}],
            "issues": ["orphan.md: parent [[gone]] does not resolve"],
        })
    }

    fn view_of(v: &Vault) -> Value {
        v.to_json()
    }

    #[test]
    fn search_hits_keep_trellis_order_and_four_decimal_scores() {
        // The risk: hits re-sorted by the desktop, so it ranks a query
        // differently from `trellis vault search` (ADR 0022 section 4); or
        // a score shown at another precision than the CLI prints.
        let mut v = Vault::default();
        let data = hits(
            (true, true),
            json!([
                {"path": "b.md", "title": "Beta", "score": 0.25},
                {"path": "a.md", "title": "Alpha", "score": 12.3456789},
                {"path": "c.md", "title": "<b>Gamma</b>", "score": 3},
            ]),
        );
        assert!(v.answer(&response(SEARCH, "s1", data)));
        let j = view_of(&v);
        assert_eq!(
            j["hits"],
            json!([
                {"path": "b.md", "title": "Beta", "score": "0.2500"},
                {"path": "a.md", "title": "Alpha", "score": "12.3457"},
                {"path": "c.md", "title": "<b>Gamma</b>", "score": "3.0000"},
            ])
        );
        assert_eq!(j["error"], Value::Null);
        assert_eq!(j["reduced"], Value::Null);

        // No match is an empty list, not an error.
        assert!(v.answer(&response(SEARCH, "s2", hits((true, true), json!([])))));
        let j = view_of(&v);
        assert_eq!(j["hits"], json!([]));
        assert_eq!(j["error"], Value::Null);

        // Before any answer there are no hits at all, which is not an
        // empty answer.
        assert_eq!(view_of(&Vault::default())["hits"], Value::Null);
    }

    #[test]
    fn reduced_search_names_what_was_reduced() {
        // The risk: a search over a vault too big for Apex's body index
        // shown as if it had searched the bodies, so a person reads "no
        // matches" as "no note says this".
        let reduced = |body, trigrams| {
            let mut v = Vault::default();
            v.answer(&response(SEARCH, "s", hits((body, trigrams), json!([]))));
            view_of(&v)["reduced"].clone()
        };
        assert_eq!(
            reduced(false, false),
            "search reduced to titles and paths: the vault is over Apex's body-index limits"
        );
        assert_eq!(
            reduced(true, false),
            "fuzzy search reduced: the vault is over Apex's trigram limit"
        );
        assert_eq!(reduced(true, true), Value::Null);
        // Titles-only says all there is to say, as the CLI prints it.
        assert_eq!(reduced(false, true), TITLES_ONLY);
    }

    #[test]
    fn graph_relations_stay_separate_lists() {
        // The risk: children, links out and links in merged into one list,
        // so a person cannot tell which way a link points or which notes
        // are children; or a note with no depth shown as depth 0.
        let mut v = Vault::default();
        assert!(v.answer(&response(GRAPH, "g", a_graph())));
        let g = view_of(&v)["graph"].clone();
        assert_eq!(g["note"], json!({"path": "child.md", "title": "Child"}));
        assert_eq!(g["depth"], "1");
        assert_eq!(g["lineage"], json!(["root.md", "child.md"]), "in order");
        assert_eq!(
            g["children"],
            json!([{"path": "child/a.md", "title": "A"}, {"path": "child/b.md", "title": "B"}])
        );
        assert_eq!(
            g["refs_out"],
            json!([{"path": "orphan.md", "title": "Orphan", "weight": 2},
                   {"path": "root.md", "title": "Root", "weight": 1}])
        );
        assert_eq!(
            g["refs_in"],
            json!([{"path": "orphan.md", "title": "Orphan", "weight": 1}])
        );
        assert!(g["refs_out"][0]["weight"].is_u64(), "a number, not text");
        assert_eq!(
            g["issues"],
            json!(["orphan.md: parent [[gone]] does not resolve"])
        );
        assert_eq!(g["unresolved_parent"], Value::Null, "null stays null");

        let mut d = a_graph();
        d["depth"] = Value::Null;
        d["unresolved_parent"] = json!("p");
        let mut v = Vault::default();
        v.answer(&response(GRAPH, "g", d));
        let g = view_of(&v)["graph"].clone();
        assert_eq!(g["depth"], "none (under a missing parent, or on a cycle)");
        assert_eq!(g["unresolved_parent"], "[[p]] does not resolve");
    }

    #[test]
    fn a_malformed_vault_answer_is_bad_data() {
        // The risk: half an answer drawn as a whole one, a hit with no
        // path that its button cannot ask about, or a score the page
        // would have to make up; and a later trellis's renamed field
        // read as absent rather than said.
        let good = json!({"path": "a.md", "title": "A", "score": 1.0});
        let mut no_path = good.clone();
        no_path.as_object_mut().unwrap().remove("path");
        let mut text_score = good.clone();
        text_score["score"] = json!("1.0");
        let mut no_note = a_graph();
        no_note.as_object_mut().unwrap().remove("note");
        let mut no_depth = a_graph();
        no_depth.as_object_mut().unwrap().remove("depth");
        let mut text_weight = a_graph();
        text_weight["refs_in"][0]["weight"] = json!("1");
        for (command, data) in [
            (SEARCH, hits((true, true), json!([good.clone(), no_path]))),
            (SEARCH, hits((true, true), json!([text_score]))),
            (SEARCH, json!({"hits": [good.clone()]})),
            (SEARCH, json!([good])),
            (GRAPH, no_note),
            (GRAPH, no_depth),
            (GRAPH, text_weight),
        ] {
            let mut v = Vault::default();
            // A good answer first, so leaving it shown would be seen.
            v.answer(&response(GRAPH, "g", a_graph()));
            v.answer(&response(SEARCH, "s", hits((true, true), json!([]))));
            v.answer(&response(GRAPH, "g", a_graph()));
            assert!(v.answer(&response(command, "bad", data.clone())));
            let j = view_of(&v);
            assert_eq!(j["hits"], Value::Null, "{command} {data}: {j}");
            assert_eq!(j["graph"], Value::Null, "{command} {data}: {j}");
            assert_eq!(j["reduced"], Value::Null, "{command} {data}: {j}");
            let err = j["error"].as_str().unwrap();
            assert!(
                err.starts_with(&format!("{command} answered with bad data")),
                "{err}"
            );
        }
        let mut v = Vault::default();
        let mut r = response(SEARCH, "s", json!({}));
        r.data = None;
        v.answer(&r);
        assert!(view_of(&v)["error"].as_str().unwrap().contains("no data"));
    }

    #[test]
    fn a_refused_vault_read_shows_trellis_error_as_text() {
        // The risk: a repository with no vault configured drawn as an empty
        // vault, or trellis's reason lost, so a person is not told what to
        // set.
        let why = "no vault: pass --vault <dir>, or set [context] vault = \"<dir>\" in \
                   .trellis/config.toml";
        for command in [SEARCH, GRAPH] {
            let mut v = Vault::default();
            v.answer(&response(SEARCH, "s", hits((true, true), json!([]))));
            v.answer(&response(GRAPH, "g", a_graph()));
            assert!(v.answer(&refused(command, why)));
            let j = view_of(&v);
            assert_eq!(j["error"], why, "{command}");
            assert_eq!(j["hits"], Value::Null, "{command}");
            assert_eq!(j["graph"], Value::Null, "{command}");
        }
        let mut v = Vault::default();
        let mut r = refused(GRAPH, "");
        r.error = None;
        v.answer(&r);
        assert_eq!(
            view_of(&v)["error"],
            "vault_graph was refused, and trellis gave no reason"
        );
    }

    #[test]
    fn vault_response_frames_become_the_vault_view() {
        // The risk: the page given vault answers as raw frames to type
        // itself (ADR 0048 section 5), or a vault answer that disturbs the
        // run panes beside it.
        use crate::state::DesktopState;
        let mut s = DesktopState::default();
        s.update(parse_stdout("a transcript line"));
        let all = |s: &DesktopState| s.view(crate::state::Changed::everything());
        let before = all(&s);

        let search = json!({"type": "response", "command": "vault_search", "success": true,
                            "id": "desktop-vault-search-1",
                            "data": hits((true, false), json!([
                                {"path": "child.md", "title": "Child", "score": 0.5}]))});
        let c = s.update(parse_stdout(&search.to_string()));
        let v = s.view(c);
        assert_eq!(
            v.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["vault"],
            "only the vault view: {v}"
        );
        assert_eq!(v["vault"]["hits"][0]["score"], "0.5000");
        assert_eq!(v["vault"]["reduced"], FUZZY_OFF);

        let graph = json!({"type": "response", "command": "vault_graph", "success": true,
                           "id": "desktop-vault-graph-1", "data": a_graph()});
        let c = s.update(parse_stdout(&graph.to_string()));
        let v = s.view(c);
        assert_eq!(v.as_object().unwrap().len(), 1, "{v}");
        assert_eq!(v["vault"]["graph"]["note"]["path"], "child.md");
        assert_eq!(v["vault"]["hits"][0]["path"], "child.md", "the hits stay");

        let mut after = all(&s);
        after.as_object_mut().unwrap().remove("vault");
        let mut before = before;
        before.as_object_mut().unwrap().remove("vault");
        assert_eq!(after, before, "the run panes are as they were");
    }

    #[test]
    fn only_the_last_question_asked_is_answered_in_the_view() {
        // The risk: two searches in flight, each answered from its own
        // thread, and the earlier one's hits drawn over the later one's,
        // under the later query.
        let mut v = Vault::default();
        v.asked(SEARCH, Some("s1"));
        v.asked(SEARCH, Some("s2"));
        let newer = hits(
            (true, true),
            json!([{"path": "new.md", "title": "N", "score": 1}]),
        );
        let older = hits(
            (true, true),
            json!([{"path": "old.md", "title": "O", "score": 1}]),
        );
        assert!(v.answer(&response(SEARCH, "s2", newer)));
        assert!(!v.answer(&response(SEARCH, "s1", older)), "a stale answer");
        assert_eq!(view_of(&v)["hits"][0]["path"], "new.md");
        // A stale graph is left out too, and the hits keep their asker.
        v.asked(GRAPH, Some("g2"));
        assert!(!v.answer(&response(GRAPH, "g1", a_graph())));
        assert!(v.answer(&response(GRAPH, "g2", a_graph())));
        assert!(!v.answer(&response(SEARCH, "s1", hits((true, true), json!([])))));
        // Another command's answer is not the vault's.
        assert!(!v.answer(&response("get_codebase", "s2", json!({}))));
    }

    #[test]
    fn a_graph_asked_before_a_search_is_not_shown_after_it() {
        // The risk (#309 review): a note's relations, asked for from the
        // last search's hits, answered after a new search, so the view
        // shows a note that is not among the hits it sits beside. A search
        // answer starts the view over, so the graph question asked before
        // it is no longer the one the view waits on.
        let mut v = Vault::default();
        v.asked(SEARCH, Some("s1"));
        assert!(v.answer(&response(SEARCH, "s1", hits((true, true), json!([])))));
        v.asked(GRAPH, Some("g1"));
        v.asked(SEARCH, Some("s2"));
        let new_hits = hits(
            (true, true),
            json!([{"path": "new.md", "title": "N", "score": 1}]),
        );
        assert!(v.answer(&response(SEARCH, "s2", new_hits)));
        assert!(
            !v.answer(&response(GRAPH, "g1", a_graph())),
            "the earlier graph answer was taken after the new search"
        );
        let j = view_of(&v);
        assert_eq!(j["graph"], Value::Null, "{j}");
        assert_eq!(j["hits"][0]["path"], "new.md");
        // A graph asked from the new hits is still taken.
        v.asked(GRAPH, Some("g2"));
        assert!(v.answer(&response(GRAPH, "g2", a_graph())));
        assert_eq!(view_of(&v)["graph"]["note"]["path"], "child.md");
        // A refused search starts over too (`refused` answers id "x").
        v.asked(GRAPH, Some("g3"));
        v.asked(SEARCH, Some("x"));
        assert!(v.answer(&refused(SEARCH, "no vault")));
        assert!(!v.answer(&response(GRAPH, "g3", a_graph())));
        assert_eq!(view_of(&v)["graph"], Value::Null);
        // With no question recorded (the core records every one it
        // forwards), any answer is still taken, as before.
        let mut v = Vault::default();
        assert!(v.answer(&response(SEARCH, "s", hits((true, true), json!([])))));
        assert!(v.answer(&response(GRAPH, "g", a_graph())));
    }
}
