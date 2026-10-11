//! `trellis overview`: the eagle eye, printed once (ADR 0050 §3, ADR 0025
//! §4, both amended 2026-10-07; #558).
//!
//! Four parts, each from a source that already exists, and none written:
//! - `recent`: the start screen's list (`start_screen::listed`), each
//!   repository with its newest trail's outcome;
//! - `last_run`: the current root's newest trail;
//! - `codebase`: `.trellis/index.bin`, only while its fingerprint matches
//!   the sources. It is never built or saved here: an overview that indexed
//!   would be the walk `root_from` refuses, started by a look;
//! - `board`: the queue's tasks, and PR and CI state from the user's own
//!   `gh`, when it is installed and logged in (owner, 2026-10-07).
//!
//! A root with no `.git` entry, the `current_dir()` fallback from a home
//! directory, is no root: only `recent` is shown, and nothing under it is
//! read.
//!
//! The overview is built once as JSON. `--json` prints that object with C1
//! controls escaped as `log --json` escapes them; the text is drawn from the
//! same object, every string through `plain()`, since trails, the queue and
//! `gh` all carry text someone else wrote.

use crate::output::escape_c1;
use crate::plain;
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long each `gh` may take before it is killed (#558).
const GH_BOUND: Duration = Duration::from_secs(10);

/// How long to wait for a pipe to close after `gh` has exited. Only a
/// grandchild still holding it makes this wait at all.
const DRAIN_BOUND: Duration = Duration::from_secs(2);

/// The most of one `gh` stream kept. More is read and dropped, so `gh` never
/// blocks on a full pipe; a list cut short fails to parse and says so.
const GH_OUTPUT_CAP: u64 = 8 << 20;

/// Modules in `codebase`, as rpc's `get_codebase` defaults to.
const CODEBASE_LIMIT: usize = 40;

const GH_PR_LIST: [&str; 6] = [
    "pr",
    "list",
    "--state",
    "open",
    "--json",
    "number,title,headRefName,isDraft,statusCheckRollup",
];
const GH_RUN_LIST: [&str; 6] = [
    "run",
    "list",
    "-L",
    "10",
    "--json",
    "databaseId,workflowName,headBranch,status,conclusion,createdAt",
];

/// Prints the overview of `root` to `out`, as one JSON object or as text.
pub(crate) fn run(root: &Path, as_json: bool, out: &mut dyn Write) -> Result<()> {
    let repo = root.join(".git").exists().then_some(root);
    let v = gather(repo);
    if as_json {
        writeln!(out, "{}", escape_c1(&v.to_string()))?;
    } else {
        text(&v, root, out)?;
    }
    Ok(())
}

/// The overview as one object: `recent`, and with a root `root`,
/// `last_run`, `codebase`, `codebase_reason` and `board`.
fn gather(root: Option<&Path>) -> Value {
    let mut o = Map::new();
    let (recent, recent_error) = recent();
    o.insert("recent".into(), recent);
    if let Some(e) = recent_error {
        o.insert("recent_error".into(), e.into());
    }
    if let Some(root) = root {
        // Absolute, so `--root .` names a place a reader can find again.
        let shown = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        o.insert("root".into(), shown.display().to_string().into());
        o.insert("last_run".into(), last_run(root));
        let (codebase, reason) = codebase(root);
        o.insert("codebase".into(), codebase);
        o.insert(
            "codebase_reason".into(),
            reason.map_or(Value::Null, Into::into),
        );
        o.insert("board".into(), board(root));
    }
    Value::Object(o)
}

/// The listed repositories, newest first, each with its newest trail. A list
/// that cannot be read is empty, with why beside it.
fn recent() -> (Value, Option<String>) {
    let Some(file) = crate::start_screen::recent_dirs_file() else {
        return (json!([]), None);
    };
    match crate::start_screen::listed(&file) {
        Ok((repos, _)) => {
            let list = repos
                .iter()
                .map(|r| {
                    json!({
                        "path": r.display().to_string(),
                        "last_run": newest(r, |_, _| {}),
                    })
                })
                .collect();
            (Value::Array(list), None)
        }
        Err(e) => (json!([]), Some(format!("{e:#}"))),
    }
}

/// The current root's newest trail, with what the run did beside how it
/// ended: turns, files edited, each check's result, the two coverage counts
/// and the tier.
fn last_run(root: &Path) -> Value {
    newest(root, |e, o| {
        let f = &e.trail.factual;
        let checks: Vec<Value> = f
            .check_outcomes
            .iter()
            .map(|c| json!({"kind": c.kind, "passed": c.passed}))
            .collect();
        // A trail that did not measure turns says nothing, never 0.
        o.insert(
            "turns".into(),
            f.context.as_ref().map(|c| c.turns_measured).into(),
        );
        o.insert("edits".into(), f.proposed.len().into());
        o.insert("checks".into(), checks.into());
        o.insert("verified".into(), f.mode_counts.verified.into());
        o.insert("reviewed".into(), f.mode_counts.reviewed.into());
        o.insert(
            "tier".into(),
            e.trail
                .provenance
                .as_ref()
                .and_then(|p| p.tier.clone())
                .into(),
        );
    })
}

/// `null` with no trail, `{"error": …}` when the newest will not parse, or
/// its `run_id`, `started_at`, `outcome`, `integrity` and `task`, plus what
/// `more` adds.
fn newest(
    root: &Path,
    more: impl Fn(&trellis_trail::query::Entry, &mut Map<String, Value>),
) -> Value {
    match trellis_trail::query::latest_entry(root) {
        Ok(None) => Value::Null,
        Err(e) => json!({ "error": e.to_string() }),
        Ok(Some(e)) => {
            let mut o = Map::new();
            o.insert("run_id".into(), e.run_id().into());
            o.insert("started_at".into(), e.trail.started_at.clone().into());
            o.insert("outcome".into(), e.outcome().into());
            o.insert(
                "integrity".into(),
                match e.integrity {
                    trellis_trail::Integrity::Intact => "intact",
                    trellis_trail::Integrity::Altered => "altered",
                    trellis_trail::Integrity::Unrecorded => "unrecorded",
                }
                .into(),
            );
            o.insert("task".into(), e.trail.task.clone().into());
            more(&e, &mut o);
            Value::Object(o)
        }
    }
}

/// `codebase_json`'s shape from the cached index when it is current; else
/// `null` and why. Reading the sources is what the fingerprint costs; the
/// parse and resolution an index needs are never run here.
fn codebase(root: &Path) -> (Value, Option<&'static str>) {
    let path = root.join(".trellis").join("index.bin");
    if !path.exists() {
        return (Value::Null, Some("not indexed; run trellis index"));
    }
    let sources = trellis_index::read_sources(root);
    match trellis_index::cache::load(&path, trellis_index::cache::fingerprint(&sources)) {
        Some(index) => (
            crate::doctor_cmd::graph_json(&trellis_index::module_graph(&index), CODEBASE_LIMIT),
            None,
        ),
        None => (Value::Null, Some("index is stale; run trellis index")),
    }
}

/// The queue's tasks as `queue_json` gives them, and `pr_ci`. A queue that
/// cannot be read leaves `tasks` null with why; `pr_ci` is unaffected.
fn board(root: &Path) -> Value {
    let mut b = Map::new();
    match crate::work::queue_json(root) {
        Ok(mut q) => {
            b.insert("tasks".into(), q["tasks"].take());
        }
        Err(e) => {
            b.insert("tasks".into(), Value::Null);
            b.insert("tasks_error".into(), e.to_string().into());
        }
    }
    b.insert("pr_ci".into(), pr_ci(root));
    Value::Object(b)
}

/// PR and CI state from the user's own `gh`, read-only, in `root`. Any
/// failure is `{"available": false, "reason": …}`; the rest of the overview
/// does not depend on it.
fn pr_ci(root: &Path) -> Value {
    let unavailable = |reason: String| json!({"available": false, "reason": reason});
    let Some(gh) = find_gh() else {
        return unavailable("gh not found on PATH".into());
    };
    let prs = match gh_list(&gh, root, &GH_PR_LIST) {
        Ok(v) => v,
        Err(r) => return unavailable(r),
    };
    let runs = match gh_list(&gh, root, &GH_RUN_LIST) {
        Ok(v) => v,
        Err(r) => return unavailable(r),
    };
    let pull_requests: Vec<Value> = prs
        .iter()
        .map(|p| {
            let mut checks = [0u64; 4];
            for c in p["statusCheckRollup"].as_array().into_iter().flatten() {
                checks[bucket(c)] += 1;
            }
            json!({
                "number": p["number"].as_u64(),
                "title": p["title"].as_str(),
                "head": p["headRefName"].as_str(),
                "draft": p["isDraft"].as_bool(),
                "checks": {
                    "passed": checks[0],
                    "failed": checks[1],
                    "pending": checks[2],
                    "other": checks[3],
                },
            })
        })
        .collect();
    let runs: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "id": r["databaseId"].as_u64(),
                "workflow": r["workflowName"].as_str(),
                "branch": r["headBranch"].as_str(),
                "status": r["status"].as_str(),
                "conclusion": r["conclusion"].as_str(),
                "created_at": r["createdAt"].as_str(),
            })
        })
        .collect();
    json!({"available": true, "pull_requests": pull_requests, "runs": runs})
}

/// Which count a PR check falls in: 0 passed, 1 failed, 2 pending, 3 other
/// (neutral, skipped, a state this does not know, or not a check at all). A status context
/// carries `state`; a check run carries `status`, and `conclusion` once it
/// has completed.
fn bucket(c: &Value) -> usize {
    if !c.is_object() {
        return 3;
    }
    let field = |k: &str| c[k].as_str().unwrap_or("").to_ascii_uppercase();
    let state = field("state");
    let verdict = if !state.is_empty() {
        state
    } else if field("status") != "COMPLETED" {
        return 2;
    } else {
        field("conclusion")
    };
    match verdict.as_str() {
        "SUCCESS" => 0,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            1
        }
        "PENDING" | "EXPECTED" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" => 2,
        _ => 3,
    }
}

/// The first `gh` on PATH, directory by directory: on Windows `gh.exe`, then
/// `gh.cmd`, in each. Only absolute entries, so an empty or relative one
/// cannot pick up a `gh` from the repository being looked at.
fn find_gh() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) {
        &["gh.exe", "gh.cmd"]
    } else {
        &["gh"]
    };
    std::env::split_paths(&path)
        .filter(|d| d.is_absolute())
        .find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// `gh <args>` in `root`, bounded, read as a JSON list; or why not, in words
/// the board shows.
fn gh_list(gh: &Path, root: &Path, args: &[&str]) -> Result<Vec<Value>, String> {
    let what = format!("gh {} {}", args[0], args[1]);
    match bounded(gh, root, args) {
        Ran::NotStarted(e) => Err(format!("{what} could not be started: {e}")),
        Ran::TimedOut => Err(format!("{what} timed out after {} s", GH_BOUND.as_secs())),
        Ran::Exited {
            code,
            stdout,
            stderr,
        } => {
            let err = String::from_utf8_lossy(&stderr);
            // gh exits 4 when it needs authentication, and says to log in.
            if code == Some(4) || err.contains("gh auth login") {
                return Err("gh is not logged in; run gh auth login".into());
            }
            if code != Some(0) {
                let line = err.lines().map(str::trim).find(|l| !l.is_empty());
                let line: String = line.unwrap_or("no message").chars().take(200).collect();
                return Err(format!(
                    "{what} failed (exit {}): {line}",
                    code.map_or("unknown".into(), |c| c.to_string())
                ));
            }
            match serde_json::from_slice::<Value>(&stdout) {
                Ok(Value::Array(list)) => Ok(list),
                _ => Err(format!("{what} did not print a JSON list")),
            }
        }
    }
}

/// How one bounded child ended.
enum Ran {
    NotStarted(std::io::Error),
    TimedOut,
    Exited {
        code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

/// Runs `program args` in `cwd` with no stdin, the key variables removed,
/// for at most [`GH_BOUND`]. Both pipes are drained on threads, so a large
/// list cannot fill one and stall the child. One that runs over is killed,
/// with what it started, and reaped; nothing else is touched.
fn bounded(program: &Path, cwd: &Path, args: &[&str]) -> Ran {
    let mut c = Command::new(program);
    c.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so a timeout reaches what it started
    // (`trellis_core::process::kill_tree`).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    trellis_core::secrets::scrub(&mut c);
    let mut child = match c.spawn() {
        Ok(child) => child,
        Err(e) => return Ran::NotStarted(e),
    };
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ran::Exited {
                    code: status.code(),
                    stdout: stdout.recv_timeout(DRAIN_BOUND).unwrap_or_default(),
                    stderr: stderr.recv_timeout(DRAIN_BOUND).unwrap_or_default(),
                };
            }
            Ok(None) if started.elapsed() < GH_BOUND => {
                std::thread::sleep(Duration::from_millis(20));
            }
            // Over the bound, or a wait that failed: either way it is
            // stopped here rather than left running.
            waited => {
                trellis_core::process::kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return match waited {
                    Err(e) => Ran::NotStarted(e),
                    _ => Ran::TimedOut,
                };
            }
        }
    }
}

/// Reads `r` to its end on a thread, keeping at most [`GH_OUTPUT_CAP`].
fn drain(r: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut r) = r {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let _ = r.by_ref().take(GH_OUTPUT_CAP).read_to_end(&mut kept);
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            let _ = tx.send(kept);
        });
    }
    rx
}

/// A string field, made safe for a terminal; `fallback` when absent.
fn s(v: &Value, fallback: &str) -> String {
    plain(v.as_str().unwrap_or(fallback))
}

/// The overview as text, from the object `gather` built.
fn text(v: &Value, root: &Path, out: &mut dyn Write) -> Result<()> {
    let recent = v["recent"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let Some(e) = v["recent_error"].as_str() {
        writeln!(out, "recent repositories: unreadable: {}", plain(e))?;
    } else if recent.is_empty() {
        writeln!(out, "recent repositories: none recorded")?;
    } else {
        writeln!(out, "recent repositories:")?;
        for r in recent {
            writeln!(out, "  {}", s(&r["path"], "?"))?;
            let last = &r["last_run"];
            if last.is_null() {
                writeln!(out, "    no runs yet")?;
            } else if let Some(e) = last["error"].as_str() {
                writeln!(out, "    newest trail unreadable: {}", plain(e))?;
            } else {
                writeln!(
                    out,
                    "    last run {}  outcome: {}  integrity: {}",
                    s(&last["run_id"], "?"),
                    s(&last["outcome"], "?"),
                    s(&last["integrity"], "?")
                )?;
                writeln!(
                    out,
                    "    {}  {}",
                    s(&last["started_at"], "start not recorded"),
                    s(&last["task"], "")
                )?;
            }
        }
    }
    writeln!(out)?;
    if v.get("root").is_none() {
        writeln!(
            out,
            "{} is not inside a git repository, so only the recent list is shown.",
            plain(&root.display().to_string())
        )?;
        return Ok(());
    }
    writeln!(out, "repository: {}", s(&v["root"], "?"))?;
    last_run_text(&v["last_run"], out)?;
    match &v["codebase"] {
        Value::Null => writeln!(out, "codebase: {}", s(&v["codebase_reason"], "?"))?,
        c => writeln!(
            out,
            "codebase: {} module(s) indexed; {} bound call site(s), {} ambiguous, {} unresolved",
            c["modules_indexed"], c["bound_call_sites"], c["ambiguous"], c["unresolved"]
        )?,
    }
    board_text(&v["board"], out)
}

fn last_run_text(last: &Value, out: &mut dyn Write) -> Result<()> {
    if last.is_null() {
        writeln!(out, "last run: no runs yet")?;
        return Ok(());
    }
    if let Some(e) = last["error"].as_str() {
        writeln!(out, "last run: newest trail unreadable: {}", plain(e))?;
        return Ok(());
    }
    writeln!(
        out,
        "last run: {}  integrity: {}",
        s(&last["run_id"], "?"),
        s(&last["integrity"], "?")
    )?;
    writeln!(out, "  task: {}", s(&last["task"], ""))?;
    writeln!(out, "  started: {}", s(&last["started_at"], "not recorded"))?;
    // The run's outcome and its checks' results are two vocabularies, so
    // each keeps its own label.
    writeln!(out, "  outcome: {}", s(&last["outcome"], "?"))?;
    let checks: Vec<String> = last["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let result = if c["passed"].as_bool() == Some(true) {
                "passed"
            } else {
                "failed"
            };
            format!("{} {result}", s(&c["kind"], "?"))
        })
        .collect();
    writeln!(
        out,
        "  checks: {}",
        if checks.is_empty() {
            "none ran".to_string()
        } else {
            checks.join(", ")
        }
    )?;
    writeln!(
        out,
        "  coverage: {} verified by an executed check, {} reviewed only",
        last["verified"], last["reviewed"]
    )?;
    let turns = match last["turns"].as_u64() {
        Some(n) => n.to_string(),
        None => "not measured".into(),
    };
    writeln!(
        out,
        "  turns: {turns}  edits: {} file(s)  tier: {}",
        last["edits"],
        s(&last["tier"], "not recorded")
    )?;
    Ok(())
}

fn board_text(b: &Value, out: &mut dyn Write) -> Result<()> {
    match b["tasks"].as_array() {
        None => writeln!(
            out,
            "queue: unreadable: {}",
            s(&b["tasks_error"], "unknown error")
        )?,
        Some(tasks) => {
            writeln!(out, "queue: {} task(s)", tasks.len())?;
            for t in tasks {
                if let Some(e) = t["error"].as_str() {
                    writeln!(out, "  {}  unreadable: {}", s(&t["id"], "?"), plain(e))?;
                } else {
                    writeln!(
                        out,
                        "  {:<9} {}  {}  {}",
                        s(&t["state"], "?"),
                        s(&t["id"], "?"),
                        s(&t["priority"], "?"),
                        s(&t["task"], "")
                    )?;
                }
            }
        }
    }
    let pr_ci = &b["pr_ci"];
    if pr_ci["available"].as_bool() != Some(true) {
        writeln!(
            out,
            "PR and CI state unavailable: {}",
            s(&pr_ci["reason"], "unknown")
        )?;
        return Ok(());
    }
    let prs = pr_ci["pull_requests"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    writeln!(out, "pull requests (from gh): {} open", prs.len())?;
    for p in prs {
        let c = &p["checks"];
        writeln!(
            out,
            "  #{} {}  [{}]{}  checks: {} passed, {} failed, {} pending, {} other",
            p["number"],
            s(&p["title"], ""),
            s(&p["head"], "?"),
            if p["draft"].as_bool() == Some(true) {
                "  draft"
            } else {
                ""
            },
            c["passed"],
            c["failed"],
            c["pending"],
            c["other"]
        )?;
    }
    let runs = pr_ci["runs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    writeln!(out, "CI runs (from gh), newest first: {}", runs.len())?;
    for r in runs {
        writeln!(
            out,
            "  {}  {}  {}/{}  {}",
            s(&r["workflow"], "?"),
            s(&r["branch"], "?"),
            s(&r["status"], "?"),
            s(&r["conclusion"], "none"),
            s(&r["created_at"], "?")
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_counts_by_its_state_or_its_conclusion_once_completed() {
        let run = |status: &str, conclusion: &str| {
            bucket(&json!({"__typename": "CheckRun", "status": status, "conclusion": conclusion}))
        };
        assert_eq!(run("COMPLETED", "SUCCESS"), 0);
        assert_eq!(run("COMPLETED", "FAILURE"), 1);
        assert_eq!(run("IN_PROGRESS", ""), 2);
        assert_eq!(run("COMPLETED", "SKIPPED"), 3);
        assert_eq!(bucket(&json!({"state": "ERROR"})), 1);
        assert_eq!(bucket(&json!({"state": "PENDING"})), 2);
        assert_eq!(bucket(&json!("not an object")), 3);
    }
}
