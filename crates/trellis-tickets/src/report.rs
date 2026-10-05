//! `trellis-tickets triage <n>`, less the spawn (ADR 0040, #77).
//!
//! The command fetches an issue's JSON, fetches the state of each issue its
//! triage block names in `blocked_by`, runs every rule, and prints each
//! value beside the rule that set it, or the abstention and its reason
//! (standard §10: "a triage system you can't interrogate is one you'll stop
//! trusting"). Fetching is a closure, so the whole path is tested over
//! recorded JSON with no `gh`; the binary passes one that spawns it.
//!
//! What `gh` prints is untrusted input. It is checked to be the issue asked
//! for, in a state the rules know, before any rule reads it.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use serde::{Deserialize, Serialize};

use crate::labels::Flag;
use crate::{
    Outcome, State, TRIAGE_VERSION, Ticket, area, effort, flags, kind, origin, parse, ready, risk,
    surface, triage,
};

/// The fields `triage` and `ready` ask `gh` for. `labels` comes back as objects; only
/// their `name` is read.
pub const GH_VIEW_FIELDS: &str = "number,title,body,labels,state";

/// The one `gh` argv `triage` and `ready` run, per issue: a read. Nothing
/// in either command writes to GitHub, and this is the only argv they build.
pub fn gh_view_args(number: u64) -> [String; 5] {
    [
        "issue".to_string(),
        "view".to_string(),
        number.to_string(),
        "--json".to_string(),
        GH_VIEW_FIELDS.to_string(),
    ]
}

/// `gh`'s output could not be read as the issue that was asked for.
#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    #[error("the output for #{asked} is not the issue JSON triage reads: {source}")]
    Json {
        asked: u64,
        #[source]
        source: serde_json::Error,
    },
    #[error("asked for #{asked} and got #{got}")]
    WrongIssue { asked: u64, got: u64 },
    #[error("#{number} has state `{state}`; the rules read OPEN or CLOSED")]
    UnknownState { number: u64, state: String },
}

/// Fetching or reading an issue failed, so no triage was produced.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    #[error("fetching #{number}: {reason}")]
    Fetch { number: u64, reason: String },
    #[error(transparent)]
    Intake(#[from] IntakeError),
}

/// One issue as `gh issue view --json` and `gh issue list --json` print it
/// with [`GH_VIEW_FIELDS`].
#[derive(Deserialize)]
pub(crate) struct GhIssue {
    pub(crate) number: u64,
    title: String,
    body: String,
    labels: Vec<GhLabel>,
    pub(crate) state: String,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

impl GhIssue {
    /// The ticket, or `None` when the state is neither of the two the rules
    /// read.
    pub(crate) fn into_ticket(self) -> Option<Ticket> {
        let state = match self.state.as_str() {
            "OPEN" => State::Open,
            "CLOSED" => State::Closed,
            _ => return None,
        };
        Some(Ticket {
            title: self.title,
            body: self.body,
            labels: self.labels.into_iter().map(|l| l.name).collect(),
            state,
        })
    }
}

/// Reads `gh issue view <asked> --json number,title,body,labels,state`.
///
/// Refuses output for another issue: a blocker's state read from the wrong
/// issue would unblock a ticket that is still waiting.
pub fn ticket_from_gh_json(asked: u64, json: &str) -> Result<Ticket, IntakeError> {
    let issue: GhIssue =
        serde_json::from_str(json).map_err(|source| IntakeError::Json { asked, source })?;
    if issue.number != asked {
        return Err(IntakeError::WrongIssue {
            asked,
            got: issue.number,
        });
    }
    let state = issue.state.clone();
    issue.into_ticket().ok_or(IntakeError::UnknownState {
        number: asked,
        state,
    })
}

/// A decided value, as `triage` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Value {
    Label(&'static str),
    Flags(Vec<&'static str>),
    Ready {
        ready: bool,
        failures: Vec<String>,
    },
    Route {
        route: &'static str,
        reasons: Vec<String>,
    },
}

/// One dimension's line: the value and its rule, or the abstention's rule
/// and reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Line {
    Decided { value: Value, rule: &'static str },
    Abstained { rule: &'static str, reason: String },
}

impl Line {
    fn from<T>(o: Outcome<T>, show: impl FnOnce(T) -> Value) -> Line {
        match o {
            Outcome::Decided { value, rule } => Line::Decided {
                value: show(value),
                rule,
            },
            Outcome::Abstained { rule, reason } => Line::Abstained { rule, reason },
        }
    }
}

/// Every rule's outcome for one issue. Serialized as the one object
/// `triage --json` prints; the field order is the triage block's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub number: u64,
    pub title: String,
    pub triage_version: u32,
    pub area: Line,
    pub kind: Line,
    pub risk: Line,
    pub surface: Line,
    pub effort: Line,
    pub origin: Line,
    pub flags: Line,
    pub ready: Line,
    pub triage: Line,
}

/// Runs every rule over `t`. `blockers` holds the state of each issue in
/// its `blocked_by`; one missing makes `ready` and `triage` abstain.
pub fn report(number: u64, t: &Ticket, blockers: &BTreeMap<u64, State>) -> Report {
    Report {
        number,
        title: t.title.clone(),
        triage_version: TRIAGE_VERSION,
        area: Line::from(area(t), |v| Value::Label(v.label())),
        kind: Line::from(kind(t), |v| Value::Label(v.label())),
        risk: Line::from(risk(t), |v| Value::Label(v.label())),
        surface: Line::from(surface(t), |v| Value::Label(v.label())),
        effort: Line::from(effort(t), |v| Value::Label(v.label())),
        origin: Line::from(origin(t), |v| Value::Label(v.label())),
        flags: Line::from(flags(t), |v| {
            Value::Flags(v.into_iter().map(Flag::label).collect())
        }),
        ready: Line::from(ready(t, blockers), |v| Value::Ready {
            ready: v.is_ready(),
            failures: v.failures,
        }),
        triage: Line::from(triage(t, blockers), |v| Value::Route {
            route: v.route.label(),
            reasons: v.reasons,
        }),
    }
}

/// Triage of issue `number`, reading every issue through `fetch`, which
/// returns what `gh issue view <n> --json` [`GH_VIEW_FIELDS`] prints.
///
/// `fetch` is called for `number`, then once for each distinct issue in its
/// `blocked_by`, and for nothing else. A `blocked_by` that cannot be read
/// fetches no blockers.
pub fn triage_with(
    number: u64,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
) -> Result<Report, TriageError> {
    let t = fetch_ticket(number, &mut fetch)?;
    let blockers = blocker_states(&t.body, &mut fetch)?;
    Ok(report(number, &t, &blockers))
}

fn fetch_ticket(
    n: u64,
    fetch: &mut dyn FnMut(u64) -> Result<String, String>,
) -> Result<Ticket, TriageError> {
    let json = fetch(n).map_err(|reason| TriageError::Fetch { number: n, reason })?;
    Ok(ticket_from_gh_json(n, &json)?)
}

/// The state of each distinct issue in `body`'s `blocked_by`, each read
/// once through `fetch`. `triage` and `new` both hand this to the rules.
pub(crate) fn blocker_states(
    body: &str,
    fetch: &mut dyn FnMut(u64) -> Result<String, String>,
) -> Result<BTreeMap<u64, State>, TriageError> {
    let mut blockers = BTreeMap::new();
    // An unreadable `blocked_by` is not dropped: the rules re-read it and
    // abstain with the reason, and that abstention is what gets printed.
    if let Ok(named) = parse::blocked_by(body) {
        for n in named {
            if let Entry::Vacant(e) = blockers.entry(n) {
                e.insert(fetch_ticket(n, fetch)?.state);
            }
        }
    }
    Ok(blockers)
}

/// Issue text with control characters escaped, so a title or body quoted in
/// a reason cannot move the cursor or recolour the terminal it prints to.
pub fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// One dimension's printed line: the value and the rule that set it, or
/// `abstained:` and the reason, then the rule that abstained.
pub(crate) fn line_text(name: &str, line: &Line) -> String {
    let (what, rule) = match line {
        Line::Decided { value, rule } => (shown(value), rule),
        Line::Abstained { rule, reason } => (format!("abstained: {reason}"), rule),
    };
    format!("{name:<8}{}  (rule: {rule})\n", printable(&what))
}

fn shown(v: &Value) -> String {
    match v {
        Value::Label(l) => (*l).to_string(),
        Value::Flags(f) if f.is_empty() => "(none)".to_string(),
        Value::Flags(f) => f.join(" "),
        Value::Ready { ready: true, .. } => "ready".to_string(),
        Value::Ready { failures, .. } => format!("not ready: {}", failures.join("; ")),
        Value::Route { route, reasons } => format!("{route}: {}", reasons.join("; ")),
    }
}

impl Report {
    /// The lines in the order printed, each with its dimension's name.
    pub fn lines(&self) -> [(&'static str, &Line); 9] {
        [
            ("area", &self.area),
            ("kind", &self.kind),
            ("risk", &self.risk),
            ("surface", &self.surface),
            ("effort", &self.effort),
            ("origin", &self.origin),
            ("flags", &self.flags),
            ("ready", &self.ready),
            ("triage", &self.triage),
        ]
    }

    /// A heading, then one line per dimension: the value and the rule that
    /// set it, or `abstained:` and the reason, then the rule that abstained.
    pub fn text(&self) -> String {
        let mut out = format!(
            "#{} {} (triage_version {})\n",
            self.number,
            printable(&self.title),
            self.triage_version
        );
        for (name, line) in self.lines() {
            out.push_str(&line_text(name, line));
        }
        out
    }

    /// One JSON object, with DEL and the C1 range escaped as in
    /// [`escape_json_controls`]: `triage --json` prints it to a terminal.
    pub fn json(&self) -> String {
        // A struct of strings, vectors and unit-tagged enums has no map with
        // non-string keys and no failing `Serialize`, so this cannot fail.
        escape_json_controls(&serde_json::to_string(self).expect("a Report always serializes"))
    }
}

/// `serde_json` escapes the C0 controls inside strings but not DEL or the
/// C1 range, and U+009B alone starts a terminal control sequence. Those are
/// escaped as `\u` sequences here, so the document still parses to the same
/// strings and cannot drive the terminal it is printed to. A newline is
/// left as it is: in serialized JSON it can only be layout between tokens.
pub(crate) fn escape_json_controls(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_control() && c != '\n' {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_json_escapes_del_and_the_c1_controls_in_a_title() {
        // U+009B is a one-byte CSI: printed raw by `triage --json`, a title
        // could drive the terminal of whoever reads the report.
        let t = Ticket {
            title: "a\u{9b}2Jb\u{7f}c\u{85}".to_string(),
            body: String::new(),
            labels: Vec::new(),
            state: State::Open,
        };
        let json = report(1, &t, &BTreeMap::new()).json();
        assert!(!json.chars().any(char::is_control), "{json:?}");
        assert!(json.contains("a\\u009b2Jb\\u007fc\\u0085"), "{json}");
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back["title"], t.title);
    }
}
