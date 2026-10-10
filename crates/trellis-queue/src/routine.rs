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
    /// `"commits"`: a scheduled fire is skipped when nothing outside
    /// `.trellis/runs/` was committed since the last one that enqueued.
    pub activity_gate: Option<String>,
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

/// What a scheduled fire waits for besides its instant (ADR 0026 §7,
/// amended for #348).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityGate {
    /// A commit changing a path outside `.trellis/runs/` since `last_head`.
    Commits,
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
    pub activity_gate: Option<ActivityGate>,
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
    let activity_gate = match file.activity_gate.as_deref() {
        None => None,
        Some("commits") => Some(ActivityGate::Commits),
        Some(o) => return Err(bad(format!("activity_gate = \"{o}\"; use commits"))),
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
        activity_gate,
        priority,
    })
}

pub fn load_all(root: &Path) -> Result<BTreeMap<String, Routine>, QueueError> {
    load_dir(&root.join(".trellis").join(DIR))
}

pub fn load_dir(dir: &Path) -> Result<BTreeMap<String, Routine>, QueueError> {
    crate::files::load_toml_dir(dir, parse)
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
            // Nor a model: `[task]` refuses unknown fields, so a routine
            // cannot set one (ADR 0026 §1).
            model: None,
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
    /// HEAD when a scheduled fire of a gated routine last enqueued a task.
    /// Untrusted when read back: see [`object_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_head: Option<String>,
}

/// The repository a routine with `activity_gate = "commits"` asks. `work`
/// supplies one over git; `None` from either method is a git error, and the
/// gate then fires rather than skips.
pub trait Repo {
    /// The commit HEAD names, or `None` when it cannot be read (an unborn
    /// branch, a git error).
    fn head(&self) -> Option<String>;
    /// The paths changed between commit `since` and commit `head`, or `None`
    /// when git cannot compare them (history rewritten, an object gone).
    /// Called only with two [`object_id`]s.
    fn changed(&self, since: &str, head: &str) -> Option<Vec<String>>;
}

/// A full object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex characters.
/// `last_head` comes from a file in the git directory that anything with
/// write access can plant, and `tree_diff` passes it to git with no
/// `--end-of-options`, so `--output=<path>` must never get that far.
fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Trellis's own trail commits (ADR 0007): changes under here are not
/// activity, or each run of a routine's task would count as some.
const TRAIL: &str = ".trellis/runs/";

/// Why a scheduled fire of a gated routine is quiet, for the log, or `None`
/// when it should go ahead: a commit outside the trail since `last_head`,
/// no usable `last_head`, or anything git could not answer.
fn quiet_reason(repo: &dyn Repo, last_head: Option<&str>, head: Option<&str>) -> Option<String> {
    let since = last_head.filter(|h| is_object_id(h))?;
    let head = head?;
    if since == head {
        return Some(format!("no commit since {since} (HEAD {head})"));
    }
    let paths = repo.changed(since, head)?;
    paths
        .iter()
        .all(|p| p.starts_with(TRAIL))
        .then(|| format!("only {TRAIL} changed between {since} and HEAD {head}"))
}

/// A repository for routines with no gate, in tests that must spawn no git:
/// it panics if asked anything.
#[cfg(test)]
pub(crate) struct NoGit;

#[cfg(test)]
impl Repo for NoGit {
    fn head(&self) -> Option<String> {
        panic!("an ungated routine read HEAD")
    }
    fn changed(&self, _: &str, _: &str) -> Option<Vec<String>> {
        panic!("an ungated routine asked what changed")
    }
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
fn open_task(q: &Queue, name: &str) -> Result<Option<String>, QueueError> {
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
        (None, _) => open_task(q, &r.name)?,
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
/// latest instant handled. `repo` is asked only for a routine with an
/// activity gate, and only when something is due.
pub fn evaluate(q: &Queue, r: &Routine, now: i64, repo: &dyn Repo) -> Result<Tick, QueueError> {
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
            last_head: None,
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
    while let Some(next) = r.cron.next_after(t, &r.tz) {
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
    // Read once per tick: every instant of a catch-up compares against the
    // same HEAD. One that is not an object id is as good as unreadable.
    let head = match r.activity_gate {
        Some(ActivityGate::Commits) => repo.head().filter(|h| is_object_id(h)),
        None => None,
    };
    for at in due {
        // A task already made for this instant is this fire (see `fire`),
        // so the gate comes after that check and before the concurrency
        // policy, which `fire` applies. Its HEAD at the time is unknown, so
        // it records none; the next fire then compares against an older
        // one, which can only make it enqueue.
        let made = already.by_at.contains_key(&at);
        if r.activity_gate.is_some()
            && !made
            && let Some(why) = quiet_reason(repo, st.last_head.as_deref(), head.as_deref())
        {
            q.log(
                Event::new(
                    Actor::Routine,
                    "routine.quiet",
                    format!("routine:{}", r.name),
                )
                .detail(format!("{}; {why}", crate::secs_to_iso(at))),
            )?;
            // Handled like a skipped fire, so catch-up does not replay it.
            st.last_fired = Some(at);
            replace(&path, &st)?;
            continue;
        }
        let f = fire(q, r, at, Actor::Routine, Some(&mut already))?;
        if let Fired::Enqueued(id) = &f {
            st.last_task = Some(id.clone());
            if !made && head.is_some() {
                st.last_head = head.clone();
            }
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
        .filter_map(|r| r.cron.next_after(now, &r.tz))
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
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert!(t.armed && t.fired.is_empty());
        assert!(pending(&q).is_empty());
    }

    #[test]
    fn the_template_is_filled_per_fire_in_the_routine_timezone() {
        let (_d, q) = queue();
        let r = routine("timezone = \"+05:00\"");
        evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        // 03:00 at +05:00 is 22:00Z the day before.
        let t = evaluate(&q, &r, at("2026-09-27T22:00:00Z"), &NoGit).unwrap();
        assert_eq!(t.fired.len(), 1);
        let tasks = pending(&q);
        assert_eq!(tasks[0].task, "triage 2026-09-28 for parser (nightly)");
        assert_eq!(tasks[0].routine.as_deref(), Some("nightly"));
    }

    #[test]
    fn skip_catch_up_fires_once_and_counts_what_it_dropped() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"");
        evaluate(&q, &r, at("2026-09-20T12:00:00Z"), &NoGit).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert_eq!((t.fired.len(), t.missed), (1, 6));
        assert_eq!(t.fired[0].0, at("2026-09-27T03:00:00Z"), "the most recent");
        // Handled means handled: evaluating again at the same moment fires
        // nothing more.
        let again = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert!(again.fired.is_empty());
        assert_eq!(pending(&q).len(), 1);
    }

    #[test]
    fn enqueue_catch_up_replays_at_most_its_cap_oldest_first() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"\ncatch_up = \"enqueue\"\ncatch_up_max = 3");
        evaluate(&q, &r, at("2026-09-20T12:00:00Z"), &NoGit).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
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
            evaluate(&q, &r, at("2026-09-24T12:00:00Z"), &NoGit).unwrap();
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
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
            evaluate(&q, &r, at("2026-09-20T12:00:00Z"), &NoGit).unwrap();
            crate::files::fail::nth_link(k);
            let first = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit);
            crate::files::fail::nth_link(0);
            // Ticks after the failure, as `work` would run them.
            for _ in 0..2 {
                evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
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
        evaluate(&q, &r, at("2026-09-20T12:00:00Z"), &NoGit).unwrap();
        SCANS.with(|c| c.set(0));
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
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
            evaluate(&q, &r, at("2026-09-20T12:00:00Z"), &NoGit).unwrap();
            SCANS.with(|c| c.set(0));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
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
                "schedule = \"* * * * *\"\ntimezone = \"Europe/Atlantis\"\n[task]\ntext = \"x\"\n"
            )
            .unwrap_err()
            .to_string()
            .contains("fixed offsets")
        );
    }

    fn file(schedule: &str, extra: &str, text: &str) -> Routine {
        parse(
            "r",
            &format!("schedule = \"{schedule}\"\n{extra}\n[task]\ntext = \"{text}\"\n"),
        )
        .unwrap()
    }

    #[test]
    fn a_one_letter_placeholder_is_read_like_any_other() {
        file("* * * * *", "", "{{x}}{{y}}\"\n[vars]\nx = \"1\"\ny = \"2");
        let e = parse(
            "r",
            "schedule = \"* * * * *\"\n[task]\ntext = \"{{x}}{{z}}\"\n[vars]\nx = \"1\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("{{z}}"), "{e}");
    }

    #[test]
    fn catch_up_max_is_bounded_at_both_ends() {
        let max = |n: u32| {
            parse(
                "r",
                &format!(
                    "schedule = \"* * * * *\"\ncatch_up = \"enqueue\"\ncatch_up_max = {n}\n\
                     [task]\ntext = \"x\"\n"
                ),
            )
        };
        assert!(max(0).is_err());
        assert!(max(1).is_ok() && max(MAX_CATCH_UP).is_ok());
        assert!(max(MAX_CATCH_UP + 1).is_err());
    }

    #[test]
    fn routines_load_from_their_folder_and_one_that_cannot_be_read_is_an_error() {
        // Read as "no routines", a broken folder stops every schedule
        // without a word.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".trellis").join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("nightly.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\n",
        )
        .unwrap();
        let set = load_all(d.path()).unwrap();
        assert_eq!(set.keys().collect::<Vec<_>>(), ["nightly"]);
        let not_a_dir = d.path().join("plain-file");
        std::fs::write(&not_a_dir, "").unwrap();
        assert!(load_dir(&not_a_dir).is_err());
    }

    #[test]
    fn state_is_none_until_the_routine_is_armed_and_then_says_when() {
        let (_d, q) = queue();
        let r = routine("");
        assert_eq!(state(&q, "nightly").unwrap(), None);
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert_eq!(t.name, "nightly");
        assert_eq!(
            state(&q, "nightly").unwrap().map(|s| s.armed_at),
            Some(at("2026-09-27T12:00:00Z"))
        );
    }

    fn missed_logged(q: &Queue) -> usize {
        q.activity()
            .tail(1000)
            .unwrap()
            .iter()
            .filter(|e| e.action == "routine.missed")
            .count()
    }

    #[test]
    fn only_a_tick_that_dropped_fires_logs_them_as_missed() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"always\"");
        evaluate(&q, &r, at("2026-09-26T12:00:00Z"), &NoGit).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert_eq!((t.fired.len(), t.missed), (1, 0));
        assert_eq!(missed_logged(&q), 0);
        let t = evaluate(&q, &r, at("2026-09-30T12:00:00Z"), &NoGit).unwrap();
        assert_eq!((t.fired.len(), t.missed), (1, 2));
        assert_eq!(missed_logged(&q), 1);
    }

    fn of_routine(q: &Queue, name: &str, text: &str) -> String {
        q.add_task(
            crate::NewTask {
                task: text.into(),
                routine: Some(name.into()),
                ..Default::default()
            },
            Actor::Human,
        )
        .unwrap()
    }

    #[test]
    fn a_manual_run_coalesces_into_this_routines_open_task_and_not_another_routines() {
        let (_d, q) = queue();
        let r = routine("concurrency = \"coalesce\"");
        of_routine(&q, "weekly", "another routine's");
        let now = at("2026-09-27T12:00:00Z");
        let Fired::Enqueued(first) = run_now(&q, &r, now).unwrap() else {
            panic!("the first manual run enqueues");
        };
        assert_eq!(run_now(&q, &r, now).unwrap(), Fired::Coalesced(first));
    }

    #[test]
    fn a_fire_coalesces_into_the_first_claimed_task_else_the_first_pending_one() {
        // The task `active` would name, so a scheduled fire and a manual one
        // note the same task.
        for claimed in [true, false] {
            let (_d, q) = queue();
            let r = routine("concurrency = \"coalesce\"");
            let ids: Vec<String> = (0..4)
                .map(|i| of_routine(&q, "nightly", &format!("t{i}")))
                .collect();
            if claimed {
                for id in &ids[..2] {
                    std::mem::forget(q.claim(id).unwrap().unwrap());
                }
            }
            evaluate(&q, &r, at("2026-09-26T12:00:00Z"), &NoGit).unwrap();
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
            // ids[0] is the first claimed one, or with none claimed the
            // first pending one.
            assert_eq!(t.fired[0].1, Fired::Coalesced(ids[0].clone()), "{claimed}");
        }
    }

    #[test]
    fn the_next_due_instant_is_the_earliest_of_the_unpaused_routines() {
        let set: BTreeMap<String, Routine> = [
            ("nightly".to_string(), routine("")),
            ("noon".to_string(), file("0 13 * * *", "paused = true", "x")),
        ]
        .into();
        assert_eq!(
            next_due(&set, at("2026-09-27T12:00:00Z")),
            Some(at("2026-09-28T03:00:00Z"))
        );
    }

    // ---- activity_gate = "commits" (ADR 0026 §7, amended for #348) ----

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A repository that answers from its fields and spawns no git. It
    /// fails the test if asked to compare anything that is not an object
    /// id, since `tree_diff` would hand that to git as an option.
    struct Fake {
        head: Option<&'static str>,
        changed: Option<Vec<&'static str>>,
        asked: std::cell::Cell<u32>,
    }

    fn fake(head: Option<&'static str>, changed: Option<Vec<&'static str>>) -> Fake {
        Fake {
            head,
            changed,
            asked: std::cell::Cell::new(0),
        }
    }

    impl Repo for Fake {
        fn head(&self) -> Option<String> {
            self.head.map(str::to_string)
        }
        fn changed(&self, since: &str, head: &str) -> Option<Vec<String>> {
            assert!(
                is_object_id(since) && is_object_id(head),
                "the repository was asked about `{since}`..`{head}`"
            );
            self.asked.set(self.asked.get() + 1);
            self.changed
                .as_ref()
                .map(|v| v.iter().map(|p| p.to_string()).collect())
        }
    }

    fn gated(extra: &str) -> Routine {
        routine(&format!("activity_gate = \"commits\"\n{extra}"))
    }

    /// Armed on the 26th, with `last_head` set as a previous fire would
    /// have left it; the next evaluation at noon on the 27th has the 03:00
    /// fire due.
    fn armed_with(q: &Queue, r: &Routine, last_head: Option<&str>) {
        evaluate(q, r, at("2026-09-26T12:00:00Z"), &NoGit).unwrap();
        let mut st = state(q, &r.name).unwrap().unwrap();
        st.last_head = last_head.map(str::to_string);
        replace(&state_path(q, &r.name), &st).unwrap();
    }

    fn last_head(q: &Queue) -> Option<String> {
        state(q, "nightly").unwrap().unwrap().last_head
    }

    fn quiet_logged(q: &Queue) -> Vec<String> {
        q.activity()
            .tail(1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.action == "routine.quiet")
            .map(|e| e.detail.unwrap_or_default())
            .collect()
    }

    #[test]
    fn routine_activity_gate_is_commits_or_absent_and_anything_else_is_refused() {
        // The risk: a misspelt gate read as no gate (fires every night) or
        // as some gate nobody defined.
        assert_eq!(routine("").activity_gate, None);
        assert_eq!(gated("").activity_gate, Some(ActivityGate::Commits));
        for v in ["Commits", "activity", ""] {
            let e = parse(
                "nightly",
                &format!(
                    "schedule = \"0 3 * * *\"\nactivity_gate = \"{v}\"\n[task]\ntext = \"x\"\n"
                ),
            )
            .unwrap_err()
            .to_string();
            assert!(
                e.contains(&format!("activity_gate = \"{v}\"")) && e.contains("use commits"),
                "{v}: {e}"
            );
        }
    }

    #[test]
    fn routine_activity_gate_the_first_fire_after_arming_enqueues_and_records_head() {
        // The risk: with nothing to compare against, the gate skips every
        // fire forever.
        let (_d, q) = queue();
        let r = gated("");
        armed_with(&q, &r, None);
        let repo = fake(Some(A), None);
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
        assert!(matches!(t.fired[..], [(_, Fired::Enqueued(_))]), "{t:?}");
        assert_eq!(last_head(&q).as_deref(), Some(A));
        assert_eq!(repo.asked.get(), 0, "nothing to compare with");
    }

    #[test]
    fn routine_activity_gate_a_quiet_fire_creates_no_task_logs_both_commits_and_is_not_replayed() {
        // The risk: a night with no commit, or with only trellis's own
        // trail commits, still queues a task; or a quiet fire is left
        // unhandled and catch-up replays it once a commit lands.
        let cases: [(&str, Option<Vec<&str>>); 3] = [
            (A, None),
            (
                B,
                Some(vec![".trellis/runs/r1/trail.json", ".trellis/runs/r2/x"]),
            ),
            (B, Some(vec![])),
        ];
        for (head, changed) in cases {
            let (_d, q) = queue();
            let r = gated("catch_up = \"enqueue\"");
            // An open task: a quiet fire is decided before the concurrency
            // policy, so it is not coalesced into it as a note.
            let open = of_routine(&q, "nightly", "open");
            armed_with(&q, &r, Some(A));
            let repo = fake(Some(head), changed);
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
            assert!(t.fired.is_empty(), "{head}: {t:?}");
            assert_eq!(pending(&q).len(), 1, "{head}");
            assert!(
                crate::read_task(&q.path(State::Pending, &open))
                    .unwrap()
                    .notes
                    .is_empty()
            );
            let logged = quiet_logged(&q);
            assert!(
                matches!(&logged[..], [d] if d.contains(A) && d.contains(head)),
                "{head}: {logged:?}"
            );
            let st = state(&q, "nightly").unwrap().unwrap();
            assert_eq!(st.last_fired, Some(at("2026-09-27T03:00:00Z")));
            assert_eq!(
                st.last_head.as_deref(),
                Some(A),
                "a quiet fire records no HEAD"
            );
            // A commit now does not replay the quiet instant.
            let busy = fake(Some(B), Some(vec!["src/lib.rs"]));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &busy).unwrap();
            assert!(t.fired.is_empty(), "{head}: replayed {t:?}");
        }
    }

    #[test]
    fn routine_activity_gate_a_commit_outside_the_trail_enqueues_and_records_the_new_head() {
        let (_d, q) = queue();
        let r = gated("concurrency = \"always\"\ncatch_up = \"enqueue\"");
        // Armed on the 24th: the 25th, 26th and 27th are due in one tick.
        evaluate(&q, &r, at("2026-09-24T12:00:00Z"), &NoGit).unwrap();
        let mut st = state(&q, "nightly").unwrap().unwrap();
        st.last_head = Some(A.into());
        replace(&state_path(&q, "nightly"), &st).unwrap();
        let repo = fake(
            Some(B),
            Some(vec![".trellis/runs/r1/trail.json", "src/lib.rs"]),
        );
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
        // The first due fire sees the commit; the two after it in the same
        // tick see HEAD equal to what that fire recorded, so are quiet.
        assert!(matches!(t.fired[..], [(_, Fired::Enqueued(_))]), "{t:?}");
        assert_eq!(t.fired[0].0, at("2026-09-25T03:00:00Z"));
        assert_eq!(last_head(&q).as_deref(), Some(B));
        assert_eq!(quiet_logged(&q).len(), 2);
        assert_eq!(repo.asked.get(), 1);
    }

    #[test]
    fn routine_activity_gate_a_git_error_or_an_unreadable_head_enqueues() {
        // The risk: a rewritten history, a pruned object or an unborn
        // branch silences the routine; the gate fails open instead.
        let (_d, q) = queue();
        let r = gated("");
        armed_with(&q, &r, Some(A));
        let repo = fake(Some(B), None);
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
        assert!(matches!(t.fired[..], [(_, Fired::Enqueued(_))]), "{t:?}");
        assert_eq!(last_head(&q).as_deref(), Some(B));
        assert_eq!(repo.asked.get(), 1);

        // HEAD unreadable, or read as something that is not an object id.
        for head in [None, Some("--output=planted")] {
            let (_d, q) = queue();
            armed_with(&q, &r, Some(A));
            let repo = fake(head, Some(vec![]));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
            assert!(
                matches!(t.fired[..], [(_, Fired::Enqueued(_))]),
                "{head:?}: {t:?}"
            );
            assert_eq!(
                last_head(&q).as_deref(),
                Some(A),
                "{head:?}: no HEAD to record"
            );
            assert_eq!(repo.asked.get(), 0, "{head:?}");
        }
    }

    #[test]
    fn routine_activity_gate_a_malformed_last_head_is_absent_and_never_reaches_git() {
        // The risk: `last_head` is read from a file in the git directory and
        // `tree_diff` passes it to git with no `--end-of-options`, so a
        // planted `--output=<path>` would be an option. `Fake::changed`
        // fails the test if it receives one.
        let bad = [
            "--output=planted".to_string(),
            A.to_uppercase(),
            A[..39].to_string(),
            format!("{A}a"),
            "g".repeat(40),
            "a".repeat(63),
        ];
        for b in bad {
            let (_d, q) = queue();
            let r = gated("");
            armed_with(&q, &r, Some(&b));
            let repo = fake(Some(A), Some(vec![]));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
            assert!(
                matches!(t.fired[..], [(_, Fired::Enqueued(_))]),
                "{b}: {t:?}"
            );
            assert_eq!(repo.asked.get(), 0, "{b}");
            assert_eq!(last_head(&q).as_deref(), Some(A), "{b}");
        }
        // A 64-character (SHA-256) id is an object id.
        assert!(is_object_id(&"c".repeat(64)) && is_object_id(A));
    }

    #[test]
    fn routine_activity_gate_last_head_moves_only_when_a_scheduled_fire_enqueues() {
        // Coalesced and skipped fires leave it, as does a manual run, which
        // is not gated at all.
        for policy in ["coalesce", "skip"] {
            let (_d, q) = queue();
            let r = gated(&format!("concurrency = \"{policy}\""));
            of_routine(&q, "nightly", "open");
            armed_with(&q, &r, Some(A));
            let repo = fake(Some(B), Some(vec!["src/lib.rs"]));
            let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
            assert!(
                matches!(t.fired[..], [(_, Fired::Coalesced(_) | Fired::Skipped(_))]),
                "{policy}: {t:?}"
            );
            assert_eq!(last_head(&q).as_deref(), Some(A), "{policy}");
        }
        let (_d, q) = queue();
        let r = gated("");
        armed_with(&q, &r, Some(A));
        assert!(matches!(
            run_now(&q, &r, at("2026-09-27T12:00:00Z")).unwrap(),
            Fired::Enqueued(_)
        ));
        assert_eq!(last_head(&q).as_deref(), Some(A), "run_now");

        // Without the key nothing asks the repository and nothing is
        // recorded: `NoGit` panics if asked.
        let (_d, q) = queue();
        let r = routine("");
        evaluate(&q, &r, at("2026-09-26T12:00:00Z"), &NoGit).unwrap();
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &NoGit).unwrap();
        assert!(matches!(t.fired[..], [(_, Fired::Enqueued(_))]));
        assert_eq!(last_head(&q), None);

        // A state file from before the field loads, and one without a HEAD
        // is written as before.
        let old: RoutineState =
            serde_json::from_str(r#"{"armed_at":5,"last_fired":7,"last_task":"t"}"#).unwrap();
        assert_eq!(old.last_head, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("last_head"));
    }

    #[test]
    fn routine_activity_gate_comes_after_the_check_for_a_task_already_made_for_the_instant() {
        // A task for this instant is this fire, done by a tick that failed
        // before saving; the gate must not call it quiet, and its HEAD at
        // the time is unknown, so none is recorded.
        let (_d, q) = queue();
        let r = gated("");
        armed_with(&q, &r, Some(A));
        let id = q
            .add_task(
                crate::NewTask {
                    task: "made".into(),
                    routine: Some("nightly".into()),
                    routine_at: Some(at("2026-09-27T03:00:00Z")),
                    ..Default::default()
                },
                Actor::Routine,
            )
            .unwrap();
        let repo = fake(Some(A), None);
        let t = evaluate(&q, &r, at("2026-09-27T12:00:00Z"), &repo).unwrap();
        assert_eq!(
            t.fired,
            vec![(at("2026-09-27T03:00:00Z"), Fired::Enqueued(id))]
        );
        assert!(quiet_logged(&q).is_empty());
        assert_eq!(last_head(&q).as_deref(), Some(A));
    }
}
