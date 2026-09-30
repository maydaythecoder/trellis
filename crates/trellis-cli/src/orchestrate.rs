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
    // Only read when a profile names tools or approvals, so a broken
    // config.toml is reported by the command that needs it rather than by
    // `agents list`. An approve list is checked against the repository's
    // own tools too, as `[control] approve` in config.toml is.
    let cfg = if set.values().any(|p| {
        p.file.tools.is_some() || p.file.control.as_ref().is_some_and(|c| c.approve.is_some())
    }) {
        Some(crate::config::RepoConfig::load(root)?)
    } else {
        None
    };
    for (n, p) in &set {
        if let (Some(list), Some(cfg)) = (&p.file.tools, &cfg) {
            crate::tooling::check_profile_tools(n, list, cfg)?;
        }
        if let Some(c) = &p.file.control
            && let Some(list) = &c.approve
            && let Some(cfg) = &cfg
        {
            for t in list {
                cfg.check_approvable(t)
                    .map_err(|e| anyhow::anyhow!("agent `{n}`: [control] {e}"))?;
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
                prompt_tokens: e.trail.factual.prompt_tokens,
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

/// The prompt-token clause printed beside the completion figure (ADR 0026
/// §5). A clause of its own and never added to the completion figure: a
/// prompt is re-sent every turn, budgets limit completion tokens only, and
/// one total would hide which of the two moved.
///
/// An unknown figure prints as `unknown`, never as 0: a run whose adapter
/// reported no prompt figure did not cost nothing. The unknown count is
/// lifetime-wide (the ledger keeps no per-month one), so it follows the
/// clause once rather than qualifying either number alone.
fn prompt_clause(s: &ledger::Spend, month: &str) -> String {
    if s.runs > 0 && s.runs_without_prompt_count == s.runs {
        return format!("prompt tokens {month}: unknown, lifetime unknown");
    }
    let known = format!(
        "prompt tokens {month}: {}, lifetime {}",
        s.prompt_month, s.prompt_lifetime
    );
    if s.runs_without_prompt_count > 0 {
        format!("{known} ({} runs unknown)", s.runs_without_prompt_count)
    } else {
        known
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

/// The approve list this profile's runs use, and where it comes from.
///
/// Since review round 10 a profile's `[control] approve` adds to the
/// layered list (the repository's `config.toml` and the user's own file)
/// instead of replacing it, so printing the profile's list alone showed
/// less than a run asks about (review round 11). This is the list
/// `run_body` builds: `RepoConfig::control`, then
/// `add_profile_approvals`. The mode does not change the list.
fn approve_text(cfg: &crate::config::RepoConfig, p: &Profile) -> Result<String> {
    let mut dial = cfg.control(None)?;
    let own = p.file.control.as_ref().and_then(|c| c.approve.as_ref());
    if let Some(list) = own {
        dial.add_profile_approvals(list);
    }
    Ok(format!(
        "approve {} ({})",
        dial.approve.join(", "),
        if own.is_some() {
            "the repository's and your user file's list, plus this profile's"
        } else {
            "the repository's and your user file's list; this profile adds none"
        }
    ))
}

fn runs_with(p: &Profile) -> String {
    match (&p.file.command, &p.file.model) {
        // Names, never values: which keys the process is given is part of
        // what it runs with, and says why it could print one.
        (Some(c), _) if !p.file.pass_env.is_empty() => format!(
            "process `{}`, passed {}",
            plain(&c.join(" ")),
            plain(&p.file.pass_env.join(", "))
        ),
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
                    detail.push(prompt_clause(&s, &month));
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
                "control:      mode {}; {}",
                control
                    .mode
                    .as_deref()
                    .unwrap_or("(the work default: auto)"),
                approve_text(&crate::config::RepoConfig::load(root)?, p)?
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
                    "              filters the skills listed to the model, and `read_skill` \
                     refuses the rest by name"
                );
            }
            match &f.tools {
                None => println!("tools:        (the repository's set)"),
                Some(t) => println!(
                    "tools:        {} — narrows the repository's set for these runs; a tool \
                     the repository does not enable or trust is not added, and run_checks \
                     is always kept",
                    t.join(", ")
                ),
            }
            // Only when set, so a profile without it shows what it showed.
            if let Some(a) = &f.allow_write {
                println!(
                    "allow_write:  {} — the only paths these runs may write; a path the \
                     repository denies stays denied",
                    if a.is_empty() {
                        "(none: no write at all)".to_string()
                    } else {
                        a.join(", ")
                    }
                );
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
                println!(
                    "              {}; not limited by the budget",
                    prompt_clause(&s, &month)
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
                    .next_after(now, &r.tz)
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
    fn agents_show_prints_the_effective_approve_list_not_the_profile_s_alone() {
        // The risk (review round 11): since round 10 a profile's approve
        // list adds to the repository's and the user's, but `agents show`
        // printed the profile's list as if it were the whole of it, so a
        // person reading it did not see `edit` or their own entries.
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join(".trellis");
        std::fs::create_dir_all(t.join("agents")).unwrap();
        std::fs::write(t.join("config.toml"), "[control]\nmode = \"approve\"\n").unwrap();
        std::fs::write(
            t.join("agents/rev.toml"),
            "role = \"reviews\"\n[control]\napprove = [\"read_file\"]\n",
        )
        .unwrap();
        std::fs::write(t.join("agents/plain.toml"), "role = \"writes\"\n").unwrap();
        let user = d.path().join("user.toml");
        std::fs::write(&user, "[control]\napprove = [\"run_checks\"]\n").unwrap();
        let cfg = crate::config::RepoConfig::load_with_user(d.path(), Some(&user)).unwrap();
        let profiles = super::load_profiles(d.path()).unwrap();

        let shown = super::approve_text(&cfg, &profiles["rev"]).unwrap();
        assert!(
            shown.starts_with("approve edit, run_checks, read_file ("),
            "{shown}"
        );
        assert!(shown.contains("plus this profile's"), "{shown}");
        let shown = super::approve_text(&cfg, &profiles["plain"]).unwrap();
        assert!(shown.starts_with("approve edit, run_checks ("), "{shown}");
        assert!(shown.contains("adds none"), "{shown}");
    }

    #[test]
    fn a_profile_may_list_a_repository_tool_for_approval_by_its_name() {
        // The risk: `[control] approve` in the repository config accepted
        // a `[[tools.custom]]` name and the same list in a profile was
        // refused, so a profile could not ask for approval of the tool the
        // repository declares (review round 8, finding 7). A typo is still
        // refused.
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join(".trellis");
        std::fs::create_dir_all(t.join("agents")).unwrap();
        std::fs::write(
            t.join("config.toml"),
            concat!(
                "[[tools.custom]]\nname = \"lint\"\ndescription = \"Lint.\"\n",
                "command = [\"python\", \"lint.py\"]\n",
                "schema = { type = \"object\", properties = {} }\n",
            ),
        )
        .unwrap();
        let profile = |approve: &str| {
            std::fs::write(
                t.join("agents/rev.toml"),
                format!("role = \"reviews\"\n[control]\napprove = [{approve}]\n"),
            )
            .unwrap();
        };
        profile("\"lint\", \"edit\"");
        super::load_profiles(d.path()).expect("a declared tool is approvable");
        profile("\"lnit\"");
        let e = super::load_profiles(d.path()).unwrap_err();
        assert!(
            format!("{e:#}").contains("`lnit`, which is not a tool"),
            "{e:#}"
        );
        assert!(
            format!("{e:#}").contains("lint"),
            "the declared tools are named: {e:#}"
        );
    }

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

    /// Every file under `examples/agents/`, relative to it, with `/`.
    fn files_under(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                files_under(&e.path(), &r, out);
            } else {
                out.push(r);
            }
        }
    }

    #[test]
    fn starter_profiles_load() {
        // The risk: the starter profiles are copied in by hand, so nothing
        // else loads them, and a field the loader stops accepting (or an
        // `instructions` path that no longer matches the file) would ship
        // a starter that fails `agents list` on first use. They are copied
        // into `.trellis/agents/` as the header of each file says, then
        // loaded through the same validation every command uses.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/agents");
        let mut files = Vec::new();
        files_under(&src, "", &mut files);
        files.sort();
        assert!(!files.is_empty(), "no files under {}", src.display());

        let d = tempfile::tempdir().unwrap();
        let dst = d.path().join(".trellis").join(trellis_queue::profile::DIR);
        for f in &files {
            let to = dst.join(f);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(src.join(f), &to).unwrap();
        }
        let profiles = super::load_profiles(d.path()).unwrap();

        // `load_dir` skips what is not `*.toml` without a word, so a file
        // is accounted for here: a profile that loaded, or the instructions
        // file a loaded profile names. Anything else is a starter the
        // loader never saw.
        let prefix = format!("{}/", trellis_queue::profile::DIR);
        for f in &files {
            let loaded = f
                .strip_suffix(".toml")
                .is_some_and(|n| profiles.contains_key(n));
            let named = profiles.values().any(|p| {
                p.file
                    .instructions
                    .as_deref()
                    .and_then(|i| i.strip_prefix(&prefix))
                    == Some(f.as_str())
            });
            assert!(
                loaded || named,
                "examples/agents/{f} is neither a profile nor a profile's instructions"
            );
        }
        for (n, p) in &profiles {
            // Rule 5 (ADR 0005): no starter hands its work to another agent
            // for judgment, and each one has instructions saying who decides.
            assert_eq!(p.file.reports_to, None, "agent `{n}` reports to another");
            assert!(
                p.file.instructions.is_some(),
                "agent `{n}` has no instructions"
            );
        }
        let implementer = &profiles["implementer"];
        assert_eq!(implementer.file.tools, None, "default tools means no list");
        assert_eq!(
            implementer
                .file
                .control
                .as_ref()
                .and_then(|c| c.mode.as_deref()),
            Some("approve")
        );
        assert_eq!(
            implementer.file.allow_write, None,
            "implementer is unscoped"
        );

        // The risk (#183): a scoped starter whose scope does not hold, so
        // "tests only" or "docs only" is enforced by nothing but its prompt.
        // Each scope is run through the same `Governance` the gate and a
        // process agent's audit consult, against paths it must admit and
        // paths it must refuse.
        let scoped: [(&str, &[&str], &[&str]); 2] = [
            (
                "test-writer",
                &[
                    "tests/test_parse.py",
                    "tests/unit/conftest.py",
                    "pkg/parse_test.py",
                    "conftest.py",
                ],
                &["pkg/parse.py", "setup.py", "docs/testing.md", "README.md"],
            ),
            (
                "docs-writer",
                &[
                    "docs/guide.md",
                    "docs/api/index.rst",
                    "README.md",
                    "pkg/NOTES.md",
                ],
                &["pkg/parse.py", "tests/test_parse.py", "pyproject.toml"],
            ),
        ];
        for (n, inside, outside) in scoped {
            let p = &profiles[n];
            let allow = p
                .file
                .allow_write
                .as_deref()
                .unwrap_or_else(|| panic!("agent `{n}` has no allow_write"));
            assert!(!allow.is_empty(), "agent `{n}` may write nothing");
            assert_eq!(
                p.file.control.as_ref().and_then(|c| c.mode.as_deref()),
                Some("approve"),
                "agent `{n}`"
            );
            let g = trellis_loop::Governance::new(&[]).with_allow_write(n, allow);
            for path in inside {
                assert_eq!(
                    g.outside_allow_write_at(d.path(), path),
                    None,
                    "agent `{n}` must be able to write {path}"
                );
            }
            for path in outside {
                assert!(
                    g.outside_allow_write_at(d.path(), path).is_some(),
                    "agent `{n}` must not be able to write {path}"
                );
            }
        }
        // The rule syntax has no infix wildcard: `test_*.py` parses as the
        // exact path "test_*.py", admits nothing, and reads as a scope that
        // is on. No starter may ship an entry like that.
        for (n, p) in &profiles {
            for entry in p.file.allow_write.iter().flatten() {
                assert!(
                    !matches!(
                        trellis_loop::Rule::parse(entry),
                        trellis_loop::Rule::Exact(e) if e.contains('*')
                    ),
                    "agent `{n}`: allow_write entry {entry:?} matches nothing"
                );
            }
        }
    }

    #[test]
    fn spend_records_read_prompt_tokens_from_the_trail_and_keep_absent_unknown() {
        // The risk: the reader drops the trail's prompt figure, or turns a
        // trail that has none (every trail before #105) into a known 0.
        // Hand-written trails: read back as untrusted data (ADR 0007), so a
        // digest that does not match is counted and flagged, not refused.
        let d = tempfile::tempdir().unwrap();
        let runs = d.path().join(".trellis").join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        let trail = |id: &str, at: &str, factual_extra: &str| {
            format!(
                r#"{{"schema":1,"run_id":"{id}","task":"t","base_commit":"abc1234","started_at":"{at}","provenance":{{"trellis_version":"0.0.1","model":null,"tier":null}},"factual":{{"proposed":[],"radius":{{"reached_resolved":0,"reached_ambiguous":0,"widest_unbound_in_touched_files":0,"radius_queries":0}},"checks":[],"check_outcomes":[],"approved_by":"Human","mode_counts":{{"verified":0,"reviewed":0}},"context":null,"actions":[],"agent":"rev"{factual_extra}}},"factual_sha256":"0000","judgment":{{"tech_debt_projection":null,"rationale":null}}}}"#
            )
        };
        std::fs::write(
            runs.join("20260930-235959-a.json"),
            trail(
                "20260930-235959-a",
                "2026-09-30T23:59:59Z",
                r#","tokens":10,"prompt_tokens":1000"#,
            ),
        )
        .unwrap();
        std::fs::write(
            runs.join("20261001-000000-b.json"),
            trail(
                "20261001-000000-b",
                "2026-10-01T00:00:00Z",
                r#","tokens":5,"prompt_tokens":200"#,
            ),
        )
        .unwrap();
        std::fs::write(
            runs.join("20261002-000000-c.json"),
            trail(
                "20261002-000000-c",
                "2026-10-02T00:00:00Z",
                r#","tokens":7"#,
            ),
        )
        .unwrap();

        let records = super::spend_records(d.path());
        let mut prompts: Vec<_> = records.iter().map(|r| r.prompt_tokens).collect();
        prompts.sort();
        assert_eq!(prompts, [None, Some(200), Some(1000)]);
        assert!(records.iter().all(|r| r.altered), "digest does not match");

        let s = trellis_queue::ledger::spend(&records, "rev", "2026-10");
        assert_eq!((s.prompt_month, s.prompt_lifetime), (200, 1200));
        assert_eq!(s.runs_without_prompt_count, 1);
        assert_eq!((s.month, s.lifetime), (12, 22));
    }

    fn spend(prompt: (u64, u64), runs: u32, unknown: u32) -> trellis_queue::ledger::Spend {
        trellis_queue::ledger::Spend {
            month: 7,
            lifetime: 30,
            runs,
            prompt_month: prompt.0,
            prompt_lifetime: prompt.1,
            runs_without_prompt_count: unknown,
            ..Default::default()
        }
    }

    #[test]
    fn prompt_tokens_known_print_as_their_own_clause_and_never_as_a_sum() {
        // The risk: the prompt figure is folded into the completion figure
        // (7 + 400) or printed as one total, which hides which one moved.
        let c = super::prompt_clause(&spend((400, 1200), 3, 0), "2026-09");
        assert_eq!(c, "prompt tokens 2026-09: 400, lifetime 1200");
        for sum in ["407", "1230"] {
            assert!(!c.contains(sum), "{c}");
        }
    }

    #[test]
    fn prompt_tokens_all_unknown_print_unknown_never_zero() {
        // The risk: runs whose adapter reported no prompt figure read as a
        // prompt of 0 tokens, i.e. as free.
        let c = super::prompt_clause(&spend((0, 0), 2, 2), "2026-09");
        assert_eq!(c, "prompt tokens 2026-09: unknown, lifetime unknown");
        assert!(!c.replace("2026-09", "").contains('0'), "{c}");
    }

    #[test]
    fn prompt_tokens_partly_unknown_print_the_known_sum_and_how_many_runs_are_missing() {
        // The risk: a lower bound shown as if it were the complete figure.
        let c = super::prompt_clause(&spend((400, 1200), 5, 2), "2026-09");
        assert_eq!(
            c,
            "prompt tokens 2026-09: 400, lifetime 1200 (2 runs unknown)"
        );
    }

    #[test]
    fn prompt_tokens_of_a_profile_that_never_ran_are_a_known_zero() {
        // No run is not an unknown run: the completion clause prints 0 for
        // the same profile, and "unknown" there would claim a run happened.
        let c = super::prompt_clause(&spend((0, 0), 0, 0), "2026-09");
        assert_eq!(c, "prompt tokens 2026-09: 0, lifetime 0");
    }
}
