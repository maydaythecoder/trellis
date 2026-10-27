//! The open-issue list and blocker states, read once for `sweep` and
//! `index`: both start from what `gh` prints for [`crate::gh_list_args`]
//! and look up each `blocked_by` entry the list does not hold.
//!
//! What `gh` prints is untrusted input. A list longer than the ceiling
//! sweep acts on, one that names an issue twice, or one that holds an issue
//! that is not open is refused as a whole: either command reporting on a
//! partial or wrong list would read as a statement about every open issue.
//!
//! `gh api --help` says each `--paginate` page is a separate JSON array;
//! the list is read as a sequence of arrays, so one merged array parses
//! too. The REST issue list holds pull
//! requests too; they are dropped here, before any check counts them.

use std::collections::BTreeMap;

use crate::report::{GhIssue, ticket_from_gh_json};
use crate::sweep::SWEEP_LIST_LIMIT;
use crate::{State, Ticket, parse};

/// The list could not be read as every open issue.
#[derive(Debug)]
pub(crate) enum ListError {
    Json(serde_json::Error),
    Truncated { limit: usize },
    Duplicate { number: u64 },
    NotOpen { number: u64, state: String },
}

/// Every issue in `list_json`, in the order `gh` listed them, without the
/// pull requests.
pub(crate) fn open_issues(list_json: &str) -> Result<Vec<(u64, Ticket)>, ListError> {
    let mut pages = serde_json::Deserializer::from_str(list_json).into_iter::<Vec<GhIssue>>();
    // No page at all is an empty output, not an empty list (`gh` prints
    // `[]` for that): parsed as one document, it fails as such.
    let first = pages
        .next()
        .unwrap_or_else(|| serde_json::from_str::<Vec<GhIssue>>(list_json));
    let mut issues: Vec<GhIssue> = Vec::new();
    for page in std::iter::once(first).chain(pages) {
        issues.extend(
            page.map_err(ListError::Json)?
                .into_iter()
                .filter(|i| i.pull_request.is_none()),
        );
    }
    if issues.len() >= SWEEP_LIST_LIMIT {
        return Err(ListError::Truncated {
            limit: SWEEP_LIST_LIMIT,
        });
    }
    let mut open: Vec<(u64, Ticket)> = Vec::with_capacity(issues.len());
    for issue in issues {
        let number = issue.number;
        if open.iter().any(|(n, _)| *n == number) {
            return Err(ListError::Duplicate { number });
        }
        let state = issue.state.clone();
        match issue.into_ticket() {
            Some(t) if t.state == State::Open => open.push((number, t)),
            _ => return Err(ListError::NotOpen { number, state }),
        }
    }
    Ok(open)
}

/// What every blocker's state is known to be: the open list's, then each
/// other one read once. A read that fails is kept as its reason, so the
/// rules see the blocker as unsupplied and abstain rather than proceed.
pub(crate) struct Blockers {
    known: BTreeMap<u64, Result<State, String>>,
}

impl Blockers {
    pub(crate) fn new(open: &[(u64, Ticket)]) -> Blockers {
        Blockers {
            known: open.iter().map(|(n, _)| (*n, Ok(State::Open))).collect(),
        }
    }

    /// The state of each blocker `body` names, reading through `fetch` any
    /// not already known, and a note for each that could not be read. An
    /// unreadable `blocked_by` reads nothing: the rules re-read it and
    /// abstain with the reason.
    pub(crate) fn of(
        &mut self,
        body: &str,
        fetch: &mut dyn FnMut(u64) -> Result<String, String>,
    ) -> (BTreeMap<u64, State>, Vec<String>) {
        let mut blockers = BTreeMap::new();
        let mut notes = Vec::new();
        if let Ok(named) = parse::blocked_by(body) {
            for b in named {
                let state = self.known.entry(b).or_insert_with(|| {
                    fetch(b).and_then(|json| {
                        ticket_from_gh_json(b, &json)
                            .map(|x| x.state)
                            .map_err(|e| e.to_string())
                    })
                });
                match state {
                    Ok(s) => {
                        blockers.insert(b, *s);
                    }
                    Err(reason) => notes.push(format!("blocker #{b} could not be read: {reason}")),
                }
            }
        }
        (blockers, notes)
    }
}
