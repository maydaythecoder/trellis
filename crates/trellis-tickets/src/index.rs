//! `trellis-tickets index`, less the spawns (ADR 0040 decision 4, #81).
//!
//! Every open issue's triage as one JSON document, so two runs can be
//! diffed: a rule change shows up as the lines it moved and nothing else.
//! That is only true if a re-run over the same issues is byte for byte the
//! same, so:
//!
//! - **Order is the index's, not `gh`'s.** Issues are sorted by story
//!   number, then issue number; an issue with no story comes after every
//!   story. Recorded labels are sorted.
//! - **Nothing depends on when it ran.** There is no `triaged_at`; the
//!   triage block's timestamp is what a run writes, and a diff full of new
//!   timestamps hides the one label that moved.
//! - **No hash-ordered collection is on the path.**
//!
//! It reports what the rules compute and abstains where they cannot see:
//! `!agent-ok` is always abstained (#195), and a computed `risk:` below the
//! recorded one is abstained with every line that reads it (#196), as
//! `sweep` holds the same label. An area the table does not map (#194)
//! arrives from the rule already abstained.

use serde::Serialize;

use crate::open::{Blockers, ListError, open_issues};
use crate::report::{Line, Value, report};
use crate::sweep::risk_rank;
use crate::{TRIAGE_VERSION, Ticket, parse};

/// `index` could not read the open issues.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("listing the open issues: {reason}")]
    List { reason: String },
    #[error("the issue list is not the JSON index reads: {0}")]
    ListJson(#[source] serde_json::Error),
    #[error(
        "the list holds {limit} or more open issues, the most one index covers; no index was written"
    )]
    Truncated { limit: usize },
    #[error("#{number} appears twice in the issue list")]
    Duplicate { number: u64 },
    #[error("#{number} is in the list of open issues with state `{state}`")]
    NotOpen { number: u64, state: String },
}

impl From<ListError> for IndexError {
    fn from(e: ListError) -> IndexError {
        match e {
            ListError::Json(e) => IndexError::ListJson(e),
            ListError::Truncated { limit } => IndexError::Truncated { limit },
            ListError::Duplicate { number } => IndexError::Duplicate { number },
            ListError::NotOpen { number, state } => IndexError::NotOpen { number, state },
        }
    }
}

/// One open issue in the index. Field order is the serialized order, and
/// is the schema `docs/TICKETING.md` documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexEntry {
    pub number: u64,
    pub title: String,
    /// `S<n>`: a ticket's from its triage block, a story's from its title.
    /// `None` when there is none, or it could not be read (then a note says
    /// why).
    pub story: Option<String>,
    /// The labels on GitHub now, sorted.
    pub labels: Vec<String>,
    /// The triage block's `blocked_by`, or `None` when it could not be read
    /// (a note says why, and `ready` and `triage` abstain).
    pub blocked_by: Option<Vec<u64>>,
    pub area: Line,
    pub kind: Line,
    pub risk: Line,
    pub surface: Line,
    pub effort: Line,
    pub origin: Line,
    pub flags: Line,
    pub ready: Line,
    pub triage: Line,
    /// Always abstained: see [`agent_ok`].
    pub agent_ok: Line,
    /// Intake that could not be read, in the order it was met.
    pub notes: Vec<String>,
    #[serde(skip)]
    story_number: Option<u64>,
}

/// Every open issue's triage, in index order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Index {
    pub triage_version: u32,
    pub issues: Vec<IndexEntry>,
}

/// The `!agent-ok` line. The rules never set the flag (rule 2 has no
/// deterministic check), so its absence from `flags` is not a decision
/// that the ticket is not agent-ok, and the index says so rather than let
/// the silence read as one.
fn agent_ok() -> Line {
    Line::Abstained {
        rule: "§5 !agent-ok rule 2",
        reason: "rule 2 (no Done when line needing judgment) has no deterministic check, so the rules neither set nor clear !agent-ok (#195)".to_string(),
    }
}

/// Replaces the lines that read a computed `risk:` below the recorded one
/// with abstentions. The risk rule cannot see rollback, worktree teardown or
/// session persistence (#196), so a lower value may be its blind spot;
/// `sweep` never lowers the label for the same reason. `triage` routes on
/// risk, and when the recorded risk is high, `!needs-adr` (in `flags`, and
/// the gate in `ready`) turns on it too.
fn hold_lower_risk(e: &mut IndexEntry) {
    let Line::Decided {
        value: Value::Label(want),
        rule,
    } = &e.risk
    else {
        return;
    };
    let (want, rule) = (*want, *rule);
    let recorded = e.labels.iter().filter_map(|l| risk_rank(l)).max();
    let (Some(have), Some(computed)) = (recorded, risk_rank(want)) else {
        return;
    };
    if computed >= have {
        return;
    }
    e.risk = Line::Abstained {
        rule,
        reason: format!(
            "the rules compute {want}, below the recorded label; the rules cannot see rollback, worktree teardown or session persistence (#196)"
        ),
    };
    let held = |line: &Line| Line::Abstained {
        rule: line_rule(line),
        reason: format!(
            "read risk, which abstained: the rules compute {want}, below the recorded label (#196)"
        ),
    };
    e.triage = held(&e.triage);
    if Some(have) == risk_rank("risk:high") {
        e.flags = held(&e.flags);
        e.ready = held(&e.ready);
    }
}

fn line_rule(line: &Line) -> &'static str {
    match line {
        Line::Decided { rule, .. } | Line::Abstained { rule, .. } => rule,
    }
}

/// One issue's entry. `blockers` and `notes` come from [`Blockers::of`].
fn entry(
    number: u64,
    t: &Ticket,
    blockers: &std::collections::BTreeMap<u64, crate::State>,
    mut notes: Vec<String>,
) -> IndexEntry {
    let is_story = t.labels.iter().any(|l| l == "type:story");
    let story = if is_story {
        let s = parse::story_of_title(&t.title);
        if s.is_none() {
            notes.push("a story whose title does not start `S<n>:`; no story id".to_string());
        }
        s
    } else {
        parse::story(&t.body).unwrap_or_else(|reason| {
            notes.push(format!("story could not be read: {reason}"));
            None
        })
    };
    let blocked_by = match parse::blocked_by(&t.body) {
        Ok(b) => Some(b),
        Err(reason) => {
            notes.push(format!("blocked_by could not be read: {reason}"));
            None
        }
    };
    let mut labels = t.labels.clone();
    labels.sort();
    labels.dedup();

    let r = report(number, t, blockers);
    let mut e = IndexEntry {
        number,
        title: t.title.clone(),
        story: story.as_ref().map(|(id, _)| id.clone()),
        labels,
        blocked_by,
        area: r.area,
        kind: r.kind,
        risk: r.risk,
        surface: r.surface,
        effort: r.effort,
        origin: r.origin,
        flags: r.flags,
        ready: r.ready,
        triage: r.triage,
        agent_ok: agent_ok(),
        notes,
        story_number: story.map(|(_, n)| n),
    };
    hold_lower_risk(&mut e);
    e
}

/// The index of every issue in `list_json` (what `gh` prints for
/// [`crate::gh_list_args`]).
///
/// `fetch` reads an issue as [`crate::triage_with`]'s does. It is called
/// only for a `blocked_by` entry that is not in the open list, once each;
/// one that cannot be read is a note on each issue it blocks, and their
/// `ready` and `triage` abstain.
pub fn build_index(
    list_json: &str,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
) -> Result<Index, IndexError> {
    let open = open_issues(list_json)?;
    let mut known = Blockers::new(&open);
    let mut issues: Vec<IndexEntry> = open
        .iter()
        .map(|(n, t)| {
            let (blockers, notes) = known.of(&t.body, &mut fetch);
            entry(*n, t, &blockers, notes)
        })
        .collect();
    // No story sorts after every story; two ids naming one number (`S7`,
    // `S07`) are told apart by the id, so the order never depends on the
    // order `gh` listed them in.
    issues.sort_by(|a, b| {
        (a.story_number.is_none(), a.story_number, &a.story, a.number).cmp(&(
            b.story_number.is_none(),
            b.story_number,
            &b.story,
            b.number,
        ))
    });
    Ok(Index {
        triage_version: TRIAGE_VERSION,
        issues,
    })
}

/// `index`: list the open issues through `list` and build the index.
pub fn index_with(
    list: impl FnOnce() -> Result<String, String>,
    fetch: impl FnMut(u64) -> Result<String, String>,
) -> Result<Index, IndexError> {
    let json = list().map_err(|reason| IndexError::List { reason })?;
    build_index(&json, fetch)
}

impl Index {
    /// The bytes `index` writes, to stdout or to `--out`: pretty-printed
    /// JSON and one final newline.
    ///
    /// `serde_json` escapes the C0 controls inside strings but not DEL or
    /// the C1 range, and U+009B alone starts a terminal control sequence.
    /// Those are escaped as `\u` sequences here, so the document still
    /// parses to the same strings and cannot drive the terminal it is
    /// printed to. The only raw control left is the newline between lines.
    pub fn json(&self) -> String {
        // Strings, integers, vectors and unit-tagged enums: no map with
        // non-string keys and no failing `Serialize`, so this cannot fail.
        let pretty = serde_json::to_string_pretty(self).expect("an Index always serializes");
        let mut out = crate::report::escape_json_controls(&pretty);
        out.push('\n');
        out
    }

    /// Writes [`Index::json`] to `path`, replacing the file. The bytes are
    /// the ones stdout gets, so a file written twice over the same issues
    /// is the same file.
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.json().as_bytes())
    }
}
