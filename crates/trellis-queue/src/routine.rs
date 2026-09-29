//! Routines: `.trellis/routines/<name>.toml` (ADR 0026 §7).
//!
//! A routine is a schedule and a task template. `work` evaluates every
//! routine on each tick; a due fire goes through the routine's concurrency
//! policy and, if it survives, becomes a task in `pending/` carrying the
//! routine's name. What each routine last fired is state in the git
//! directory, written under a lock like every other rewritten file.

use crate::activity::{Actor, Event};
use crate::cron::{Cron, Tz};
use crate::files::{Lock, read_json_opt, replace, settle};
use crate::{NewTask, Priority, Queue, QueueError, State};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

pub const DIR: &str = "routines";
/// The most fires `catch_up = "enqueue"` may be set to replay.
pub const MAX_CATCH_UP: u32 = 100;
const BUILTINS: [&str; 3] = ["date", "time", "routine"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateFile {
    pub text: String,
    pub focus: Option<String>,
    pub prompt: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    #[serde(default)]
    pub needs_approval: bool,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineFile {
    pub schedule: String,
    pub timezone: Option<String>,
    pub concurrency: Option<String>,
    pub catch_up: Option<String>,
    pub catch_up_max: Option<u32>,
    #[serde(default)]
    pub paused: bool,
    pub task: TemplateFile,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

/// What a fire does while a task this routine created is still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concurrency {
    /// No new task; a note on the open one says the routine fired again.
    Coalesce,
    /// No new task; the fire is logged as skipped.
    Skip,
    /// A new task every time.
    Always,
}

/// What happens to fires missed while nothing was evaluating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUp {
    /// Only the most recent due fire runs.
    Skip,
    /// The most recent `n` due fires run, oldest first.
    Enqueue(u32),
}

#[derive(Debug, Clone)]
pub struct Routine {
    pub name: String,
    pub file: RoutineFile,
    pub cron: Cron,
    pub tz: Tz,
    pub concurrency: Concurrency,
    pub catch_up: CatchUp,
    pub priority: Priority,
}

fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        out.push(after[..j].trim().to_string());
        rest = &after[j + 2..];
    }
    out
}

pub fn parse(name: &str, text: &str) -> Result<Routine, QueueError> {
    let bad = |why: String| QueueError::Invalid(format!("routine `{name}`: {why}"));
    if !crate::profile::valid_name(name) {
        return Err(bad(
            "a name is lowercase ASCII letters, digits, `-` and `_`".into(),
        ));
    }
    let file: RoutineFile = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
    let cron = Cron::parse(&file.schedule).map_err(|e| bad(e.to_string()))?;
    let tz =
        Tz::parse(file.timezone.as_deref().unwrap_or("UTC")).map_err(|e| bad(e.to_string()))?;
    let concurrency = match file.concurrency.as_deref().unwrap_or("coalesce") {
        "coalesce" => Concurrency::Coalesce,
        "skip" => Concurrency::Skip,
        "always" => Concurrency::Always,
        o => {
            return Err(bad(format!(
                "concurrency = \"{o}\"; use coalesce, skip or always"
            )));
        }
    };
    let catch_up = match (
        file.catch_up.as_deref().unwrap_or("skip"),
        file.catch_up_max,
    ) {
        ("skip", None) => CatchUp::Skip,
        ("skip", Some(_)) => {
            return Err(bad(
                "catch_up_max applies to catch_up = \"enqueue\"; with \"skip\" it would \
                 be ignored"
                    .into(),
            ));
        }
        ("enqueue", n) => {
            let n = n.unwrap_or(5);
            if n == 0 || n > MAX_CATCH_UP {
                return Err(bad(format!(
                    "catch_up_max = {n} is outside 1-{MAX_CATCH_UP}"
                )));
            }
            CatchUp::Enqueue(n)
        }
        (o, _) => return Err(bad(format!("catch_up = \"{o}\"; use skip or enqueue"))),
    };
    let priority = match &file.task.priority {
        None => Priority::Medium,
        Some(p) => Priority::parse(p)
            .ok_or_else(|| bad(format!("priority `{p}`; use critical, high, medium or low")))?,
    };
    if file.task.text.trim().is_empty() && file.task.prompt.is_none() {
        return Err(bad(
            "[task] needs text, or a prompt template, or both".into()
        ));
    }
    // Checked when the routine is loaded, not when it fires: a missing var
    // found at 03:00 by an unattended heartbeat is a task nobody wanted.
    for k in file.vars.keys() {
        if BUILTINS.contains(&k.as_str()) {
            return Err(bad(format!(
                "[vars] `{k}` would shadow the built-in {{{{{k}}}}}"
            )));
        }
    }
    for p in placeholders(&file.task.text) {
        if !BUILTINS.contains(&p.as_str()) && !file.vars.contains_key(&p) {
            return Err(bad(format!(
                "the template uses {{{{{p}}}}}, which is neither a built-in (date, time, \
                 routine) nor in [vars]"
            )));
        }
    }
    Ok(Routine {
        name: name.to_string(),
        file,
        cron,
        tz,
        concurrency,
        catch_up,
        priority,
    })
}

pub fn load_all(root: &Path) -> Result<BTreeMap<String, Routine>, QueueError> {
    load_dir(&root.join(".trellis").join(DIR))
}

pub fn load_dir(dir: &Path) -> Result<BTreeMap<String, Routine>, QueueError> {
    let mut set = BTreeMap::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(set),
        Err(e) => return Err(crate::files::io(dir)(e)),
    };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".toml"))
        else {
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(crate::files::io(&path))?;
        set.insert(name.to_string(), parse(name, &text)?);
    }
    Ok(set)
}

impl Routine {
    /// The template filled for a fire scheduled at `at`.
    pub fn render(&self, at: i64) -> String {
        let (date, time) = self.tz.date_time(at);
        let mut out = String::new();
        let mut rest = self.file.task.text.as_str();
        while let Some(i) = rest.find("{{") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            let Some(j) = after.find("}}") else {
                out.push_str(&rest[i..]);
                rest = "";
                break;
            };
            let key = after[..j].trim();
            let v = match key {
                "date" => date.clone(),
                "time" => time.clone(),
                "routine" => self.name.clone(),
                k => self.file.vars.get(k).cloned().unwrap_or_default(),
            };
            out.push_str(&v);
            rest = &after[j + 2..];
        }
        out.push_str(rest);
        out
    }

    fn new_task(&self, at: i64) -> NewTask {
        let t = &self.file.task;
        NewTask {
            task: self.render(at),
            focus: t.focus.clone(),
            prompt: t.prompt.clone(),
            // A routine's `[task]` names no template arguments (ADR 0026 §7).
            args: None,
            priority: self.priority,
            labels: t.labels.clone(),
            parent: t.parent.clone(),
            assignee: t.assignee.clone(),
            blockers: Vec::new(),
            needs_approval: t.needs_approval,
            backlog: false,
            routine: Some(self.name.clone()),
            routine_at: None,
        }
    }
}

/// What the git directory remembers about a routine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineState {
    /// When this checkout first saw the routine. Nothing before it fires.
    pub armed_at: i64,
    /// The scheduled instant most recently handled, fired or dropped.
    #[serde(default)]
    pub last_fired: Option<i64>,
    #[serde(default)]
    pub last_task: Option<String>,
}

fn state_path(q: &Queue, name: &str) -> PathBuf {
    q.trellis_dir().join(DIR).join(format!("{name}.json"))
}

pub fn state(q: &Queue, name: &str) -> Result<Option<RoutineState>, QueueError> {
    read_json_opt(&state_path(q, name))
}

/// How one fire went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fired {
    Enqueued(String),
    /// Folded into this open task, as a note.
    Coalesced(String),
    /// Dropped because this task was still open.
    Skipped(String),
}

/// What one evaluation of one routine did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tick {
    pub name: String,
    pub armed: bool,
    pub fired: Vec<(i64, Fired)>,
    /// Due fires dropped by the catch-up policy.
    pub missed: u64,
}

/// A task this routine created that is still pending or claimed.
fn active(q: &Queue, name: &str) -> Result<Option<String>, QueueError> {
    #[cfg(test)]
    SCANS.with(|c| c.set(c.get() + 1));
    for s in [State::Claimed, State::Pending] {
        for id in q.ids(s)? {
            if let Ok(t) = crate::files::read_json::<crate::Task>(&q.path(s, &id))
                && t.routine.as_deref() == Some(name)
            {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
thread_local! {
    /// Whole-queue scans made for `seen` and `active`, for the test that holds
    /// a tick to one.
    static SCANS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// What one read of the queue says about a routine, for a whole tick.
struct Seen {
    /// Every task it already created for a scheduled instant, in any
    /// state, by that instant.
    by_at: std::collections::BTreeMap<i64, String>,
    /// The task [`active`] would name: its first claimed one, else its
    /// first pending one. Kept current as the tick fires.
    open: Option<String>,
}

/// [`Seen`] for this tick. One read of the queue per tick, taken under the
/// routine's lock like the fires it guards: the whole queue was listed
/// once per due instant before round 8, and the open task (for `coalesce`
/// and `skip`) read once per due instant before round 9, so a long
/// catch-up read every task file once per instant. The instants this tick
/// fires are distinct, so a task it creates never needs to be found by a
/// later fire of the same tick; it does become the open task, which
/// `fire` records.
fn seen(q: &Queue, name: &str) -> Result<Seen, QueueError> {
    #[cfg(test)]
    SCANS.with(|c| c.set(c.get() + 1));
    let mut by_at = std::collections::BTreeMap::new();
    let (mut claimed, mut pending) = (None, None);
    for e in q.list()? {
        let Ok(t) = e.task else { continue };
        if t.routine.as_deref() != Some(name) {
            continue;
        }
        match e.state {
            State::Claimed if claimed.is_none() => claimed = Some(e.id.clone()),
            State::Pending if pending.is_none() => pending = Some(e.id.clone()),
            _ => {}
        }
        if let Some(at) = t.routine_at {
            by_at.entry(at).or_insert(e.id);
        }
    }
    Ok(Seen {
        by_at,
        open: claimed.or(pending),
    })
}

/// `scheduled`: `at` is an instant of the schedule, recorded on the task
/// and fired at most once, and `Seen` is this tick's read of the queue. A
/// manual `run_now` is not one, reads the open task itself, and two of
/// them in one second are two tasks.
fn fire(
    q: &Queue,
    r: &Routine,
    at: i64,
    actor: Actor,
    mut scheduled: Option<&mut Seen>,
) -> Result<Fired, QueueError> {
    let entity = format!("routine:{}", r.name);
    let when = crate::secs_to_iso(at);
    // Before the concurrency policy: a task for this very instant is this
    // fire, already done by a tick that failed before it could record it.
    // Under `coalesce` it would otherwise be noted as a second fire.
    if let Some(id) = scheduled.as_ref().and_then(|s| s.by_at.get(&at)) {
        return Ok(Fired::Enqueued(id.clone()));
    }
    let open = match (&scheduled, r.concurrency) {
        (_, Concurrency::Always) => None,
        (Some(s), _) => s.open.clone(),
        (None, _) => active(q, &r.name)?,
    };
    if let Some(open) = open {
        if r.concurrency == Concurrency::Coalesce {
            // The open task may be claimed right now; a note it cannot
            // take is still a coalesced fire, recorded in the log.
            let _ = q.edit(&open, |t, _| {
                t.note(
                    actor,
                    format!("routine `{}` fired again for {when}; coalesced", r.name),
                );
                Ok(None)
            });
            q.log(
                Event::new(actor, "routine.coalesce", entity)
                    .detail(format!("{when} into task:{open}")),
            )?;
            return Ok(Fired::Coalesced(open));
        }
        q.log(
            Event::new(actor, "routine.skip", entity)
                .detail(format!("{when}; task:{open} still open")),
        )?;
        return Ok(Fired::Skipped(open));
    }
    let mut new = r.new_task(at);
    new.routine_at = scheduled.is_some().then_some(at);
    let id = q.add_task(new, actor)?;
    // A pending task of this routine now: the open one for the next fire,
    // unless a claimed one already was (`active` prefers it).
    if let Some(s) = scheduled.as_mut()
        && s.open.is_none()
    {
        s.open = Some(id.clone());
    }
    q.log(Event::new(actor, "routine.fire", entity).detail(format!("{when} -> task:{id}")))?;
    Ok(Fired::Enqueued(id))
}

/// Evaluates one routine at `now` (ADR 0026 §7): arms it if new, fires what
/// is due under its catch-up and concurrency policies, and records the
/// latest instant handled.
pub fn evaluate(q: &Queue, r: &Routine, now: i64) -> Result<Tick, QueueError> {
    let mut tick = Tick {
        name: r.name.clone(),
        ..Tick::default()
    };
    if r.file.paused {
        return Ok(tick);
    }
    let path = state_path(q, &r.name);
    // The state is saved after each fire rather than once at the end: a
    // fire that fails partway through a tick leaves the earlier ones
    // recorded, and the `routine_at` read in `seen` covers a failure
    // between a fire's task and its save. Saving only at the end
    // re-enqueued every earlier fire on every later tick.
    let _lock = Lock::take(&q.locks(), &format!("routine-{}", r.name))?;
    settle(&path)?;
    let Some(mut st) = read_json_opt::<RoutineState>(&path)? else {
        tick.armed = true;
        let st = RoutineState {
            armed_at: now,
            last_fired: None,
            last_task: None,
        };
        replace(&path, &st)?;
        return Ok(tick);
    };
    // A year of every-minute fires is 525,600 steps; clamp the look-back
    // so a routine left for a decade cannot stall a tick.
    let base = st.last_fired.unwrap_or(st.armed_at).max(now - 400 * 86_400);
    let keep = match r.catch_up {
        CatchUp::Skip => 1,
        CatchUp::Enqueue(n) => n as usize,
    };
    let mut due: VecDeque<i64> = VecDeque::new();
    let mut total = 0u64;
    let mut t = base;
    while let Some(next) = r.cron.next_after(t, r.tz) {
        if next > now {
            break;
        }
        total += 1;
        due.push_back(next);
        if due.len() > keep {
            due.pop_front();
        }
        t = next;
    }
    if total == 0 {
        return Ok(tick);
    }
    tick.missed = total - due.len() as u64;
    if tick.missed > 0 {
        q.log(
            Event::new(
                Actor::Routine,
                "routine.missed",
                format!("routine:{}", r.name),
            )
            .detail(format!(
                "{} due fire(s) dropped by catch_up = {}",
                tick.missed,
                match r.catch_up {
                    CatchUp::Skip => "skip".to_string(),
                    CatchUp::Enqueue(n) => format!("enqueue (max {n})"),
                }
            )),
        )?;
    }
    let mut already = seen(q, &r.name)?;
    for at in due {
        let f = fire(q, r, at, Actor::Routine, Some(&mut already))?;
        if let Fired::Enqueued(id) = &f {
            st.last_task = Some(id.clone());
        }
        st.last_fired = Some(at);
        replace(&path, &st)?;
        tick.fired.push((at, f));
    }
    Ok(tick)
}

/// Fires a routine now, by hand. Concurrency still applies, as Paperclip's
/// manual run does; catch-up does not, and the schedule's state is left
/// alone so the next scheduled fire is unaffected.
pub fn run_now(q: &Queue, r: &Routine, now: i64) -> Result<Fired, QueueError> {
    fire(q, r, now, Actor::Human, None)
}

/// The next scheduled instant after `now` across every unpaused routine.
pub fn next_due(routines: &BTreeMap<String, Routine>, now: i64) -> Option<i64> {
    routines
        .values()
        .filter(|r| !r.file.paused)
        .filter_map(|r| r.cron.next_after(now, r.tz))
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_iso;

    fn queue() -> (tempfile::TempDir, Queue) {
        let d = tempfile::tempdir().unwrap();
        let q = Queue::at(d.path().join("trellis").join("queue"));
        (d, q)
    }

    fn at(s: &str) -> i64 {
        parse_iso(s).unwrap()
    }

    fn routine(extra: &str) -> Routine {
        parse(
            "nightly",
            &format!(
                "schedule = \"0 3 * * *\"\n{extra}\n[task]\ntext = \"triage {{{{date}}}} for {{{{area}}}} ({{{{routine}}}})\"\n[vars]\narea = \"parser\"\n"
            ),
        )
        .unwrap()
    }

    fn pending(q: &Queue) -> Vec<crate::Task> {
        q.ids(State::Pending)
            .unwrap()
            .iter()
            .map(|id| crate::read_task(&q.path(State::Pending, id)).unwrap())
            .collect()
    }

    #[test]
    fn a_new_routine_is_armed_and_does_not_fire_for_the_past() {
        // The risk: the first `work` after adding a routine replays every
        // instant since the epoch, or since whenever cron says it "was due".
        let (_d, q) = queue();
        let r = routine("");
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        assert!(t.armed && t.fired.is_empty());
        assert!(pending(&q).is_empty());
    }

    #[test]
    fn the_template_is_filled_per_fire_in_the_routine_timezone() {
        let (_d, q) = queue();
        let r = routine("timezone = \"+05:00\"");
        evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        // 03:00 at +05:00 is 22:00Z the day before.
        let t = evaluate(&q, &r, at("2026-09-27T22:00:00Z")).unwrap();
        assert_eq!(t.fired.len(), 1);
        let tasks = pending(&q);
        assert_eq!(tasks[0].task, "triage 2026-09-28 for parser (nightly)");
        assert_eq!(tasks[0].routine.as_deref(), Some("nightly"));
    }

    #[test]
    fn skip_catch_up_fires_once_and_counts_what_it_dropped() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"");
        evaluate(&q, &r, at("2026-09-20T12:00:00Z")).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        assert_eq!((t.fired.len(), t.missed), (1, 6));
        assert_eq!(t.fired[0].0, at("2026-09-27T03:00:00Z"), "the most recent");
        // Handled means handled: evaluating again at the same moment fires
        // nothing more.
        let again = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        assert!(again.fired.is_empty());
        assert_eq!(pending(&q).len(), 1);
    }

    #[test]
    fn enqueue_catch_up_replays_at_most_its_cap_oldest_first() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"\ncatch_up = \"enqueue\"\ncatch_up_max = 3");
        evaluate(&q, &r, at("2026-09-20T12:00:00Z")).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        let times: Vec<i64> = t.fired.iter().map(|(a, _)| *a).collect();
        assert_eq!(
            times,
            vec![
                at("2026-09-25T03:00:00Z"),
                at("2026-09-26T03:00:00Z"),
                at("2026-09-27T03:00:00Z")
            ]
        );
        assert_eq!(t.missed, 4);
    }

    #[test]
    fn coalesce_and_skip_create_no_task_while_one_is_open() {
        // Paperclip's coalescing (inventory 42): a routine never piles a
        // second task on top of one nobody has run yet.
        for (policy, coalesced) in [("coalesce", true), ("skip", false)] {
            let (_d, q) = queue();
            let r = routine(&format!(
                "concurrency = \"{policy}\"\ncatch_up = \"enqueue\""
            ));
            evaluate(&q, &r, at("2026-09-24T12:00:00Z")).unwrap();
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
            assert!(matches!(t.fired[0].1, Fired::Enqueued(_)));
            assert!(t.fired[1..].iter().all(|(_, f)| if coalesced {
                matches!(f, Fired::Coalesced(_))
            } else {
                matches!(f, Fired::Skipped(_))
            }));
            let tasks = pending(&q);
            assert_eq!(tasks.len(), 1, "{policy}");
            assert_eq!(
                tasks[0].notes.len(),
                if coalesced { 2 } else { 0 },
                "{policy}"
            );
        }
    }

    #[test]
    fn a_failure_partway_through_a_tick_never_enqueues_an_instant_twice() {
        // The risk: three due fires, the second one fails, and the state
        // holding `last_fired` was saved only at the end — so the first
        // fire's task existed with nothing recording it, and every later
        // tick enqueued it again. Every write that can fail is failed in
        // turn, and each instant must end up queued exactly once.
        for k in 1..=24 {
            let (_d, q) = queue();
            let r = routine("concurrency = \"always\"\ncatch_up = \"enqueue\"\ncatch_up_max = 3");
            evaluate(&q, &r, at("2026-09-20T12:00:00Z")).unwrap();
            crate::files::fail::nth_link(k);
            let first = evaluate(&q, &r, at("2026-09-27T12:00:00Z"));
            crate::files::fail::nth_link(0);
            // Ticks after the failure, as `work` would run them.
            for _ in 0..2 {
                evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
            }
            let mut texts: Vec<String> = pending(&q).into_iter().map(|t| t.task).collect();
            texts.sort();
            let want = [
                "triage 2026-09-25 for parser (nightly)",
                "triage 2026-09-26 for parser (nightly)",
                "triage 2026-09-27 for parser (nightly)",
            ];
            assert_eq!(texts, want, "failing link {k} (first tick: {first:?})");
        }
    }

    #[test]
    fn a_tick_scans_the_queue_once_however_many_instants_are_due() {
        // The risk: the "already fired for this instant?" test listed the
        // whole queue, every task file read, once per due instant, under
        // the routine's lock (review round 8, finding 9). A catch-up of a
        // year of minutes was a year of whole-queue reads.
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"\ncatch_up = \"enqueue\"\ncatch_up_max = 5");
        for i in 0..20 {
            q.add_task(
                crate::NewTask {
                    task: format!("unrelated {i}"),
                    ..Default::default()
                },
                Actor::Human,
            )
            .unwrap();
        }
        evaluate(&q, &r, at("2026-09-20T12:00:00Z")).unwrap();
        SCANS.with(|c| c.set(0));
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
        assert_eq!(t.fired.len(), 5);
        assert_eq!(SCANS.with(|c| c.get()), 1, "one scan for five instants");
    }

    #[test]
    fn a_coalescing_tick_scans_the_queue_once_however_many_instants_are_due() {
        // The risk (review round 9, finding 8): round 8 made the "already
        // fired?" read one per tick, but `coalesce` and `skip` still read
        // every open task once per due instant to find the open one.
        for (policy, second) in [("coalesce", "Coalesced"), ("skip", "Skipped")] {
            let (_d, q) = queue();
            let r = routine(&format!(
                "concurrency = \"{policy}\"\ncatch_up = \"enqueue\"\ncatch_up_max = 5"
            ));
            for i in 0..20 {
                q.add_task(
                    crate::NewTask {
                        task: format!("unrelated {i}"),
                        ..Default::default()
                    },
                    Actor::Human,
                )
                .unwrap();
            }
            evaluate(&q, &r, at("2026-09-20T12:00:00Z")).unwrap();
            SCANS.with(|c| c.set(0));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z")).unwrap();
            assert_eq!(t.fired.len(), 5);
            let Fired::Enqueued(first) = &t.fired[0].1 else {
                panic!("{:?}", t.fired)
            };
            for (_, f) in &t.fired[1..] {
                let (kind, id) = match f {
                    Fired::Coalesced(id) => ("Coalesced", id),
                    Fired::Skipped(id) => ("Skipped", id),
                    Fired::Enqueued(id) => ("Enqueued", id),
                };
                assert_eq!((kind, id), (second, first), "{policy}: {:?}", t.fired);
            }
            assert_eq!(SCANS.with(|c| c.get()), 1, "{policy}: one scan");
        }
    }

    #[test]
    fn a_template_error_is_found_when_the_routine_loads() {
        let e = parse(
            "r",
            "schedule = \"* * * * *\"\n[task]\ntext = \"{{nope}}\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("{{nope}}"), "{e}");
        let e = parse(
            "r",
            "schedule = \"* * * * *\"\n[task]\ntext = \"x\"\n[vars]\ndate = \"1\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("shadow"), "{e}");
        assert!(
            parse(
                "r",
                "schedule = \"* * * * *\"\ntimezone = \"Europe/Paris\"\n[task]\ntext = \"x\"\n"
            )
            .unwrap_err()
            .to_string()
            .contains("fixed offsets")
        );
    }
}
