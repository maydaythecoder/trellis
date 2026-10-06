//! `trellis queue` and `trellis work` (ADR 0020, ADR 0026).
//!
//! The queue and the heartbeat's decisions live in `trellis-queue`. This is
//! the part that needs the binary: a [`Runner`] whose `run` is `cmd_run`,
//! the same path `trellis run` takes, so a queued task and a typed one cannot
//! drift into two ways of running — or, for a process agent, the command
//! followed by `trellis check`'s two checks.

use crate::orchestrate::{load_profiles, load_routines, spend_records, this_month};
use crate::{Human, RunControl, RunExit, RunSeeding, cmd_run, config};
use anyhow::{Context, Result};
use clap::Subcommand;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use trellis_queue::portable::{self, Action, OnConflict};
use trellis_queue::profile::Profile;
use trellis_queue::routine::{self, Routine};
use trellis_queue::{
    Actor, Blocked, Event, NewTask, Priority, Queue, QueueError, RunEnd, Runner, State, Status,
    WorkOptions, approval, ledger, plain,
};

const PRIORITIES: [&str; 4] = ["critical", "high", "medium", "low"];
const STATUSES: [&str; 8] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "blocked",
    "done",
    "cancelled",
    "failed",
];

#[derive(Subcommand)]
pub(crate) enum QueueAction {
    /// Add a task. `work` runs the highest priority first, then the oldest.
    Add {
        /// Optional when `--prompt` names a template.
        #[arg(default_value = "")]
        task: String,
        /// The vault note to compile for this task's run, as `run --focus`.
        /// The task's goal (ADR 0020 §6).
        #[arg(long, visible_alias = "goal")]
        focus: Option<String>,
        /// A template in `.trellis/prompts/<name>.md`, as `run --prompt`.
        #[arg(long)]
        prompt: Option<String>,
        /// Arguments for the `--prompt` template, as `run --args`: one
        /// string, split shell-style when the task runs.
        #[arg(long, requires = "prompt", value_name = "STRING")]
        args: Option<String>,
        /// The model this task runs on, as `run --model`, over its
        /// assignee profile's and `work --model`. Counts as the
        /// repository's choice for keys: a hosted endpoint is sent a key
        /// only if your own config names it (ADR 0024 §3).
        #[arg(long, value_name = "SPEC")]
        model: Option<String>,
        #[arg(long, value_parser = PRIORITIES, default_value = "medium")]
        priority: String,
        /// Repeatable.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// A task this one is part of. Structure only: the parent does not
        /// wait for it unless set to `--status blocked --reason children`.
        #[arg(long)]
        parent: Option<String>,
        /// An agent profile in .trellis/agents/.
        #[arg(long)]
        assignee: Option<String>,
        /// A task that must be `done` first. Repeatable.
        #[arg(long = "blocked-by", value_name = "ID")]
        blocked_by: Vec<String>,
        /// `work` will not claim it until `trellis queue approve <id>`.
        #[arg(long)]
        needs_approval: bool,
        /// Start in backlog: listed, never claimed until set to todo.
        #[arg(long)]
        backlog: bool,
    },
    /// Every task: state, status, priority, and why a pending one waits.
    List {
        #[arg(long, value_parser = STATUSES)]
        status: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        #[arg(long, value_parser = PRIORITIES)]
        priority: Option<String>,
        /// Only tasks whose id, text, labels or notes contain this,
        /// ignoring case.
        #[arg(long)]
        text: Option<String>,
    },
    /// Change a task that is not running.
    Set {
        id: String,
        #[arg(long, value_parser = PRIORITIES)]
        priority: Option<String>,
        /// Add a label. Repeatable.
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// Remove a label. Repeatable.
        #[arg(long = "unlabel", value_name = "LABEL")]
        unlabels: Vec<String>,
        #[arg(long, conflicts_with = "unassign")]
        assignee: Option<String>,
        #[arg(long)]
        unassign: bool,
        #[arg(long, conflicts_with = "no_parent")]
        parent: Option<String>,
        #[arg(long)]
        no_parent: bool,
        /// Add a blocker. Repeatable.
        #[arg(long = "blocked-by", value_name = "ID")]
        blocked_by: Vec<String>,
        /// Remove a blocker. Repeatable.
        #[arg(long = "unblock", value_name = "ID")]
        unblock: Vec<String>,
        #[arg(long)]
        needs_approval: Option<bool>,
        /// `in_progress` is set only by a claim.
        #[arg(long, value_parser = ["backlog", "todo", "in_review", "blocked", "done", "cancelled", "failed"])]
        status: Option<String>,
        /// With `--status blocked`: what it waits for.
        #[arg(long, value_parser = ["blockers", "children", "manual", "until"])]
        reason: Option<String>,
        /// With `--reason manual`: what unblocks it.
        #[arg(long)]
        detail: Option<String>,
        /// With `--reason until`: the RFC 3339 instant it waits for, such as
        /// `2026-10-06T12:00:00Z`. Stored in UTC; the first reconcile at or
        /// after it sets the task to `todo`.
        #[arg(long, value_name = "INSTANT")]
        at: Option<String>,
    },
    /// Append a note to a task.
    Note { id: String, text: String },
    /// Cancel a task that is not running.
    Cancel {
        id: String,
        #[arg(long)]
        reason: Option<String>,
        /// Also cancel every pending descendant by `parent`, at any depth.
        /// Refused if any of them is running.
        #[arg(long)]
        children: bool,
    },
    /// Approve a task that needs it, or a budget override request.
    Approve {
        id: String,
        /// For a budget request: raise the limit that tripped by this many
        /// tokens and resume the agent.
        #[arg(long, value_name = "TOKENS")]
        raise: Option<u64>,
    },
    /// Reject a task (it is cancelled) or a budget request (the agent
    /// stays paused).
    Reject {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Put a task from claimed/ or failed/ back in pending/.
    ///
    /// For a claim a crashed process left behind. `work` never re-runs one
    /// on its own, because it cannot tell a dead claimant from a live one.
    Requeue { id: String },
    /// The activity log: every queue, agent, routine and approval change.
    Log {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Write profiles, routines and tasks to a new folder.
    Export { dir: PathBuf },
    /// Read a folder written by `export`.
    Import {
        dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_parser = ["skip", "replace", "rename"], default_value = "skip")]
        on_conflict: String,
    },
}

#[derive(clap::Args)]
pub(crate) struct WorkArgs {
    /// Run at most one task, then exit.
    #[arg(long, conflicts_with_all = ["every", "max_tasks"])]
    once: bool,
    /// When nothing is claimable, look again after this many seconds (or
    /// when the next routine is due, if sooner) instead of exiting. Runs in
    /// the foreground; Ctrl-C stops it.
    #[arg(long, value_name = "SECONDS")]
    every: Option<u64>,
    /// Run at most this many tasks in this invocation.
    #[arg(long, value_name = "N")]
    max_tasks: Option<u32>,
    /// As `trellis run --model`; falls back to `[model] spec`. A task that
    /// names a model (`queue add --model`) uses its own; otherwise a task
    /// assigned to a profile that names a model uses the profile's.
    #[arg(long)]
    model: Option<String>,
}

fn parse_priority(s: &str) -> Priority {
    Priority::parse(s).unwrap_or_default()
}

/// Reconciles and prints the wakes: after any change that can end a task,
/// whatever it was blocking or holding may now run.
fn reconcile(q: &Queue) -> Result<()> {
    for (id, why) in q.reconcile(Actor::Human)? {
        println!("woken:    {id} ({why})");
    }
    Ok(())
}

/// The variables trellis sets on the children of a run, and only there:
/// `TRELLIS_RUN_ID` for `bash`, custom tools and hooks (`tooling.rs`), and
/// `TRELLIS_TASK_ID` for a process agent, which is not given a run id
/// (`process_agent.rs`). Either one present means the caller descends from
/// a run. A child that removes them on purpose is not caught here; that
/// writer is the sandbox's to stop (ADR 0032).
const RUN_MARKERS: [&str; 2] = ["TRELLIS_RUN_ID", "TRELLIS_TASK_ID"];

/// The first run marker present, and its value. Presence is what counts,
/// an empty value included: no person's shell sets these.
fn run_marker(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<(&'static str, String)> {
    RUN_MARKERS
        .iter()
        .find_map(|&name| var(name).map(|v| (name, v.to_string_lossy().into_owned())))
}

pub(crate) fn queue(root: &Path, action: QueueAction) -> Result<()> {
    let q = Queue::for_repo(root)?;
    match action {
        QueueAction::Add {
            task,
            focus,
            prompt,
            args,
            model,
            priority,
            labels,
            parent,
            assignee,
            blocked_by,
            needs_approval,
            backlog,
        } => {
            if task.trim().is_empty() && prompt.is_none() {
                anyhow::bail!("nothing to queue: give a task, or --prompt <name>, or both");
            }
            // Checked now rather than when the heartbeat reaches the task,
            // which may be hours later with nobody watching.
            if let Some(name) = &prompt {
                config::RepoConfig::prompt(root, name)?;
            }
            // An unclosed quote would otherwise fail the run, not the add.
            if let Some(a) = &args {
                crate::templates::split_args(a)
                    .with_context(|| format!("--args `{}`", plain(a)))?;
            }
            // Parsed as `run --model` parses it, for the same reason. Only
            // parsed: whether a key may go to its endpoint is decided when
            // the task runs, against the configuration in force then.
            if let Some(m) = &model {
                let _: trellis_model::ModelSpec = m.parse()?;
            }
            if let Some(stem) = &focus {
                check_focus(root, stem)?;
            }
            if let Some(a) = &assignee {
                let profiles = load_profiles(root)?;
                if !profiles.contains_key(a) {
                    anyhow::bail!("no agent `{a}` in .trellis/agents/");
                }
            }
            let id = q.add_task(
                NewTask {
                    task,
                    focus,
                    prompt,
                    args,
                    model,
                    priority: parse_priority(&priority),
                    labels,
                    parent,
                    assignee,
                    blockers: blocked_by,
                    needs_approval,
                    backlog,
                    routine: None,
                    routine_at: None,
                },
                Actor::Human,
            )?;
            let (_, t) = q.find(&id)?;
            println!("queued:   {id} ({})", t.status_in(State::Pending).as_str());
            println!("          {}", q.dir().join("pending").display());
            Ok(())
        }
        QueueAction::List {
            status,
            label,
            assignee,
            priority,
            text,
        } => {
            // An empty needle matches every task, which would read as a
            // search that found everything.
            if text.as_deref().is_some_and(|s| s.trim().is_empty()) {
                anyhow::bail!("--text is empty: give the words to look for");
            }
            let text = text.map(|s| s.to_lowercase());
            let entries = q.list()?;
            let ix = Queue::index_of(&entries);
            let shown: Vec<&trellis_queue::Entry> = entries
                .iter()
                .filter(|e| {
                    let Ok(t) = &e.task else {
                        return status.is_none()
                            && label.is_none()
                            && assignee.is_none()
                            && priority.is_none();
                    };
                    status
                        .as_deref()
                        .is_none_or(|s| t.status_in(e.state).as_str() == s)
                        && label.as_ref().is_none_or(|l| t.labels.contains(l))
                        && assignee
                            .as_deref()
                            .is_none_or(|a| t.assignee.as_deref() == Some(a))
                        && priority.as_deref().is_none_or(|p| t.priority.as_str() == p)
                        && text.as_deref().is_none_or(|s| {
                            std::iter::once(&e.id)
                                .chain(std::iter::once(&t.task))
                                .chain(&t.labels)
                                .chain(t.notes.iter().map(|n| &n.text))
                                .any(|h| h.to_lowercase().contains(s))
                        })
                })
                .collect();
            if entries.is_empty() {
                println!("queue empty ({})", q.dir().display());
            } else if shown.is_empty() {
                println!("no task matches ({} in the queue)", entries.len());
            }
            for e in shown {
                println!("{}", list_line(e, &ix));
            }
            let requests = approval::list(&q, "pending")?;
            if !requests.is_empty() {
                println!();
                println!("approvals waiting:");
                for r in requests {
                    println!(
                        "  {}  budget_override for agent `{}`: {} of {} {} tokens ({})",
                        r.id,
                        r.agent,
                        r.spent,
                        r.limit,
                        r.window.as_str(),
                        r.month
                    );
                }
            }
            Ok(())
        }
        QueueAction::Set {
            id,
            priority,
            labels,
            unlabels,
            assignee,
            unassign,
            parent,
            no_parent,
            blocked_by,
            unblock,
            needs_approval,
            status,
            reason,
            detail,
            at,
        } => {
            if let Some(a) = &assignee
                && !load_profiles(root)?.contains_key(a)
            {
                anyhow::bail!("no agent `{a}` in .trellis/agents/");
            }
            let status = status.as_deref().and_then(Status::parse);
            if reason.is_some() && status != Some(Status::Blocked) {
                anyhow::bail!("--reason goes with --status blocked");
            }
            if detail.is_some() && reason.as_deref() != Some("manual") {
                anyhow::bail!("--detail goes with --reason manual");
            }
            if at.is_some() && reason.as_deref() != Some("until") {
                anyhow::bail!("--at goes with --reason until");
            }
            let blocked = match (status, reason.as_deref()) {
                (Some(Status::Blocked), None) => anyhow::bail!(
                    "--status blocked needs --reason blockers|children|manual|until: a block \
                     that names nothing to wait for routes to nobody"
                ),
                (Some(Status::Blocked), Some("blockers")) => Some(Blocked::Blockers),
                (Some(Status::Blocked), Some("children")) => Some(Blocked::Children),
                (Some(Status::Blocked), Some("until")) => {
                    let at = at.as_deref().context(
                        "--reason until needs --at <instant>, such as 2026-10-06T12:00:00Z",
                    )?;
                    Some(Blocked::until(at).with_context(|| {
                        format!(
                            "--at {at:?} is not an RFC 3339 instant, such as 2026-10-06T12:00:00Z"
                        )
                    })?)
                }
                (Some(Status::Blocked), Some(_)) => Some(Blocked::Manual {
                    detail: detail
                        .clone()
                        .filter(|d| !d.trim().is_empty())
                        .context("--reason manual needs --detail naming what unblocks it")?,
                }),
                _ => None,
            };
            let ix = q.index()?;
            for b in parent.iter().chain(blocked_by.iter()) {
                if b == &id {
                    anyhow::bail!("a task cannot depend on or contain itself");
                }
                if !ix.by_id.contains_key(b) {
                    return Err(QueueError::Unknown(b.clone()).into());
                }
            }
            let children = ix.children.get(&id).map(Vec::len).unwrap_or(0);
            let mut changes = Vec::new();
            let (from, to) = q.edit(&id, |t, s| {
                if let Some(p) = &priority {
                    t.priority = parse_priority(p);
                    changes.push(format!("priority {p}"));
                }
                for l in &labels {
                    if !t.labels.contains(l) {
                        t.labels.push(l.clone());
                        changes.push(format!("+label {l}"));
                    }
                }
                for l in &unlabels {
                    t.labels.retain(|x| x != l);
                    changes.push(format!("-label {l}"));
                }
                if let Some(a) = &assignee {
                    t.assignee = Some(a.clone());
                    changes.push(format!("assignee {a}"));
                }
                if unassign {
                    t.assignee = None;
                    changes.push("unassigned".into());
                }
                if let Some(p) = &parent {
                    t.parent = Some(p.clone());
                    changes.push(format!("parent {p}"));
                }
                if no_parent {
                    t.parent = None;
                    changes.push("no parent".into());
                }
                for b in &blocked_by {
                    if !t.blockers.contains(b) {
                        t.blockers.push(b.clone());
                        changes.push(format!("+blocker {b}"));
                    }
                }
                for b in &unblock {
                    t.blockers.retain(|x| x != b);
                    changes.push(format!("-blocker {b}"));
                }
                if let Some(n) = needs_approval {
                    t.needs_approval = n;
                    if n {
                        // A new requirement needs a new decision.
                        t.approval = None;
                    }
                    changes.push(format!("needs_approval {n}"));
                }
                let mut target = None;
                if let Some(st) = status {
                    match &blocked {
                        Some(Blocked::Blockers) if t.blockers.is_empty() => {
                            return Err(QueueError::Invalid(
                                "--reason blockers, but the task has no blockers; add \
                                 them with --blocked-by"
                                    .into(),
                            ));
                        }
                        Some(Blocked::Children) if children == 0 => {
                            return Err(QueueError::Invalid(
                                "--reason children, but no task names this one as its \
                                 parent"
                                    .into(),
                            ));
                        }
                        _ => {}
                    }
                    t.status = Some(st);
                    t.blocked = blocked.clone();
                    t.claimed_at = None;
                    t.claimed_by_pid = None;
                    if st.home() == State::Pending && s != State::Pending {
                        // Back to waiting: an old outcome on it would be
                        // read as this attempt's, as `requeue` says.
                        t.outcome = None;
                    }
                    target = Some(st.home());
                    changes.push(format!("status {}", st.as_str()));
                } else if s == State::Pending
                    && !blocked_by.is_empty()
                    && t.status_in(s) == Status::Todo
                    && !ix.unresolved(t).is_empty()
                {
                    // New blockers that are not done: the task now waits,
                    // and says so, rather than sitting in todo unclaimable.
                    t.status = Some(Status::Blocked);
                    t.blocked = Some(Blocked::Blockers);
                    changes.push("status blocked (blockers)".into());
                }
                Ok(target)
            })?;
            if changes.is_empty() {
                println!("nothing to change");
                return Ok(());
            }
            q.log(
                Event::new(Actor::Human, "task.set", format!("task:{id}"))
                    .detail(changes.join(", ")),
            )?;
            println!(
                "set:      {id}: {}{}",
                changes.join(", "),
                if from != to {
                    format!(" ({}/ -> {}/)", from.dir_name(), to.dir_name())
                } else {
                    String::new()
                }
            );
            reconcile(&q)
        }
        QueueAction::Note { id, text } => {
            // Before the edit: a child of a run that wrote this note would
            // be an agent waking an agent (ADR 0005, ADR 0026 §1).
            if let Some((var, value)) = run_marker(|name| std::env::var_os(name)) {
                let run_id = (var == "TRELLIS_RUN_ID").then(|| value.clone());
                q.log(
                    Event::new(Actor::Human, "task.note_refused", format!("task:{id}"))
                        .run(run_id)
                        .detail(format!("{var}={value} is set: {text}")),
                )?;
                anyhow::bail!(
                    "refused: {var} is set, so this runs inside a trellis run, and a run \
                     cannot write a person's note (ADR 0026 §1). Nothing was written to \
                     {id}; the refusal is in `trellis queue log`"
                );
            }
            // The one writer of a person's note: an `@name` in it wakes that
            // profile's task on the next `work` tick (trellis_queue::mention).
            q.edit(&id, |t, _| {
                t.note_from_person(text.clone());
                Ok(None)
            })?;
            q.log(Event::new(Actor::Human, "task.note", format!("task:{id}")).detail(text))?;
            println!("noted:    {id}");
            Ok(())
        }
        QueueAction::Cancel {
            id,
            reason,
            children,
        } => {
            if !children {
                if let Some(from) = cancel_task(&q, &id, reason.as_deref(), None)? {
                    println!("cancelled: {id} ({}/ -> cancelled/)", from.dir_name());
                }
                return reconcile(&q);
            }
            let id = id.strip_suffix(".json").unwrap_or(&id).to_string();
            let ix = q.index()?;
            // The subtree by `parent`, as `queue set` reads it. `seen` stops
            // a parent loop, which `queue set --parent` does not refuse.
            let mut seen = BTreeSet::from([id.clone()]);
            let mut next = vec![id.clone()];
            while let Some(p) = next.pop() {
                for c in ix.children.get(&p).into_iter().flatten() {
                    if seen.insert(c.clone()) {
                        next.push(c.clone());
                    }
                }
            }
            seen.remove(&id);
            let descendants: Vec<String> = seen.into_iter().collect();
            // `Queue::edit` refuses a claimed task one at a time; refusing
            // the whole command up front means it never stops half done
            // because of a run the person could have seen.
            let claimed: Vec<&str> = std::iter::once(&id)
                .chain(&descendants)
                .filter(|t| matches!(ix.by_id.get(*t), Some((State::Claimed, _))))
                .map(String::as_str)
                .collect();
            if !claimed.is_empty() {
                anyhow::bail!(
                    "refused: claimed, so running until its run ends: {}. Nothing was \
                     cancelled; retry when the runs end, or `trellis queue requeue` a claim \
                     a crashed process left behind",
                    claimed.join(", ")
                );
            }
            if let Some(from) = cancel_task(&q, &id, reason.as_deref(), None)? {
                println!("cancelled: {id} ({}/ -> cancelled/)", from.dir_name());
            }
            // Best-effort from here: the queue cannot hold every task at
            // once, so a `work` tick may claim a descendant in between
            // (ADR 0026 §3).
            let mut missed = Vec::new();
            for d in &descendants {
                let Some((State::Pending, _)) = ix.by_id.get(d) else {
                    let st = ix.by_id.get(d).map(|(s, st)| (s.dir_name(), st.as_str()));
                    let (dir, st) = st.unwrap_or(("?", "?"));
                    println!("left:      {d} ({dir}/, {st})");
                    continue;
                };
                match cancel_task(&q, d, reason.as_deref(), Some(&id)) {
                    Ok(Some(from)) => {
                        println!("cancelled: {d} ({}/ -> cancelled/)", from.dir_name())
                    }
                    Ok(None) => {
                        println!("not cancelled: {d} (left pending/ while this ran)");
                        missed.push(d.as_str());
                    }
                    Err(QueueError::Claimed { .. }) => {
                        println!("not cancelled: {d} (claimed while this ran)");
                        missed.push(d.as_str());
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            reconcile(&q)?;
            if !missed.is_empty() {
                anyhow::bail!(
                    "not cancelled, because each left pending/ while this ran: {}",
                    missed.join(", ")
                );
            }
            Ok(())
        }
        QueueAction::Approve { id, raise } => {
            if approval::is_request_id(&id) {
                let raise = raise.context(
                    "a budget override needs --raise <tokens>: how far to lift the limit \
                     that tripped. `trellis queue reject <id>` keeps the agent paused",
                )?;
                let r = approval::approve_budget(&q, &id, raise, Actor::Human)?;
                println!(
                    "approved: {id}; agent `{}` {} limit raised by {raise} and resumed",
                    r.agent,
                    r.window.as_str()
                );
            } else {
                if raise.is_some() {
                    anyhow::bail!("--raise applies to a budget override request, not a task");
                }
                approval::approve_task(&q, &id, Actor::Human)?;
                println!("approved: {id}; `work` may now claim it");
            }
            Ok(())
        }
        QueueAction::Reject { id, reason } => {
            if approval::is_request_id(&id) {
                let r = approval::reject_budget(&q, &id, reason.as_deref(), Actor::Human)?;
                println!("rejected: {id}; agent `{}` stays paused", r.agent);
            } else {
                approval::reject_task(&q, &id, reason.as_deref(), Actor::Human)?;
                println!("rejected: {id}; cancelled");
                reconcile(&q)?;
            }
            Ok(())
        }
        QueueAction::Requeue { id } => {
            let from = q.requeue(&id)?;
            println!(
                "requeued: {} ({}/ -> pending/)",
                plain(&id),
                from.dir_name()
            );
            Ok(())
        }
        QueueAction::Log { limit } => {
            let events = q.activity().tail(limit)?;
            if events.is_empty() {
                println!("no activity ({})", q.activity().path().display());
            }
            for e in events {
                println!(
                    "{} {:<7} {:<18} {}{}{}",
                    plain(&e.at),
                    e.actor.as_str(),
                    plain(&e.action),
                    plain(&e.entity),
                    e.run_id
                        .as_deref()
                        .map(|r| format!(" run {}", plain(r)))
                        .unwrap_or_default(),
                    e.detail
                        .as_deref()
                        .map(|d| format!(" — {}", plain(d)))
                        .unwrap_or_default()
                );
            }
            Ok(())
        }
        QueueAction::Export { dir } => {
            load_profiles(root)?;
            let n = portable::export(root, &q, &dir)?;
            println!(
                "exported: {} agent(s), {} instructions file(s), {} routine(s), {} task(s) to {}",
                n.agents,
                n.instructions,
                n.routines,
                n.tasks,
                dir.display()
            );
            // First: the one left-behind state that changes what an import
            // does. The exported file does not say `paused`, so it runs there.
            for name in &n.paused_by_overlay {
                println!(
                    "          agent `{name}` is paused here (`agents pause` or a budget stop), \
                     not in its file: an import does not carry its pause, and will run it"
                );
            }
            if n.claimed_skipped > 0 {
                println!(
                    "          {} claimed task(s) not exported: they belong to a run in \
                     progress",
                    n.claimed_skipped
                );
            }
            // This checkout's runtime state, not definitions (ADR 0026 §10).
            for (count, what) in [
                (n.overlays, "agent overlay(s) (pauses, budget raises)"),
                (
                    n.routine_states,
                    "routine state file(s) (when each last fired)",
                ),
                (n.approval_requests, "approval request(s)"),
                (n.activity_lines, "activity log line(s)"),
            ] {
                if count > 0 {
                    println!(
                        "          {count} {what} not exported: runtime state of this checkout"
                    );
                }
            }
            println!(
                "          nothing secret to scrub: profiles name a model spec, and keys \
                 are never in files trellis reads (ADR 0010)"
            );
            Ok(())
        }
        QueueAction::Import {
            dir,
            dry_run,
            on_conflict,
        } => {
            let on = OnConflict::parse(&on_conflict).unwrap_or(OnConflict::Skip);
            let plan = portable::plan_import(root, &q, &dir, on)?;
            if plan.items.is_empty() {
                println!("nothing to import in {}", dir.display());
                return Ok(());
            }
            for it in &plan.items {
                println!(
                    "{:<8} {:<8} {}",
                    it.kind.as_str(),
                    match &it.action {
                        Action::Create => "create".to_string(),
                        Action::Skip => "skip".to_string(),
                        Action::Replace => "replace".to_string(),
                        Action::Rename(_) => "rename".to_string(),
                    },
                    match &it.action {
                        Action::Rename(to) => format!("{} -> {to}", it.name),
                        _ => it.name.clone(),
                    }
                );
            }
            if dry_run {
                println!("dry run: nothing written");
                return Ok(());
            }
            portable::apply_import(root, &q, &plan)?;
            println!(
                "imported. Profiles and routines were written into .trellis/; review and \
                 commit them like any change there"
            );
            reconcile(&q)
        }
    }
}

/// Cancels one task and logs it, as `queue cancel` always has. With `via`,
/// the id a `--children` cancel was given: the task is one of its
/// descendants, is cancelled only while still in pending/, and its log line
/// names `via`. Returns the directory the task left, or `None` for a
/// descendant no longer pending, which is left as it is.
fn cancel_task(
    q: &Queue,
    id: &str,
    reason: Option<&str>,
    via: Option<&str>,
) -> std::result::Result<Option<State>, QueueError> {
    let mut moved = false;
    let edited = q.edit(id, |t, s| {
        if via.is_some() && s != State::Pending {
            // A `work` run finished it after the command read the index.
            moved = true;
            return Err(QueueError::Invalid(format!("{id} left pending/")));
        }
        t.status = Some(Status::Cancelled);
        t.blocked = None;
        t.note(
            Actor::Human,
            match reason {
                Some(r) => format!("cancelled: {r}"),
                None => "cancelled".into(),
            },
        );
        Ok(Some(State::Cancelled))
    });
    let (from, _) = match edited {
        Err(_) if moved => return Ok(None),
        r => r?,
    };
    let mut e = Event::new(Actor::Human, "task.cancel", format!("task:{id}"));
    let detail = match (via, reason) {
        (Some(v), Some(r)) => Some(format!("{r} (queue cancel {v} --children)")),
        (Some(v), None) => Some(format!("queue cancel {v} --children")),
        (None, r) => r.map(str::to_string),
    };
    if let Some(d) = detail {
        e = e.detail(d);
    }
    q.log(e)?;
    Ok(Some(from))
}

/// The note must exist and compile, exactly as the run will ask.
fn check_focus(root: &Path, stem: &str) -> Result<()> {
    let cfg = config::RepoConfig::load(root)?;
    if let Some(v) = cfg.vault(root, Some(stem))? {
        trellis_vault::compile(&v.dir, stem, trellis_vault::DEFAULT_CAP).with_context(|| {
            format!(
                "the vault at {} would not compile for focus `{stem}`",
                v.source
            )
        })?;
    }
    Ok(())
}

/// `rpc`'s `get_queue`: every task `queue list` prints with no filter, in its
/// order, as data. Only reads: `Queue::at` creates nothing until a write.
///
/// Strings are given as stored; `queue list` escapes control characters for
/// the terminal, and JSON escapes its own. The approvals block is not a task
/// and is left out.
pub(crate) fn queue_json(root: &Path) -> std::result::Result<serde_json::Value, QueueError> {
    let q = Queue::for_repo(root)?;
    let entries = q.list()?;
    let ix = Queue::index_of(&entries);
    let tasks: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let state = e.state.dir_name();
            match &e.task {
                Err(why) => serde_json::json!({"id": e.id, "state": state, "error": why}),
                Ok(t) => {
                    // The same condition `list_line` prints `waiting:` under.
                    let waiting = (e.state == State::Pending)
                        .then(|| Queue::waiting_reason(t, &ix))
                        .flatten();
                    serde_json::json!({
                        "id": e.id,
                        "state": state,
                        "status": t.status_in(e.state).as_str(),
                        "priority": t.priority.as_str(),
                        "task": t.task,
                        "labels": t.labels,
                        "assignee": t.assignee,
                        "waiting": waiting,
                    })
                }
            }
        })
        .collect();
    Ok(serde_json::json!({ "tasks": tasks }))
}

fn list_line(e: &trellis_queue::Entry, ix: &trellis_queue::Index) -> String {
    let age = e.age.map(ago).unwrap_or_else(|| "?".to_string());
    let state = e.state.dir_name();
    let t = match &e.task {
        Ok(t) => t,
        Err(why) => return format!("{state:<9} {}  unreadable: {}", e.id, plain(why)),
    };
    let st = t.status_in(e.state);
    let mut line = format!(
        "{state:<9} {}  {} {}  ",
        e.id,
        st.as_str(),
        t.priority.as_str()
    );
    match e.state {
        State::Pending => line.push_str(&format!("added {age} ago")),
        State::Claimed => line.push_str(&format!(
            "claimed {age} ago by pid {}",
            t.claimed_by_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into())
        )),
        State::Done | State::Failed | State::Cancelled => {
            if let Some(o) = &t.outcome {
                line.push_str(&format!(
                    "{} ({})",
                    plain(&o.exit),
                    o.run_id.as_deref().map(plain).unwrap_or("no trail".into())
                ));
            }
        }
    }
    if t.attempts > 0 {
        line.push_str(&format!(", attempts {}", t.attempts));
    }
    line.push_str(&format!("\n          {}", plain(&t.task)));
    if let Some(f) = &t.focus {
        line.push_str(&format!(" [goal {}]", plain(f)));
    }
    if let Some(p) = &t.prompt {
        line.push_str(&format!(" [prompt {}]", plain(p)));
    }
    if let Some(a) = &t.args {
        line.push_str(&format!(" [args {}]", plain(a)));
    }
    if let Some(a) = &t.assignee {
        line.push_str(&format!(" [agent {}]", plain(a)));
    }
    if !t.labels.is_empty() {
        line.push_str(&format!(" [labels {}]", plain(&t.labels.join(","))));
    }
    if let Some(p) = &t.parent {
        line.push_str(&format!(" [parent {}]", plain(p)));
    }
    if !t.blockers.is_empty() {
        line.push_str(&format!(" [blocked by {}]", plain(&t.blockers.join(","))));
    }
    if let Some(r) = &t.routine {
        line.push_str(&format!(" [routine {}]", plain(r)));
    }
    if !t.notes.is_empty() {
        line.push_str(&format!(" [{} note(s)]", t.notes.len()));
    }
    if e.state == State::Pending
        && let Some(why) = Queue::waiting_reason(t, ix)
    {
        line.push_str(&format!("\n          waiting: {}", plain(&why)));
    }
    line
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// The heartbeat. Returns the process exit code.
pub(crate) fn work(root: &Path, args: WorkArgs) -> Result<i32> {
    let cfg = config::RepoConfig::load(root)?;
    // Optional now: a queue of process-agent tasks, or of tasks whose
    // profiles name their own model, needs none. A task that does need it
    // is left waiting with that reason rather than failed.
    let model_from_user = args.model.is_some();
    let model = args.model.or_else(|| cfg.model());
    // Parsed before anything is claimed, so a typo costs nothing rather than
    // failing the task at the head of the queue.
    if let Some(m) = &model {
        let _: trellis_model::ModelSpec = m.parse()?;
    }
    // Profiles and routines are checked before anything is claimed, for the
    // same reason.
    let profiles = load_profiles(root)?;
    load_routines(root, &profiles)?;
    let stale_claims = cfg.stale_claims()?;
    let mut q = Queue::for_repo(root)?;
    if stale_claims == trellis_queue::StaleClaims::Release {
        q = with_claimant(q);
    }

    let stop = Arc::new(AtomicBool::new(false));
    listen_for_ctrl_c(stop.clone());

    // ADR 0025 §4: the human stream, so `work --json` keeps stdout JSONL.
    let _ = writeln!(
        crate::output::human(),
        "work:     queue at {}",
        q.dir().display()
    );
    let mut runner = CliRunner {
        root,
        model,
        model_from_user,
        unconstrained: cfg.unconstrained(),
        stop,
        profiles,
        queue: q.clone(),
        routines: BTreeMap::new(),
        current: None,
        said: BTreeSet::new(),
        trust_check: crate::tooling::require_trusted,
        trust: None,
    };
    let opts = WorkOptions {
        max_tasks: if args.once { Some(1) } else { args.max_tasks },
        every: args.every.map(Duration::from_secs),
        stale_claims,
    };
    let (ran, why) = trellis_queue::work(&q, &mut runner, opts, &mut crate::output::human())?;
    let _ = writeln!(
        crate::output::human(),
        "work:     {ran} run(s) this invocation; stopped: {why}"
    );
    Ok(why.code())
}

/// Under `stale_claims = "release"`, every claim this heartbeat makes
/// records where it was made and when this process started, so that a later
/// heartbeat can tell whether its holder is gone (ADR 0020 §5). Probed once
/// per invocation, not per claim: on Windows the probe is a PowerShell
/// start. A machine whose name cannot be read records nothing, and its
/// claims are then never released automatically.
fn with_claimant(q: Queue) -> Queue {
    use trellis_queue::{Claimant, Liveness, Probe};
    let live = crate::liveness::SystemLiveness;
    let Some(host) = live.host() else {
        let _ = writeln!(
            crate::output::human(),
            "work:     this machine's hostname could not be read; claims made now \
             cannot be released automatically if this process dies"
        );
        return q;
    };
    let started = match live.probe(std::process::id()) {
        Probe::Running { started } => started,
        Probe::Gone | Probe::Unknown(_) => None,
    };
    q.with_claimant(Claimant {
        host,
        os: live.os(),
        started,
    })
}

/// Ctrl-C sets a flag the heartbeat reads between runs and while idle.
///
/// Needed for more than tidiness. `cmd_run` races its run against
/// `tokio::signal::ctrl_c()`, and once that has been called the process's
/// SIGINT handler is tokio's for good — so after the first task a plain
/// `sleep` in `--every` would swallow Ctrl-C and the loop could not be
/// stopped. During a run both listeners fire: `cmd_run` rolls back, and the
/// flag stops the heartbeat after it.
fn listen_for_ctrl_c(flag: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async {
            while tokio::signal::ctrl_c().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });
    });
}

struct CliRunner<'a> {
    root: &'a Path,
    model: Option<String>,
    /// Whether `model` came from `--model` rather than `[model] spec`
    /// (ADR 0024 §3). A profile's own model is the repository's, and so is
    /// a task's (ADR 0026 §1).
    model_from_user: bool,
    unconstrained: bool,
    stop: Arc<AtomicBool>,
    /// Re-read every tick: a profile edited while `work --every` runs takes
    /// effect at the next claim.
    profiles: BTreeMap<String, Profile>,
    /// Held rather than re-resolved: the gate runs per pending task per
    /// tick, and finding the queue spawns `git rev-parse`.
    queue: Queue,
    routines: BTreeMap<String, Routine>,
    /// The id of the task being run, for `TRELLIS_TASK_ID`.
    current: Option<String>,
    /// Lines already printed this invocation. A budget warning or a load
    /// error that holds for hours is said once, not once per tick.
    said: BTreeSet<String>,
    /// How trust is decided: `tooling::require_trusted`, which walks and
    /// hashes `.trellis/`. A field so a test can count the calls.
    trust_check: fn(&Path) -> std::result::Result<(), String>,
    /// This tick's answer, computed at the first process task the gate
    /// sees and cleared by `tick`: once per tick, not once per pending
    /// process task, and never older than the tick, so `trellis trust`
    /// run while `work --every` waits takes effect at the next one.
    trust: Option<std::result::Result<(), String>>,
}

impl CliRunner<'_> {
    fn once(&mut self, out: &mut dyn Write, line: String) {
        if self.said.insert(line.clone()) {
            let _ = writeln!(out, "{line}");
        }
    }

    /// The task text as `trellis run --prompt` builds it, with the
    /// profile's instructions ahead of it.
    fn text(&self, t: &trellis_queue::Task, profile: Option<&Profile>) -> Result<String> {
        let mut text = crate::templates::task_text(
            self.root,
            t.prompt.as_deref(),
            t.args.as_deref(),
            &t.task,
        )?;
        if let Some(p) = profile
            && let Some(instructions) = instructions(self.root, p)?
        {
            text = format!("{}\n\n{text}", instructions.trim_end());
        }
        // After the task: the question the last run ended on, quoted as
        // that run's output, and the person's answer as their input
        // (ADR 0026 §1). A task that never asked gets the text it always did.
        if let Some(answers) = t.answers_since_question() {
            text = format!("{text}\n\n{answers}");
        }
        Ok(text)
    }

    fn approve_mode(p: &Profile) -> bool {
        p.file.control.as_ref().and_then(|c| c.mode.as_deref()) == Some("approve")
    }
}

/// A profile's `instructions` file, or `None` when it names none. Read
/// through the one prompt-input reader, so a committed
/// `.trellis/agents/r.md -> ~/.aws/credentials` is refused by name rather
/// than put ahead of the task. Named but absent is an error, as it was: a
/// profile that says it has instructions and runs without them is worse off
/// than one that never had any.
fn instructions(root: &Path, p: &Profile) -> Result<Option<String>> {
    let Some(path) = p.instructions_path(root) else {
        return Ok(None);
    };
    match crate::context_files::read_repo_text(root, &path)
        .with_context(|| format!("agent `{}`: instructions were refused", p.name))?
    {
        Some(t) => Ok(Some(t)),
        None => anyhow::bail!(
            "agent `{}`: cannot read instructions {}: no such file",
            p.name,
            path.display()
        ),
    }
}

/// The newest trail's run id: its file stem, which `record_trail` makes
/// `<timestamp>-<slug>`. Read by name only — the contents are untrusted, and
/// the name is all that is needed.
fn newest_run_id(root: &Path) -> Option<String> {
    let rd = std::fs::read_dir(root.join(".trellis").join("runs")).ok()?;
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let n = e.file_name().to_str()?.to_string();
            n.strip_suffix(".json").map(str::to_string)
        })
        .max()
}

/// The line for a budget pause `enforce` lifted, with its reason: a
/// rollover and a raised limit call for different things from the person.
fn resumed_line(agent: &str, why: &str) -> String {
    format!("budget:   agent `{agent}` resumed; {why}")
}

fn has_terminal() -> bool {
    matches!(Human::terminal(), Human::Line(_))
}

impl Runner for CliRunner<'_> {
    fn blocker(&mut self) -> Option<String> {
        match trellis_vcs::pending(self.root) {
            Ok(p) if p.is_empty() => None,
            Ok(p) => Some(format!(
                "the worktree is not clean ({}), and a run needs a clean one (ADR 0009). \
                 If a converged change is waiting, accept or revert it, then run \
                 `trellis work` again",
                p.iter().map(|l| plain(l)).collect::<Vec<_>>().join("; ")
            )),
            Err(e) => Some(e.to_string()),
        }
    }

    fn tick(&mut self, q: &Queue, out: &mut dyn Write) {
        self.trust = None;
        match load_profiles(self.root) {
            Ok(p) => self.profiles = p,
            Err(e) => self.once(
                out,
                format!("work:     profiles not reloaded, keeping the last good set: {e:#}"),
            ),
        }
        match load_routines(self.root, &self.profiles) {
            Ok(r) => self.routines = r,
            Err(e) => self.once(
                out,
                format!("work:     routines not reloaded, keeping the last good set: {e:#}"),
            ),
        }
        let now = trellis_queue::now_secs();
        let routines: Vec<Routine> = self.routines.values().cloned().collect();
        for r in &routines {
            match routine::evaluate(q, r, now) {
                Ok(t) => {
                    if t.armed {
                        let _ = writeln!(out, "routine:  {} armed", r.name);
                    }
                    if t.missed > 0 {
                        let _ = writeln!(
                            out,
                            "routine:  {} dropped {} missed fire(s) (catch-up policy)",
                            r.name, t.missed
                        );
                    }
                    for (at, f) in t.fired {
                        let when = trellis_queue::secs_to_iso(at);
                        let _ = writeln!(
                            out,
                            "routine:  {} fired for {when}: {}",
                            r.name,
                            match f {
                                routine::Fired::Enqueued(id) => format!("queued {id}"),
                                routine::Fired::Coalesced(id) => format!("coalesced into {id}"),
                                routine::Fired::Skipped(id) => format!("skipped; {id} still open"),
                            }
                        );
                    }
                }
                Err(e) => self.once(out, format!("routine:  {} not evaluated: {e}", r.name)),
            }
        }
        // Budgets: thresholds, hard stops, and monthly rollovers.
        let month = this_month();
        let records = spend_records(self.root);
        let profiles: Vec<Profile> = self.profiles.values().cloned().collect();
        for p in profiles.iter().filter(|p| p.file.budget.is_some()) {
            let s = ledger::spend(&records, &p.name, &month);
            match ledger::enforce(q, p, &s, &month, Actor::Work) {
                Ok(done) => {
                    if let Some(why) = &done.resumed {
                        let _ = writeln!(out, "{}", resumed_line(&p.name, why));
                    }
                    if let Some(m) = done.message {
                        self.once(out, format!("budget:   {m}"));
                    }
                }
                Err(e) => self.once(out, format!("budget:   `{}` not checked: {e}", p.name)),
            }
        }
    }

    fn gate(&mut self, _id: &str, t: &trellis_queue::Task, _out: &mut dyn Write) -> Option<String> {
        let Some(name) = &t.assignee else {
            return (self.model.is_none() && t.model.is_none()).then(crate::first_run::no_model);
        };
        let Some(p) = self.profiles.get(name) else {
            return Some(format!("no agent profile `{name}` in .trellis/agents/"));
        };
        let o = match trellis_queue::profile::overlay(&self.queue, name) {
            Ok(o) => o,
            // An overlay that cannot be read may hold a pause; running the
            // task anyway would ignore it.
            Err(e) => return Some(format!("agent `{name}`: overlay unreadable: {e}")),
        };
        if let Some(why) = trellis_queue::profile::paused_reason(p, &o) {
            return Some(why);
        }
        if Self::approve_mode(p) && !has_terminal() {
            return Some(format!(
                "agent `{name}` is in approve mode, and `work` has no terminal to ask on"
            ));
        }
        // A process agent's command is repository code (ADR 0026 §11), and
        // so is a service's (§12), so either waits for the same decision a
        // hook does (ADR 0023 §4).
        if p.is_process() || !p.file.services.is_empty() {
            let (check, root) = (self.trust_check, self.root);
            if let Err(why) = self.trust.get_or_insert_with(|| check(root)) {
                return Some(format!("agent `{name}` runs a repository command; {why}"));
            }
        }
        if !p.is_process() && p.file.model.is_none() && self.model.is_none() && t.model.is_none() {
            return Some(format!(
                "agent `{name}` names no model, and `work` has none: pass --model"
            ));
        }
        None
    }

    fn next_due(&self) -> Option<Duration> {
        let now = trellis_queue::now_secs();
        routine::next_due(&self.routines, now).map(|t| Duration::from_secs((t - now).max(1) as u64))
    }

    fn starting(&mut self, id: &str) {
        self.current = Some(id.to_string());
    }

    fn liveness(&self) -> Option<&dyn trellis_queue::Liveness> {
        Some(&crate::liveness::SystemLiveness)
    }

    /// The set `tick` just reloaded, or the last good one: the same set
    /// `gate` decides against, so a mention never wakes a task for a
    /// profile the gate would not know.
    fn profiles(&self) -> Option<&BTreeMap<String, Profile>> {
        Some(&self.profiles)
    }

    fn run(&mut self, t: &trellis_queue::Task) -> RunEnd {
        let profile = t
            .assignee
            .as_ref()
            .and_then(|a| self.profiles.get(a))
            .cloned();
        let text = match self.text(t, profile.as_ref()) {
            Ok(s) => s,
            Err(e) => {
                return RunEnd::Error {
                    run_id: None,
                    error: format!("{e:#}"),
                };
            }
        };
        let id = self.current.clone().unwrap_or_default();
        // Started now, after the task text is known to build and before the
        // run, and held to the end of this function: every way out of it (a
        // result, an error, Ctrl-C, which the run returns as `Interrupted`,
        // or a panic unwinding) drops the guard, which kills each service
        // with its process tree (ADR 0026 §12).
        let services = match profile.as_ref().filter(|p| !p.file.services.is_empty()) {
            Some(p) => {
                // Checked again right before the spawn, as a process agent's
                // runner does: the gate's answer is the tick's, and
                // `.trellis/` may have changed since.
                if let Err(why) = (self.trust_check)(self.root) {
                    return RunEnd::Error {
                        run_id: None,
                        error: format!("agent `{}` runs a repository command; {why}", p.name),
                    };
                }
                match crate::services::Services::start(
                    self.root,
                    &self.queue.trellis_dir(),
                    p,
                    &id,
                    &mut crate::output::human(),
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        return RunEnd::Error {
                            run_id: None,
                            error: format!("{e:#}"),
                        };
                    }
                }
            }
            None => crate::services::Services::none(),
        };
        let service_actions = services.trail_actions();
        if let Some(p) = profile.as_ref().filter(|p| p.is_process()) {
            return crate::process_agent::run(
                crate::process_agent::ProcessRun {
                    root: self.root,
                    profile: p,
                    task_id: &id,
                    text: &text,
                    stop: &self.stop,
                    services: &service_actions,
                },
                // The human stream, not stdout: under `work --json` stdout
                // carries JSONL only (ADR 0025 §4).
                &mut crate::output::human(),
            );
        }
        // The task's own model, then the profile's, then `work --model` or
        // `[model] spec` (ADR 0026 §1). Only `work --model` is the user's
        // choice for keys: a task file can arrive through `queue import`,
        // so its model counts as the repository's, as a profile's does
        // (ADR 0024 §3).
        let model_from_user = self.model_from_user
            && t.model.is_none()
            && profile.as_ref().is_none_or(|p| p.file.model.is_none());
        let Some(model) = t
            .model
            .clone()
            .or_else(|| profile.as_ref().and_then(|p| p.file.model.clone()))
            .or_else(|| self.model.clone())
        else {
            return RunEnd::Error {
                run_id: None,
                error: "no model for this task".into(),
            };
        };
        if let Some(p) = &profile {
            let _ = writeln!(crate::output::human(), "agent:    {}", p.name);
        }
        // Auto, unless the profile asks for approve mode — and the gate let
        // that task through only with a terminal attached. The heartbeat is
        // otherwise the hands-off end of the dial (ADR 0020 §5): approve
        // mode with nobody to answer would refuse every task.
        let control = match profile.as_ref().filter(|p| Self::approve_mode(p)) {
            Some(_) => RunControl {
                flag: Some(config::ControlMode::Approve),
                human: Human::terminal(),
                steering: None,
            },
            None => RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
                steering: None,
            },
        };
        let before = newest_run_id(self.root);
        let exit = cmd_run(
            self.root,
            &text,
            &model,
            self.unconstrained,
            RunSeeding {
                resume: false,
                focus: t.focus.as_deref(),
                profile: profile.as_ref(),
                model_from_user,
                queued: true,
                services: &service_actions,
            },
            control,
            &mut crate::output::human(),
        );
        let after = newest_run_id(self.root);
        let run_id = if after != before { after } else { None };
        match exit {
            Ok(RunExit::Converged) => RunEnd::Converged { run_id },
            Ok(RunExit::NotConverged { restored: true }) => RunEnd::RolledBack { run_id },
            Ok(RunExit::NotConverged { restored: false }) => RunEnd::NotRestored { run_id },
            Ok(RunExit::Interrupted { restored }) => RunEnd::Interrupted { run_id, restored },
            Ok(RunExit::Asked { question, options }) => RunEnd::Asked {
                run_id,
                question,
                options,
            },
            Err(e) => RunEnd::Error {
                run_id,
                error: format!("{e:#}"),
            },
        }
    }

    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn idle(&mut self, d: Duration) -> bool {
        let until = Instant::now() + d;
        while Instant::now() < until {
            if self.stop_requested() {
                return false;
            }
            std::thread::sleep(
                Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())),
            );
        }
        !self.stop_requested()
    }
}

/// One `CliRunner` for every test module, so a new field is one edit.
#[cfg(test)]
fn test_runner(
    root: &Path,
    queue: Queue,
    trust_check: fn(&Path) -> std::result::Result<(), String>,
) -> CliRunner<'_> {
    CliRunner {
        root,
        model: None,
        model_from_user: false,
        unconstrained: false,
        stop: Arc::new(AtomicBool::new(false)),
        profiles: BTreeMap::new(),
        queue,
        routines: BTreeMap::new(),
        current: None,
        said: BTreeSet::new(),
        trust_check,
        trust: None,
    }
}

#[cfg(test)]
fn trusted(_: &Path) -> std::result::Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod resumed_tests {
    use super::*;

    #[test]
    fn a_lift_is_reported_with_enforce_s_reason_not_always_as_a_rollover() {
        // The risk: every lift printed "its monthly window rolled over",
        // including one for a lifetime limit raised in the profile file.
        let d = tempfile::tempdir().unwrap();
        let q = Queue::at(d.path().join("queue"));
        let p = |lifetime: u64| {
            trellis_queue::profile::parse(
                "rev",
                &format!("[budget]\nlifetime_tokens = {lifetime}\n"),
            )
            .unwrap()
        };
        let s = trellis_queue::ledger::Spend {
            month: 100,
            lifetime: 100,
            ..Default::default()
        };
        trellis_queue::ledger::enforce(&q, &p(100), &s, "2026-09", Actor::Work).unwrap();
        let done =
            trellis_queue::ledger::enforce(&q, &p(1000), &s, "2026-09", Actor::Work).unwrap();
        let line = resumed_line("rev", done.resumed.as_deref().expect("lifted"));
        assert!(!line.contains("rolled over"), "{line}");
        assert!(line.contains("no longer over its token limit"), "{line}");
    }
}

#[cfg(test)]
mod instructions_tests {
    use super::*;

    #[test]
    fn instructions_linked_outside_the_repository_are_refused() {
        // The risk: a committed `.trellis/agents/r.md -> ~/.aws/credentials`
        // was read through the link and put ahead of every task the profile
        // ran, into a hosted request body.
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("credentials");
        std::fs::write(&secret, "SECRET").unwrap();
        std::fs::create_dir_all(d.path().join(".trellis/agents")).unwrap();
        let p = trellis_queue::profile::parse("r", "instructions = \"agents/r.md\"\n").unwrap();
        let at = d.path().join(".trellis/agents/r.md");
        if let Err(why) = trellis_core::paths::symlink_for_test(&secret, &at) {
            eprintln!("{why}");
            return;
        }
        let e = format!("{:#}", instructions(d.path(), &p).unwrap_err());
        assert!(e.contains("outside the repository"), "{e}");
        assert!(!e.contains("SECRET"), "{e}");

        // A plain file still loads.
        std::fs::remove_file(&at).unwrap();
        std::fs::write(&at, "Review only.").unwrap();
        assert_eq!(
            instructions(d.path(), &p).unwrap().as_deref(),
            Some("Review only.")
        );
    }
}

#[cfg(test)]
mod trust_per_tick_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static TRUSTED: AtomicBool = AtomicBool::new(false);

    fn counted(_: &Path) -> std::result::Result<(), String> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        if TRUSTED.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("not trusted: run trellis trust".into())
        }
    }

    #[test]
    fn trust_is_decided_once_per_tick_and_again_at_the_next() {
        // The risk: `require_trusted` walks and hashes `.trellis/`, and the
        // gate ran it for every pending process task on every tick. And the
        // other side: a cached answer must not outlive its tick, or a
        // `trellis trust` while `work --every` waits is never noticed.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join(".trellis/agents")).unwrap();
        std::fs::write(
            root.join(".trellis/agents/p.toml"),
            "command = [\"true\"]\n",
        )
        .unwrap();
        let q = Queue::at(root.join("queue"));
        let mut r = test_runner(root, q.clone(), counted);
        r.profiles = load_profiles(root).unwrap();
        let task = trellis_queue::Task {
            task: "t".into(),
            assignee: Some("p".into()),
            ..Default::default()
        };
        let mut out = Vec::new();
        r.tick(&q, &mut out);
        for id in ["a", "b", "c"] {
            let why = r.gate(id, &task, &mut out).expect("untrusted: waits");
            assert!(why.contains("not trusted"), "{why}");
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "once for three tasks");

        TRUSTED.store(true, Ordering::SeqCst);
        r.tick(&q, &mut out);
        assert_eq!(
            r.gate("a", &task, &mut out),
            None,
            "the next tick sees trust"
        );
        assert_eq!(r.gate("b", &task, &mut out), None);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }
}

#[cfg(test)]
mod queue_add_args_tests {
    use super::*;

    fn add(root: &Path, task: &str, prompt: &str, args: &str) -> Result<()> {
        queue(
            root,
            QueueAction::Add {
                task: task.into(),
                focus: None,
                prompt: Some(prompt.into()),
                args: Some(args.into()),
                model: None,
                priority: "medium".into(),
                labels: Vec::new(),
                parent: None,
                assignee: None,
                blocked_by: Vec::new(),
                needs_approval: false,
                backlog: false,
            },
        )
    }

    #[test]
    fn queue_add_args_expand_byte_for_byte_as_run_args() {
        // The risk: a queued template run hours later with its arguments
        // split, substituted or joined differently from the `run` a person
        // would have typed, so the same command means two things depending
        // on whether it went through the queue. And the one before it: the
        // queue storing the arguments but `work` never passing them on.
        let root = crate::testutil::scratch("queue-add-args");
        crate::testutil::git_init(&root);
        let prompts = root.join(".trellis").join("prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(
            prompts.join("fix.md"),
            "---\ndescription: fix one file\nargument-hint: <file> [mode]\n---\n\
             Fix $1 in ${2:-lenient} mode. All: $@\n",
        )
        .unwrap();
        // Uses no argument, so the arguments are appended, as `run` does.
        std::fs::write(prompts.join("review.md"), "Review the module.\n").unwrap();

        let cases = [
            (
                "fix",
                r#""app/x y.py" strict"#,
                "then run the tests",
                "Fix app/x y.py in strict mode. All: app/x y.py strict\n\nthen run the tests",
            ),
            (
                "fix",
                "app/z.py",
                "",
                "Fix app/z.py in lenient mode. All: app/z.py",
            ),
            ("review", "a 'b c'", "", "Review the module.\n\na b c"),
        ];
        let q = Queue::for_repo(&root).unwrap();
        for (prompt, args, task, expected) in cases {
            add(&root, task, prompt, args).unwrap();
            let t = q
                .list()
                .unwrap()
                .into_iter()
                .filter_map(|e| e.task.ok())
                .find(|t| t.prompt.as_deref() == Some(prompt) && t.args.as_deref() == Some(args))
                .expect("the task is queued with its arguments unsplit");
            let r = test_runner(&root, q.clone(), trusted);
            let queued = r.text(&t, None).unwrap();
            assert_eq!(queued, expected, "{prompt} {args}");
            // What `trellis run --prompt <p> --args <a> <task>` sends.
            let run = crate::templates::task_text(&root, Some(prompt), Some(args), task).unwrap();
            assert_eq!(queued.as_bytes(), run.as_bytes(), "{prompt} {args}");
        }

        // Checked at add time, not when the heartbeat reaches it.
        let before = q.list().unwrap().len();
        let e = format!("{:#}", add(&root, "", "fix", "\"unclosed").unwrap_err());
        assert!(e.contains("--args"), "{e}");
        assert_eq!(q.list().unwrap().len(), before, "nothing was queued");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn queue_add_args_an_older_task_file_reads_back_unchanged() {
        // The risk: `args` made required on disk, so every task queued
        // before it no longer loads, or written as `"args": null`, so a
        // rewritten older task stops being byte-identical to what an older
        // binary wrote and reads.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let prompts = root.join(".trellis").join("prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        // No frontmatter and no arguments: the `$1` survives verbatim, as
        // it did before templates took arguments.
        std::fs::write(prompts.join("old.md"), "Check $1 by hand.\n").unwrap();
        let q = Queue::at(root.join("queue"));
        let pending = q.dir().join("pending");
        std::fs::create_dir_all(&pending).unwrap();
        let old = r#"{"task":"and the docs","focus":null,"prompt":"old","added_at":"2026-01-01T00:00:00Z","attempts":0}"#;
        let id = "20260101-000000-000000001-old";
        std::fs::write(pending.join(format!("{id}.json")), old).unwrap();

        let (_, t) = q.find(id).unwrap();
        assert_eq!(t.args, None);
        assert_eq!(t.prompt.as_deref(), Some("old"));
        assert_eq!(serde_json::to_string(&t).unwrap(), old);
        let r = test_runner(root, q.clone(), trusted);
        assert_eq!(
            r.text(&t, None).unwrap(),
            "Check $1 by hand.\n\nand the docs"
        );
    }
}

#[cfg(test)]
mod question_tests {
    use super::*;

    #[test]
    fn a_task_that_asked_runs_with_the_answer_after_its_text_and_one_that_did_not_is_unchanged() {
        // The risk: the answer recorded with `queue note` never reached the
        // next run, which would ask the same question again; or a task that
        // never asked had its prompt changed.
        let d = tempfile::tempdir().unwrap();
        let q = Queue::at(d.path().join("queue"));
        let r = test_runner(d.path(), q, trusted);
        let mut t = trellis_queue::Task {
            task: "retire the v1 endpoint".into(),
            ..Default::default()
        };
        assert_eq!(r.text(&t, None).unwrap(), "retire the v1 endpoint");
        t.note(
            Actor::Model,
            trellis_queue::question_note("Keep v1 for mobile?", &[]),
        );
        t.note(Actor::Human, "No, mobile moved to v2 in June.");
        let text = r.text(&t, None).unwrap();
        let (task, block) = text.split_once("\n\n").unwrap();
        assert_eq!(task, "retire the v1 endpoint");
        assert!(block.starts_with("--- user input:"), "{text}");
        assert!(block.contains(r#""Keep v1 for mobile?""#), "{text}");
        assert!(block.contains("No, mobile moved to v2 in June."), "{text}");
    }
}

#[cfg(test)]
mod mention_wake_cli_tests {
    use super::*;

    #[test]
    fn a_person_note_wakes_its_profile_through_the_real_heartbeat() {
        // The risk: the queue can wake on a mention, but the binary never
        // hands its heartbeat the profiles, or `queue note` writes a note
        // without the id that marks it a person's, and `@reviewer` in a
        // note does nothing with no error anywhere.
        let root = crate::testutil::scratch("mention-wake");
        crate::testutil::git_init(&root);
        std::fs::create_dir_all(root.join(".trellis/agents")).unwrap();
        std::fs::write(root.join(".trellis/agents/reviewer.toml"), "").unwrap();
        crate::testutil::git_commit_all(&root, "fixture");

        let q = Queue::for_repo(&root).unwrap();
        let id = q.add("x", None, None).unwrap();
        q.edit(&id, |t, _| {
            t.status = Some(Status::Cancelled);
            Ok(Some(State::Cancelled))
        })
        .unwrap();
        queue(
            &root,
            QueueAction::Note {
                id: id.clone(),
                text: "@reviewer please look again".into(),
            },
        )
        .unwrap();

        let mut r = test_runner(&root, q.clone(), trusted);
        let mut out = Vec::new();
        // No model anywhere, so the woken task waits at the gate rather
        // than starting a run: what is under test is the wake, not a run.
        let (n, stop) =
            trellis_queue::work(&q, &mut r, trellis_queue::WorkOptions::default(), &mut out)
                .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!((n, stop), (0, trellis_queue::Stop::Waiting(1)), "{out}");
        assert!(out.contains("-> todo for `reviewer`"), "{out}");
        let (s, t) = q.find(&id).unwrap();
        assert_eq!(s, State::Pending);
        assert_eq!(t.assignee.as_deref(), Some("reviewer"));
        assert_eq!(t.notes[0].id.as_deref(), Some("n1"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
