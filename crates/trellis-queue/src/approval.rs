//! Approvals (ADR 0026 §6): a task that needs one before it runs, and a
//! request to override a budget hard stop.
//!
//! A task's approval lives on the task. A budget request is its own file
//! under `<git-dir>/trellis/approvals/{pending,approved,rejected}/`, moved
//! between them by publish-then-remove like a task.

use crate::activity::{Actor, Event};
use crate::files::{move_to, publish, read_json};
use crate::profile::{self, PausedBy, Raise, Window};
use crate::{Decision, Queue, QueueError, State, Verdict};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetRequest {
    pub id: String,
    /// Always `budget_override`, Paperclip's name for it.
    pub kind: String,
    pub agent: String,
    pub window: Window,
    pub month: String,
    pub spent: u64,
    pub limit: u64,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised_by: Option<u64>,
}

const DIRS: [&str; 3] = ["pending", "approved", "rejected"];

fn dir(q: &Queue, which: &str) -> PathBuf {
    q.trellis_dir().join("approvals").join(which)
}

fn path(q: &Queue, which: &str, id: &str) -> PathBuf {
    dir(q, which).join(format!("{id}.json"))
}

pub fn is_request_id(id: &str) -> bool {
    id.starts_with("budget-")
}

/// Every request in one of `pending`, `approved`, `rejected`, oldest first.
pub fn list(q: &Queue, which: &str) -> Result<Vec<BudgetRequest>, QueueError> {
    let d = dir(q, which);
    let rd = match std::fs::read_dir(&d) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(crate::files::io(&d)(e)),
    };
    let mut names: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
        })
        .collect();
    names.sort();
    names.iter().map(|p| read_json(p)).collect()
}

/// Opens a budget override request for `agent`, or returns the id of the
/// one already pending. One open request per profile: a hard stop that
/// re-fires every tick must not bury the person in duplicates.
pub fn open_budget_request(
    q: &Queue,
    agent: &str,
    window: Window,
    month: &str,
    spent: u64,
    limit: u64,
    actor: Actor,
) -> Result<String, QueueError> {
    if let Some(r) = list(q, "pending")?.into_iter().find(|r| r.agent == agent) {
        return Ok(r.id);
    }
    let r = BudgetRequest {
        id: format!("budget-{agent}-{}", crate::stamp::sortable_now()),
        kind: "budget_override".into(),
        agent: agent.into(),
        window,
        month: month.into(),
        spent,
        limit,
        requested_at: crate::iso_now(),
        decision: None,
        raised_by: None,
    };
    publish(&path(q, "pending", &r.id), &r)?;
    q.log(
        Event::new(actor, "approval.request", format!("approval:{}", r.id)).detail(format!(
            "budget_override for agent `{agent}`: {spent} of {limit} {} tokens",
            window.as_str()
        )),
    )?;
    Ok(r.id)
}

/// Approves a budget request: raises the window that tripped by `raise`
/// tokens and lifts the budget pause. A pause a person set is left alone.
pub fn approve_budget(
    q: &Queue,
    id: &str,
    raise: u64,
    actor: Actor,
) -> Result<BudgetRequest, QueueError> {
    let from = path(q, "pending", id);
    if !from.exists() {
        return Err(QueueError::Invalid(format!(
            "no pending approval request `{id}`{}",
            if DIRS.iter().any(|d| path(q, d, id).exists()) {
                " (it has already been decided)"
            } else {
                ""
            }
        )));
    }
    if raise == 0 {
        return Err(QueueError::Invalid(
            "--raise 0 would resume a profile that is still over its limit; \
             give a positive raise, or `queue reject` to keep it paused"
                .into(),
        ));
    }
    let mut r: BudgetRequest = read_json(&from)?;
    profile::update_overlay(q, &r.agent, |o| {
        o.raises.push(Raise {
            window: r.window,
            month: (r.window == Window::Monthly).then(|| r.month.clone()),
            tokens: raise,
            at: crate::iso_now(),
        });
        if matches!(&o.paused, Some(p) if p.by == PausedBy::Budget) {
            o.paused = None;
        }
    })?;
    r.decision = Some(Decision {
        decision: Verdict::Approved,
        at: crate::iso_now(),
        reason: None,
    });
    r.raised_by = Some(raise);
    move_to(&from, &path(q, "approved", id), &r)?;
    q.log(
        Event::new(actor, "approval.approve", format!("approval:{id}")).detail(format!(
            "agent `{}` {} limit raised by {raise}; resumed",
            r.agent,
            r.window.as_str()
        )),
    )?;
    Ok(r)
}

/// Rejects a budget request: the profile stays paused.
pub fn reject_budget(
    q: &Queue,
    id: &str,
    reason: Option<&str>,
    actor: Actor,
) -> Result<BudgetRequest, QueueError> {
    let from = path(q, "pending", id);
    if !from.exists() {
        return Err(QueueError::Invalid(format!(
            "no pending approval request `{id}`"
        )));
    }
    let mut r: BudgetRequest = read_json(&from)?;
    r.decision = Some(Decision {
        decision: Verdict::Rejected,
        at: crate::iso_now(),
        reason: reason.map(str::to_string),
    });
    move_to(&from, &path(q, "rejected", id), &r)?;
    let detail = match reason {
        Some(why) => format!("agent `{}` stays paused: {why}", r.agent),
        None => format!("agent `{}` stays paused", r.agent),
    };
    q.log(Event::new(actor, "approval.reject", format!("approval:{id}")).detail(detail))?;
    Ok(r)
}

/// Approves a task that needed it (ADR 0026 §6).
pub fn approve_task(q: &Queue, id: &str, actor: Actor) -> Result<(), QueueError> {
    q.edit(id, |t, s| {
        if s != State::Pending {
            return Err(QueueError::Invalid(format!(
                "task `{id}` is in {}/, and approval gates a task before it runs",
                s.dir_name()
            )));
        }
        if !t.needs_approval {
            return Err(QueueError::Invalid(format!(
                "task `{id}` does not need approval"
            )));
        }
        t.approval = Some(Decision {
            decision: Verdict::Approved,
            at: crate::iso_now(),
            reason: None,
        });
        t.note(actor, "approved");
        Ok(None)
    })?;
    q.log(Event::new(actor, "approval.approve", format!("task:{id}")))
}

/// Rejects a task: recorded on it, and the task is cancelled.
pub fn reject_task(
    q: &Queue,
    id: &str,
    reason: Option<&str>,
    actor: Actor,
) -> Result<(), QueueError> {
    q.edit(id, |t, s| {
        if s != State::Pending {
            return Err(QueueError::Invalid(format!(
                "task `{id}` is in {}/; only a task waiting to run can be rejected",
                s.dir_name()
            )));
        }
        t.approval = Some(Decision {
            decision: Verdict::Rejected,
            at: crate::iso_now(),
            reason: reason.map(str::to_string),
        });
        t.status = Some(crate::Status::Cancelled);
        t.blocked = None;
        t.note(
            actor,
            match reason {
                Some(r) => format!("rejected: {r}"),
                None => "rejected".to_string(),
            },
        );
        Ok(Some(State::Cancelled))
    })?;
    let mut e = Event::new(actor, "approval.reject", format!("task:{id}"));
    if let Some(r) = reason {
        e = e.detail(r);
    }
    q.log(e)
}
