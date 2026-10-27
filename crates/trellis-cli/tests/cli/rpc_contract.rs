use super::*;

// ---------------------------------------------------------------------------
// rpc's client contract: the fields the desktop's four views read (#559)
// ---------------------------------------------------------------------------

/// Every record `trellis [--root <root>] rpc` writes for `commands`, run in
/// `cwd`, with stdin closed after the last one. Bounded: a command that
/// never answers (a walk blocked on a file it should not have opened) fails
/// the test instead of hanging it.
pub(super) fn rpc_records(
    cwd: &Path,
    root: Option<&Path>,
    commands: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    rpc_records_env(cwd, root, &[], commands)
}

/// [`rpc_records`], with `env` set on the child.
pub(super) fn rpc_records_env(
    cwd: &Path,
    root: Option<&Path>,
    env: &[(&str, &Path)],
    commands: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use std::io::{BufRead, Write};
    use std::process::Stdio;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_trellis"));
    if let Some(r) = root {
        cmd.arg("--root").arg(r);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .arg("rpc")
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary must be built before its integration tests run");
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        std::io::BufReader::new(stdout)
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(&l.unwrap()).expect("stdout is JSONL")
            })
            .collect::<Vec<_>>()
    });
    {
        let mut stdin = child.stdin.take().unwrap();
        for c in commands {
            writeln!(stdin, "{c}").unwrap();
        }
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "rpc exited {status}");
            break;
        }
        if std::time::Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rpc did not answer {commands:?} within 60 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    reader.join().unwrap()
}

/// The response to the command sent with `id`.
pub(super) fn rpc_answer<'a>(records: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    records
        .iter()
        .find(|r| r["type"] == "response" && r["id"] == id)
        .unwrap_or_else(|| panic!("no response to {id}: {records:?}"))
}

/// Whether `record` carries `path` as the contract writes one: dotted keys,
/// `[]` meaning every element of an array. Unlike `contract_carries`, an
/// empty array carries every path under it, since a module that calls
/// nothing has no edge to carry fields; the tests below show the elements
/// exist where they assert on them.
pub(super) fn carries_each(record: &serde_json::Value, path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (path, None),
    };
    let (key, each) = match head.strip_suffix("[]") {
        Some(k) => (k, true),
        None => (head, false),
    };
    let Some(v) = record.get(key) else {
        return false;
    };
    match (each, rest) {
        (false, None) => true,
        (false, Some(r)) => carries_each(v, r),
        (true, r) => v
            .as_array()
            .is_some_and(|a| a.iter().all(|e| r.is_none_or(|r| carries_each(e, r)))),
    }
}

/// The paths under one of the contract's lists.
pub(super) fn contract_paths(list: &serde_json::Value) -> Vec<&str> {
    list.as_array()
        .unwrap_or_else(|| panic!("a contract list, not {list}"))
        .iter()
        .map(|p| p.as_str().expect("a path"))
        .collect()
}

pub(super) fn client_contract() -> serde_json::Value {
    serde_json::from_str(include_str!("../client_contract.json")).expect("the contract is JSON")
}

#[test]
fn rpc_contract_get_codebase_modules_carry_their_path_relative_to_the_root() {
    // The risk: the desktop's graph view (ADR 0048 §10, amended for #557)
    // opens a module's file from `path`, and a module named only by its
    // dotted name leaves it to guess `app/core.py` from `app.core`, which is
    // wrong for every package `__init__.py` and every namespace layout.
    let d = repo("rpc-contract-codebase");
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "g", "type": "get_codebase"})],
    );
    let r = rpc_answer(&records, "g");
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    let modules = data["modules"].as_array().expect("modules");
    assert!(!modules.is_empty(), "{data}");
    for m in modules {
        let p = m["path"].as_str().unwrap_or_else(|| panic!("no path: {m}"));
        assert!(!Path::new(p).is_absolute() && !p.contains('\\'), "{m}");
        assert!(d.join(p).is_file(), "{p} is not a file under the root: {m}");
    }
    let core = modules
        .iter()
        .find(|m| m["module"] == "app.core")
        .unwrap_or_else(|| panic!("app.core: {data}"));
    assert_eq!(core["path"], "app/core.py", "{core}");
    let init = modules
        .iter()
        .find(|m| m["module"] == "app")
        .unwrap_or_else(|| panic!("app: {data}"));
    assert_eq!(init["path"], "app/__init__.py", "{init}");
    assert!(
        modules
            .iter()
            .any(|m| m["calls"].as_array().is_some_and(|c| !c.is_empty())),
        "the fixture's cross-module call is drawn: {data}"
    );

    let c = client_contract();
    let always = contract_paths(&c["rpc"]["responses"]["get_codebase"]["always"]);
    assert!(always.contains(&"modules[].path"), "{always:?}");
    let gaps: Vec<&&str> = always.iter().filter(|p| !carries_each(data, p)).collect();
    assert!(
        gaps.is_empty(),
        "get_codebase lacks what tests/client_contract.json lists: {gaps:?}\n{data}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_get_queue_tasks_carry_blockers_and_parent() {
    // The risk: the desktop's board (ADR 0048 §10, amended for #557) draws
    // which task waits on which, and with neither field on the wire it can
    // only parse `queue list`'s `[blocked by ...]` text, or draw no chains.
    // Both are always present, so a client can tell "none" from "not sent".
    let d = repo("rpc-contract-queue");
    let added = |args: &[&str]| -> String {
        let o = run(&d, args);
        assert!(o.status.success(), "{}", combined(&o));
        let s = stdout(&o);
        let line = s.lines().find(|l| l.starts_with("queued:")).expect(&s);
        line.split_whitespace().nth(1).unwrap().to_string()
    };
    let a = added(&["queue", "add", "fix the parser"]);
    let b = added(&["queue", "add", "write the docs"]);
    let child = added(&[
        "queue",
        "add",
        "then ship",
        "--parent",
        &a,
        "--blocked-by",
        &a,
        "--blocked-by",
        &b,
    ]);
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "q", "type": "get_queue"})],
    );
    let r = rpc_answer(&records, "q");
    assert_eq!(r["success"], true, "{r}");
    let tasks = r["data"]["tasks"].as_array().expect("tasks");
    let task = |id: &str| {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("{id}: {tasks:?}"))
    };
    let t = task(&child);
    assert_eq!(t["parent"], a.as_str(), "{t}");
    assert_eq!(t["blockers"], serde_json::json!([a, b]), "{t}");
    for id in [&a, &b] {
        let t = task(id);
        assert_eq!(t.get("parent"), Some(&serde_json::Value::Null), "{t}");
        assert_eq!(t["blockers"], serde_json::json!([]), "{t}");
    }

    let c = client_contract();
    let entry = &c["rpc"]["responses"]["get_queue"];
    let listed: Vec<&str> = contract_paths(&entry["always"])
        .into_iter()
        .chain(contract_paths(&entry["readable_only"]))
        .collect();
    for f in ["tasks[].blockers", "tasks[].parent"] {
        assert!(listed.contains(&f), "{entry}");
    }
    let gaps: Vec<&&str> = listed
        .iter()
        .filter(|p| !carries_each(&r["data"], p))
        .collect();
    assert!(
        gaps.is_empty(),
        "get_queue lacks what tests/client_contract.json lists: {gaps:?}\n{r}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_get_codebase_and_get_radius_refuse_a_root_with_no_git_and_index_nothing() {
    // The risk: `trellis --root ~ rpc` from the desktop's start screen, then
    // `get_codebase`, walking the whole home folder (25,147 files the last
    // time an index did it) and answering with a graph of it. The refusal is
    // answered on the reader thread, so it precedes the `get_state` sent
    // after it; that no indexing thread starts at all is held by
    // `rpc_contract_refusal_starts_no_indexing_thread` in `src/rpc.rs`.
    let d = scratch("rpc-contract-no-git");
    std::fs::write(d.join("loose.py"), "def f():\n    return g()\n").unwrap();
    let records = rpc_records(
        &d,
        Some(&d),
        &[
            serde_json::json!({"id": "g", "type": "get_codebase"}),
            serde_json::json!({"id": "r", "type": "get_radius", "symbol": "f"}),
            serde_json::json!({"id": "s", "type": "get_state"}),
        ],
    );
    let at = |id: &str| {
        records
            .iter()
            .position(|r| r["type"] == "response" && r["id"] == id)
            .unwrap_or_else(|| panic!("no response to {id}: {records:?}"))
    };
    assert!(at("g") < at("s") && at("r") < at("s"), "{records:?}");
    for (id, command) in [("g", "get_codebase"), ("r", "get_radius")] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["command"], command, "{r}");
        assert_eq!(r["success"], false, "{r}");
        assert!(r.get("data").is_none(), "{r}");
        let e = r["error"].as_str().unwrap_or_else(|| panic!("{r}"));
        assert!(e.contains(command), "names the command: {e}");
        assert!(e.contains("no .git"), "names the rule: {e}");
        assert!(e.contains(&d.display().to_string()), "names the root: {e}");
    }
    assert!(
        !d.join(".trellis").exists(),
        "nothing was written under the root"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_contract_lists_every_field_get_state_answers() {
    // The risk: the desktop's run view and the TUI's status bar read
    // `get_state` fields the contract never promised, so a rename passes
    // every contract test. Both directions: each field answered is listed,
    // and each field listed is answered.
    let d = repo("rpc-contract-state");
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "s", "type": "get_state"})],
    );
    let r = rpc_answer(&records, "s");
    assert_eq!(r["success"], true, "{r}");
    let answered: std::collections::BTreeSet<&str> = r["data"]
        .as_object()
        .expect("get_state's data is an object")
        .keys()
        .map(String::as_str)
        .collect();
    let c = client_contract();
    let listed: std::collections::BTreeSet<&str> =
        contract_paths(&c["rpc"]["responses"]["get_state"]["always"])
            .into_iter()
            .collect();
    assert_eq!(
        answered, listed,
        "get_state's fields and tests/client_contract.json's responses.get_state.always differ"
    );
    let _ = std::fs::remove_dir_all(&d);
}
