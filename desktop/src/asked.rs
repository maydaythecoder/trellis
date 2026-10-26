//! Which answer the core takes, and how it words a refusal (#823).
//!
//! The page asks trellis rpc questions that are answered on threads of
//! their own, so the answer to an earlier question can arrive after the
//! answer to a later one, and showing it would show the wrong question's
//! answer. Each kind of question keeps the id it was last asked with, and
//! only that answer is taken. That filter was written three times
//! (queue.rs, state/vault.rs, atlas.rs); it is `Asked` here. A refused
//! command's text was worded in five places; it is `refusal` here, and
//! `error` is the `{"error": …}` event a view draws in its place.

use crate::frames::Response;
use serde_json::{Value, json};

/// The id the last question of one kind was asked with, or none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asked(Option<String>);

impl Asked {
    /// The page asked with `id`: from now on, only that answer is taken.
    pub fn ask(&mut self, id: Option<&str>) {
        self.0 = id.map(str::to_string);
    }

    /// Whether an answer with `id` is taken: the last question's, or any
    /// while nothing has been asked with an id (the board and the vault,
    /// which a test or an older page may ask without one).
    pub fn takes(&self, id: Option<&str>) -> bool {
        self.0.is_none() || self.0.as_deref() == id
    }

    /// Whether `id` is exactly the one the last question was asked with,
    /// none matching none: the Atlas, which asks every question with an
    /// id of its own and takes nothing else.
    pub fn is(&self, id: Option<&str>) -> bool {
        self.0.as_deref() == id
    }

    /// Whether a question was asked with an id.
    pub fn is_asked(&self) -> bool {
        self.0.is_some()
    }
}

/// What a refused `command` says: trellis's own words, or that it gave
/// none, so a refusal is never drawn as an empty view.
pub fn refusal(command: &str, r: &Response) -> String {
    r.error
        .clone()
        .unwrap_or_else(|| format!("{command} was refused, and trellis gave no reason"))
}

/// The event a view draws in place of an answer it could not take.
pub fn error(why: impl Into<String>) -> Value {
    json!({"error": why.into()})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(id: Option<&str>, error: Option<&str>) -> Response {
        Response {
            command: Some("get_queue".into()),
            id: id.map(str::to_string),
            success: Some(error.is_none()),
            error: error.map(str::to_string),
            data: None,
        }
    }

    #[test]
    fn asked_takes_only_the_last_question_s_answer() {
        // The risk: an earlier question's answer, arriving late, is drawn
        // as the answer to the question the person asked last.
        let mut a = Asked::default();
        assert!(
            a.takes(Some("q1")) && a.takes(None),
            "nothing asked takes any"
        );
        a.ask(Some("q1"));
        a.ask(Some("q2"));
        assert!(!a.takes(Some("q1")), "the earlier question's answer");
        assert!(!a.takes(None), "an answer with no id");
        assert!(a.takes(Some("q2")));
        a.ask(None);
        assert!(a.takes(Some("q1")), "asked with no id takes any again");
    }

    #[test]
    fn asked_is_matches_exactly_for_the_atlas() {
        let mut a = Asked::default();
        assert!(a.is(None) && !a.is(Some("desktop-atlas-1")) && !a.is_asked());
        a.ask(Some("desktop-atlas-1"));
        assert!(a.is(Some("desktop-atlas-1")) && !a.is(None) && a.is_asked());
    }

    #[test]
    fn asked_refusal_keeps_trellis_s_words_or_says_it_gave_none() {
        assert_eq!(
            refusal("get_queue", &response(None, Some("no .git entry"))),
            "no .git entry"
        );
        assert_eq!(
            refusal("get_queue", &response(None, None)),
            "get_queue was refused, and trellis gave no reason"
        );
        assert_eq!(error("why"), json!({"error": "why"}));
    }
}
