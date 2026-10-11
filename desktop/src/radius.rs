//! The symbol radius view, from a `get_radius` answer (#308).
//!
//! `trellis radius` prints a blast radius as two blocks: reach, the call
//! sites that reach the symbol, and doubt, every call site in the files the
//! radius touches that bound to nothing, under "doubt — a different
//! population, not a fraction of the reach". The two count different
//! populations and are never divided (ADR 0014, CLAUDE.md rule 3). DESK-14
//! made the answer data; this types it into the same two blocks, collapsed
//! as the printed report collapses them (the top three reasons and five
//! files, the rest counted), and the core emits the result as the `radius`
//! event. `ui/radius.js` only draws it.
//!
//! The rule is checked here too, not only in trellis: an answer carrying a
//! key with a ratio word in it (`ratio`, `percent`, `fraction`, `total`) is
//! treated as bad data and not shown, because a later trellis that added
//! one would put the 3.7% line back on screen. A figure the answer left out
//! is `null`, never 0. The symbol, file paths, reasons and trellis's own
//! messages are repository or trellis text, which the page writes through
//! `textContent` only (ADR 0007, S10).

use crate::frames::Response;
use serde_json::{Map, Value, json};

/// The reasons and files shown before the rest are counted, as `trellis
/// radius` prints them.
pub const REASONS_SHOWN: usize = 3;
pub const FILES_SHOWN: usize = 5;

/// The words no key of a radius may contain (CLAUDE.md rule 3; the same
/// list DESK-14's test holds trellis to), each as a word of the key.
const RATIO_WORDS: [&str; 4] = ["ratio", "percent", "fraction", "total"];

/// What may follow a ratio word in one word of a key: its plural, and
/// `percentage`.
const RATIO_ENDINGS: [&str; 4] = ["", "s", "age", "ages"];

/// Call sites that reach the symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Reach {
    pub resolved: Option<u64>,
    pub ambiguous: Option<u64>,
    pub symbols_reached: Option<u64>,
}

/// One reason a call site bound to nothing, or one file such sites are in.
#[derive(Debug, Clone, PartialEq)]
pub struct Counted {
    /// The reason's name, or the file's path.
    pub name: String,
    /// What the reason means, as trellis explains it; `None` for a file.
    pub explain: Option<String>,
    pub call_sites: Option<u64>,
}

/// The rows past those shown: how many there are, and the call sites they
/// hold between them, as `trellis radius` prints "(other) n across k
/// further reasons". `call_sites` is `None` when any of them carried no
/// count, rather than a sum that leaves one out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rest {
    pub rows: usize,
    pub call_sites: Option<u64>,
}

/// Every call site in the files the radius touches that bound to nothing:
/// a different population from the reach.
#[derive(Debug, Clone, PartialEq)]
pub struct Doubt {
    pub unbound_in_touched_files: Option<u64>,
    pub touched_files: Option<u64>,
    pub reasons: Vec<Counted>,
    pub reasons_not_shown: Option<Rest>,
    pub files: Vec<Counted>,
    pub files_not_shown: Option<Rest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Radius {
    pub symbol: Option<String>,
    pub file: Option<String>,
    pub reach: Reach,
    pub doubt: Doubt,
}

/// Whether one of `k`'s words is a ratio word. Words are split at every
/// character that is not a letter or digit and where lower case turns to
/// upper (`resolvedRatio`), so `duration_ms`, `iteration`, `declaration`
/// and `subtotal`, which only hold the letters, are not refused as if they
/// carried a ratio.
fn names_a_ratio(k: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut lower_before = false;
    for c in k.chars() {
        if !c.is_alphanumeric() {
            words.push(std::mem::take(&mut word));
            lower_before = false;
            continue;
        }
        if c.is_uppercase() && lower_before {
            words.push(std::mem::take(&mut word));
        }
        lower_before = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    words.push(word);
    words.iter().any(|w| {
        RATIO_WORDS.iter().any(|r| {
            w.strip_prefix(r)
                .is_some_and(|rest| RATIO_ENDINGS.contains(&rest))
        })
    })
}

/// The first key in `v`, at any depth, that contains a ratio word. The
/// Atlas refuses a radius by it too (`atlas.rs`).
pub(crate) fn ratio_key(v: &Value) -> Option<String> {
    match v {
        Value::Object(o) => o.iter().find_map(|(k, x)| {
            if names_a_ratio(k) {
                Some(k.clone())
            } else {
                ratio_key(x)
            }
        }),
        Value::Array(a) => a.iter().find_map(ratio_key),
        _ => None,
    }
}

fn count(o: Option<&Map<String, Value>>, k: &str) -> Option<u64> {
    o?.get(k).and_then(Value::as_u64)
}

fn text(o: Option<&Map<String, Value>>, k: &str) -> Option<String> {
    o?.get(k).and_then(Value::as_str).map(str::to_string)
}

/// The rows of `list` named by `name_key`, the first `shown` of them, and
/// the rest counted. A row with no string name is left out: there is
/// nothing to show it by.
fn rows(
    list: Option<&Value>,
    name_key: &str,
    with_explain: bool,
    shown: usize,
) -> (Vec<Counted>, Option<Rest>) {
    let all: Vec<Counted> = list
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_object)
                .filter_map(|o| {
                    Some(Counted {
                        name: o.get(name_key)?.as_str()?.to_string(),
                        explain: if with_explain {
                            text(Some(o), "explain")
                        } else {
                            None
                        },
                        call_sites: count(Some(o), "call_sites"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let rest = (all.len() > shown).then(|| Rest {
        rows: all.len() - shown,
        call_sites: all[shown..].iter().map(|c| c.call_sites).sum(),
    });
    (all.into_iter().take(shown).collect(), rest)
}

/// Types a `get_radius` answer's `data`, or says why it will not be shown.
pub fn read(data: &Value) -> Result<Radius, String> {
    if !data.is_object() {
        return Err("get_radius answered with no radius".into());
    }
    if let Some(k) = ratio_key(data) {
        return Err(format!(
            "get_radius answered with a key shaped like a ratio, `{k}`, so the answer is not \
             shown: reach and doubt count different populations and are never divided \
             (CLAUDE.md rule 3, ADR 0014)"
        ));
    }
    let top = data.as_object();
    let reach = data.get("reach").and_then(Value::as_object);
    let doubt = data.get("doubt").and_then(Value::as_object);
    let (reasons, reasons_not_shown) = rows(
        doubt.and_then(|d| d.get("reasons")),
        "reason",
        true,
        REASONS_SHOWN,
    );
    let (files, files_not_shown) = rows(
        doubt.and_then(|d| d.get("files")),
        "path",
        false,
        FILES_SHOWN,
    );
    Ok(Radius {
        symbol: text(top, "symbol"),
        file: text(top, "file"),
        reach: Reach {
            resolved: count(reach, "resolved"),
            ambiguous: count(reach, "ambiguous"),
            symbols_reached: count(reach, "symbols_reached"),
        },
        doubt: Doubt {
            unbound_in_touched_files: count(doubt, "unbound_in_touched_files"),
            touched_files: count(doubt, "touched_files"),
            reasons,
            reasons_not_shown,
            files,
            files_not_shown,
        },
    })
}

fn counted_json(c: &Counted) -> Value {
    json!({"name": c.name, "explain": c.explain, "call_sites": c.call_sites})
}

fn rest_json(r: Option<Rest>) -> Value {
    match r {
        Some(r) => json!({"rows": r.rows, "call_sites": r.call_sites}),
        None => Value::Null,
    }
}

impl Radius {
    pub fn to_json(&self) -> Value {
        let d = &self.doubt;
        json!({
            "symbol": self.symbol,
            "file": self.file,
            "reach": {
                "resolved": self.reach.resolved,
                "ambiguous": self.reach.ambiguous,
                "symbols_reached": self.reach.symbols_reached,
            },
            "doubt": {
                "unbound_in_touched_files": d.unbound_in_touched_files,
                "touched_files": d.touched_files,
                "reasons": d.reasons.iter().map(counted_json).collect::<Vec<_>>(),
                "reasons_not_shown": rest_json(d.reasons_not_shown),
                "files": d.files.iter().map(counted_json).collect::<Vec<_>>(),
                "files_not_shown": rest_json(d.files_not_shown),
            },
        })
    }
}

/// The `radius` event for a `get_radius` response: the two blocks, or the
/// reason there are none (an unknown or ambiguous symbol, a root with no
/// `.git` entry, bad data), as text. It carries the response's `id`, so the
/// page shows the answer to the question it asked last.
pub fn event(r: &Response) -> Value {
    let mut v = match (r.success, &r.data) {
        (Some(true), Some(data)) => match read(data) {
            Ok(radius) => radius.to_json(),
            Err(why) => json!({"error": why}),
        },
        (Some(true), None) => json!({"error": "get_radius answered with no radius"}),
        _ => json!({
            "error": r
                .error
                .clone()
                .unwrap_or_else(|| "get_radius was refused, and trellis gave no reason".into()),
        }),
    };
    v["id"] = json!(r.id);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `get_radius` answer as `index_cmd::radius_json` writes it.
    fn answer(reasons: usize, files: usize) -> Value {
        json!({
            "symbol": "app.core.f",
            "file": "app/core.py",
            "reach": {"resolved": 4, "ambiguous": 1, "symbols_reached": 6},
            "doubt": {
                "unbound_in_touched_files": 23,
                "touched_files": 7,
                "reasons": (0..reasons).map(|i| json!({
                    "reason": format!("Reason{i}"),
                    "call_sites": 10 - i as u64,
                    "explain": format!("why {i}"),
                })).collect::<Vec<_>>(),
                "files": (0..files).map(|i| json!({
                    "path": format!("app/m{i}.py"),
                    "call_sites": 9 - i as u64,
                })).collect::<Vec<_>>(),
            },
        })
    }

    fn response(success: bool, data: Option<Value>, error: Option<&str>) -> Response {
        Response {
            command: Some("get_radius".into()),
            success: Some(success),
            id: Some("desktop-radius-3".into()),
            data,
            error: error.map(str::to_string),
        }
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

    fn ui_file(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn radius_view_keeps_reach_and_doubt_apart() {
        // The risk (CLAUDE.md rule 3, ADR 0014): reach and doubt merged or
        // divided into one figure, which once read as a resolver twenty
        // times worse than audited; or the doubt's residue dropped, so the
        // screen understates it.
        let v = event(&response(true, Some(answer(5, 8)), None));
        assert_eq!(v["id"], "desktop-radius-3");
        assert_eq!(v["symbol"], "app.core.f");
        assert_eq!(
            v["reach"],
            json!({"resolved": 4, "ambiguous": 1, "symbols_reached": 6})
        );
        let d = &v["doubt"];
        assert_eq!(d["unbound_in_touched_files"], 23);
        assert_eq!(d["touched_files"], 7);
        assert_eq!(d["reasons"].as_array().unwrap().len(), REASONS_SHOWN);
        assert_eq!(
            d["reasons"][0],
            json!({"name": "Reason0", "explain": "why 0", "call_sites": 10})
        );
        // Reasons 3 and 4: 7 + 6 call sites, across two further reasons.
        assert_eq!(d["reasons_not_shown"], json!({"rows": 2, "call_sites": 13}));
        assert_eq!(d["files"].as_array().unwrap().len(), FILES_SHOWN);
        assert_eq!(d["files"][4]["name"], "app/m4.py");
        // Files 5, 6 and 7: 4 + 3 + 2.
        assert_eq!(d["files_not_shown"], json!({"rows": 3, "call_sites": 9}));
        // Two blocks, neither inside the other.
        for k in [
            "unbound_in_touched_files",
            "touched_files",
            "reasons",
            "files",
        ] {
            assert!(v["reach"].get(k).is_none(), "{k} in reach");
        }
        for k in ["resolved", "ambiguous", "symbols_reached"] {
            assert!(d.get(k).is_none(), "{k} in doubt");
        }
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| RATIO_WORDS.iter().any(|w| k.to_lowercase().contains(w)))
            .collect();
        assert!(bad.is_empty(), "the view derives a figure: {bad:?}");

        // Within the cut, nothing is said to be left out.
        let v = event(&response(true, Some(answer(3, 1)), None));
        assert_eq!(v["doubt"]["reasons_not_shown"], Value::Null);
        assert_eq!(v["doubt"]["files_not_shown"], Value::Null);

        // A count the answer did not carry is null, not 0; and a residue
        // with an uncounted row is not a sum that leaves it out.
        let mut a = answer(4, 0);
        a["reach"].as_object_mut().unwrap().remove("ambiguous");
        a["doubt"]["reasons"][3]
            .as_object_mut()
            .unwrap()
            .remove("call_sites");
        let v = event(&response(true, Some(a), None));
        assert_eq!(v["reach"]["ambiguous"], Value::Null);
        assert_eq!(v["reach"]["resolved"], 4);
        assert_eq!(
            v["doubt"]["reasons_not_shown"],
            json!({"rows": 1, "call_sites": null})
        );

        // radius.js draws the two blocks under trellis's own headings and
        // computes no percentage.
        let js = ui_file("radius.js");
        for want in [
            "not a fraction of the reach",
            "textContent",
            "\"get_radius\"",
        ] {
            assert!(js.contains(want), "radius.js lacks {want}");
        }
        for bad in ["%", "innerHTML", "toFixed", ".style"] {
            assert!(!js.contains(bad), "radius.js uses {bad}");
        }
    }

    #[test]
    fn a_ratio_key_makes_the_answer_bad_data() {
        // The risk: a later trellis adds a ratio beside the counts and the
        // desktop shows it, putting the 3.7% line back on screen.
        // (the key planted, the object it goes in, by JSON pointer)
        let cases = [
            ("resolved_ratio", "/reach"),
            ("Percent", ""),
            ("fraction_resolved", "/doubt"),
            ("total_sites", "/doubt/files/0"),
            ("call_sites_total", "/doubt/reasons/0"),
        ];
        for (key, at) in cases {
            let mut a = answer(1, 1);
            a.pointer_mut(at)
                .and_then(Value::as_object_mut)
                .unwrap_or_else(|| panic!("no object at {at:?}"))
                .insert(key.to_string(), json!(0.037));
            let v = event(&response(true, Some(a), None));
            let err = v["error"]
                .as_str()
                .unwrap_or_else(|| panic!("{key}: shown: {v}"));
            assert!(err.contains(&format!("`{key}`")), "{key}: {err}");
            assert!(err.contains("never divided"), "{err}");
            assert!(
                v.get("reach").is_none() && v.get("doubt").is_none(),
                "{key}: {v}"
            );
            assert_eq!(v["id"], "desktop-radius-3");
        }
        // A plain answer is not refused.
        assert!(read(&answer(2, 2)).is_ok());
    }

    #[test]
    fn ratio_words_match_whole_words_only() {
        // The risk (#308 review): a key matched on a substring, so a later
        // trellis that adds `duration_ms` (it holds "ratio") has every
        // radius refused as bad data; or a ratio spelt in camel case or
        // inflected slipping past a whole-word match.
        for fine in [
            "duration_ms",
            "iteration",
            "declaration",
            "subtotal",
            "call_sites",
            "touched_files",
        ] {
            assert!(!names_a_ratio(fine), "{fine} refused");
            let mut a = answer(1, 1);
            a["reach"][fine] = json!(1);
            assert!(read(&a).is_ok(), "{fine}: an answer refused");
        }
        for bad in [
            "ratio",
            "resolved_ratio",
            "Percent",
            "resolved_percentage",
            "fraction_resolved",
            "total_sites",
            "call_sites_total",
            "resolvedRatio",
            "doubtFraction",
            "totals",
            "resolved-percent",
        ] {
            assert!(names_a_ratio(bad), "{bad} passed");
        }
        // The note under the blocks cites where the figure comes from, as
        // the CLI's radius text does (index_cmd.rs, "Constraint 1").
        let js = ui_file("radius.js");
        assert!(
            js.contains("statically invisible in Python (Constraint 1)"),
            "radius.js gives the three-in-ten figure without its source"
        );
    }

    #[test]
    fn radius_script_draws_only_the_last_question_s_answer() {
        // The risk (#308 review): rpc answers each get_radius on its own
        // thread, so an older question's answer can come after the newer
        // one's. radius.js forgot the id it waited on once an answer came,
        // and the late older answer then replaced the newer one under the
        // newer symbol. No JavaScript runs under cargo test, so this holds
        // the text; a node smoke script with a fake DOM ran the module
        // (see the commit).
        let js = ui_file("radius.js");
        assert!(
            js.contains("if (payload.id !== waiting) return;"),
            "radius.js does not compare every answer with the last id asked"
        );
        let reset = js
            .lines()
            .filter(|l| l.contains("waiting = null") && !l.trim_start().starts_with("let "))
            .count();
        assert_eq!(reset, 0, "radius.js forgets the last id asked");
    }

    #[test]
    fn an_unknown_symbol_shows_trellis_message_as_text() {
        // The risk: trellis's refusal dropped, so an unknown symbol looks
        // like one nothing calls; or the message, which carries the user's
        // query, treated as markup.
        let msg = "no first-party definition matches `<img src=x onerror=alert(1)>`. Note that \
                   only definitions inside the repository root are indexed.";
        let v = event(&response(false, None, Some(msg)));
        assert_eq!(v, json!({"id": "desktop-radius-3", "error": msg}));
        let v = event(&response(false, None, None));
        assert!(v["error"].as_str().unwrap().contains("gave no reason"));
        let v = event(&response(true, None, None));
        assert!(v["error"].as_str().unwrap().contains("no radius"));

        // radius.js writes the error as text, never as markup.
        let js = ui_file("radius.js");
        assert!(js.contains("payload.error"), "radius.js shows no error");
        for bad in ["innerHTML", "outerHTML", "insertAdjacentHTML", "DOMParser"] {
            assert!(!js.contains(bad), "radius.js uses {bad}");
        }
    }
}
