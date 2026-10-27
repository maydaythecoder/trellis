//! `trellis-tickets sweep`, less the spawns (ADR 0040 decision 4, #80).
//!
//! Re-triages every open issue and reports where its labels differ from
//! what the rules compute. Listing, reading and editing are closures: the
//! binary passes ones that run `gh`, the tests pass recordings and a
//! recorder, so nothing here reaches GitHub on its own.
//!
//! It is a dry run unless `apply` is set, and the whole plan is shown
//! before the first write. What it will and will not change, and why:
//!
//! - **`parked` and `split` issues are skipped** before any rule runs
//!   (ADR 0040): one waits on the owner, the other's work is in its
//!   children.
//! - **An abstaining rule changes nothing.** Its dimension is reported as
//!   `abstained` with the reason, and whatever label is there stays. The
//!   standard is "fail to human, never guess" (§1), and a label removed on
//!   an abstention is a guess in the other direction.
//! - **`ready` is not touched**, neither the label nor the block's `ready:`
//!   and `ready_failures:` lines: `scripts/tickets/sync-ready.py` owns them
//!   (ADR 0040 decision 7), and two writers of one label would undo each
//!   other on every run.
//! - **`!agent-ok` is not touched.** The rules never set it (#195: rule 2
//!   has no deterministic check), so they cannot say it is wrong either.
//! - **`risk:` is never lowered.** The low rule cannot see rollback,
//!   worktree teardown or session persistence (#196), so a computed value
//!   below the recorded one may be the rule's blind spot, not the label's
//!   error.
//! - **`triage:` is applied only when every dimension was decided and
//!   applied.** A `needs-human` that follows from an abstention (most often
//!   #194's unmapped crates) would move issues out of `triage:auto` on the
//!   strength of an open question, and change what `next.sh` hands agents.
//! - **The body changes only inside the generated triage block**, only on
//!   an issue whose labels change, and only on the lines that record what
//!   was changed. Every other byte, line endings included, is kept.
//! - **An issue that changed after it was read is not written.** It is read
//!   again before its edit; the REST `PATCH` replaces the whole body and the
//!   whole label list, so a write over a newer one (a `sync-ready.py` run, a
//!   person's edit) would silently undo it.

use std::collections::BTreeMap;

use crate::labels::Flag;
use crate::open::{Blockers, ListError, open_issues};
use crate::parse::{TRIAGE_END, TRIAGE_START};
use crate::report::{Line, Value, printable, report, ticket_from_gh_json};
use crate::{State, TRIAGE_VERSION, Ticket};

/// The most open issues one sweep or index acts on. The list is read with
/// `--paginate`, so `gh` no longer stops at a count it was given; the
/// ceiling stays from when it did (`gh issue list --limit`), and a list
/// this long is still refused rather than read (#957).
pub const SWEEP_LIST_LIMIT: usize = 1000;

/// The `gh` argv that lists every open issue through REST. `--paginate`
/// follows every page: without it the endpoint returns the first 30 and a
/// sweep would report "no drift" for issues it never read. The list holds
/// pull requests too; [`plan_sweep`] drops them.
pub fn gh_list_args() -> Vec<String> {
    [
        "api",
        "--paginate",
        "repos/{owner}/{repo}/issues?state=open&per_page=100",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// `sweep` could not read the open issues, or a write failed.
#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error("listing the open issues: {reason}")]
    List { reason: String },
    #[error("the issue list is not the JSON sweep reads: {0}")]
    ListJson(#[source] serde_json::Error),
    #[error(
        "the list holds {limit} or more open issues, the most one sweep acts on; nothing was swept"
    )]
    Truncated { limit: usize },
    #[error("#{number} appears twice in the issue list")]
    Duplicate { number: u64 },
    #[error("#{number} is in the list of open issues with state `{state}`")]
    NotOpen { number: u64, state: String },
    #[error("writing #{number}: {reason}")]
    Write { number: u64, reason: String },
}

/// One dimension's label change: every label removed and the one added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub dimension: &'static str,
    pub remove: Vec<String>,
    pub add: Vec<String>,
    pub rule: &'static str,
}

/// A dimension sweep leaves as it is, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub dimension: &'static str,
    pub rule: &'static str,
    pub reason: String,
}

/// What sweep found for one open issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuePlan {
    pub number: u64,
    pub title: String,
    /// `parked` or `split`: no rule ran and nothing is written.
    pub skipped: Option<&'static str>,
    pub changes: Vec<Change>,
    /// Every dimension reported as `abstained`.
    pub abstained: Vec<Held>,
    /// Anything else the plan needs to say (a blocker that could not be
    /// read, a triage block that could not be rewritten).
    pub notes: Vec<String>,
    /// Each rewritten triage-block line, before and after, without its line
    /// ending.
    pub body_changes: Vec<(String, String)>,
    /// The body to write, when the triage block changes.
    pub new_body: Option<String>,
    read_labels: Vec<String>,
    read_body: String,
}

impl IssuePlan {
    /// Labels added, across every change.
    pub fn added(&self) -> Vec<&str> {
        self.changes
            .iter()
            .flat_map(|c| c.add.iter().map(String::as_str))
            .collect()
    }

    /// Labels removed, across every change.
    pub fn removed(&self) -> Vec<&str> {
        self.changes
            .iter()
            .flat_map(|c| c.remove.iter().map(String::as_str))
            .collect()
    }

    /// Whether `apply` would write anything to this issue.
    pub fn writes(&self) -> bool {
        !self.changes.is_empty() || self.new_body.is_some()
    }

    /// The `gh api` argv (a REST `PATCH`) that writes this plan. The body,
    /// when there is one, goes on stdin (`-F body=@-`). REST sets the whole
    /// label list, so every label the issue keeps is named, not only the
    /// changes; an issue left with none gets `labels[]`, gh's empty array.
    pub fn gh_edit_args(&self) -> Vec<String> {
        let mut args = vec![
            "api".to_string(),
            "-X".to_string(),
            "PATCH".to_string(),
            format!("repos/{{owner}}/{{repo}}/issues/{}", self.number),
        ];
        let labels = after(&self.read_labels, &self.changes);
        if labels.is_empty() {
            args.push("-f".to_string());
            args.push("labels[]".to_string());
        }
        for l in labels {
            args.push("-f".to_string());
            args.push(format!("labels[]={l}"));
        }
        if self.new_body.is_some() {
            args.push("-F".to_string());
            args.push("body=@-".to_string());
        }
        args
    }

    /// Every line of the plan for this issue, each starting `#n:`. Issue
    /// text quoted in any of them is escaped.
    pub fn text(&self) -> String {
        let n = self.number;
        let mut out = String::new();
        if let Some(label) = self.skipped {
            out.push_str(&format!(
                "#{n}: skipped ({label}); sweep never touches it\n"
            ));
            return out;
        }
        for c in &self.changes {
            let words: Vec<String> = c
                .remove
                .iter()
                .map(|l| format!("-{l}"))
                .chain(c.add.iter().map(|l| format!("+{l}")))
                .collect();
            out.push_str(&format!(
                "#{n}: {}  (rule: {})\n",
                printable(&words.join(" ")),
                c.rule
            ));
        }
        for h in &self.abstained {
            out.push_str(&format!(
                "#{n}: {} abstained: {}  (rule: {})\n",
                h.dimension,
                printable(&h.reason),
                h.rule
            ));
        }
        for note in &self.notes {
            out.push_str(&format!("#{n}: {}\n", printable(note)));
        }
        for (old, new) in &self.body_changes {
            out.push_str(&format!(
                "#{n}: triage block: `{}` -> `{}`\n",
                printable(old),
                printable(new)
            ));
        }
        out
    }
}

/// Every open issue's plan, in the order `gh` listed them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPlan {
    pub issues: Vec<IssuePlan>,
}

impl SweepPlan {
    /// Every issue's lines, then a count.
    pub fn text(&self) -> String {
        let mut out: String = self.issues.iter().map(IssuePlan::text).collect();
        let skipped = self.issues.iter().filter(|i| i.skipped.is_some()).count();
        let drift = self.issues.iter().filter(|i| !i.changes.is_empty()).count();
        let abstained = self
            .issues
            .iter()
            .filter(|i| !i.abstained.is_empty())
            .count();
        out.push_str(&format!(
            "{} open issue(s): {drift} with label drift, {abstained} with an abstention, {skipped} skipped (parked or split)\n",
            self.issues.len()
        ));
        out
    }
}

/// What `sweep` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepOutcome {
    pub plan: SweepPlan,
    /// Issues edited.
    pub written: Vec<u64>,
    /// Issues with a plan that were not edited, and why.
    pub not_written: Vec<(u64, String)>,
}

/// The dimensions sweep labels, by their label prefix, in the order the
/// triage block's `labels:` line lists them. `ready` is not among them.
const LABEL_DIMENSIONS: [(&str, &str); 6] = [
    ("area", "area:"),
    ("kind", "kind:"),
    ("risk", "risk:"),
    ("surface", "surface:"),
    ("effort", "effort:"),
    ("origin", "origin:"),
];

const FLAG_ORDER: [Flag; 5] = [
    Flag::AgentOk,
    Flag::NeedsAdr,
    Flag::Breaking,
    Flag::Security,
    Flag::BlockedExt,
];

pub(crate) fn risk_rank(label: &str) -> Option<u8> {
    match label {
        "risk:low" => Some(0),
        "risk:medium" => Some(1),
        "risk:high" => Some(2),
        _ => None,
    }
}

/// The labels after `changes`: the current ones in their order, less what
/// is removed, then what is added.
fn after(labels: &[String], changes: &[Change]) -> Vec<String> {
    let removed: Vec<&String> = changes.iter().flat_map(|c| &c.remove).collect();
    let mut out: Vec<String> = labels
        .iter()
        .filter(|l| !removed.contains(l))
        .cloned()
        .collect();
    for c in changes {
        for a in &c.add {
            if !out.contains(a) {
                out.push(a.clone());
            }
        }
    }
    out
}

/// A change setting dimension `prefix` to exactly `want`, or `None` when
/// the labels already say so.
fn set_to(
    dimension: &'static str,
    prefix: &str,
    want: &str,
    labels: &[String],
    rule: &'static str,
) -> Option<Change> {
    let remove: Vec<String> = labels
        .iter()
        .filter(|l| l.starts_with(prefix) && *l != want)
        .cloned()
        .collect();
    let add: Vec<String> = if labels.iter().any(|l| l == want) {
        Vec::new()
    } else {
        vec![want.to_string()]
    };
    if remove.is_empty() && add.is_empty() {
        None
    } else {
        Some(Change {
            dimension,
            remove,
            add,
            rule,
        })
    }
}

/// The plan for one open issue that is neither parked nor split.
fn plan_issue(
    number: u64,
    t: &Ticket,
    blockers: &BTreeMap<u64, State>,
    notes: Vec<String>,
    now: &str,
) -> IssuePlan {
    let r = report(number, t, blockers);
    let mut changes = Vec::new();
    let mut abstained = Vec::new();

    // Set when a lower computed risk is held over a recorded `risk:high`:
    // the `!needs-adr` the rules derive turns on that risk, so it is held
    // too, as `index` holds it.
    let mut hold_flags: Option<String> = None;
    let lines = r.lines();
    for (name, prefix) in LABEL_DIMENSIONS {
        let Some((_, line)) = lines.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        match line {
            Line::Abstained { rule, reason } => abstained.push(Held {
                dimension: name,
                rule,
                reason: reason.clone(),
            }),
            Line::Decided {
                value: Value::Label(want),
                rule,
            } => {
                if name == "risk" {
                    let recorded = t.labels.iter().filter_map(|l| risk_rank(l)).max();
                    if let (Some(have), Some(computed)) = (recorded, risk_rank(want))
                        && computed < have
                    {
                        abstained.push(Held {
                            dimension: name,
                            rule,
                            reason: format!(
                                "the rules compute {want}, below the recorded label; sweep never lowers risk, because the rules cannot see rollback, worktree teardown or session persistence (#196)"
                            ),
                        });
                        if Some(have) == risk_rank("risk:high") {
                            hold_flags = Some(format!(
                                "read risk, which was held: the rules compute {want}, below the recorded label (#196)"
                            ));
                        }
                        continue;
                    }
                }
                changes.extend(set_to(name, prefix, want, &t.labels, rule));
            }
            Line::Decided { .. } => {}
        }
    }

    match &r.flags {
        Line::Abstained { rule, reason } => abstained.push(Held {
            dimension: "flags",
            rule,
            reason: reason.clone(),
        }),
        Line::Decided { rule, .. } if hold_flags.is_some() => abstained.push(Held {
            dimension: "flags",
            rule,
            reason: hold_flags.unwrap_or_default(),
        }),
        Line::Decided {
            value: Value::Flags(f),
            rule,
        } => {
            // `!breaking`, `!security` and `!blocked-ext` are carried from
            // the labels, so they cannot differ; `!needs-adr` is the one
            // flag the rules derive.
            let needs = Flag::NeedsAdr.label();
            let want = f.contains(&needs);
            let have = t.labels.iter().any(|l| l == needs);
            if want != have {
                changes.push(Change {
                    dimension: "flags",
                    remove: if have {
                        vec![needs.to_string()]
                    } else {
                        vec![]
                    },
                    add: if want {
                        vec![needs.to_string()]
                    } else {
                        vec![]
                    },
                    rule,
                });
            }
        }
        Line::Decided { .. } => {}
    }
    if t.labels.iter().any(|l| l == Flag::AgentOk.label()) {
        abstained.push(Held {
            dimension: "!agent-ok",
            rule: "§5 !agent-ok rule 2",
            reason: "set by hand; rule 2 has no deterministic check, so sweep neither sets nor clears it (#195)".to_string(),
        });
    }

    // The route is applied only over dimensions that were all decided and
    // applied; `!agent-ok` does not feed it.
    let held: Vec<&str> = abstained
        .iter()
        .map(|h| h.dimension)
        .filter(|d| *d != "!agent-ok")
        .collect();
    let mut route_applied: Option<(&'static str, &Vec<String>)> = None;
    match &r.triage {
        Line::Abstained { rule, reason } => abstained.push(Held {
            dimension: "triage",
            rule,
            reason: reason.clone(),
        }),
        Line::Decided {
            value: Value::Route { route, reasons },
            rule,
        } => {
            if held.is_empty() {
                changes.extend(set_to("triage", "triage:", route, &t.labels, rule));
                route_applied = Some((route, reasons));
            } else {
                abstained.push(Held {
                    dimension: "triage",
                    rule,
                    reason: format!(
                        "the route ({route}) follows from {} not being applied; the label is left as it is",
                        held.join(", ")
                    ),
                });
            }
        }
        Line::Decided { .. } => {}
    }

    let mut plan = IssuePlan {
        number,
        title: t.title.clone(),
        skipped: None,
        changes,
        abstained,
        notes,
        body_changes: Vec::new(),
        new_body: None,
        read_labels: t.labels.clone(),
        read_body: t.body.clone(),
    };
    if !plan.changes.is_empty() {
        block_update(&mut plan, route_applied, now);
    }
    plan
}

/// The triage-block lines that record `plan`'s label changes, rewritten.
fn block_update(plan: &mut IssuePlan, route: Option<(&'static str, &Vec<String>)>, now: &str) {
    let labels = after(&plan.read_labels, &plan.changes);
    let dims: Vec<&str> = LABEL_DIMENSIONS
        .iter()
        .flat_map(|(_, prefix)| labels.iter().filter(move |l| l.starts_with(prefix)))
        .map(String::as_str)
        .collect();
    let flags: Vec<&str> = FLAG_ORDER
        .iter()
        .map(|f| f.label())
        .filter(|f| labels.iter().any(|l| l == f))
        .chain(
            labels
                .iter()
                .map(String::as_str)
                .filter(|l| l.starts_with('!') && !FLAG_ORDER.iter().any(|f| f.label() == *l)),
        )
        .collect();
    let mut fields: Vec<(&str, String)> = vec![
        ("labels", format!("[{}]", dims.join(", "))),
        ("flags", format!("[{}]", flags.join(", "))),
    ];
    if let Some((route, reasons)) = route {
        let bare = route.strip_prefix("triage:").unwrap_or(route);
        fields.push(("triage", bare.to_string()));
        // JSON strings are YAML flow scalars, and escape any control
        // character a reason quotes from the issue.
        fields.push((
            "triage_reasons",
            serde_json::to_string(reasons).expect("a list of strings serializes"),
        ));
    }
    fields.push(("triaged_at", now.to_string()));
    fields.push(("triage_version", TRIAGE_VERSION.to_string()));

    match rewrite_block(&plan.read_body, &fields) {
        Ok(rw) => {
            plan.notes.extend(
                rw.missing.into_iter().map(|k| {
                    format!("the triage block has no `{k}:` line; sweep does not add one")
                }),
            );
            if !rw.changed.is_empty() {
                plan.body_changes = rw.changed;
                plan.new_body = Some(rw.body);
            }
        }
        Err(reason) => plan
            .notes
            .push(format!("{reason}; the labels change and the body does not")),
    }
}

/// A body with some triage-block lines replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rewrite {
    pub(crate) body: String,
    /// `(old, new)` for each line whose value changed, without line endings.
    pub(crate) changed: Vec<(String, String)>,
    /// Keys with no line in the block.
    pub(crate) missing: Vec<String>,
}

/// Replaces the value of each `key:` line inside the one generated triage
/// block, keeping every other byte of `body`: the text outside the block,
/// every other line inside it, and each line's own ending (issue bodies
/// come back with `\r\r\n` endings, and a rewrite that normalised them
/// would be a body change nobody asked for).
///
/// Only the first line of the block starting with `key:` is rewritten. A
/// body with no block, two blocks, or a block never closed is refused.
pub(crate) fn rewrite_block(body: &str, fields: &[(&str, String)]) -> Result<Rewrite, String> {
    let starts: Vec<usize> = body.match_indices(TRIAGE_START).map(|(i, _)| i).collect();
    let start = match starts.as_slice() {
        [] => return Err("the body has no triage block".to_string()),
        [one] => *one,
        _ => return Err("the body has more than one triage block".to_string()),
    };
    let end = body[start..]
        .find(TRIAGE_END)
        .map(|i| start + i)
        .ok_or_else(|| "the triage block is opened and never closed".to_string())?;

    let mut out = String::with_capacity(body.len());
    out.push_str(&body[..start]);
    let mut done: Vec<&str> = Vec::new();
    let mut changed = Vec::new();
    for line in body[start..end].split_inclusive('\n') {
        let content_len = line.find(['\r', '\n']).unwrap_or(line.len());
        let (content, ending) = line.split_at(content_len);
        let hit = fields.iter().find(|(k, _)| {
            !done.contains(k)
                && content
                    .strip_prefix(*k)
                    .is_some_and(|rest| rest.starts_with(':'))
        });
        match hit {
            Some((k, v)) => {
                done.push(k);
                let new = format!("{k}: {v}");
                if new != content {
                    changed.push((content.to_string(), new.clone()));
                }
                out.push_str(&new);
                out.push_str(ending);
            }
            None => out.push_str(line),
        }
    }
    out.push_str(&body[end..]);
    let missing = fields
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !done.contains(k))
        .map(str::to_string)
        .collect();
    Ok(Rewrite {
        body: out,
        changed,
        missing,
    })
}

fn same_labels(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort();
    b.sort();
    a == b
}

/// Re-triages every issue in `list_json` (what `gh` prints for
/// [`gh_list_args`]) and returns the plan. Nothing is written.
///
/// `fetch` reads an issue as [`crate::triage_with`]'s does. It is called
/// only for a `blocked_by` entry that is not in the open list, once each.
/// `now` is the `triaged_at` a rewritten block records.
pub fn plan_sweep(
    list_json: &str,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
    now: &str,
) -> Result<SweepPlan, SweepError> {
    let open = open_issues(list_json).map_err(|e| match e {
        ListError::Json(e) => SweepError::ListJson(e),
        ListError::Truncated { limit } => SweepError::Truncated { limit },
        ListError::Duplicate { number } => SweepError::Duplicate { number },
        ListError::NotOpen { number, state } => SweepError::NotOpen { number, state },
    })?;
    let mut known = Blockers::new(&open);
    let mut plans = Vec::with_capacity(open.len());
    for (number, t) in &open {
        if let Some(label) = ["parked", "split"]
            .into_iter()
            .find(|l| t.labels.iter().any(|x| x == l))
        {
            plans.push(IssuePlan {
                number: *number,
                title: t.title.clone(),
                skipped: Some(label),
                changes: Vec::new(),
                abstained: Vec::new(),
                notes: Vec::new(),
                body_changes: Vec::new(),
                new_body: None,
                read_labels: t.labels.clone(),
                read_body: t.body.clone(),
            });
            continue;
        }
        let (blockers, notes) = known.of(&t.body, &mut fetch);
        plans.push(plan_issue(*number, t, &blockers, notes, now));
    }
    Ok(SweepPlan { issues: plans })
}

/// `sweep`: list the open issues, plan, show the whole plan through `show`,
/// then, only when `apply` is set, edit each issue that has one.
///
/// `show` receives the plan before `edit` is first called. Before each
/// edit the issue is read again through `fetch`; if its labels or body are
/// no longer the ones the plan was made from, it is not written and the
/// reason is shown. `edit` receives [`IssuePlan::gh_edit_args`] and the new
/// body for stdin, if the body changes. A failed edit stops the sweep; the
/// issues already written have been shown.
pub fn sweep_with(
    apply: bool,
    now: &str,
    list: impl FnOnce() -> Result<String, String>,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
    mut show: impl FnMut(&str),
    mut edit: impl FnMut(&[String], Option<&str>) -> Result<(), String>,
) -> Result<SweepOutcome, SweepError> {
    let json = list().map_err(|reason| SweepError::List { reason })?;
    let plan = plan_sweep(&json, &mut fetch, now)?;
    show(&plan.text());
    let mut outcome = SweepOutcome {
        plan,
        written: Vec::new(),
        not_written: Vec::new(),
    };
    if !apply {
        show("dry run: nothing written; `--apply` writes the changes above\n");
        return Ok(outcome);
    }
    for p in outcome.plan.issues.iter().filter(|p| p.writes()) {
        let n = p.number;
        let current = fetch(n)
            .map_err(|e| e.to_string())
            .and_then(|json| ticket_from_gh_json(n, &json).map_err(|e| e.to_string()));
        let why_not = match &current {
            Err(reason) => Some(format!("could not be read again ({reason})")),
            Ok(c) if c.body != p.read_body || !same_labels(&c.labels, &p.read_labels) => {
                Some("changed since it was read".to_string())
            }
            Ok(_) => None,
        };
        if let Some(why) = why_not {
            show(&format!("#{n}: {}; not written\n", printable(&why)));
            outcome.not_written.push((n, why));
            continue;
        }
        edit(&p.gh_edit_args(), p.new_body.as_deref())
            .map_err(|reason| SweepError::Write { number: n, reason })?;
        show(&format!("#{n}: written\n"));
        outcome.written.push(n);
    }
    Ok(outcome)
}

/// `secs` after the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`, the form
/// `triaged_at` is written in. The calendar arithmetic is Howard Hinnant's
/// `civil_from_days`, so no date crate is needed.
pub fn utc_timestamp(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamp_matches_a_known_instant() {
        // 2026-09-28T21:13:17Z, the triaged_at on the S13 issues.
        assert_eq!(utc_timestamp(1_790_629_997), "2026-09-28T21:13:17Z");
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn rewrite_block_keeps_every_byte_outside_the_lines_it_replaces() {
        let body = "labels: [x]\r\r\n<!-- triage:start -->\r\r\nlabels: [a]\r\r\ntriage_reasons: []\r\r\ntriage: auto\r\r\n<!-- triage:end -->\r\r\nlabels: [y]";
        let rw = rewrite_block(
            body,
            &[
                ("triage", "blocked".to_string()),
                ("labels", "[b]".to_string()),
                ("flags", "[]".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(
            rw.body,
            "labels: [x]\r\r\n<!-- triage:start -->\r\r\nlabels: [b]\r\r\ntriage_reasons: []\r\r\ntriage: blocked\r\r\n<!-- triage:end -->\r\r\nlabels: [y]"
        );
        assert_eq!(rw.missing, ["flags"]);
        assert!(rewrite_block("no block", &[]).is_err());
        assert!(rewrite_block("<!-- triage:start -->\nlabels: []", &[]).is_err());
    }

    #[test]
    fn sweep_holds_needs_adr_when_it_holds_a_lower_risk_under_a_recorded_high() {
        // The rules compute medium, below the recorded high (#196's blind
        // spot), and so derive no `!needs-adr`; removing the flag on that
        // risk would undo the label sweep refused to lower, and disagree
        // with `index`, which holds `flags` here.
        let t = Ticket {
            title: "Add a thing".to_string(),
            body: "**Files:** crates/trellis-cli/src/main.rs\n\n## Done when\n- it holds\n\n## Verify\n```\nscripts/tickets/verify-tests.sh trellis-cli thing_ 1\n```\n".to_string(),
            labels: ["risk:high", "!needs-adr"].map(String::from).to_vec(),
            state: State::Open,
        };
        let plan = plan_issue(1, &t, &BTreeMap::new(), Vec::new(), "now");
        assert!(
            plan.abstained
                .iter()
                .any(|h| h.dimension == "risk" && h.reason.contains("risk:medium")),
            "{plan:?}"
        );
        assert!(
            plan.changes.iter().all(|c| c.dimension != "flags"),
            "{plan:?}"
        );
        assert!(
            plan.abstained.iter().any(|h| h.dimension == "flags"),
            "{plan:?}"
        );
    }
}
