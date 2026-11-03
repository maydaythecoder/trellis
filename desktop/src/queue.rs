//! The board's queue view, from a `get_queue` answer (#311).
//!
//! TRELLIS's queue is its own task list in the git directory (ADR 0020),
//! and each task carries one of eight statuses (`Status` in
//! `crates/trellis-queue/src/lib.rs`). colibri's scheduler panel shows four
//! counts; eight statuses do not fit four cells without summing two of
//! them, so this view has eight, one per status, and no sum of any (the
//! same reason CLAUDE.md rule 4 keeps verified and reviewed apart). DESK-15
//! made the queue data; the reducer (`state.rs`) takes `get_queue`'s answer
//! into this view, which the next `view` event carries under `queue`, and
//! `ui/queue.js` only draws it (ADR 0048 section 5).
//!
//! `counts` is a list of eight `{status, count}` entries in the order
//! `Status::as_str` lists them, not an object: the desktop's `serde_json`
//! writes an object's keys sorted, and JSON does not promise key order, so
//! the order is carried by the list.
//!
//! A task file that does not parse is listed with its id, state and the
//! queue's error, and counted nowhere: its status is not known. A status
//! outside the eight makes the whole answer bad data, so no cell is shown
//! that the desktop could not place. The view only reads: the queue's
//! writes are `trellis queue` and `trellis work` (ADR 0020 section 5), and
//! rpc_send forwards none of them. Task text, labels, waiting reasons and
//! errors are written by others, which the page writes through
//! `textContent` only (ADR 0007, S10).

use crate::asked::{self, Asked};
use crate::frames::Response;
use crate::json;
use serde_json::{Map, Value, json};

pub const COMMAND: &str = "get_queue";

/// The eight statuses, in the order `Status::as_str` lists them.
pub const STATUSES: [&str; 8] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "blocked",
    "done",
    "cancelled",
    "failed",
];

/// One task as `queue_json` gives it.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    Readable {
        id: String,
        state: String,
        status: String,
        priority: String,
        task: String,
        labels: Vec<String>,
        assignee: Option<String>,
        waiting: Option<String>,
        blockers: Vec<String>,
        parent: Option<String>,
    },
    /// A task file that does not parse: the queue's error, as text.
    Unreadable {
        id: String,
        state: String,
        error: String,
    },
}

/// The queue view.
#[derive(Debug, Default)]
pub struct Queue {
    /// `None` until an answer is taken, and after one that is refused or
    /// bad data.
    tasks: Option<Vec<Task>>,
    /// trellis's refusal, or the reason the last answer was bad data.
    error: Option<String>,
    /// The last `get_queue` the page sent; only its answer is taken, since
    /// each is answered from its own thread.
    asked: Asked,
}

// A task's fields, read as `queue_json` writes them: text, text or `null`
// (the contract's `null_when`), and lists it writes empty when there is
// none (`empty_when`).
fn text(o: &Map<String, Value>, k: &str) -> Result<String, String> {
    json::text(o, k, "a task")
}

fn text_or_null(o: &Map<String, Value>, k: &str) -> Result<Option<String>, String> {
    json::text_or_null(o, k, "a task")
}

fn texts(o: &Map<String, Value>, k: &str) -> Result<Vec<String>, String> {
    json::texts(o, k, "a task")
}

fn task(v: &Value) -> Result<Task, String> {
    let o = v.as_object().ok_or("a task is not an object")?;
    let (id, state) = (text(o, "id")?, text(o, "state")?);
    if o.contains_key("error") {
        return Ok(Task::Unreadable {
            id,
            state,
            error: text(o, "error")?,
        });
    }
    let status = text(o, "status")?;
    if !STATUSES.contains(&status.as_str()) {
        return Err(format!(
            "task {id}'s status {status:?} is not one of the eight ({})",
            STATUSES.join(", ")
        ));
    }
    Ok(Task::Readable {
        id,
        state,
        status,
        priority: text(o, "priority")?,
        task: text(o, "task")?,
        labels: texts(o, "labels")?,
        assignee: text_or_null(o, "assignee")?,
        waiting: text_or_null(o, "waiting")?,
        blockers: texts(o, "blockers")?,
        parent: text_or_null(o, "parent")?,
    })
}

/// A `get_queue` answer's `data`: every task, in the answer's order.
pub fn read(data: &Value) -> Result<Vec<Task>, String> {
    data.get("tasks")
        .and_then(Value::as_array)
        .ok_or("it has no list of tasks")?
        .iter()
        .map(task)
        .collect()
}

/// One count per status, in [`STATUSES`] order, 0 for a status no task
/// has. An unreadable task is in none.
fn counts(tasks: &[Task]) -> Vec<(&'static str, u64)> {
    STATUSES
        .iter()
        .map(|&s| {
            let n = tasks
                .iter()
                .filter(|t| matches!(t, Task::Readable { status, .. } if status == s))
                .count();
            (s, n as u64)
        })
        .collect()
}

fn task_json(t: &Task) -> Value {
    match t {
        Task::Readable {
            id,
            state,
            status,
            priority,
            task,
            labels,
            assignee,
            waiting,
            blockers,
            parent,
        } => json!({
            "id": id, "state": state, "status": status, "priority": priority,
            "task": task, "labels": labels, "assignee": assignee,
            "waiting": waiting, "blockers": blockers, "parent": parent,
        }),
        Task::Unreadable { id, state, error } => json!({"id": id, "state": state, "error": error}),
    }
}

impl Queue {
    /// The page sent `get_queue` with `id`: only that answer is taken.
    pub fn asked(&mut self, id: Option<&str>) {
        self.asked.ask(id);
    }

    /// Takes a `get_queue` response into the view, and says whether it
    /// changed. Another command's response, or an answer to an earlier
    /// question than the last one asked, changes nothing.
    pub fn answer(&mut self, r: &Response) -> bool {
        if r.command.as_deref() != Some(COMMAND) {
            return false;
        }
        if !self.asked.takes(r.id.as_deref()) {
            return false;
        }
        let read = match (r.success, &r.data) {
            (Some(true), Some(data)) => read(data),
            (Some(true), None) => Err("it has no data".into()),
            _ => {
                self.tasks = None;
                self.error = Some(asked::refusal(COMMAND, r));
                return true;
            }
        };
        match read {
            Ok(tasks) => {
                self.tasks = Some(tasks);
                self.error = None;
            }
            Err(why) => {
                self.tasks = None;
                self.error = Some(format!(
                    "get_queue answered with bad data, so nothing from it is shown: {why}"
                ));
            }
        }
        true
    }

    pub fn to_json(&self) -> Value {
        json!({
            "error": self.error,
            "counts": self.tasks.as_ref().map(|ts| {
                counts(ts)
                    .into_iter()
                    .map(|(s, n)| json!({"status": s, "count": n}))
                    .collect::<Vec<_>>()
            }),
            "tasks": self.tasks.as_ref().map(|ts| ts.iter().map(task_json).collect::<Vec<_>>()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::parse_stdout;

    const CONTRACT: &str = include_str!("../../crates/trellis-cli/tests/client_contract.json");

    fn readable(id: &str, status: &str) -> Value {
        json!({"id": id, "state": "pending", "status": status, "priority": "medium",
               "task": format!("task {id}"), "labels": [], "assignee": null,
               "waiting": null, "blockers": [], "parent": null})
    }

    fn answer(tasks: Value) -> Response {
        Response {
            command: Some(COMMAND.into()),
            success: Some(true),
            id: Some("q".into()),
            data: Some(json!({ "tasks": tasks })),
            error: None,
        }
    }

    fn view_after(r: &Response) -> Value {
        let mut q = Queue::default();
        assert!(q.answer(r));
        q.to_json()
    }

    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn queue_counts_one_cell_per_status_and_no_sum() {
        // The risk: two statuses summed into one cell, as colibri's four
        // cells would need, or a total beside the eight that reads as how
        // much work there is; or a status no task has left out rather
        // than shown as 0.
        let v = view_after(&answer(json!([
            readable("a", "todo"),
            readable("b", "todo"),
            readable("c", "in_review"),
            readable("d", "failed"),
            readable("e", "backlog"),
        ])));
        let counts = v["counts"].as_array().unwrap();
        let statuses: Vec<&str> = counts
            .iter()
            .map(|c| c["status"].as_str().unwrap())
            .collect();
        assert_eq!(
            statuses,
            [
                "backlog",
                "todo",
                "in_progress",
                "in_review",
                "blocked",
                "done",
                "cancelled",
                "failed"
            ],
            "eight cells, in Status::as_str's order"
        );
        let n: Vec<u64> = counts
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .collect();
        assert_eq!(n, [1, 2, 0, 1, 0, 0, 0, 1]);
        for c in counts {
            assert_eq!(c.as_object().unwrap().len(), 2, "{c}");
        }
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        for k in &ks {
            let k = k.to_lowercase();
            for bad in ["total", "sum", "active", "open"] {
                assert!(!k.contains(bad), "{k} in {v}");
            }
        }
        // An empty queue is eight zeros, not no cells.
        let v = view_after(&answer(json!([])));
        assert_eq!(v["counts"].as_array().unwrap().len(), 8);
        assert!(
            v["counts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["count"] == 0)
        );
        assert_eq!(v["tasks"], json!([]));
    }

    #[test]
    fn an_unreadable_task_is_listed_and_counted_nowhere() {
        // The risk: a task file that does not parse dropped from the board,
        // so a person never learns it is there; or counted under a status
        // nobody read from it.
        let why = "t3.json is not a task file: expected ident at line 1 column 2";
        let v = view_after(&answer(json!([
            readable("a", "done"),
            {"id": "t3", "state": "pending", "error": why},
        ])));
        assert_eq!(
            v["tasks"][1],
            json!({"id": "t3", "state": "pending", "error": why})
        );
        let n: u64 = v["counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum();
        assert_eq!(n, 1, "only the readable task is counted: {v}");
        assert_eq!(v["error"], Value::Null);
    }

    #[test]
    fn an_unknown_status_makes_the_answer_bad_data() {
        // The risk: a status a later trellis adds, or a hand-edited file's,
        // counted in a cell it does not belong to or quietly in none, so
        // the eight cells add up to a queue that is not this one.
        for bad in [json!("archived"), json!("Todo"), json!(3), Value::Null] {
            let mut t = readable("a", "todo");
            t["status"] = bad.clone();
            let v = view_after(&answer(json!([readable("b", "done"), t])));
            assert_eq!(v["counts"], Value::Null, "{bad}: {v}");
            assert_eq!(v["tasks"], Value::Null, "{bad}: {v}");
            let err = v["error"].as_str().unwrap();
            assert!(err.contains("bad data"), "{err}");
        }
        let mut t = readable("a", "todo");
        t["status"] = json!("archived");
        let err = view_after(&answer(json!([t])))["error"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(err.contains("\"archived\""), "the status is named: {err}");
        // Other shapes are bad data too.
        for data in [json!({}), json!({"tasks": {}}), json!({"tasks": ["x"]})] {
            let mut r = answer(json!([]));
            r.data = Some(data.clone());
            let v = view_after(&r);
            assert_eq!(v["counts"], Value::Null, "{data}");
            assert!(v["error"].as_str().unwrap().contains("bad data"), "{data}");
        }
    }

    #[test]
    fn a_refused_get_queue_shows_trellis_error_as_text() {
        // The risk: a queue trellis could not read (no git directory)
        // drawn as an empty queue, which reads as "nothing queued".
        let why = "not a git repository: /tmp/x";
        let mut q = Queue::default();
        q.answer(&answer(json!([readable("a", "todo")])));
        let r = Response {
            command: Some(COMMAND.into()),
            success: Some(false),
            id: Some("q".into()),
            data: None,
            error: Some(why.into()),
        };
        assert!(q.answer(&r));
        let v = q.to_json();
        assert_eq!(v["error"], why);
        assert_eq!(v["counts"], Value::Null);
        assert_eq!(v["tasks"], Value::Null);
    }

    #[test]
    fn queue_response_frame_becomes_the_queue_view() {
        // The risk: the page given the queue as a raw frame to count
        // itself (ADR 0048 section 5), or a queue answer that disturbs the
        // run panes beside it.
        use crate::state::{Changed, DesktopState};
        let mut s = DesktopState::default();
        s.update(parse_stdout("a transcript line"));
        let before = s.view(Changed::everything());
        let line = json!({"type": "response", "command": "get_queue", "success": true,
                          "id": "desktop-queue-1",
                          "data": {"tasks": [readable("a", "blocked")]}});
        let c = s.update(parse_stdout(&line.to_string()));
        let v = s.view(c);
        assert_eq!(
            v.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["queue"],
            "only the queue view: {v}"
        );
        assert_eq!(
            v["queue"]["counts"][4],
            json!({"status": "blocked", "count": 1})
        );
        assert_eq!(v["queue"]["tasks"][0]["id"], "a");
        let mut after = s.view(Changed::everything());
        let mut before = before;
        before.as_object_mut().unwrap().remove("queue");
        after.as_object_mut().unwrap().remove("queue");
        assert_eq!(after, before, "the run panes are as they were");
    }

    #[test]
    fn only_the_last_get_queue_asked_is_answered_in_the_view() {
        // The risk: two reads in flight, each answered from its own
        // thread, and the older queue drawn over the newer.
        let mut q = Queue::default();
        q.asked(Some("q1"));
        q.asked(Some("q2"));
        let mut newer = answer(json!([readable("new", "todo")]));
        newer.id = Some("q2".into());
        let mut older = answer(json!([readable("old", "todo")]));
        older.id = Some("q1".into());
        assert!(q.answer(&newer));
        assert!(!q.answer(&older));
        assert_eq!(q.to_json()["tasks"][0]["id"], "new");
        let mut other = answer(json!([]));
        other.command = Some("get_codebase".into());
        other.id = Some("q2".into());
        assert!(!q.answer(&other), "another command's answer");
    }

    #[test]
    fn every_queue_field_in_the_contract_is_read() {
        // The risk (ADR 0048 section 6): trellis keeps a task field the
        // contract promises and the board drops it, so a column goes
        // blank while every trellis test passes. Each field gets a
        // distinct value of its shape, and the view must carry it.
        let contract: Value = serde_json::from_str(CONTRACT).expect("the contract is JSON");
        let spec = &contract["rpc"]["responses"]["get_queue"];
        let names = |k: &str| -> Vec<String> {
            spec[k]
                .as_array()
                .unwrap_or_else(|| panic!("get_queue.{k}"))
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect()
        };
        let field = |p: &str| p.strip_prefix("tasks[].").map(str::to_string);
        let mut readable = Map::new();
        let mut unreadable = Map::new();
        let mut planted = Vec::new();
        for p in names("always") {
            if let Some(f) = field(&p) {
                let (v, u) = (json!(format!("tok-{f}")), json!(format!("tok-u-{f}")));
                readable.insert(f.clone(), v.clone());
                unreadable.insert(f.clone(), u.clone());
                planted.extend([v, u]);
            }
        }
        for p in names("readable_only") {
            let f = field(&p).expect("a task field");
            let v = match f.as_str() {
                "status" => json!("in_review"),
                "labels" | "blockers" => json!([format!("tok-{f}")]),
                _ => json!(format!("tok-{f}")),
            };
            readable.insert(f, v.clone());
            planted.push(v);
        }
        for p in names("unreadable_only") {
            let f = field(&p).expect("a task field");
            let v = json!(format!("tok-{f}"));
            unreadable.insert(f, v.clone());
            planted.push(v);
        }
        // id and state on both tasks, eight readable-only, one error.
        assert!(
            planted.len() >= 13,
            "the contract lists fewer fields: {planted:?}"
        );
        let v = view_after(&answer(json!([readable, unreadable])));
        assert_eq!(v["error"], Value::Null, "{v}");
        let shown = v["tasks"].to_string();
        let lost: Vec<String> = planted
            .iter()
            .map(|x| x.to_string())
            .filter(|x| !shown.contains(x.as_str()))
            .collect();
        assert!(
            lost.is_empty(),
            "the board drops what client_contract.json lists: {lost:?}\n{v}"
        );
    }
}
