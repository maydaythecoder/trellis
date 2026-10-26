//! Rows a run streams to the page, sent only once each (#827).
//!
//! The transcript and the trellis messages pane grow for as long as a run
//! lasts. Each view used to carry the whole of them, so once the cap was
//! full every new line redrew two thousand rows. A [`Feed`] keeps its rows
//! in a [`Capped`] store, held to a count and to bytes as before, numbers
//! each as it comes, and puts in a view only the rows the page has not been
//! sent, plus any row changed since it was (a steer once it is delivered).
//! A view that carries every pane carries the whole feed instead: it is for
//! a page that has drawn nothing yet, or one that asked for the run again
//! (`get_state`), and the page replaces what it holds with it.
//!
//! Who uses it: `state.rs`, for the transcript's typed rows and for stderr.
//! The page's side is dom.js's `drawFeed`.

use crate::capped::{Capped, Item};
use serde_json::{Map, Value, json};

/// What a feed's row is in a view: an object, to which the feed adds the
/// row's number as `seq`.
pub trait Row: Item {
    fn to_json(&self) -> Map<String, Value>;
}

/// stderr's rows are its lines, as text.
impl Row for String {
    fn to_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("text".into(), json!(self));
        m
    }
}

/// A row and its number. Numbers only grow, so the page tells a row it has
/// not drawn (a number past its last) from one sent again because it
/// changed (a number it holds).
#[derive(Debug)]
struct Numbered<T> {
    seq: u64,
    row: T,
}

impl<T: Item> Item for Numbered<T> {
    fn bytes(&self) -> usize {
        self.row.bytes()
    }

    fn fit(self, max: usize) -> Self {
        Self {
            seq: self.seq,
            row: self.row.fit(max),
        }
    }
}

/// Rows held to `MAX_LEN` and `MAX_BYTES`, each sent to the page once, and
/// again only when it changes.
#[derive(Debug)]
pub struct Feed<const MAX_LEN: usize, const MAX_BYTES: usize, T> {
    rows: Capped<MAX_LEN, MAX_BYTES, Numbered<T>>,
    /// The number the next row takes.
    next: u64,
    /// Every row numbered below this one has been sent.
    sent: u64,
    /// Rows changed after they were sent, to send again.
    changed: Vec<u64>,
    /// Set when the rows were cleared: the next view is the whole feed, so
    /// the page drops what it drew.
    whole: bool,
}

impl<const MAX_LEN: usize, const MAX_BYTES: usize, T> Default for Feed<MAX_LEN, MAX_BYTES, T> {
    fn default() -> Self {
        Self {
            rows: Capped::default(),
            next: 0,
            sent: 0,
            changed: Vec::new(),
            whole: false,
        }
    }
}

impl<const MAX_LEN: usize, const MAX_BYTES: usize, T: Row> Feed<MAX_LEN, MAX_BYTES, T> {
    /// Adds `row` as the newest.
    pub fn push(&mut self, row: T) {
        let seq = self.next;
        self.next += 1;
        self.rows.push(Numbered { seq, row });
    }

    /// Changes the newest row `pick` chooses with `change`, which must leave
    /// its size as it was (`Capped::change`), and sends it again in the next
    /// view. Answers whether one was chosen.
    pub fn change(&mut self, pick: impl Fn(&T) -> bool, change: impl FnOnce(&mut T)) -> bool {
        let mut seq = None;
        let found = self.rows.change(
            |n| pick(&n.row),
            |n| {
                seq = Some(n.seq);
                change(&mut n.row);
            },
        );
        if let Some(seq) = seq.filter(|s| *s < self.sent && !self.changed.contains(s)) {
            self.changed.push(seq);
        }
        found
    }

    /// The rows, oldest first.
    pub fn rows(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.rows.items().map(|n| &n.row)
    }

    /// Empties the feed. Numbers go on from where they were, and the next
    /// view is the whole feed, so the page drops what it drew.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.changed.clear();
        self.whole = true;
    }

    /// The feed as the page receives it: `rows` in order, each with its
    /// `seq`; `whole` when they replace what the page holds (asked with
    /// `whole`, or after a clear), otherwise only the rows not sent yet and
    /// those changed since; and `keep`, the cap the page holds its rows to.
    /// Marks what it carries as sent.
    pub fn view(&mut self, whole: bool) -> Value {
        let whole = whole || std::mem::take(&mut self.whole);
        let rows: Vec<Value> = self
            .rows
            .items()
            .filter(|n| whole || n.seq >= self.sent || self.changed.contains(&n.seq))
            .map(|n| {
                let mut m = n.row.to_json();
                m.insert("seq".into(), json!(n.seq));
                Value::Object(m)
            })
            .collect();
        self.sent = self.next;
        self.changed.clear();
        json!({"whole": whole, "rows": rows, "keep": MAX_LEN})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seqs(v: &Value) -> Vec<u64> {
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["seq"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn a_feed_sends_each_row_once_and_a_changed_row_again() {
        // The risk (#827): every view carrying the whole feed, so a capped
        // transcript redraws two thousand rows for each new one; or a row
        // changed after it was sent (a steer delivered) never reaching the
        // page; or a resend that leaves rows out.
        let mut f = Feed::<4, 1024, String>::default();
        f.push("a".into());
        f.push("b".into());
        let v = f.view(false);
        assert_eq!(seqs(&v), [0, 1]);
        assert_eq!(v["whole"], false);
        assert_eq!(v["keep"], 4);
        // Nothing new, nothing sent.
        assert_eq!(seqs(&f.view(false)), Vec::<u64>::new());
        f.push("c".into());
        assert!(f.change(|r| r == "a", |r| r.make_ascii_uppercase()));
        let v = f.view(false);
        assert_eq!(seqs(&v), [0, 2], "the new row and the changed one");
        assert_eq!(v["rows"][0]["text"], "A");
        // A row not sent yet that changes goes once, as new.
        f.push("d".into());
        assert!(f.change(|r| r == "d", |r| r.make_ascii_uppercase()));
        assert_eq!(seqs(&f.view(false)), [3]);
        // Asked whole, every row it holds.
        assert_eq!(seqs(&f.view(true)), [0, 1, 2, 3]);
        // Past the cap the oldest go, and numbers go on.
        f.push("e".into());
        assert_eq!(seqs(&f.view(false)), [4]);
        assert_eq!(seqs(&f.view(true)), [1, 2, 3, 4]);
        // Cleared, the next view is whole and empty, so the page drops its
        // rows; then rows come numbered on.
        f.clear();
        let v = f.view(false);
        assert_eq!((v["whole"].clone(), seqs(&v)), (json!(true), vec![]));
        f.push("f".into());
        let v = f.view(false);
        assert_eq!((v["whole"].clone(), seqs(&v)), (json!(false), vec![5]));
    }

    #[test]
    fn a_change_that_moves_a_rows_size_is_refused() {
        // The risk: a row grown behind the store's back, so the byte cap no
        // longer bounds what the feed holds.
        let mut f = Feed::<4, 1024, String>::default();
        f.push("a".into());
        let grew = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.change(|r| r == "a", |r| r.push_str("longer"))
        }));
        assert!(grew.is_err(), "a row grew past what the cap counted");
    }
}
