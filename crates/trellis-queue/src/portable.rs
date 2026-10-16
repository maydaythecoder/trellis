//! `queue export` and `queue import` (ADR 0026 §10).
//!
//! The folder layout:
//!
//! ```text
//! agents/<name>.toml
//! instructions/<path under .trellis/>     each profile's instructions file
//! routines/<name>.toml
//! tasks/<state>/<id>.json
//! ```
//!
//! Nothing secret exists to scrub: a profile names a model spec, and a key
//! never goes in a file trellis reads (ADR 0010). Overlays, routine state,
//! approval requests and the activity log are this checkout's run-time
//! state, not definitions, and are not exported.

use crate::activity::{Actor, Event};
use crate::files::read_json;
use crate::profile::{self, Profile};
use crate::routine;
use crate::{Queue, QueueError, State, Task};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    Skip,
    Replace,
    Rename,
}

impl OnConflict {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "skip" => OnConflict::Skip,
            "replace" => OnConflict::Replace,
            "rename" => OnConflict::Rename,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Agent,
    Routine,
    Task,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Routine => "routine",
            Kind::Task => "task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create,
    Skip,
    Replace,
    Rename(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub action: Action,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub items: Vec<Item>,
    agents: BTreeMap<String, Profile>,
    agent_text: BTreeMap<String, String>,
    routines: BTreeMap<String, String>,
    tasks: Vec<(State, String, Task)>,
    src: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Exported {
    pub agents: usize,
    pub instructions: usize,
    pub routines: usize,
    pub tasks: usize,
    /// Claimed tasks are in the middle of a run and are not exported.
    pub claimed_skipped: usize,
    /// This checkout's run-time state, which export leaves behind (ADR 0026
    /// §10), counted so the export can say so rather than drop it silently.
    /// Overlays that carry something (a pause, a raise) of the exported
    /// profiles.
    pub overlays: usize,
    /// Exported routines with a last-fired state here.
    pub routine_states: usize,
    /// Budget override requests, pending or decided.
    pub approval_requests: usize,
    pub activity_lines: usize,
    /// Profiles paused by their overlay (`agents pause`, a budget hard stop)
    /// and not by their file: the exported file does not say `paused`, so an
    /// import elsewhere runs them.
    pub paused_by_overlay: Vec<String>,
}

fn io(p: &Path) -> impl FnOnce(std::io::Error) -> QueueError + '_ {
    crate::files::io(p)
}

fn copy(from: &Path, to: &Path) -> Result<(), QueueError> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(io(d))?;
    }
    std::fs::copy(from, to).map_err(io(from))?;
    Ok(())
}

fn tomls(dir: &Path) -> Result<Vec<(String, PathBuf)>, QueueError> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(dir)(e)),
    };
    let mut v: Vec<(String, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let n = p.file_name()?.to_str()?.strip_suffix(".toml")?.to_string();
            Some((n, p))
        })
        .collect();
    v.sort();
    Ok(v)
}

/// Writes profiles, routines and tasks to `dest`, which must not exist or
/// be empty: mixing an export into another folder's contents would make the
/// import that reads it back import both.
pub fn export(root: &Path, q: &Queue, dest: &Path) -> Result<Exported, QueueError> {
    if dest.exists() && std::fs::read_dir(dest).map_err(io(dest))?.next().is_some() {
        return Err(QueueError::Invalid(format!(
            "{} is not empty; export into a new or empty folder",
            dest.display()
        )));
    }
    let mut n = Exported::default();
    let trellis = root.join(".trellis");
    let profiles = profile::load_all(root)?;
    for (name, p) in &profiles {
        copy(
            &trellis.join(profile::DIR).join(format!("{name}.toml")),
            &dest.join("agents").join(format!("{name}.toml")),
        )?;
        n.agents += 1;
        let o = profile::overlay(q, name)?;
        if o != profile::Overlay::default() {
            n.overlays += 1;
        }
        if o.paused.is_some() && !p.file.paused {
            n.paused_by_overlay.push(name.clone());
        }
        if let Some(rel) = &p.file.instructions {
            let from = trellis.join(rel);
            if from.exists() {
                copy(&from, &dest.join("instructions").join(rel))?;
                n.instructions += 1;
            }
        }
    }
    routine::load_all(root)?;
    for (name, path) in tomls(&trellis.join(routine::DIR))? {
        copy(&path, &dest.join("routines").join(format!("{name}.toml")))?;
        n.routines += 1;
        if routine::state(q, &name)?.is_some() {
            n.routine_states += 1;
        }
    }
    for which in ["pending", "approved", "rejected"] {
        n.approval_requests += crate::approval::list(q, which)?.len();
    }
    let log = q.activity();
    n.activity_lines = match std::fs::read(log.path()) {
        Ok(b) => b
            .split(|&c| c == b'\n')
            .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
            .count(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(io(log.path())(e)),
    };
    for s in State::ALL {
        for id in q.ids(s)? {
            if s == State::Claimed {
                n.claimed_skipped += 1;
                continue;
            }
            // Read as `ids()` listed it: a task whose only copy is the side
            // file of a replacement cut short is still the task.
            let bytes = crate::files::read_settled(&q.path(s, &id), |p| std::fs::read(p))?;
            let to = dest
                .join("tasks")
                .join(s.dir_name())
                .join(format!("{id}.json"));
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(io(d))?;
            }
            std::fs::write(&to, bytes).map_err(io(&to))?;
            n.tasks += 1;
        }
    }
    Ok(n)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn free_name(taken: &dyn Fn(&str) -> bool, base: &str) -> String {
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|n| !taken(n))
        .expect("an unbounded range finds a free name")
}

/// Reads `src` and decides what each item would do. Writes nothing.
///
/// Refused as a whole — nothing written — when the profiles that would
/// result are not a valid set (a `reports_to` cycle across old and new, a
/// dangling manager), or when an item cannot be read.
pub fn plan_import(root: &Path, q: &Queue, src: &Path, on: OnConflict) -> Result<Plan, QueueError> {
    let mut plan = Plan {
        src: src.to_path_buf(),
        ..Plan::default()
    };
    let existing = profile::load_all(root)?;
    let existing_routines = routine::load_all(root)?;
    // Parsed but not validated on their own: a folder of profiles that
    // reports to managers only this repository has is still importable.
    // The combined set is what is validated, below.
    let mut incoming = BTreeMap::new();
    for (n, p) in tomls(&src.join("agents"))? {
        let text = std::fs::read_to_string(&p).map_err(io(&p))?;
        incoming.insert(n.clone(), profile::parse(&n, &text)?);
    }

    // Agents first: their renames rewrite references everywhere else.
    let mut agent_map: HashMap<String, String> = HashMap::new();
    for (name, p) in incoming {
        let text = std::fs::read_to_string(src.join("agents").join(format!("{name}.toml")))
            .map_err(io(&src.join("agents")))?;
        let action = if !existing.contains_key(&name) {
            Action::Create
        } else {
            match on {
                OnConflict::Skip => Action::Skip,
                OnConflict::Replace => Action::Replace,
                OnConflict::Rename => {
                    let fresh = free_name(
                        &|n| existing.contains_key(n) || plan.agents.contains_key(n),
                        &name,
                    );
                    agent_map.insert(name.clone(), fresh.clone());
                    Action::Rename(fresh)
                }
            }
        };
        plan.items.push(Item {
            kind: Kind::Agent,
            name: name.clone(),
            action,
        });
        plan.agent_text.insert(name.clone(), text);
        plan.agents.insert(name, p);
    }
    let agent = |n: &str| agent_map.get(n).cloned().unwrap_or_else(|| n.to_string());

    // The combined profile set, as it would be after the import.
    let mut combined = existing.clone();
    for it in plan.items.iter().filter(|i| i.kind == Kind::Agent) {
        let mut p = plan.agents[&it.name].clone();
        if let Some(r) = &p.file.reports_to {
            p.file.reports_to = Some(agent(r));
        }
        match &it.action {
            Action::Skip => {}
            Action::Rename(to) => {
                p.name = to.clone();
                combined.insert(to.clone(), p);
            }
            Action::Create | Action::Replace => {
                combined.insert(it.name.clone(), p);
            }
        }
    }
    profile::validate(&combined).map_err(|e| {
        QueueError::Invalid(format!("the import would leave the profiles invalid: {e}"))
    })?;
    // An instructions file is shared by path, not owned by a profile name,
    // so a created or renamed profile must not overwrite a different local
    // file under the same path. Only `replace` says to.
    for it in plan.items.iter().filter(|i| i.kind == Kind::Agent) {
        if matches!(it.action, Action::Skip | Action::Replace) {
            continue;
        }
        if let Some(rel) = &plan.agents[&it.name].file.instructions {
            let (from, to) = (
                src.join("instructions").join(rel),
                root.join(".trellis").join(rel),
            );
            if from.exists()
                && to.exists()
                && std::fs::read(&from).map_err(io(&from))?
                    != std::fs::read(&to).map_err(io(&to))?
            {
                return Err(QueueError::Invalid(format!(
                    "agent `{}`: its instructions file .trellis/{rel} already exists here \
                     with different content; nothing was imported. Use --on-conflict \
                     replace to overwrite it, or move one of the two",
                    it.name
                )));
            }
        }
    }

    for (name, path) in tomls(&src.join("routines"))? {
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        routine::parse(&name, &text)?;
        let action = if !existing_routines.contains_key(&name) {
            Action::Create
        } else {
            match on {
                OnConflict::Skip => Action::Skip,
                OnConflict::Replace => Action::Replace,
                OnConflict::Rename => Action::Rename(free_name(
                    &|n| existing_routines.contains_key(n) || plan.routines.contains_key(n),
                    &name,
                )),
            }
        };
        plan.items.push(Item {
            kind: Kind::Routine,
            name: name.clone(),
            action,
        });
        plan.routines.insert(name, text);
    }

    let existing_ids = q.index()?;
    let mut task_map: HashMap<String, String> = HashMap::new();
    let mut tasks = Vec::new();
    for s in State::ALL {
        let d = src.join("tasks").join(s.dir_name());
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        files.sort();
        for p in files {
            let Some(id) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
            else {
                continue;
            };
            if s == State::Claimed {
                return Err(QueueError::Invalid(format!(
                    "{}: a claimed task cannot be imported; it belongs to a run in \
                     another checkout",
                    p.display()
                )));
            }
            if !valid_id(&id) {
                return Err(QueueError::Invalid(format!(
                    "{}: `{id}` is not a task id",
                    p.display()
                )));
            }
            let t: Task = read_json(&p)?;
            let action = match existing_ids.by_id.get(&id) {
                None => Action::Create,
                // Found here rather than by `remove_task` in `apply_import`,
                // which would refuse it only after every item ahead of it
                // had been written. A claim made between this plan and the
                // apply is still refused there, mid-import.
                Some((State::Claimed, _)) if on == OnConflict::Replace => {
                    return Err(QueueError::Invalid(format!(
                        "task `{id}` is claimed by a run here, and --on-conflict replace \
                         cannot replace a task a run holds; nothing was imported. Wait \
                         for the run, or use --on-conflict skip or rename"
                    )));
                }
                Some(_) => match on {
                    OnConflict::Skip => Action::Skip,
                    OnConflict::Replace => Action::Replace,
                    OnConflict::Rename => {
                        let fresh = Queue::fresh_id(&crate::slugify(&t.task));
                        task_map.insert(id.clone(), fresh.clone());
                        Action::Rename(fresh)
                    }
                },
            };
            plan.items.push(Item {
                kind: Kind::Task,
                name: id.clone(),
                action,
            });
            tasks.push((s, id, t));
            // Distinct stamps for consecutive renames.
            std::thread::sleep(std::time::Duration::from_micros(2));
        }
    }
    // References within the imported set follow the renames.
    let tid = |n: &String| task_map.get(n).cloned().unwrap_or_else(|| n.clone());
    for (_, _, t) in &mut tasks {
        t.assignee = t.assignee.as_deref().map(agent);
        t.parent = t.parent.as_ref().map(tid);
        t.blockers = t.blockers.iter().map(tid).collect();
    }
    plan.tasks = tasks;
    plan.agents.values_mut().for_each(|p| {
        if let Some(r) = &p.file.reports_to {
            p.file.reports_to = Some(agent(r));
        }
    });
    // Routine templates' assignees follow agent renames too.
    for text in plan.routines.values_mut() {
        if agent_map.is_empty() {
            break;
        }
        let mut f: routine::RoutineFile =
            toml::from_str(text).map_err(|e| QueueError::Invalid(e.to_string()))?;
        if let Some(a) = &f.task.assignee
            && agent_map.contains_key(a)
        {
            f.task.assignee = Some(agent(a));
            *text = toml::to_string(&f).map_err(|e| QueueError::Invalid(e.to_string()))?;
        }
    }
    for (n, p) in &plan.agents {
        let original = profile::parse(n, &plan.agent_text[n])?;
        if original.file.reports_to != p.file.reports_to {
            let t = toml::to_string(&p.file).map_err(|e| QueueError::Invalid(e.to_string()))?;
            plan.agent_text.insert(n.clone(), t);
        }
    }
    Ok(plan)
}

/// Refuses, before an import writes anything, a link at `.trellis`,
/// `.trellis/agents`, `.trellis/routines` or any file the import would write
/// (#725). A repository can commit either directory as a link to one
/// outside itself, and the import would then overwrite files there with
/// the imported profiles and routines.
fn refuse_links(root: &Path, plan: &Plan) -> Result<(), QueueError> {
    let trellis = root.join(".trellis");
    let mut paths = vec![
        trellis.clone(),
        trellis.join(profile::DIR),
        trellis.join(routine::DIR),
    ];
    for it in &plan.items {
        let target = match &it.action {
            Action::Skip => continue,
            Action::Rename(to) => to,
            Action::Create | Action::Replace => &it.name,
        };
        match it.kind {
            Kind::Agent => {
                paths.push(trellis.join(profile::DIR).join(format!("{target}.toml")));
                if let Some(rel) = &plan.agents[&it.name].file.instructions {
                    paths.push(trellis.join(rel));
                }
            }
            Kind::Routine => {
                paths.push(trellis.join(routine::DIR).join(format!("{target}.toml")));
            }
            // Tasks go into the queue in the git directory, not under
            // `.trellis/`.
            Kind::Task => {}
        }
    }
    for p in &paths {
        trellis_core::paths::refuse_links_under(root, p)?;
    }
    Ok(())
}

/// Carries out a plan from [`plan_import`].
pub fn apply_import(root: &Path, q: &Queue, plan: &Plan) -> Result<(), QueueError> {
    let trellis = root.join(".trellis");
    refuse_links(root, plan)?;
    let write = |path: &Path, text: &str| -> Result<(), QueueError> {
        trellis_core::paths::replace_file(root, path, text.as_bytes()).map_err(io(path))
    };
    // Through `replace_file` like the profiles, so the destination is
    // never followed; the source is the export being imported.
    let copy = |from: &Path, to: &Path| -> Result<(), QueueError> {
        let bytes = std::fs::read(from).map_err(io(from))?;
        trellis_core::paths::replace_file(root, to, &bytes).map_err(io(to))
    };
    let mut tasks = plan.tasks.iter();
    for it in &plan.items {
        let target = match &it.action {
            Action::Skip => {
                if it.kind == Kind::Task {
                    tasks.next();
                }
                continue;
            }
            Action::Rename(to) => to.clone(),
            Action::Create | Action::Replace => it.name.clone(),
        };
        match it.kind {
            Kind::Agent => {
                write(
                    &trellis.join(profile::DIR).join(format!("{target}.toml")),
                    &plan.agent_text[&it.name],
                )?;
                if let Some(rel) = &plan.agents[&it.name].file.instructions {
                    let from = plan.src.join("instructions").join(rel);
                    if from.exists() {
                        copy(&from, &trellis.join(rel))?;
                    }
                }
            }
            Kind::Routine => {
                write(
                    &trellis.join(routine::DIR).join(format!("{target}.toml")),
                    &plan.routines[&it.name],
                )?;
            }
            Kind::Task => {
                let (s, _, t) = tasks.next().expect("one planned item per task");
                if it.action == Action::Replace {
                    q.remove_task(&it.name)?;
                }
                if !q.publish_as(*s, &target, t)? {
                    return Err(QueueError::Invalid(format!(
                        "task `{target}` appeared while importing; nothing overwritten"
                    )));
                }
            }
        }
        q.log(
            Event::new(
                Actor::Human,
                "import",
                format!("{}:{target}", it.kind.as_str()),
            )
            .detail(match &it.action {
                Action::Create => "created".to_string(),
                Action::Replace => "replaced".to_string(),
                Action::Rename(_) => format!("renamed from {}", it.name),
                Action::Skip => unreachable!("skipped above"),
            }),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, Queue) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(root.join(".trellis/agents")).unwrap();
        std::fs::create_dir_all(root.join(".trellis/routines")).unwrap();
        let q = Queue::at(d.path().join("git/trellis/queue"));
        (d, root, q)
    }

    #[test]
    fn export_then_import_elsewhere_reproduces_definitions_and_tasks() {
        let (d, root, q) = setup();
        std::fs::write(
            root.join(".trellis/agents/rev.toml"),
            "role = \"r\"\ninstructions = \"agents/rev.md\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".trellis/agents/rev.md"), "be careful").unwrap();
        std::fs::write(
            root.join(".trellis/routines/n.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\nassignee = \"rev\"\n",
        )
        .unwrap();
        let a = q.add("one", None, None).unwrap();
        let out = d.path().join("export");
        let n = export(&root, &q, &out).unwrap();
        assert_eq!(
            (n.agents, n.instructions, n.routines, n.tasks),
            (1, 1, 1, 1)
        );
        assert!(
            export(&root, &q, &out).is_err(),
            "never into a non-empty folder"
        );

        let (_d2, root2, q2) = setup();
        let plan = plan_import(&root2, &q2, &out, OnConflict::Skip).unwrap();
        assert!(plan.items.iter().all(|i| i.action == Action::Create));
        apply_import(&root2, &q2, &plan).unwrap();
        assert!(root2.join(".trellis/agents/rev.md").exists());
        assert_eq!(q2.ids(State::Pending).unwrap(), vec![a]);
    }

    #[test]
    fn rename_on_conflict_rewrites_references_inside_the_imported_set() {
        // The risk: renaming an incoming profile but leaving its tasks
        // assigned to the *local* profile of the old name — work would run
        // them with somebody else's model, budget and instructions.
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/rev.toml"), "role = \"mine\"\n").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::create_dir_all(src.join("tasks/pending")).unwrap();
        std::fs::write(src.join("agents/rev.toml"), "role = \"theirs\"\n").unwrap();
        std::fs::write(src.join("agents/junior.toml"), "reports_to = \"rev\"\n").unwrap();
        let t = Task {
            task: "x".into(),
            added_at: "2026-09-27T00:00:00Z".into(),
            assignee: Some("rev".into()),
            ..Task::default()
        };
        std::fs::write(
            src.join("tasks/pending/20260927-000000-000000001-x.json"),
            serde_json::to_string(&t).unwrap(),
        )
        .unwrap();

        let plan = plan_import(&root, &q, &src, OnConflict::Rename).unwrap();
        assert!(
            plan.items
                .iter()
                .any(|i| i.name == "rev" && i.action == Action::Rename("rev-2".into()))
        );
        apply_import(&root, &q, &plan).unwrap();
        let (_, got) = q.find("20260927-000000-000000001-x").unwrap();
        assert_eq!(got.assignee.as_deref(), Some("rev-2"));
        let junior = std::fs::read_to_string(root.join(".trellis/agents/junior.toml")).unwrap();
        assert!(junior.contains("rev-2"), "{junior}");
        assert!(
            std::fs::read_to_string(root.join(".trellis/agents/rev.toml"))
                .unwrap()
                .contains("mine"),
            "the local profile is untouched"
        );
    }

    #[test]
    fn replace_over_a_claimed_task_is_refused_before_anything_is_written() {
        // The risk: `--on-conflict replace` against a task another process
        // has claimed was only discovered in `apply_import`, after the
        // profiles ahead of it had been written: half an import, and an
        // error saying nothing was done.
        let (d, root, q) = setup();
        let id = q.add("local", None, None).unwrap();
        let claim = q.claim(&id).unwrap().expect("claimed");
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::create_dir_all(src.join("tasks/pending")).unwrap();
        std::fs::write(src.join("agents/newcomer.toml"), "role = \"x\"\n").unwrap();
        let t = Task {
            task: "theirs".into(),
            added_at: "2026-09-27T00:00:00Z".into(),
            ..Task::default()
        };
        std::fs::write(
            src.join(format!("tasks/pending/{id}.json")),
            serde_json::to_string(&t).unwrap(),
        )
        .unwrap();

        let e = plan_import(&root, &q, &src, OnConflict::Replace)
            .map(|p| apply_import(&root, &q, &p))
            .and_then(|r| r)
            .expect_err("a claimed task cannot be replaced");
        assert!(e.to_string().contains(&id), "{e}");
        assert!(
            !root.join(".trellis/agents/newcomer.toml").exists(),
            "nothing is written when the import is refused"
        );
        claim.release(false).unwrap();
    }

    #[test]
    fn profiles_reporting_to_a_manager_only_this_repository_has_import() {
        // The risk: the incoming folder judged on its own, where `lead` is
        // missing, refusing an import whose result is a valid set.
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/lead.toml"), "").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(src.join("agents/dev.toml"), "reports_to = \"lead\"\n").unwrap();
        let plan = plan_import(&root, &q, &src, OnConflict::Skip).unwrap();
        assert!(
            plan.items
                .iter()
                .any(|i| i.name == "dev" && i.action == Action::Create)
        );
    }

    #[test]
    fn an_incoming_profile_is_judged_by_the_set_it_would_leave() {
        // A profile skipped on conflict is not written, so it cannot make
        // the result invalid; replacing with it does.
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/a.toml"), "").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(src.join("agents/a.toml"), "reports_to = \"a\"\n").unwrap();
        let plan = plan_import(&root, &q, &src, OnConflict::Skip).unwrap();
        assert!(plan.items.iter().all(|i| i.action == Action::Skip));
        let e = plan_import(&root, &q, &src, OnConflict::Replace).unwrap_err();
        assert!(e.to_string().contains("itself"), "{e}");
    }

    #[test]
    fn an_import_that_would_create_a_cycle_writes_nothing() {
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/a.toml"), "reports_to = \"b\"\n").unwrap();
        std::fs::write(root.join(".trellis/agents/b.toml"), "").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(src.join("agents/b.toml"), "reports_to = \"a\"\n").unwrap();
        let e = plan_import(&root, &q, &src, OnConflict::Replace).unwrap_err();
        assert!(e.to_string().contains("cycle"), "{e}");
        assert_eq!(
            std::fs::read_to_string(root.join(".trellis/agents/b.toml")).unwrap(),
            ""
        );
    }

    #[test]
    fn conflict_policies_and_item_kinds_have_the_names_the_cli_and_the_log_use() {
        for (s, on) in [
            ("skip", OnConflict::Skip),
            ("replace", OnConflict::Replace),
            ("rename", OnConflict::Rename),
        ] {
            assert_eq!(OnConflict::parse(s), Some(on));
        }
        assert_eq!(OnConflict::parse("merge"), None);
        assert_eq!(
            [Kind::Agent, Kind::Routine, Kind::Task].map(Kind::as_str),
            ["agent", "routine", "task"]
        );
    }

    fn pending_task(src: &Path, id: &str, text: &str) {
        let t = Task {
            task: text.into(),
            added_at: "2026-09-27T00:00:00Z".into(),
            ..Task::default()
        };
        std::fs::create_dir_all(src.join("tasks/pending")).unwrap();
        std::fs::write(
            src.join(format!("tasks/pending/{id}.json")),
            serde_json::to_string(&t).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn export_counts_the_claimed_tasks_it_leaves_behind() {
        let (d, root, q) = setup();
        let id = q.add("held", None, None).unwrap();
        let claim = q.claim(&id).unwrap().unwrap();
        let n = export(&root, &q, &d.path().join("out")).unwrap();
        assert_eq!((n.tasks, n.claimed_skipped), (0, 1));
        claim.release(false).unwrap();
    }

    #[test]
    fn export_counts_a_pause_routine_state_and_request_it_leaves_behind() {
        // The risk: a profile paused only by its overlay is exported as a
        // file that does not say `paused`, so an import elsewhere runs it,
        // and the export said nothing about the pause or the other runtime
        // state it did not carry.
        let (d, root, q) = setup();
        for n in ["held", "stopped", "both"] {
            std::fs::write(root.join(format!(".trellis/agents/{n}.toml")), "").unwrap();
        }
        std::fs::write(root.join(".trellis/agents/both.toml"), "paused = true\n").unwrap();
        std::fs::write(root.join(".trellis/agents/idle.toml"), "").unwrap();
        let pause = |by| profile::Pause {
            by,
            at: crate::iso_now(),
            reason: None,
            window: None,
            month: None,
        };
        for (n, by) in [
            ("held", profile::PausedBy::Human),
            ("stopped", profile::PausedBy::Budget),
            ("both", profile::PausedBy::Human),
        ] {
            profile::update_overlay(&q, n, |o| o.paused = Some(pause(by))).unwrap();
        }
        // An overlay emptied by an unpause carries nothing to leave behind.
        profile::update_overlay(&q, "idle", |o| o.paused = None).unwrap();
        std::fs::write(
            root.join(".trellis/routines/nightly.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".trellis/routines/fresh.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"y\"\n",
        )
        .unwrap();
        let routines = routine::load_all(&root).unwrap();
        routine::evaluate(&q, &routines["nightly"], 1_790_000_000, &routine::NoGit).unwrap();
        crate::approval::open_budget_request(
            &q,
            "stopped",
            profile::Window::Monthly,
            "2026-10",
            120,
            100,
            Actor::Human,
        )
        .unwrap();
        let lines = q.activity().tail(usize::MAX).unwrap().len();

        let n = export(&root, &q, &d.path().join("out")).unwrap();
        assert_eq!(
            n.paused_by_overlay,
            vec!["held".to_string(), "stopped".to_string()],
            "`both` is paused by its file, which the export carries"
        );
        assert_eq!(
            (
                n.overlays,
                n.routine_states,
                n.approval_requests,
                n.activity_lines
            ),
            (3, 1, 1, lines)
        );
        assert!(lines >= 1, "the request was logged");
    }

    #[test]
    fn export_counts_nothing_left_behind_for_a_fresh_queue() {
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/rev.toml"), "").unwrap();
        std::fs::write(
            root.join(".trellis/routines/nightly.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\n",
        )
        .unwrap();
        let n = export(&root, &q, &d.path().join("out")).unwrap();
        assert_eq!((n.agents, n.routines), (1, 1));
        assert_eq!(
            n,
            Exported {
                agents: 1,
                routines: 1,
                ..Exported::default()
            }
        );
    }

    #[test]
    fn an_import_folder_that_cannot_be_read_or_a_file_that_is_not_a_task_id_is_refused() {
        let (d, root, q) = setup();
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("agents"), "").unwrap();
        assert!(plan_import(&root, &q, &src, OnConflict::Skip).is_err());

        let src = d.path().join("src2");
        pending_task(&src, "Not_An_Id", "x");
        let e = plan_import(&root, &q, &src, OnConflict::Skip).unwrap_err();
        assert!(e.to_string().contains("is not a task id"), "{e}");
    }

    #[test]
    fn a_rename_takes_the_first_name_free_here_and_in_the_import() {
        // `rev-2` is taken here, so the incoming `rev` is `rev-3`; read as
        // free, `rev-2` would be written over the local profile.
        let (d, root, q) = setup();
        for n in ["rev", "rev-2"] {
            std::fs::write(root.join(format!(".trellis/agents/{n}.toml")), "").unwrap();
            std::fs::write(
                root.join(format!(".trellis/routines/{n}.toml")),
                "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\n",
            )
            .unwrap();
        }
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::create_dir_all(src.join("routines")).unwrap();
        std::fs::write(src.join("agents/rev.toml"), "").unwrap();
        std::fs::write(
            src.join("routines/rev.toml"),
            "schedule = \"0 4 * * *\"\n[task]\ntext = \"y\"\n",
        )
        .unwrap();
        let id = q.add("local", None, None).unwrap();
        pending_task(&src, &id, "theirs");
        let plan = plan_import(&root, &q, &src, OnConflict::Rename).unwrap();
        let renamed = |kind: Kind| {
            plan.items
                .iter()
                .find(|i| i.kind == kind)
                .map(|i| i.action.clone())
        };
        assert_eq!(renamed(Kind::Agent), Some(Action::Rename("rev-3".into())));
        assert_eq!(renamed(Kind::Routine), Some(Action::Rename("rev-3".into())));
        let Some(Action::Rename(fresh)) = renamed(Kind::Task) else {
            panic!("{:?}", plan.items);
        };
        assert!(fresh != id && fresh.ends_with("-theirs"), "{fresh}");
        assert!(valid_id(&fresh), "{fresh}");
    }

    #[test]
    fn a_new_profile_may_not_overwrite_a_different_local_instructions_file() {
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/a.md"), "mine").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::create_dir_all(src.join("instructions/agents")).unwrap();
        std::fs::write(
            src.join("agents/b.toml"),
            "instructions = \"agents/a.md\"\n",
        )
        .unwrap();
        std::fs::write(src.join("instructions/agents/a.md"), "theirs").unwrap();
        let e = plan_import(&root, &q, &src, OnConflict::Skip).unwrap_err();
        assert!(e.to_string().contains("different content"), "{e}");
        std::fs::write(src.join("instructions/agents/a.md"), "mine").unwrap();
        plan_import(&root, &q, &src, OnConflict::Skip).unwrap();
    }

    #[test]
    fn a_task_claimed_here_is_skipped_on_conflict_not_refused() {
        // Only `replace` would touch the claimed task; `skip` leaves it.
        let (d, root, q) = setup();
        let id = q.add("local", None, None).unwrap();
        let claim = q.claim(&id).unwrap().unwrap();
        let src = d.path().join("src");
        pending_task(&src, &id, "theirs");
        let plan = plan_import(&root, &q, &src, OnConflict::Skip).unwrap();
        assert!(plan.items.iter().all(|i| i.action == Action::Skip));
        claim.release(false).unwrap();
    }

    #[test]
    fn a_skipped_profile_does_not_shift_which_task_is_written_under_which_id() {
        let (d, root, q) = setup();
        std::fs::write(root.join(".trellis/agents/rev.toml"), "").unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(src.join("agents/rev.toml"), "").unwrap();
        pending_task(&src, "20260927-000000-000000001-x", "x");
        let plan = plan_import(&root, &q, &src, OnConflict::Skip).unwrap();
        apply_import(&root, &q, &plan).unwrap();
        assert_eq!(q.find("20260927-000000-000000001-x").unwrap().1.task, "x");
    }

    /// An export holding one agent `rev` (with its instructions file
    /// `agents/rev.md`), one routine `n` and one task.
    fn an_export() -> (tempfile::TempDir, PathBuf) {
        let (d, root, q) = setup();
        std::fs::write(
            root.join(".trellis/agents/rev.toml"),
            "role = \"r\"\ninstructions = \"agents/rev.md\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".trellis/agents/rev.md"), "be careful").unwrap();
        std::fs::write(
            root.join(".trellis/routines/n.toml"),
            "schedule = \"0 3 * * *\"\n[task]\ntext = \"x\"\n",
        )
        .unwrap();
        q.add("one", None, None).unwrap();
        let out = d.path().join("export");
        export(&root, &q, &out).unwrap();
        (d, out)
    }

    /// Replaces `root/<rel>` with a link to `target`; false (the test skips)
    /// where the platform will not make one.
    fn link_at(root: &Path, rel: &str, target: &Path) -> bool {
        let link = root.join(rel);
        let _ = std::fs::remove_dir_all(&link);
        let _ = std::fs::remove_file(&link);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        match trellis_core::paths::symlink_for_test(target, &link) {
            Ok(()) => true,
            Err(why) => {
                eprintln!("{why}");
                false
            }
        }
    }

    /// Plans and applies the import of `from` into `root`, returning the
    /// path the refusal names.
    fn refused_import(root: &Path, q: &Queue, from: &Path) -> PathBuf {
        let plan = plan_import(root, q, from, OnConflict::Replace).unwrap();
        match apply_import(root, q, &plan) {
            Err(QueueError::LinkInTheWay(l)) => {
                assert!(l.to_string().contains("nothing was written"), "{l}");
                l.path
            }
            other => panic!("expected the link refused, got {other:?}"),
        }
    }

    #[test]
    fn import_symlink_agents_directory_linked_outside_is_refused_and_its_files_untouched() {
        // #725: a committed `.trellis/agents` link to a directory outside the
        // repository, and an import with replace overwrote the files there.
        let (_e, from) = an_export();
        let (d, root, q) = setup();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("rev.toml"), "role = \"mine\"\n").unwrap();
        if !link_at(&root, ".trellis/agents", &outside) {
            return;
        }
        assert_eq!(
            refused_import(&root, &q, &from),
            root.join(".trellis/agents")
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("rev.toml")).unwrap(),
            "role = \"mine\"\n"
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        // Refused before anything was written: no task published either.
        assert!(q.ids(State::Pending).unwrap().is_empty());
        assert!(!root.join(".trellis/routines/n.toml").exists());
    }

    #[test]
    fn import_symlink_routines_directory_linked_outside_is_refused() {
        let (_e, from) = an_export();
        let (d, root, q) = setup();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        if !link_at(&root, ".trellis/routines", &outside) {
            return;
        }
        assert_eq!(
            refused_import(&root, &q, &from),
            root.join(".trellis/routines")
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(
            !root.join(".trellis/agents/rev.toml").exists(),
            "nothing written"
        );
    }

    #[test]
    fn import_symlink_dot_trellis_linked_outside_is_refused() {
        let (_e, from) = an_export();
        let (d, root, q) = setup();
        let outside = d.path().join("outside");
        std::fs::create_dir_all(outside.join("agents")).unwrap();
        std::fs::create_dir_all(outside.join("routines")).unwrap();
        if !link_at(&root, ".trellis", &outside) {
            return;
        }
        assert_eq!(refused_import(&root, &q, &from), root.join(".trellis"));
        assert_eq!(
            std::fs::read_dir(outside.join("agents")).unwrap().count(),
            0
        );
    }

    #[test]
    fn import_symlink_dangling_link_at_a_file_it_writes_creates_nothing_at_the_target() {
        // `.trellis/agents/rev.md`, the instructions file the import copies,
        // committed as a dangling link: followed, it would be created
        // outside the repository.
        let (_e, from) = an_export();
        let (d, root, q) = setup();
        let target = d.path().join("made-outside.md");
        if !link_at(&root, ".trellis/agents/rev.md", &target) {
            return;
        }
        let plan = match plan_import(&root, &q, &from, OnConflict::Replace) {
            Ok(plan) => plan,
            // Refused at planning, as an instructions file already there:
            // nothing can have been written.
            Err(_) => {
                assert!(std::fs::symlink_metadata(&target).is_err());
                return;
            }
        };
        match apply_import(&root, &q, &plan) {
            Err(QueueError::LinkInTheWay(l)) => {
                assert_eq!(l.path, root.join(".trellis/agents/rev.md"));
            }
            other => panic!("expected the link refused, got {other:?}"),
        }
        assert!(
            std::fs::symlink_metadata(&target).is_err(),
            "created through the link"
        );
    }
}
