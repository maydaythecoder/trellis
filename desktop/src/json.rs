//! One set of readers for the fields of trellis's JSON (#822).
//!
//! The core reads every frame, answer and trail as `serde_json` values, and
//! each module wrote its own readers: six copies of "this key as a string,
//! or none" across frames.rs, atlas.rs, codebase.rs, radius.rs, queue.rs and
//! state/vault.rs. They live here once. A field that is missing or of
//! another type reads as `None`: the view then says it was not reported,
//! and never draws a default in its place (ADR 0048 section 8).
//!
//! An integer must also survive the trip to the page, whose JavaScript
//! numbers hold integers exactly only up to 2^53 - 1
//! (`Number.MAX_SAFE_INTEGER`). A larger one would be drawn as a different
//! number, so it reads as not reported instead.

use serde_json::{Map, Value};

/// The largest integer the page's numbers hold exactly: JavaScript's
/// `Number.MAX_SAFE_INTEGER`, 2^53 - 1.
pub const MAX_SAFE: u64 = (1 << 53) - 1;

/// `o[k]` as a string.
pub fn string(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).map(str::to_string)
}

/// `o[k]` as a whole number from 0 to [`MAX_SAFE`].
pub fn u64(o: &Map<String, Value>, k: &str) -> Option<u64> {
    o.get(k).and_then(Value::as_u64).filter(|n| *n <= MAX_SAFE)
}

/// `o[k]` as a whole number within [`MAX_SAFE`] of 0, either side: an exit
/// code, a count that may be negative.
pub fn i64(o: &Map<String, Value>, k: &str) -> Option<i64> {
    o.get(k)
        .and_then(Value::as_i64)
        .filter(|n| n.unsigned_abs() <= MAX_SAFE)
}

/// `o[k]` as a boolean.
pub fn bool(o: &Map<String, Value>, k: &str) -> Option<bool> {
    o.get(k).and_then(Value::as_bool)
}

/// `o[k]` as an array.
pub fn array<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a Vec<Value>> {
    o.get(k).and_then(Value::as_array)
}

/// "a task" as the owner of a field: "a task's", and "it" as "its".
fn owner(what: &str) -> String {
    if what == "it" {
        "its".to_string()
    } else {
        format!("{what}'s")
    }
}

/// `o[k]` as text, where an answer without it is bad data: the reason
/// names `what` holds the field, so a person reads which one was wrong
/// ("a task has no id", "a hit's path is not text").
pub fn text(o: &Map<String, Value>, k: &str, what: &str) -> Result<String, String> {
    string(o, k).ok_or_else(|| match o.get(k) {
        None | Some(Value::Null) => format!("{what} has no {k}"),
        Some(_) => format!("{} {k} is not text", owner(what)),
    })
}

/// `o[k]` as text or `null` (a contract's `null_when`), where anything else
/// is bad data, as [`text`]. Absent is not null: a field the answer left out
/// is not a claim that there is none.
pub fn text_or_null(o: &Map<String, Value>, k: &str, what: &str) -> Result<Option<String>, String> {
    match o.get(k) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        None => Err(format!("{what} has no {k}")),
        Some(_) => Err(format!("{} {k} is not text or null", owner(what))),
    }
}

/// `o[k]` as a list of text, where anything else is bad data, as [`text`].
pub fn texts(o: &Map<String, Value>, k: &str, what: &str) -> Result<Vec<String>, String> {
    let whose = owner(what);
    array(o, k)
        .ok_or_else(|| format!("{whose} {k} is not a list"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("an entry of {whose} {k} is not text"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("an object")
    }

    #[test]
    fn json_u64_refuses_past_two_to_the_fifty_three() {
        // The risk: a count past 2^53 - 1 reaches the page, whose number
        // for it is a different integer, and is drawn as a figure trellis
        // never reported.
        let o = obj(json!({"safe": MAX_SAFE, "past": MAX_SAFE + 1, "max": u64::MAX, "zero": 0}));
        assert_eq!(u64(&o, "safe"), Some(MAX_SAFE));
        assert_eq!(u64(&o, "past"), None);
        assert_eq!(u64(&o, "max"), None);
        assert_eq!(u64(&o, "zero"), Some(0));
        let o =
            obj(json!({"low": -(MAX_SAFE as i64), "under": -(MAX_SAFE as i64) - 1, "exit": -1}));
        assert_eq!(i64(&o, "low"), Some(-(MAX_SAFE as i64)));
        assert_eq!(i64(&o, "under"), None);
        assert_eq!(i64(&o, "exit"), Some(-1));
    }

    #[test]
    fn json_a_field_of_another_type_reads_as_absent() {
        // The risk: a reader that coerces ("3" to 3, 1 to true, a negative
        // to a count) draws a figure trellis did not report.
        let o =
            obj(json!({"s": "3", "n": 3, "neg": -3, "f": 1.5, "b": true, "a": [1], "null": null}));
        assert_eq!(string(&o, "n"), None);
        assert_eq!(u64(&o, "s"), None);
        assert_eq!(u64(&o, "neg"), None);
        assert_eq!(u64(&o, "f"), None);
        assert_eq!(i64(&o, "f"), None);
        assert_eq!(bool(&o, "n"), None);
        assert_eq!(array(&o, "s"), None);
        // A null is no value, for every reader.
        assert_eq!(string(&o, "null"), None);
        assert_eq!(u64(&o, "null"), None);
        assert_eq!(i64(&o, "null"), None);
        assert_eq!(bool(&o, "null"), None);
        assert_eq!(array(&o, "null"), None);
    }

    #[test]
    fn json_text_and_texts_say_which_field_was_wrong() {
        // The risk: bad data reported without the field that made it so,
        // which leaves a person nothing to look for in trellis's answer.
        let o = obj(json!({"id": "t1", "n": 3, "null": null, "ls": ["a", 2], "ok": ["a"]}));
        assert_eq!(text(&o, "id", "a task").as_deref(), Ok("t1"));
        assert_eq!(text(&o, "gone", "a task"), Err("a task has no gone".into()));
        assert_eq!(text(&o, "null", "a task"), Err("a task has no null".into()));
        assert_eq!(
            text(&o, "n", "a task"),
            Err("a task's n is not text".into())
        );
        assert_eq!(text(&o, "n", "it"), Err("its n is not text".into()));
        assert_eq!(text_or_null(&o, "null", "it"), Ok(None));
        assert_eq!(text_or_null(&o, "id", "it"), Ok(Some("t1".into())));
        assert_eq!(text_or_null(&o, "gone", "it"), Err("it has no gone".into()));
        assert_eq!(
            text_or_null(&o, "n", "a task"),
            Err("a task's n is not text or null".into())
        );
        assert_eq!(texts(&o, "ok", "it"), Ok(vec!["a".to_string()]));
        assert_eq!(texts(&o, "id", "it"), Err("its id is not a list".into()));
        assert_eq!(
            texts(&o, "ls", "a task"),
            Err("an entry of a task's ls is not text".into())
        );
    }

    #[test]
    fn json_a_missing_field_reads_as_absent_and_a_present_one_as_given() {
        let o = obj(json!({"s": "text", "n": 7, "b": false, "a": ["x"]}));
        assert_eq!(string(&o, "s").as_deref(), Some("text"));
        assert_eq!(u64(&o, "n"), Some(7));
        assert_eq!(i64(&o, "n"), Some(7));
        assert_eq!(bool(&o, "b"), Some(false));
        assert_eq!(array(&o, "a").map(Vec::len), Some(1));
        for k in ["missing", ""] {
            assert_eq!(string(&o, k), None);
            assert_eq!(u64(&o, k), None);
            assert_eq!(i64(&o, k), None);
            assert_eq!(bool(&o, k), None);
            assert_eq!(array(&o, k), None);
        }
    }
}
