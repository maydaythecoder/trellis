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
    }
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

/// Carries out a plan from [`plan_import`].
pub fn apply_import(root: &Path, q: &Queue, plan: &Plan) -> Result<(), QueueError> {
    let trellis = root.join(".trellis");
    let write = |path: &Path, text: &str| -> Result<(), QueueError> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(io(d))?;
        }
        std::fs::write(path, text).map_err(io(path))
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
}
