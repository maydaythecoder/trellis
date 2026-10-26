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
