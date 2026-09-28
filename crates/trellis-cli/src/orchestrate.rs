//! `trellis agents` and `trellis routines` (ADR 0026 §4, §7), and what the
//! queue and the heartbeat share with them: loading profiles with the
//! checks only the binary can make, and turning trails into spend records.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::collections::BTreeMap;
use std::path::Path;
use trellis_queue::ledger::{self, SpendRecord};
use trellis_queue::profile::{self, PausedBy, Profile, Window};
use trellis_queue::routine;
use trellis_queue::{Actor, Event, Queue, plain};

#[derive(Subcommand)]
pub(crate) enum AgentsAction {
    /// Every profile in .trellis/agents/, with its state and spend.
    List,
    /// One profile in full, with what each setting does and does not do.
    Show { name: String },
    /// Stop `work` from claiming this profile's tasks.
    ///
    /// Recorded in the git directory, not the profile file: editing the
    /// tracked file would dirty the worktree every run needs clean.
    Pause {
        name: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Lift a pause made with `agents pause`.
    Resume { name: String },
}

#[derive(Subcommand)]
pub(crate) enum RoutinesAction {
    /// Every routine in .trellis/routines/: schedule, policies, next and
    /// last fire.
    List,
    /// Fire a routine now. Its concurrency policy still applies.
    Run { name: String },
}

/// Profiles, validated as a set by `trellis-queue` and then against what
/// only the binary knows: the approvable tools and the model spec grammar.
/// Every command that reads profiles goes through here, so a typo is found
/// by `agents list` rather than by an unattended `work`.
pub(crate) fn load_profiles(root: &Path) -> Result<BTreeMap<String, Profile>> {
    let set = profile::load_all(root)?;
    for (n, p) in &set {
        if let Some(c) = &p.file.control
            && let Some(list) = &c.approve
        {
            for t in list {
                if !trellis_loop::APPROVABLE_TOOLS.contains(&t.as_str()) {
                    anyhow::bail!(
                        "agent `{n}`: [control] approve lists `{t}`, which is not a tool; \
                         the tools are {}",
                        trellis_loop::APPROVABLE_TOOLS.join(", ")
                    );
                }
            }
        }
        if let Some(m) = &p.file.model {
            let _: trellis_model::ModelSpec = m
                .parse()
                .with_context(|| format!("agent `{n}`: model `{m}`"))?;
        }
        if let Some(path) = p.instructions_path(root)
            && !path.is_file()
        {
            anyhow::bail!(
                "agent `{n}`: instructions file {} does not exist",
                path.display()
            );
        }
    }
    Ok(set)
}

/// Routines, with every template's assignee checked against the profiles.
pub(crate) fn load_routines(
    root: &Path,
    profiles: &BTreeMap<String, Profile>,
) -> Result<BTreeMap<String, routine::Routine>> {
    let set = routine::load_all(root)?;
    for r in set.values() {
        if let Some(a) = &r.file.task.assignee
            && !profiles.contains_key(a)
        {
            anyhow::bail!(
                "routine `{}`: [task] assignee `{a}` is not a profile in .trellis/agents/",
                r.name
            );
        }
    }
    Ok(set)
}

/// Every trail that names an agent, as spend. Read as untrusted data
/// (ADR 0007): only the numbers are taken, and a trail whose digest does
/// not match is counted and flagged.
pub(crate) fn spend_records(root: &Path) -> Vec<SpendRecord> {
    trellis_trail::query::all(root)
        .into_iter()
        .filter_map(|e| {
            Some(SpendRecord {
                agent: e.trail.factual.agent.clone()?,
                tokens: e.trail.factual.tokens,
                started_at: e.trail.started_at.clone().unwrap_or_default(),
                altered: e.integrity == trellis_trail::Integrity::Altered,
            })
        })
        .collect()
}

pub(crate) fn this_month() -> String {
    trellis_queue::month_of(&trellis_queue::iso_now()).unwrap_or_default()
}

fn limit_text(o: &profile::Overlay, base: Option<u64>, w: Window, month: &str) -> String {
    match o.limit(base, w, month) {
        Some(l) => format!("of {l}"),
        None => "(no limit)".into(),
    }
}

fn state_text(p: &Profile, o: &profile::Overlay) -> String {
    match profile::paused_reason(p, o) {
        None => "active".into(),
        Some(_) if p.file.paused => "paused (in its file)".into(),
        Some(_) => match o.paused.as_ref().map(|x| x.by) {
            Some(PausedBy::Budget) => "paused (budget)".into(),
            _ => "paused".into(),
        },
    }
}

fn runs_with(p: &Profile) -> String {
    match (&p.file.command, &p.file.model) {
        (Some(c), _) => format!("process `{}`", plain(&c.join(" "))),
        (None, Some(m)) => format!("model {}", plain(m)),
        (None, None) => "the `work` model".into(),
    }
}

pub(crate) fn agents(root: &Path, action: AgentsAction) -> Result<()> {
    let q = Queue::for_repo(root)?;
    let profiles = load_profiles(root)?;
    let month = this_month();
    let get = |name: &str| -> Result<&Profile> {
        profiles.get(name).with_context(|| {
            format!(
                "no agent `{name}` in .trellis/agents/{}",
                if profiles.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; the profiles are {}",
                        profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                }
            )
        })
    };
    match action {
        AgentsAction::List => {
            if profiles.is_empty() {
                println!("no agent profiles (.trellis/agents/<name>.toml)");
                return Ok(());
            }
            let records = spend_records(root);
            for (n, p) in &profiles {
                let o = profile::overlay(&q, n)?;
                let s = ledger::spend(&records, n, &month);
                let b = p.file.budget.clone().unwrap_or_default();
                println!("{n:<14} {:<22} {}", state_text(p, &o), runs_with(p));
                let mut detail = Vec::new();
                if let Some(r) = &p.file.role {
                    detail.push(format!("role {}", plain(r)));
                }
                if let Some(r) = &p.file.reports_to {
                    detail.push(format!("reports to {r}"));
                }
                if p.is_process() {
                    detail.push("tokens not visible to trellis".into());
                } else {
                    detail.push(format!(
                        "{month}: {} {}; lifetime {} {} completion tokens",
                        s.month,
                        limit_text(&o, b.monthly_tokens, Window::Monthly, &month),
                        s.lifetime,
                        limit_text(&o, b.lifetime_tokens, Window::Lifetime, &month),
                    ));
                }
                println!("               {}", detail.join("; "));
            }
            Ok(())
        }
        AgentsAction::Show { name } => {
            let p = get(&name)?;
            let o = profile::overlay(&q, &name)?;
            let records = spend_records(root);
            let s = ledger::spend(&records, &name, &month);
            let f = &p.file;
            println!("agent:        {name}  (.trellis/agents/{name}.toml)");
            println!("state:        {}", state_text(p, &o));
            if let Some(why) = profile::paused_reason(p, &o) {
                println!("              {why}");
            }
            println!(
                "role:         {}",
                f.role.as_deref().map(plain).unwrap_or("-".into())
            );
            println!("reports to:   {}", f.reports_to.as_deref().unwrap_or("-"));
            println!("runs with:    {}", runs_with(p));
            if p.is_process() {
                println!(
                    "timeout:      {}s",
                    f.timeout_s.unwrap_or(profile::DEFAULT_TIMEOUT_S)
                );
                println!(
                    "              converges only if it changes files and `trellis check`'s \
                     two checks pass; its exit code is recorded, never trusted"
                );
            }
            println!(
                "instructions: {}",
                f.instructions
                    .as_deref()
                    .map(|i| format!(".trellis/{i}, prepended to each task"))
                    .unwrap_or("-".into())
            );
            let control = f.control.clone().unwrap_or_default();
            println!(
                "control:      mode {}; approve {}",
                control
                    .mode
                    .as_deref()
                    .unwrap_or("(the work default: auto)"),
                control
                    .approve
                    .map(|a| a.join(", "))
                    .unwrap_or("(the repository's list)".into())
            );
            if control.mode.as_deref() == Some("approve") {
                println!(
                    "              approve mode needs a person: `work` claims these tasks \
                     only when attached to a terminal"
                );
            }
            match &f.skills {
                None => println!("skills:       every skill"),
                Some(sk) => println!(
                    "skills:       allow {}; deny {}",
                    sk.allow
                        .as_ref()
                        .map(|a| a.join(", "))
                        .unwrap_or("(all)".into()),
                    if sk.deny.is_empty() {
                        "-".into()
                    } else {
                        sk.deny.join(", ")
                    }
                ),
            }
            if f.skills.is_some() {
                println!(
                    "              filters the skills listed to the model; `read_skill` by \
                     exact name is not filtered"
                );
            }
            match &f.tools {
                None => println!("tools:        -"),
                Some(t) => println!(
                    "tools:        {} — stored only; no effect until the tool registry \
                     (ADR 0023) reads it",
                    t.join(", ")
                ),
            }
            let b = f.budget.clone().unwrap_or_default();
            if p.is_process() {
                println!("budget:       none possible; trellis does not see a process's tokens");
            } else {
                println!(
                    "budget:       {month}: {} {}; lifetime {} {}; warn at {:.0}%",
                    s.month,
                    limit_text(&o, b.monthly_tokens, Window::Monthly, &month),
                    s.lifetime,
                    limit_text(&o, b.lifetime_tokens, Window::Lifetime, &month),
                    p.warn_at() * 100.0
                );
                println!(
                    "              completion tokens as the runtime adapter reported them, \
                     from {} trail(s) in this checkout",
                    s.runs
                );
                if s.runs_without_count > 0 {
                    println!(
                        "              {} run(s) recorded no count (scripted, or written \
                         before counts were recorded)",
                        s.runs_without_count
                    );
                }
                if s.altered > 0 {
                    println!(
                        "              {} trail(s) were edited after they were written; \
                         their counts are included as found",
                        s.altered
                    );
                }
                for r in &o.raises {
                    println!(
                        "              raised {} by {} at {}",
                        r.month.clone().unwrap_or_else(|| r.window.as_str().into()),
                        r.tokens,
                        r.at
                    );
                }
            }
            Ok(())
        }
        AgentsAction::Pause { name, reason } => {
            get(&name)?;
            let already = profile::update_overlay(&q, &name, |o| {
                if o.paused.is_some() {
                    return true;
                }
                o.paused = Some(profile::Pause {
                    by: PausedBy::Human,
                    at: trellis_queue::iso_now(),
                    reason: reason.clone(),
                    window: None,
                    month: None,
                });
                false
            })?;
            if already {
                println!("agent `{name}` is already paused");
                return Ok(());
            }
            let mut e = Event::new(Actor::Human, "agent.pause", format!("agent:{name}"));
            if let Some(r) = &reason {
                e = e.detail(r.clone());
            }
            q.log(e)?;
            println!(
                "paused:   {name}; `work` skips its tasks until `trellis agents resume {name}`"
            );
            Ok(())
        }
        AgentsAction::Resume { name } => {
            let p = get(&name)?;
            let o = profile::overlay(&q, &name)?;
            match o.paused.as_ref().map(|x| x.by) {
                Some(PausedBy::Budget) => anyhow::bail!(
                    "agent `{name}` is paused by its token budget. Approve its override \
                     request with a raise (`trellis queue list` shows it), or wait for the \
                     monthly window to roll over"
                ),
                Some(PausedBy::Human) => {
                    profile::update_overlay(&q, &name, |o| o.paused = None)?;
                    q.log(Event::new(
                        Actor::Human,
                        "agent.resume",
                        format!("agent:{name}"),
                    ))?;
                    println!("resumed:  {name}");
                }
                None if !p.file.paused => println!("agent `{name}` is not paused"),
                None => {}
            }
            if p.file.paused {
                println!(
                    "agent `{name}` is also paused in .trellis/agents/{name}.toml \
                     (`paused = true`); edit the file to resume it"
                );
            }
            Ok(())
        }
    }
}

pub(crate) fn routines(root: &Path, action: RoutinesAction) -> Result<()> {
    let q = Queue::for_repo(root)?;
    let profiles = load_profiles(root)?;
    let set = load_routines(root, &profiles)?;
    let now = trellis_queue::now_secs();
    match action {
        RoutinesAction::List => {
            if set.is_empty() {
                println!("no routines (.trellis/routines/<name>.toml)");
                return Ok(());
            }
            for r in set.values() {
                let st = routine::state(&q, &r.name)?;
                let next = r
                    .cron
                    .next_after(now, r.tz)
                    .map(|t| {
                        let (d, h) = r.tz.date_time(t);
                        format!("{d} {h} {}", r.tz)
                    })
                    .unwrap_or_else(|| "never".into());
                println!(
                    "{:<14} {:<16} {}{}",
                    r.name,
                    r.cron.as_str(),
                    r.tz,
                    if r.file.paused { "  PAUSED" } else { "" }
                );
                println!(
                    "               next {next}; concurrency {}; catch-up {}",
                    match r.concurrency {
                        routine::Concurrency::Coalesce => "coalesce",
                        routine::Concurrency::Skip => "skip",
                        routine::Concurrency::Always => "always",
                    },
                    match r.catch_up {
                        routine::CatchUp::Skip => "skip".to_string(),
                        routine::CatchUp::Enqueue(n) => format!("enqueue up to {n}"),
                    }
                );
                println!(
                    "               {}",
                    match &st {
                        None => "not armed yet: `work` arms it on its next tick".to_string(),
                        Some(s) => format!(
                            "armed {}; last fired {}{}",
                            trellis_queue::secs_to_iso(s.armed_at),
                            s.last_fired
                                .map(trellis_queue::secs_to_iso)
                                .unwrap_or_else(|| "never".into()),
                            s.last_task
                                .as_deref()
                                .map(|t| format!(" -> {t}"))
                                .unwrap_or_default()
                        ),
                    }
                );
            }
            Ok(())
        }
        RoutinesAction::Run { name } => {
            let r = set
                .get(&name)
                .with_context(|| format!("no routine `{name}` in .trellis/routines/"))?;
            match routine::run_now(&q, r, now)? {
                routine::Fired::Enqueued(id) => println!("fired:    {name} -> queued {id}"),
                routine::Fired::Coalesced(id) => println!(
                    "fired:    {name}, coalesced into the open task {id} (concurrency = coalesce)"
                ),
                routine::Fired::Skipped(id) => println!(
                    "fired:    {name}, skipped: task {id} is still open (concurrency = skip)"
                ),
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_trail_started_at_is_a_form_the_ledger_can_bucket_by_month() {
        // The ledger buckets by `month_of(started_at)`. A trail writer that
        // added fractional seconds would make monthly spend silently zero
        // while lifetime spend still counted.
        let iso = trellis_trail::iso_now();
        assert!(trellis_queue::month_of(&iso).is_some(), "{iso}");
        assert_eq!(
            trellis_queue::month_of(&iso),
            trellis_queue::month_of(&trellis_queue::iso_now())
        );
    }
}
