use super::*;

// ---------------------------------------------------------------------------
// `trellis overview` (ADR 0050 §3, ADR 0025 §4, #558)
// ---------------------------------------------------------------------------

/// What `gh` the overview finds on PATH.
enum Gh {
    /// None: every PATH entry holding a `gh` is left out, so the real one on
    /// a developer's machine is never run by the suite.
    Absent,
    /// A stub, first on PATH, whose body is `windows` (a `.cmd`) or `unix`
    /// (a shell script), with `{dir}` replaced by its own directory.
    Stub {
        windows: &'static str,
        unix: &'static str,
    },
}

/// A `gh` that prints fixed JSON for `pr list` and `run list`, and records
/// the directory it was started in.
const GH_OK: Gh = Gh::Stub {
    windows: "@echo off\r\ncd > \"{dir}\\cwd.txt\"\r\n\
              if \"%1\"==\"pr\" type \"{dir}\\pr.json\"\r\n\
              if \"%1\"==\"run\" type \"{dir}\\run.json\"\r\n",
    unix: "#!/bin/sh\npwd > '{dir}/cwd.txt'\n\
           case \"$1\" in pr) /bin/cat '{dir}/pr.json';; run) /bin/cat '{dir}/run.json';; esac\n",
};

/// PATH without any directory holding a `gh`, so `git` is still found.
fn path_without_gh() -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|d| {
            !["gh", "gh.exe", "gh.cmd"]
                .iter()
                .any(|n| d.join(n).is_file())
        })
        .collect()
}

/// On Unix `gh` often shares `/usr/bin` with `git` and `kill`, so leaving
/// out the directories that hold a `gh` loses them too: the queue needs
/// `git`, and a timed-out `gh`'s group is killed with `kill`. A directory
/// holding only wrappers that run the real ones puts them back; the stubs
/// name `/bin/cat` and `/bin/sleep` for the same reason. `None` on Windows,
/// where `gh` has a directory of its own.
fn tools_beside_gh(name: &str) -> Option<Scratch> {
    if cfg!(windows) {
        return None;
    }
    let kept = path_without_gh();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dir = scratch(&format!("{name}-tools"));
    for tool in ["git", "kill"] {
        if kept.iter().any(|d| d.join(tool).is_file()) {
            continue;
        }
        let Some(real) = std::env::split_paths(&path)
            .map(|d| d.join(tool))
            .find(|p| p.is_file())
        else {
            continue;
        };
        let p = dir.join(tool);
        let body = format!("#!/bin/sh\nexec '{}' \"$@\"\n", real.display());
        std::fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    Some(dir)
}

/// The stub's directory: `gh.cmd` or `gh`, beside the JSON it prints.
fn gh_stub(name: &str, windows: &str, unix: &str) -> Scratch {
    let dir = scratch(&format!("{name}-gh"));
    std::fs::write(
        dir.join("pr.json"),
        r#"[{"number":42,"title":"teach the board","headRefName":"t558-overview","isDraft":true,
            "statusCheckRollup":[
              {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
              {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE"},
              {"__typename":"StatusContext","context":"ci/x","state":"PENDING"}]}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("run.json"),
        r#"[{"databaseId":9001,"workflowName":"CI","headBranch":"t558-overview",
             "status":"completed","conclusion":"success","createdAt":"2026-10-07T10:00:00Z"}]"#,
    )
    .unwrap();
    let d = dir.display().to_string();
    if cfg!(windows) {
        std::fs::write(dir.join("gh.cmd"), windows.replace("{dir}", &d)).unwrap();
    } else {
        let p = dir.join("gh");
        std::fs::write(&p, unix.replace("{dir}", &d)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    dir
}

/// `trellis <args>` in `cwd`, with `user` as its user directory and the
/// `gh` that `gh` names on PATH.
fn overview_in(name: &str, cwd: &Path, user: &Path, gh: Gh, args: &[&str]) -> Output {
    overview_with_stub(name, cwd, user, gh, args).0
}

/// [`overview_in`], with the stub's directory when `gh` is one, for a test
/// that reads what the stub wrote there. The wrappers' directory goes when
/// this returns: `output()` has waited for trellis, and trellis for its
/// `gh`.
fn overview_with_stub(
    name: &str,
    cwd: &Path,
    user: &Path,
    gh: Gh,
    args: &[&str],
) -> (Output, Option<Scratch>) {
    let mut dirs = path_without_gh();
    let tools = tools_beside_gh(name);
    if let Some(tools) = &tools {
        dirs.insert(0, tools.to_path_buf());
    }
    let stub = match gh {
        Gh::Stub { windows, unix } => Some(gh_stub(name, windows, unix)),
        Gh::Absent => None,
    };
    if let Some(stub) = &stub {
        dirs.insert(0, stub.to_path_buf());
    }
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env("TRELLIS_CONFIG_DIR", user)
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .expect("the binary is built before its tests");
    (out, stub)
}

/// The one JSON object `--json overview` prints: the whole of stdout.
fn overview_json(o: &Output) -> serde_json::Value {
    assert!(o.status.success(), "{}", combined(o));
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON value ({e}):\n{}", combined(o)))
}

/// A user directory whose `recent.json` lists `repos`, newest first.
fn user_dir_listing(name: &str, repos: &[&Path]) -> Scratch {
    let dir = scratch(&format!("{name}-user"));
    let list: Vec<String> = repos.iter().map(|r| r.display().to_string()).collect();
    std::fs::write(
        dir.join("recent.json"),
        serde_json::json!({"version": 1, "repositories": list}).to_string(),
    )
    .unwrap();
    dir
}

/// A trail at `<root>/.trellis/runs/<stamp>-run.json` asking `task`: two
/// passing checks, two files proposed, five turns, 3 verified and 1
/// reviewed, tier 2.
fn write_trail(root: &Path, stamp: &str, task: &str) {
    let runs = root.join(".trellis").join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let trail = serde_json::json!({
        "schema": 1,
        "run_id": format!("{stamp}-run"),
        "task": task,
        "base_commit": "abc1234",
        "started_at": "2026-10-07T09:30:00Z",
        "provenance": {"trellis_version": "0.0.1", "model": null, "tier": "2"},
        "factual": {
            "proposed": ["app/a.py", "app/b.py"],
            "radius": {"reached_resolved": 0, "reached_ambiguous": 0,
                       "widest_unbound_in_touched_files": 0, "radius_queries": 0},
            "checks": [],
            "check_outcomes": [{"kind": "Typecheck", "passed": true},
                               {"kind": "Tests", "passed": true}],
            "approved_by": "Human",
            "mode_counts": {"verified": 3, "reviewed": 1},
            "context": {"first_prompt_bytes": 1, "peak_prompt_bytes": 2, "turns_measured": 5},
            "actions": []
        },
        "factual_sha256": "0000",
        "judgment": {"tech_debt_projection": null, "rationale": null}
    });
    std::fs::write(runs.join(format!("{stamp}-run.json")), trail.to_string()).unwrap();
}

#[test]
fn overview_outside_any_repository_exits_0_and_prints_only_recent() {
    // ADR 0050 §3, amended: the overview runs with or without a repository.
    // Outside one there is no current root, so the list is all there is.
    let here = scratch("overview-outside");
    let listed = repo("overview-outside-listed");
    write_trail(&listed, "20261007-093000-000", "rename the parser");
    let user = user_dir_listing("overview-outside", &[&listed]);
    let o = overview_in(
        "overview-outside",
        &here,
        &user,
        Gh::Absent,
        &["--json", "overview"],
    );
    let v = overview_json(&o);
    assert_eq!(keys(&v), ["recent"], "{v}");
    assert_eq!(v["recent"].as_array().unwrap().len(), 1, "{v}");
    let text = overview_in("overview-outside", &here, &user, Gh::Absent, &["overview"]);
    assert!(text.status.success(), "{}", combined(&text));
    let s = stdout(&text);
    assert!(s.contains("rename the parser"), "{s}");
    assert!(s.contains("not inside a git repository"), "{s}");
    assert!(!s.contains("board"), "no root, so no board: {s}");
    for d in [&here, &listed, &user] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn overview_treats_a_root_without_git_as_no_root_and_reads_nothing_under_it() {
    // The `current_dir()` fallback from a home directory, here named with
    // `--root`: neither the trail nor the config under it may be read, the
    // config invalid so that reading it would fail the command.
    let tree = scratch("overview-no-git");
    write_trail(
        &tree,
        "20261007-093000-000",
        "a trail that must stay unread",
    );
    std::fs::write(tree.join(".trellis/config.toml"), "this is = = not toml").unwrap();
    let user = scratch("overview-no-git-user");
    let root = tree.display().to_string();
    let args = ["--root", root.as_str(), "--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-git",
        &tree,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(keys(&v), ["recent"], "{v}");
    let o = overview_in("overview-no-git", &tree, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        !combined(&o).contains("must stay unread"),
        "{}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_never_creates_the_index_and_says_the_repository_is_not_indexed() {
    // The overview reads `.trellis/index.bin` only when it is current, and
    // never builds or saves one: printing a picture must not write the tree.
    let d = repo("overview-no-index");
    let user = scratch("overview-no-index-user");
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-index",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["codebase"].is_null(), "{v}");
    assert_eq!(
        v["codebase_reason"], "not indexed; run trellis index",
        "{v}"
    );
    let o = overview_in("overview-no-index", &d, &user, Gh::Absent, &["overview"]);
    assert!(
        stdout(&o).contains("not indexed; run trellis index"),
        "{}",
        combined(&o)
    );
    assert!(
        !d.join(".trellis/index.bin").exists(),
        "the overview wrote an index"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_reads_a_current_index_and_calls_a_changed_tree_stale() {
    // The cache is shown only while its fingerprint matches the sources; a
    // stale one is named, never shown as current, never rebuilt.
    let d = repo("overview-index");
    let user = scratch("overview-index-user");
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-index", &d, &user, Gh::Absent, &args));
    assert!(
        v["codebase"]["modules_indexed"]
            .as_u64()
            .is_some_and(|n| n > 0),
        "{v}"
    );
    assert!(v["codebase"]["bound_call_sites"].is_u64(), "{v}");
    let s = stdout(&overview_in(
        "overview-index",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("module(s) indexed"), "{s}");
    let before = std::fs::read(d.join(".trellis/index.bin")).unwrap();
    std::fs::write(
        d.join("app/core.py"),
        "def greet(who):\n    return who + '!'\n",
    )
    .unwrap();
    let v = overview_json(&overview_in("overview-index", &d, &user, Gh::Absent, &args));
    assert!(v["codebase"].is_null(), "{v}");
    assert_eq!(
        v["codebase_reason"], "index is stale; run trellis index",
        "{v}"
    );
    assert_eq!(
        std::fs::read(d.join(".trellis/index.bin")).unwrap(),
        before,
        "the overview rewrote the index"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_indexed_by_get_codebase_shows_the_saved_index_not_run_trellis_index() {
    // #654. The risk: the desktop's Codebase section draws the graph and
    // the overview beside it says "not indexed; run trellis index" for the
    // same root, two answers to one question in one window. The owner's
    // decision of 2026-10-12: `get_codebase` saves `.trellis/index.bin`,
    // and the overview, which still never indexes, reads it. The save must
    // not dirty the worktree, or the next run refuses to start (ADR 0009).
    let d = repo("overview-indexed");
    let user = scratch("overview-indexed-user");
    let args = ["--json", "overview"];
    let before = overview_json(&overview_in(
        "overview-indexed",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(
        before["codebase_reason"], "not indexed; run trellis index",
        "{before}"
    );

    let r = get_codebase_in(&d, serde_json::json!({}));
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(data["index_saved"], true, "{data}");
    assert!(data.get("index_not_saved").is_none(), "{data}");
    let saved = std::fs::metadata(d.join(".trellis/index.bin"))
        .expect("get_codebase saved the index")
        .len();
    assert_eq!(data["index_bytes"], saved, "{data}");
    assert_eq!(porcelain_unlocked(&d), "", "the save dirtied the worktree");

    let v = overview_json(&overview_in(
        "overview-indexed",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v.get("codebase_reason").is_none_or(|r| r.is_null()), "{v}");
    assert_eq!(
        v["codebase"]["modules_indexed"], data["modules_indexed"],
        "the overview reads the index get_codebase drew: {v}"
    );
    assert_eq!(
        v["codebase"]["bound_call_sites"], data["bound_call_sites"],
        "{v}"
    );
    let s = stdout(&overview_in(
        "overview-indexed",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(!s.contains("run trellis index"), "{s}");
    assert!(s.contains("module(s) indexed"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_indexed_by_get_codebase_replaces_a_stale_cache_with_a_current_one() {
    // #654. The risk: a save that keeps the old blob, or writes one keyed
    // to the wrong fingerprint, so the overview still calls the index
    // stale after the desktop has drawn the tree as it now stands.
    let d = repo("overview-indexed-stale");
    let user = scratch("overview-indexed-stale-user");
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    std::fs::write(
        d.join("app/extra.py"),
        "from app.core import greet

def again(who):
    return greet(who)
",
    )
    .unwrap();
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-indexed-stale",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(
        v["codebase_reason"], "index is stale; run trellis index",
        "{v}"
    );

    let r = get_codebase_in(&d, serde_json::json!({}));
    assert_eq!(r["data"]["index_saved"], true, "{r}");
    let v = overview_json(&overview_in(
        "overview-indexed-stale",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v.get("codebase_reason").is_none_or(|r| r.is_null()), "{v}");
    assert_eq!(
        v["codebase"]["modules_indexed"], r["data"]["modules_indexed"],
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_indexed_is_never_a_commit_s_graph_saved_as_the_worktree_s() {
    // #654. The risk: `get_codebase` at a `ref` (the Atlas's snapshot)
    // saving that commit's index as `.trellis/index.bin`. Its fingerprint
    // would not match the worktree, so nothing would show it, but a commit
    // whose `.py` files equal the worktree's would be read as the current
    // index of a tree it was never built from; and the answer must say
    // nothing was saved rather than leave the client to guess.
    let d = repo("overview-indexed-ref");
    let r = get_codebase_in(&d, serde_json::json!({"ref": "HEAD"}));
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(r["data"]["index_saved"], false, "{r}");
    assert!(r["data"].get("index_bytes").is_none(), "{r}");
    let why = r["data"]["index_not_saved"]
        .as_str()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(why.contains(".trellis/index.bin"), "{why}");
    assert!(
        !d.join(".trellis").exists(),
        "a ref's answer wrote under .trellis"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overview_indexed_get_codebase_still_answers_its_graph_when_the_save_fails() {
    // #654. The risk: a cache that cannot be written (here `.trellis` is a
    // file, so its directory cannot be made) failing the graph the user
    // asked for, or being reported as saved. The cache is derived state:
    // the answer stands, and says why nothing was saved.
    let d = repo("overview-indexed-unsaved");
    std::fs::write(d.join(".trellis"), "not a directory\n").unwrap();
    let r = get_codebase_in(&d, serde_json::json!({}));
    assert_eq!(r["success"], true, "{r}");
    assert!(module_in(&r["data"], "app.core").is_some(), "{r}");
    assert_eq!(r["data"]["index_saved"], false, "{r}");
    let why = r["data"]["index_not_saved"]
        .as_str()
        .unwrap_or_else(|| panic!("{r}"));
    assert!(why.starts_with(".trellis/index.bin not written: "), "{why}");
    assert_eq!(
        std::fs::read_to_string(d.join(".trellis")).unwrap(),
        "not a directory\n",
        "the file in the way was left as it was"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overview_shows_a_listed_repository_s_last_outcome_and_the_current_last_run() {
    // The recent list carries each repository's last outcome (ADR 0021
    // rows 44-47, amended). The current root's last run keeps the run's
    // outcome and its checks' results under separate labels, and coverage
    // as two counts.
    let d = repo("overview-outcome");
    write_trail(&d, "20261007-080000-000", "an older run");
    write_trail(&d, "20261007-093000-000", "rename the parser");
    let user = user_dir_listing("overview-outcome", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-outcome",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    let last = &v["recent"][0]["last_run"];
    assert_eq!(last["run_id"], "20261007-093000-000-run", "{v}");
    assert_eq!(last["outcome"], "converged", "{v}");
    assert_eq!(last["task"], "rename the parser", "{v}");
    assert_eq!(last["started_at"], "2026-10-07T09:30:00Z", "{v}");
    assert!(last["integrity"].is_string(), "{v}");
    let run = &v["last_run"];
    assert_eq!(run["turns"], 5, "{v}");
    assert_eq!(run["edits"], 2, "{v}");
    assert_eq!(run["verified"], 3, "{v}");
    assert_eq!(run["reviewed"], 1, "{v}");
    assert_eq!(run["tier"], "2", "{v}");
    assert_eq!(
        run["checks"][1],
        serde_json::json!({"kind": "Tests", "passed": true}),
        "{v}"
    );
    let s = stdout(&overview_in(
        "overview-outcome",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("outcome: converged"), "{s}");
    assert!(s.contains("checks: Typecheck passed, Tests passed"), "{s}");
    assert!(
        s.contains("3 verified by an executed check, 1 reviewed only"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_shows_null_for_a_repository_with_no_runs() {
    let d = repo("overview-no-runs");
    let user = user_dir_listing("overview-no-runs", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-no-runs",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["recent"][0]["last_run"].is_null(), "{v}");
    assert!(v["last_run"].is_null(), "{v}");
    let s = stdout(&overview_in(
        "overview-no-runs",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("no runs yet"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_reports_a_corrupt_newest_trail_as_an_error_and_still_exits_0() {
    // One unreadable file must not hide the rest of the picture, nor be
    // passed over for an older trail as if it were the last run.
    let d = repo("overview-corrupt");
    write_trail(&d, "20261007-080000-000", "an older run");
    std::fs::write(
        d.join(".trellis/runs/20261007-093000-000-run.json"),
        "{ not a trail",
    )
    .unwrap();
    let user = user_dir_listing("overview-corrupt", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-corrupt",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert!(v["recent"][0]["last_run"]["error"].is_string(), "{v}");
    assert!(v["last_run"]["error"].is_string(), "{v}");
    let o = overview_in("overview-corrupt", &d, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        stdout(&o).contains("newest trail unreadable"),
        "{}",
        combined(&o)
    );
    assert!(!stdout(&o).contains("an older run"), "{}", combined(&o));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_strips_an_escape_from_trail_text_and_escapes_it_in_json() {
    // A trail is untrusted (ADR 0007): ESC and the C1 CSI in a task must not
    // reach the terminal raw, nor leave the JSON unescaped.
    let d = repo("overview-esc");
    write_trail(&d, "20261007-093000-000", "fix\u{1b}[2Jthis\u{9b}2J");
    let user = user_dir_listing("overview-esc", &[&d]);
    let o = overview_in("overview-esc", &d, &user, Gh::Absent, &["overview"]);
    let s = stdout(&o);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(!s.contains('\u{1b}') && !s.contains('\u{9b}'), "{s:?}");
    assert!(s.contains("fix\\u{1b}[2Jthis"), "{s}");
    let j = overview_in(
        "overview-esc",
        &d,
        &user,
        Gh::Absent,
        &["--json", "overview"],
    );
    let raw = String::from_utf8_lossy(&j.stdout).into_owned();
    assert!(
        !raw.contains('\u{1b}') && !raw.contains('\u{9b}'),
        "{raw:?}"
    );
    assert!(raw.contains("\\u001b") && raw.contains("\\u009b"), "{raw}");
    assert_eq!(
        overview_json(&j)["last_run"]["task"],
        "fix\u{1b}[2Jthis\u{9b}2J"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_lists_pull_requests_and_runs_from_a_gh_on_path() {
    // PR and CI state come live from the user's own `gh` (owner,
    // 2026-10-07), started in the current root.
    let d = repo("overview-gh");
    let user = scratch("overview-gh-user");
    let (o, stub) = overview_with_stub("overview-gh", &d, &user, GH_OK, &["--json", "overview"]);
    let v = overview_json(&o);
    let pr_ci = &v["board"]["pr_ci"];
    assert_eq!(pr_ci["available"], true, "{v}");
    let pr = &pr_ci["pull_requests"][0];
    assert_eq!(pr["number"], 42, "{v}");
    assert_eq!(pr["title"], "teach the board", "{v}");
    assert_eq!(pr["head"], "t558-overview", "{v}");
    assert_eq!(pr["draft"], true, "{v}");
    assert_eq!(
        pr["checks"],
        serde_json::json!({"passed": 1, "failed": 1, "pending": 1, "other": 0}),
        "{v}"
    );
    let r = &pr_ci["runs"][0];
    assert_eq!(r["id"], 9001, "{v}");
    assert_eq!(r["workflow"], "CI", "{v}");
    assert_eq!(r["branch"], "t558-overview", "{v}");
    assert_eq!(r["conclusion"], "success", "{v}");
    let stub = stub.expect("GH_OK is a stub");
    let cwd = std::fs::read_to_string(stub.join("cwd.txt")).unwrap();
    let name = d.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        cwd.trim().ends_with(&name),
        "gh started in {cwd}, not the root"
    );
    let s = stdout(&overview_in("overview-gh", &d, &user, GH_OK, &["overview"]));
    assert!(s.contains("#42 teach the board"), "{s}");
    assert!(s.contains("CI  t558-overview  completed/success"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn wait_line_overview_json_stdout_stays_one_json_object_while_gh_is_asked() {
    // #650 says what bare `trellis` waits on, on stderr and only at a
    // terminal. The risk is the desktop's `--json overview`: a line on its
    // stdout would leave a body no client can parse.
    let d = repo("overview-wait-line");
    let user = scratch("overview-wait-line-user");
    let o = overview_in(
        "overview-wait-line",
        &d,
        &user,
        GH_OK,
        &["--json", "overview"],
    );
    let v = overview_json(&o);
    assert_eq!(v["board"]["pr_ci"]["available"], true, "gh was asked: {v}");
    assert!(
        !String::from_utf8_lossy(&o.stderr).contains("asking gh"),
        "stderr is a pipe here, so nothing is said: {}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_without_gh_on_path_says_pr_ci_is_unavailable_and_exits_0() {
    let d = repo("overview-gh-absent");
    let user = scratch("overview-gh-absent-user");
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in(
        "overview-gh-absent",
        &d,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("not found")),
        "{v}"
    );
    assert!(
        v["board"]["tasks"].is_array(),
        "the rest is unaffected: {v}"
    );
    let o = overview_in("overview-gh-absent", &d, &user, Gh::Absent, &["overview"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        stdout(&o).contains("PR and CI state unavailable"),
        "{}",
        combined(&o)
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_says_gh_is_not_logged_in_when_it_asks_for_auth() {
    let d = repo("overview-gh-auth");
    let user = scratch("overview-gh-auth-user");
    let gh = Gh::Stub {
        windows: "@echo off\r\n\
                  echo To get started with GitHub CLI, please run:  gh auth login 1>&2\r\n\
                  exit /b 4\r\n",
        unix: "#!/bin/sh\n\
               echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n",
    };
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-gh-auth", &d, &user, gh, &args));
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("not logged in")),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_gives_up_on_a_gh_that_runs_past_its_bound() {
    // Each `gh` is bounded to 10 s; one that sleeps past it is killed and
    // the board says it timed out, well before the stub would have ended.
    let d = repo("overview-gh-slow");
    let user = scratch("overview-gh-slow-user");
    let gh = Gh::Stub {
        windows: "@echo off\r\nping -n 60 127.0.0.1 >nul\r\n",
        unix: "#!/bin/sh\n/bin/sleep 60\n",
    };
    let started = std::time::Instant::now();
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-gh-slow", &d, &user, gh, &args));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(v["board"]["pr_ci"]["available"], false, "{v}");
    assert!(
        v["board"]["pr_ci"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("timed out")),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

#[test]
fn overview_board_lists_the_queue_s_tasks() {
    let d = repo("overview-queue");
    let user = scratch("overview-queue-user");
    let o = run(&d, &["queue", "add", "tidy the parser"]);
    assert!(o.status.success(), "{}", combined(&o));
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-queue", &d, &user, Gh::Absent, &args));
    assert_eq!(v["board"]["tasks"][0]["task"], "tidy the parser", "{v}");
    let s = stdout(&overview_in(
        "overview-queue",
        &d,
        &user,
        Gh::Absent,
        &["overview"],
    ));
    assert!(s.contains("tidy the parser"), "{s}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&user);
}

/// Whether `record` carries `path`: dotted keys, with `[]` meaning every
/// element of a non-empty array. A key present with `null` is carried. The
/// reading `tests/modes.rs` gives the rpc and `log_json` entries.
fn contract_carries(record: &serde_json::Value, path: &str) -> bool {
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
        (false, Some(r)) => contract_carries(v, r),
        (true, r) => v.as_array().is_some_and(|a| {
            !a.is_empty() && a.iter().all(|e| r.is_none_or(|r| contract_carries(e, r)))
        }),
    }
}

#[test]
fn client_contract_overview_lists_what_the_overview_object_carries() {
    // The risk: a field the desktop reads from `--json overview` (ADR 0048
    // §2, amended) is renamed or dropped, and its view goes blank with
    // nothing failing.
    let c: serde_json::Value = serde_json::from_str(include_str!("../client_contract.json"))
        .expect("the contract is JSON");
    let entry = &c["overview"];
    let d = repo("overview-contract");
    write_trail(&d, "20261007-093000-000", "rename the parser");
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let o = run(&d, &["queue", "add", "tidy the parser"]);
    assert!(o.status.success(), "{}", combined(&o));
    let user = user_dir_listing("overview-contract", &[&d]);
    let args = ["--json", "overview"];
    let v = overview_json(&overview_in("overview-contract", &d, &user, GH_OK, &args));
    let always = entry["always"]
        .as_array()
        .expect("overview.always is a list");
    assert!(always.len() > 10, "{entry}");
    let gaps: Vec<&str> = always
        .iter()
        .map(|p| p.as_str().expect("a path"))
        .filter(|p| !contract_carries(&v, p))
        .collect();
    assert!(
        gaps.is_empty(),
        "--json overview lacks what tests/client_contract.json lists: {gaps:?}\n{v}"
    );
    let outside: Vec<&str> = entry["outside_a_repository"]
        .as_array()
        .expect("overview.outside_a_repository lists the keys without a root")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    let here = scratch("overview-contract-outside");
    let v = overview_json(&overview_in(
        "overview-contract-outside",
        &here,
        &user,
        Gh::Absent,
        &args,
    ));
    assert_eq!(keys(&v), outside, "{v}");
    for p in [&d, &user, &here] {
        let _ = std::fs::remove_dir_all(p);
    }
}
