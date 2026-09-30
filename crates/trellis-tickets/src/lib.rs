//! The ticketing standard (`docs/TICKETING.md`) as pure functions over one
//! GitHub issue (ADR 0040).
//!
//! One function per dimension: [`area`], [`kind`], [`risk`], [`surface`],
//! [`effort`], [`origin`], [`flags`], [`ready`] and [`triage`]. Each returns
//! an [`Outcome`]: the value and the rule that set it, or an abstention and
//! why. An abstention is not an error. The standard's design is "fail to
//! human, never guess" (§1), and a rule that cannot decide saying so is the
//! rule working.
//!
//! There is no I/O here. The caller fetches the issue (the `trellis-tickets`
//! commands do it by spawning `gh`) and hands over its text, so every rule is
//! tested from a string with no network and no `gh`.
//!
//! [`ready_with`] is `ready <n>`'s gate: G0's checks as
//! `scripts/gates/ticket-ready.sh` reads them, beside the named extras from
//! [`ready`]'s rule.

mod gate;
mod labels;
mod parse;
mod report;
mod rules;

pub use gate::{Abstention, ReadyReport, ready_gate, ready_with};
pub use labels::{Area, Effort, Flag, Kind, Origin, Risk, Route, Surface};
pub use report::{
    GH_VIEW_FIELDS, IntakeError, Line, Report, TriageError, Value, gh_view_args, report,
    ticket_from_gh_json, triage_with,
};
pub use rules::{
    Readiness, Triage, area, effort, flags, kind, origin, ready, risk, surface, triage,
};

/// The rule set's version, written as `triage_version` in every triage
/// block. Bump it when a rule changes, so a sweep re-triages everything
/// labelled under the old rules (standard §7).
pub const TRIAGE_VERSION: u32 = 2;

/// What one rule decided about one ticket.
///
/// `rule` names the part of the standard that applied, so `triage <n>` can
/// say why a label is there (§10: "a triage system you can't interrogate is
/// one you'll stop trusting").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<T> {
    Decided { value: T, rule: &'static str },
    Abstained { rule: &'static str, reason: String },
}

impl<T> Outcome<T> {
    /// The decided value, or `None` when the rule abstained.
    pub fn decided(&self) -> Option<&T> {
        match self {
            Outcome::Decided { value, .. } => Some(value),
            Outcome::Abstained { .. } => None,
        }
    }

    /// The rule that decided or abstained.
    pub fn rule(&self) -> &'static str {
        match self {
            Outcome::Decided { rule, .. } | Outcome::Abstained { rule, .. } => rule,
        }
    }
}

/// An issue's state on GitHub. The project board's Status (`draft`, `todo`,
/// ...) is not read: triage never changes status (§1), and the rules need
/// only whether the issue is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Closed,
}

/// One issue, as `gh issue view --json title,body,labels,state` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub title: String,
    pub body: String,
    /// Label names as they are on GitHub, e.g. `origin:manual`, `!security`.
    pub labels: Vec<String>,
    pub state: State,
}

impl Ticket {
    fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l == name)
    }
}
