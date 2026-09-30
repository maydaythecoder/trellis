//! `trellis-tickets ready <n>`, less the spawn (ADR 0040 decision 4, #78).
//!
//! Two sets of checks, kept apart:
//!
//! - **G0's**, read as `scripts/gates/ticket-ready.sh` reads them, with its
//!   failure strings. That script stays the gate until a parity check under
//!   `scripts/checks/` shows the two agree on every open issue (ADR 0040
//!   decision 7), and that comparison is over these checks only, so they
//!   follow the script's matching rather than `parse`'s stricter readings.
//! - **The four named extras** (`no verify command`, `decision words`,
//!   `effort:l`, `needs ADR`), from the rules in `rules.rs` and with their
//!   strings.
//!
//! Every check runs, so every failure is printed at once. `effort:l` and
//! `needs ADR` come from rules that can abstain (an unreadable `**Files:**`
//! line, a path no table maps: #194, #196). An abstention is reported as
//! such, never as a pass or a failure, and it keeps the gate from passing:
//! a check that could not be made is not one that held.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use crate::labels::Effort;
use crate::report::{TriageError, printable, ticket_from_gh_json};
use crate::rules::needs_adr;
use crate::{Outcome, State, Ticket, effort, parse};

/// One of the extras whose rule could not decide, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abstention {
    /// The failure name the check would report: `effort:l` or `needs ADR`.
    pub check: &'static str,
    pub reason: String,
}

/// What `ready <n>` found for one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyReport {
    pub number: u64,
    pub title: String,
    /// G0's failures, in the order the script lists them.
    pub g0: Vec<String>,
    /// The failures of the four extras.
    pub extra: Vec<String>,
    pub abstained: Vec<Abstention>,
}

impl ReadyReport {
    /// Ready only when no check failed and none abstained.
    pub fn is_ready(&self) -> bool {
        self.g0.is_empty() && self.extra.is_empty() && self.abstained.is_empty()
    }

    /// A heading, then one line per failure and per abstention. Issue text
    /// quoted in either is escaped: the title and body are anyone's.
    pub fn text(&self) -> String {
        let verdict = if self.is_ready() {
            "ready"
        } else {
            "not ready"
        };
        let mut out = format!("#{} {}: {verdict}\n", self.number, printable(&self.title));
        for f in self.g0.iter().chain(&self.extra) {
            out.push_str(&format!("  - {}\n", printable(f)));
        }
        for a in &self.abstained {
            out.push_str(&format!(
                "  - abstained: {} ({})\n",
                a.check,
                printable(&a.reason)
            ));
        }
        out
    }
}

/// G0's `grep -qE '^## Done when'`: a line that starts with the heading.
pub(crate) fn has_done_when(body: &str) -> bool {
    body.lines().any(|l| l.starts_with("## Done when"))
}

/// G0's awk: a line starting with three backticks anywhere after the first
/// line starting with `## Verify`.
pub(crate) fn has_fenced_verify(body: &str) -> bool {
    body.lines()
        .skip_while(|l| !l.starts_with("## Verify"))
        .skip(1)
        .any(|l| l.starts_with("```"))
}

/// G0's `grep -qE '^blocked_by: \[[^]]'`: a `blocked_by` list whose first
/// character is not the closing bracket.
fn has_blocked_by(body: &str) -> bool {
    body.lines().any(|l| {
        l.strip_prefix("blocked_by: [")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c != ']')
    })
}

/// G0's `sed -n 's/^blocked_by: \[\(.*\)\]/\1/p' | tr ',"' '  '` and the
/// shell's word split: every word between `blocked_by: [` and the last `]`
/// on each such line, and whatever follows that bracket.
///
/// Unlike `parse::blocked_by`, a word that is not a number is returned, not
/// an error: G0 names it as a failure and still looks up the others.
pub(crate) fn g0_blocked_by(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("blocked_by: [") else {
            continue;
        };
        let Some(close) = rest.rfind(']') else {
            continue;
        };
        let kept = format!("{}{}", &rest[..close], &rest[close + 1..]);
        out.extend(
            kept.split(|c: char| c == ',' || c == '"' || c.is_whitespace())
                .filter(|w| !w.is_empty())
                .map(str::to_string),
        );
    }
    out
}

/// A `blocked_by` word G0 looks up: digits only (its `*[!0-9]*` case).
fn issue_number(word: &str) -> Option<u64> {
    if word.bytes().all(|b| b.is_ascii_digit()) {
        word.parse().ok()
    } else {
        None
    }
}

/// Every G0 check and every extra over `t`.
///
/// `blockers` holds what was learned of each `blocked_by` issue: its state,
/// or why it could not be read. One missing is reported as G0 reports a
/// lookup it could not make, `UNKNOWN`: an unknown dependency is not a
/// satisfied one.
pub fn ready_gate(
    number: u64,
    t: &Ticket,
    blockers: &BTreeMap<u64, Result<State, String>>,
) -> ReadyReport {
    let has = |l: &str| t.labels.iter().any(|x| x == l);
    let mut g0 = Vec::new();
    if t.state == State::Closed {
        g0.push("issue is CLOSED".to_string());
    }
    if !has_done_when(&t.body) {
        g0.push("no '## Done when' section".to_string());
    }
    if !has_fenced_verify(&t.body) {
        g0.push("no fenced command under '## Verify'".to_string());
    }
    if has("triage:blocked") && !has_blocked_by(&t.body) {
        g0.push("labelled triage:blocked with no blocked_by".to_string());
    }
    if has("parked") {
        g0.push("parked: waits on the owner's answer".to_string());
    }
    if has("triage:needs-human") {
        g0.push("labelled triage:needs-human: a person routes this one".to_string());
    }
    if has("split") {
        g0.push("split: work happens in its child tickets".to_string());
    }
    for word in g0_blocked_by(&t.body) {
        let Some(n) = issue_number(&word) else {
            g0.push(format!("blocked_by entry '{word}' is not an issue number"));
            continue;
        };
        match blockers.get(&n) {
            Some(Ok(State::Closed)) => {}
            Some(Ok(State::Open)) => g0.push(format!("blocked_by #{n} is OPEN")),
            Some(Err(reason)) => g0.push(format!("blocked_by #{n} is UNKNOWN ({reason})")),
            None => g0.push(format!(
                "blocked_by #{n} is UNKNOWN (its state was not looked up)"
            )),
        }
    }

    let mut extra = Vec::new();
    let mut abstained = Vec::new();
    if parse::verify_commands(&t.body).is_empty() {
        extra.push("no verify command".to_string());
    }
    let words = parse::decision_words(&t.title, &t.body);
    if !words.is_empty() {
        extra.push(format!("decision words: {}", words.join(", ")));
    }
    match effort(t) {
        Outcome::Decided {
            value: Effort::L, ..
        } => extra.push("effort:l".to_string()),
        Outcome::Decided { .. } => {}
        Outcome::Abstained { reason, .. } => abstained.push(Abstention {
            check: "effort:l",
            reason,
        }),
    }
    match needs_adr(t) {
        Ok(true) => extra.push("needs ADR".to_string()),
        Ok(false) => {}
        Err(reason) => abstained.push(Abstention {
            check: "needs ADR",
            reason,
        }),
    }

    ReadyReport {
        number,
        title: t.title.clone(),
        g0,
        extra,
        abstained,
    }
}

/// `ready` for issue `number`, reading every issue through `fetch`, which
/// returns what `gh issue view <n> --json` [`crate::GH_VIEW_FIELDS`]
/// prints (the argv [`crate::gh_view_args`] builds, the same read `triage`
/// makes).
///
/// `fetch` is called for `number`, then once for each distinct issue
/// number G0 would look up in its `blocked_by`, and for nothing else. The
/// issue itself failing to read is an error, as it is for G0. A blocker
/// failing to read is a named failure carrying the reason, as G0 reports
/// `UNKNOWN` for it.
pub fn ready_with(
    number: u64,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
) -> Result<ReadyReport, TriageError> {
    let json = fetch(number).map_err(|reason| TriageError::Fetch { number, reason })?;
    let t = ticket_from_gh_json(number, &json)?;
    let mut blockers = BTreeMap::new();
    for n in g0_blocked_by(&t.body)
        .iter()
        .filter_map(|w| issue_number(w))
    {
        if let Entry::Vacant(e) = blockers.entry(n) {
            let state = fetch(n).and_then(|json| {
                ticket_from_gh_json(n, &json)
                    .map(|b| b.state)
                    .map_err(|e| e.to_string())
            });
            e.insert(state);
        }
    }
    Ok(ready_gate(number, &t, &blockers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g0_blocked_by_keeps_what_the_script_would_split() {
        // The script drops only `,` and `"`; a quoted entry is a number to
        // it, and a word after the bracket is still an entry.
        assert_eq!(
            g0_blocked_by("blocked_by: [75, \"76\"]\nblocked_by: [child-a] x"),
            ["75", "76", "child-a", "x"]
        );
        assert!(g0_blocked_by("blocked_by: []").is_empty());
        assert!(!has_blocked_by("blocked_by: []"));
        assert!(has_blocked_by("blocked_by: [75]"));
    }
}
