use super::*;

// ---------------------------------------------------------------------------
// rpc for the desktop's Atlas section, the owner's request of 2026-10-11:
// "the current view is nice in the trellis atlas, could you add that to the
// current view without removing the other ones"
// ---------------------------------------------------------------------------

/// Every file under `dir`, `.git` included, by relative path with its
/// bytes: the before and after of a read that must write nothing.
fn bytes_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.insert(p.strip_prefix(dir).unwrap().to_path_buf(), bytes);
            }
        }
    }
    out
}

/// `git <args>` in `root` with the committer date set, through
/// `trellis_vcs::command()`.
fn git_dated(root: &Path, date: &str, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The committer date of `v1` in [`atlas_repo`], in a zone other than UTC.
const V1_DATE: &str = "2024-06-01T00:00:00+02:00";

/// A repository with two commits. The first, tagged `v1` (annotated),
/// holds `app/old.py`; the second deletes it and adds `app/new.py`. In
/// both, `hub.py` calls into twelve modules, two more than `get_codebase`
/// draws per module by default. Returns the root and `v1`'s commit.
fn atlas_repo(name: &str) -> (PathBuf, String) {
    let d = empty_repo(name);
    let mut hub = String::new();
    let mut body = String::from("\ndef hub():\n    return 0");
    for i in 0..12 {
        std::fs::write(
            d.join(format!("m{i}.py")),
            format!("def f{i}():\n    return {i}\n"),
        )
        .unwrap();
        hub.push_str(&format!("from m{i} import f{i}\n"));
        body.push_str(&format!(" + f{i}()"));
    }
    std::fs::write(d.join("hub.py"), format!("{hub}{body}\n")).unwrap();
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(d.join("app/core.py"), "def greet(who):\n    return who\n").unwrap();
    std::fs::write(
        d.join("app/old.py"),
        "from app.core import greet\n\ndef legacy():\n    return greet('old')\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git_dated(&d, V1_DATE, &["commit", "-qm", "first"]);
    git(&d, &["tag", "-a", "v1", "-m", "release"]);
    let v1 = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    git(&d, &["rm", "-q", "app/old.py"]);
    std::fs::write(
        d.join("app/new.py"),
        "from app.core import greet\n\ndef modern():\n    return greet('new')\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git_dated(
        &d,
        "2024-07-01T12:00:00+00:00",
        &["commit", "-qm", "second"],
    );
    (d, v1)
}

#[test]
fn rpc_atlas_get_codebase_edges_all_lists_every_module_edge() {
    // The risk: the Atlas drawing a module's edges from a list cut at ten,
    // so a hub module looks coupled to ten modules when it calls twelve,
    // and package edges summed from it undercount. Without the argument
    // the answer stays what it was (#292).
    let (d, _) = atlas_repo("rpc-atlas-edges");
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "d", "type": "get_codebase", "limit": 1000}),
            serde_json::json!({"id": "a", "type": "get_codebase", "limit": 1000, "edges": "all"}),
        ],
    );
    let drawn = rpc_answer(&records, "d");
    let all = rpc_answer(&records, "a");
    assert_eq!(drawn["success"], true, "{drawn}");
    assert_eq!(all["success"], true, "{all}");
    let (drawn, all) = (&drawn["data"], &all["data"]);
    let hub = module_in(drawn, "hub").unwrap();
    assert_eq!(hub["calls"].as_array().unwrap().len(), 10, "{hub}");
    assert_eq!(hub["calls_not_drawn"], 2, "{hub}");
    let hub = module_in(all, "hub").unwrap();
    assert_eq!(hub["calls"].as_array().unwrap().len(), 12, "{hub}");
    let mut sum = 0;
    for m in all["modules"].as_array().unwrap() {
        assert_eq!(m["calls_not_drawn"], 0, "{m}");
        for c in m["calls"].as_array().unwrap() {
            sum += c["call_sites"].as_u64().unwrap();
        }
    }
    assert_eq!(serde_json::json!(sum), all["bound_call_sites"], "{all}");
    for k in [
        "modules_indexed",
        "bound_call_sites",
        "ambiguous",
        "unresolved",
        "modules_not_drawn",
        "commit",
        "date",
    ] {
        assert_eq!(drawn[k], all[k], "{k}");
    }
    let names = |v: &serde_json::Value| -> Vec<String> {
        v["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["module"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(drawn),
        names(all),
        "the same modules in the same order"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_ref_answers_the_codebase_at_that_commit_and_writes_nothing() {
    // The risks: a snapshot indexed from the worktree rather than the
    // commit; one taken by a checkout, which rewrites the repository's
    // index, or a `worktree add`, which writes `.git/worktrees`; a copy
    // left behind in the temporary directory; and a date in this
    // machine's zone rather than the commit's.
    let (d, v1) = atlas_repo("rpc-atlas-ref");
    let tmp = scratch("rpc-atlas-ref-tmp");
    // A status first, so an index refresh it might make is not counted.
    assert_eq!(porcelain_unlocked(&d), "");
    let before = bytes_under(&d);
    let head = git(&d, &["rev-parse", "HEAD"]).trim().to_string();
    // trellis answers UTC as `+00:00` whichever spelling this machine's git
    // uses (some write `Z`), so the expectation is put in that form too.
    let head_date = git(&d, &["log", "-1", "--format=%cI", "HEAD"]);
    let head_date = match head_date.trim().strip_suffix('Z') {
        Some(rest) => format!("{rest}+00:00"),
        None => head_date.trim().to_string(),
    };
    let records = rpc_records_env(
        &d,
        None,
        &[("TMPDIR", &tmp), ("TMP", &tmp), ("TEMP", &tmp)],
        &[
            serde_json::json!({"id": "r", "type": "get_codebase", "limit": 1000, "ref": "v1"}),
            serde_json::json!({"id": "c", "type": "get_codebase", "limit": 1000, "ref": v1}),
        ],
    );
    assert_eq!(bytes_under(&d), before, "the repository changed");
    assert_eq!(porcelain_unlocked(&d), "");
    let left: Vec<_> = std::fs::read_dir(&tmp).unwrap().collect();
    assert!(left.is_empty(), "left in the temporary directory: {left:?}");
    // The worktree's answer, asked for after the check above: it saves
    // `.trellis/index.bin` (#654), which a commit's never does.
    let mut records = records;
    records.extend(rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "h", "type": "get_codebase", "limit": 1000})],
    ));

    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], true, "{r}");
    let at = &r["data"];
    assert_eq!(at["ref"], "v1", "{at}");
    assert_eq!(at["commit"], v1.as_str(), "the commit, not the tag object");
    assert_eq!(at["date"], V1_DATE, "{at}");
    let old = module_in(at, "app.old").unwrap_or_else(|| panic!("app.old at v1: {at}"));
    assert_eq!(old["path"], "app/old.py", "{old}");
    assert!(module_in(at, "app.new").is_none(), "{at}");
    for m in at["modules"].as_array().unwrap() {
        let p = m["path"].as_str().unwrap();
        assert!(!Path::new(p).is_absolute() && !p.contains('\\'), "{m}");
        git(&d, &["cat-file", "-e", &format!("{v1}:{p}")]);
    }
    let core = module_in(at, "app.old").unwrap()["calls"].clone();
    assert_eq!(
        core,
        serde_json::json!([{"module": "app.core", "call_sites": 1}]),
        "indexed, not only listed"
    );
    let by_id = rpc_answer(&records, "c");
    assert_eq!(by_id["data"]["commit"], v1.as_str(), "{by_id}");
    assert_eq!(by_id["data"]["modules"], at["modules"], "{by_id}");

    let h = rpc_answer(&records, "h");
    let now = &h["data"];
    assert!(now.get("ref").is_none(), "no ref was asked for: {now}");
    assert_eq!(now["commit"], head.as_str(), "{now}");
    assert_eq!(now["date"], head_date.as_str(), "{now}");
    assert!(module_in(now, "app.new").is_some(), "{now}");
    assert!(module_in(now, "app.old").is_none(), "{now}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn rpc_atlas_get_codebase_refuses_an_option_an_unknown_ref_and_edges_other_than_all() {
    // The risks: a client's `ref` read by git as an option (`--output`
    // writes a file); an unknown ref answered as HEAD's codebase, which
    // the Atlas would draw as that snapshot; an argument it does not know
    // taken as the default.
    let (d, _) = atlas_repo("rpc-atlas-refuse");
    let tree = git(&d, &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    let cases = [
        (serde_json::json!("-n"), "`-n`"),
        (serde_json::json!("--output=out.txt"), "`--output=out.txt`"),
        (serde_json::json!("no-such-ref"), "`no-such-ref`"),
        (serde_json::json!(tree), tree.as_str()),
        (serde_json::json!(""), "`ref`"),
        (serde_json::json!("v1\nHEAD"), "`ref`"),
        (serde_json::json!(7), "`ref`"),
    ];
    let mut commands: Vec<serde_json::Value> = cases
        .iter()
        .enumerate()
        .map(|(i, (r, _))| serde_json::json!({"id": format!("r{i}"), "type": "get_codebase", "ref": r}))
        .collect();
    commands.push(serde_json::json!({"id": "e1", "type": "get_codebase", "edges": "some"}));
    commands.push(serde_json::json!({"id": "e2", "type": "get_codebase", "edges": true}));
    let before = bytes_under(&d);
    let records = rpc_records(&d, None, &commands);
    assert_eq!(bytes_under(&d), before, "a refusal wrote something");
    for (i, (_, named)) in cases.iter().enumerate() {
        let r = rpc_answer(&records, &format!("r{i}"));
        assert_eq!(r["command"], "get_codebase", "{r}");
        assert_eq!(r["success"], false, "{r}");
        assert!(r.get("data").is_none(), "{r}");
        let e = r["error"].as_str().unwrap();
        assert!(e.contains(named), "names {named}: {e}");
    }
    for id in ["e1", "e2"] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains("`edges`"), "{r}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_with_no_commit_or_no_git_says_so() {
    // The risks: a repository with no commit yet answered with an invented
    // commit, or refused outright, when its worktree can still be drawn;
    // and `ref` opening a way past #559's refusal of a root with no `.git`.
    let d = empty_repo("rpc-atlas-unborn");
    std::fs::write(d.join("a.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "g", "type": "get_codebase"}),
            serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"}),
        ],
    );
    let g = rpc_answer(&records, "g");
    assert_eq!(g["success"], true, "{g}");
    assert_eq!(g["data"]["commit"], serde_json::Value::Null, "{g}");
    assert_eq!(g["data"]["date"], serde_json::Value::Null, "{g}");
    assert!(module_in(&g["data"], "a").is_some(), "{g}");
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("`HEAD`"), "{r}");

    let bare = scratch("rpc-atlas-no-git");
    std::fs::write(bare.join("loose.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"})],
    );
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("no .git"), "{r}");
    assert!(!bare.join(".trellis").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

/// A repository whose `app/shapes.py` has a class hierarchy (so a call
/// through a declared base is ambiguous between two overrides), a helper
/// called from two places in it, and `app/use.py` calling the helper from
/// another module.
fn symbols_repo(name: &str) -> PathBuf {
    let d = empty_repo(name);
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/shapes.py"),
        concat!(
            "class Base:\n",
            "    def run(self):\n",
            "        return helper()\n",
            "\n",
            "\n",
            "class Child(Base):\n",
            "    def run(self):\n",
            "        return 2\n",
            "\n",
            "\n",
            "def helper():\n",
            "    return 1\n",
            "\n",
            "\n",
            "def go(b: Base):\n",
            "    return b.run() + helper() + b.missing()\n",
        ),
    )
    .unwrap();
    std::fs::write(
        d.join("app/use.py"),
        "from app.shapes import helper\n\n\ndef use():\n    return helper()\n",
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "symbols"]);
    d
}

#[test]
fn rpc_atlas_get_symbols_lists_each_definition_with_its_lines_and_call_site_counts() {
    // The risks: the Atlas's near zoom reading symbol lines from a second
    // parser that disagrees with the index (the prototype used Python's
    // ast); a call through a declared base counted as bound to one
    // override; a count folded into another, or a share of them; and a
    // read that writes the index cache.
    let d = symbols_repo("rpc-atlas-symbols");
    let before = bytes_under(&d);
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "all", "type": "get_symbols"}),
            serde_json::json!({"id": "one", "type": "get_symbols", "module": "app.use"}),
        ],
    );
    assert_eq!(bytes_under(&d), before, "get_symbols wrote something");
    let all = rpc_answer(&records, "all");
    assert_eq!(all["command"], "get_symbols", "{all}");
    assert_eq!(all["success"], true, "{all}");
    let data = &all["data"];
    assert_eq!(data["modules_indexed"], 3, "{data}");
    let row = |module: &str, name: &str, kind: &str, lines: (u64, u64), counts: (u64, u64, u64)| {
        serde_json::json!({
            "module": module,
            "path": format!("{}.py", module.replace('.', "/")),
            "name": name,
            "kind": kind,
            "line": lines.0,
            "end_line": lines.1,
            "calls_in": counts.0,
            "calls_in_ambiguous": counts.1,
            "calls_out": counts.2,
        })
    };
    let want = serde_json::json!([
        row("app.shapes", "Base", "class", (1, 3), (0, 0, 0)),
        row("app.shapes", "run", "method", (2, 3), (0, 1, 1)),
        row("app.shapes", "Child", "class", (6, 8), (0, 0, 0)),
        row("app.shapes", "run", "method", (7, 8), (0, 1, 0)),
        row("app.shapes", "helper", "function", (11, 12), (3, 0, 0)),
        row("app.shapes", "go", "function", (15, 16), (0, 0, 1)),
        row("app.use", "use", "function", (4, 5), (0, 0, 1)),
    ]);
    assert_eq!(data["symbols"], want, "{data}");
    let mut all_keys = Vec::new();
    for s in data["symbols"].as_array().unwrap() {
        all_keys.extend(s.as_object().unwrap().keys().cloned());
    }
    for k in &all_keys {
        for banned in ["ratio", "percent", "fraction", "total", "share"] {
            assert!(!k.contains(banned), "key `{k}`");
        }
    }

    let one = rpc_answer(&records, "one");
    assert_eq!(one["success"], true, "{one}");
    assert_eq!(one["data"]["modules_indexed"], 3, "{one}");
    assert_eq!(
        one["data"]["symbols"],
        serde_json::json!([want[6]]),
        "{one}"
    );

    // Last: `index` writes the cache, which is its job. Every definition
    // it counts is listed, and no other.
    let o = run(&d, &["index"]);
    assert!(o.status.success(), "{}", combined(&o));
    let s = stdout(&o);
    let defs: u64 = s
        .lines()
        .find_map(|l| l.trim().strip_prefix("definitions:"))
        .unwrap_or_else(|| panic!("{s}"))
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        data["symbols"].as_array().unwrap().len() as u64,
        defs,
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_symbols_refuses_an_unknown_module_a_non_string_and_a_root_with_no_git() {
    // The risks: a module the index never saw answered as one with no
    // definitions, which the Atlas would draw as empty; and #559's
    // refusal skipped by a new read, walking a home folder.
    let d = symbols_repo("rpc-atlas-symbols-refuse");
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "u", "type": "get_symbols", "module": "app.nowhere"}),
            serde_json::json!({"id": "n", "type": "get_symbols", "module": 3}),
        ],
    );
    let u = rpc_answer(&records, "u");
    assert_eq!(u["success"], false, "{u}");
    assert!(
        u["error"].as_str().unwrap().contains("`app.nowhere`"),
        "{u}"
    );
    let n = rpc_answer(&records, "n");
    assert_eq!(n["success"], false, "{n}");
    assert!(n["error"].as_str().unwrap().contains("`module`"), "{n}");

    let bare = scratch("rpc-atlas-symbols-no-git");
    std::fs::write(bare.join("loose.py"), "def f():\n    return 1\n").unwrap();
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[
            serde_json::json!({"id": "s", "type": "get_symbols"}),
            serde_json::json!({"id": "t", "type": "get_state"}),
        ],
    );
    let at = |id: &str| {
        records
            .iter()
            .position(|r| r["type"] == "response" && r["id"] == id)
            .unwrap()
    };
    assert!(
        at("s") < at("t"),
        "refused on the reader thread: {records:?}"
    );
    let s = rpc_answer(&records, "s");
    assert_eq!(s["success"], false, "{s}");
    let e = s["error"].as_str().unwrap();
    assert!(e.contains("get_symbols") && e.contains("no .git"), "{e}");
    assert!(!bare.join(".trellis").exists());
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn rpc_atlas_get_radius_lists_the_symbols_reached_and_the_sites_beside_the_counts() {
    // The risks: the Atlas tinting reached modules from a list trellis
    // never gave (the prototype decoded index.bin for it); an ambiguous
    // site listed as certain; and the lists replacing or changing the
    // counts, which stay whole and separate (ADR 0014).
    let d = empty_repo("rpc-atlas-radius");
    std::fs::create_dir_all(d.join("app")).unwrap();
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("app/shapes.py"),
        concat!(
            "class Base:\n",
            "    def area(self):\n",
            "        return 0\n",
            "\n",
            "\n",
            "class Square(Base):\n",
            "    def side(self):\n",
            "        return 1\n",
            "\n",
            "    def area(self):\n",
            "        return self.side() * self.side()\n",
            "\n",
            "\n",
            "def measure(s: Base):\n",
            "    return s.area()\n",
        ),
    )
    .unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "shapes"]);
    let records = rpc_records(
        &d,
        None,
        &[serde_json::json!({"id": "r", "type": "get_radius", "symbol": "side"})],
    );
    let r = rpc_answer(&records, "r");
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(
        data["reach"],
        serde_json::json!({"resolved": 2, "ambiguous": 1, "symbols_reached": 2}),
        "{data}"
    );
    assert_eq!(
        data["reached"],
        serde_json::json!([
            {"symbol": "app.shapes.area", "module": "app.shapes", "path": "app/shapes.py",
             "kind": "method", "line": 10},
            {"symbol": "app.shapes.measure", "module": "app.shapes", "path": "app/shapes.py",
             "kind": "function", "line": 14},
        ]),
        "{data}"
    );
    assert_eq!(
        data["sites"],
        serde_json::json!([
            {"path": "app/shapes.py", "line": 11, "certain": true},
            {"path": "app/shapes.py", "line": 11, "certain": true},
            {"path": "app/shapes.py", "line": 15, "certain": false},
        ]),
        "{data}"
    );
    assert_eq!(data["reached_not_listed"], 0, "{data}");
    assert_eq!(data["sites_not_listed"], 0, "{data}");
    let _ = std::fs::remove_dir_all(&d);
}

/// [`history_repo`]'s commits, by the names its comment gives them.
struct History {
    root: PathBuf,
    c: std::collections::BTreeMap<&'static str, String>,
}

/// A repository whose first-parent line is c0 (2023-06-01, tagged `old`),
/// c1 (2024-01-01), c2 (2024-03-01, tagged `v1`, annotated), c3
/// (2025-01-15), c5 (2025-02-10, merging `side`'s c4, tagged `side-tag`)
/// and c6 (2025-02-20, HEAD); `elsewhere` tags a commit HEAD cannot reach.
/// All in UTC but c6, whose offset is +01:00.
fn history_repo(name: &str) -> History {
    let d = empty_repo(name);
    let main = git(&d, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_string();
    let mut c = std::collections::BTreeMap::new();
    let commit = |c: &mut std::collections::BTreeMap<&'static str, String>,
                  id: &'static str,
                  date: &str,
                  subject: &str,
                  files: &[(&str, &str)]| {
        for (p, text) in files {
            std::fs::write(d.join(p), text).unwrap();
        }
        git(&d, &["add", "-A"]);
        git_dated(&d, date, &["commit", "-qm", subject]);
        c.insert(id, git(&d, &["rev-parse", "HEAD"]).trim().to_string());
    };
    commit(
        &mut c,
        "c0",
        "2023-06-01T00:00:00+00:00",
        "start",
        &[
            ("a.py", "A = 0\n"),
            ("b.py", "B = 0\n"),
            ("e.py", "E = 0\n"),
        ],
    );
    git(&d, &["tag", "old"]);
    commit(
        &mut c,
        "c1",
        "2024-01-01T00:00:00+00:00",
        "e once",
        &[("e.py", "E = 1\n")],
    );
    commit(
        &mut c,
        "c2",
        "2024-03-01T00:00:00+00:00",
        "a once",
        &[("a.py", "A = 1\n")],
    );
    git(&d, &["tag", "-a", "v1", "-m", "release"]);
    commit(
        &mut c,
        "c3",
        "2025-01-15T00:00:00+00:00",
        "b and c",
        &[("b.py", "B = 1\n"), ("c.py", "C = 0\n")],
    );
    git(&d, &["checkout", "-qb", "side"]);
    commit(
        &mut c,
        "c4",
        "2025-02-01T00:00:00+00:00",
        "d on a side",
        &[("d.py", "D = 0\n")],
    );
    git(&d, &["tag", "side-tag"]);
    git(&d, &["checkout", "-q", &main]);
    git_dated(
        &d,
        "2025-02-10T00:00:00+00:00",
        &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
    );
    c.insert("c5", git(&d, &["rev-parse", "HEAD"]).trim().to_string());
    commit(
        &mut c,
        "c6",
        "2025-02-20T01:00:00+01:00",
        "a\tagain <b>",
        &[("a.py", "A = 2\n")],
    );
    git(&d, &["checkout", "-qb", "other", &c["c0"]]);
    commit(
        &mut c,
        "c7",
        "2025-03-01T00:00:00+00:00",
        "unmerged",
        &[("z.py", "Z = 0\n")],
    );
    git(&d, &["tag", "elsewhere"]);
    git(&d, &["checkout", "-q", &main]);
    assert_eq!(porcelain_unlocked(&d), "");
    History { root: d, c }
}

#[test]
fn rpc_atlas_get_history_measures_churn_back_from_head_and_lists_commits_and_tags() {
    // The risks: windows measured from the wall clock, so the same
    // repository answers differently tomorrow; a merge's changes missed or
    // a side branch's commits counted on the first-parent line; a tag HEAD
    // cannot reach offered as a snapshot; churn dropping a path changed
    // since the oldest tag but not in the last year; and a read that
    // writes.
    let h = history_repo("rpc-atlas-history");
    let c = |id: &str| h.c[id].as_str();
    let before = bytes_under(&h.root);
    let records = rpc_records(
        &h.root,
        None,
        &[
            serde_json::json!({"id": "d", "type": "get_history"}),
            serde_json::json!({"id": "n", "type": "get_history", "commits": 2, "tags": 2}),
            serde_json::json!({"id": "z", "type": "get_history", "commits": 0, "tags": 0}),
        ],
    );
    assert_eq!(bytes_under(&h.root), before, "get_history wrote something");
    let d = rpc_answer(&records, "d");
    assert_eq!(d["command"], "get_history", "{d}");
    assert_eq!(d["success"], true, "{d}");
    let data = &d["data"];
    assert_eq!(
        data["head"],
        serde_json::json!({"commit": c("c6"), "date": "2025-02-20T01:00:00+01:00"}),
        "{data}"
    );
    assert_eq!(data["windows_days"], serde_json::json!([30, 90, 365]));
    assert_eq!(
        data["tags"],
        serde_json::json!([
            {"name": "side-tag", "commit": c("c4"), "date": "2025-02-01T00:00:00+00:00"},
            {"name": "v1", "commit": c("c2"), "date": "2024-03-01T00:00:00+00:00"},
            {"name": "old", "commit": c("c0"), "date": "2023-06-01T00:00:00+00:00"},
        ]),
        "{data}"
    );
    let logged = |id: &str, date: &str, subject: &str, paths: &[&str]| serde_json::json!({"commit": c(id), "date": date, "subject": subject, "paths": paths});
    assert_eq!(
        data["commits"],
        serde_json::json!([
            logged("c6", "2025-02-20T01:00:00+01:00", "a\tagain <b>", &["a.py"]),
            logged("c5", "2025-02-10T00:00:00+00:00", "merge side", &["d.py"]),
            logged(
                "c3",
                "2025-01-15T00:00:00+00:00",
                "b and c",
                &["b.py", "c.py"]
            ),
            logged("c2", "2024-03-01T00:00:00+00:00", "a once", &["a.py"]),
            logged("c1", "2024-01-01T00:00:00+00:00", "e once", &["e.py"]),
            logged(
                "c0",
                "2023-06-01T00:00:00+00:00",
                "start",
                &["a.py", "b.py", "e.py"]
            ),
        ]),
        "{data}"
    );
    let row = |path: &str, d: [u64; 3], commits: serde_json::Value, last: &str, date: &str| {
        serde_json::json!({
            "path": path, "d30": d[0], "d90": d[1], "d365": d[2], "commits": commits,
            "last_commit": c(last), "last_date": date,
        })
    };
    let n = |v: u64| serde_json::json!(v);
    // 30 days back from c6 reaches c5; 90 reaches c3; 365 reaches c2; the
    // oldest tag, `old` at c0, puts c1 in range too, so e.py is a row.
    assert_eq!(
        data["churn"],
        serde_json::json!([
            row("a.py", [1, 1, 2], n(2), "c6", "2025-02-20T01:00:00+01:00"),
            row("b.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("c.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("d.py", [1, 1, 1], n(1), "c5", "2025-02-10T00:00:00+00:00"),
            row("e.py", [0, 0, 0], n(1), "c1", "2024-01-01T00:00:00+00:00"),
        ]),
        "{data}"
    );

    // Two commits and two tags: the oldest returned is v1 (c2), so
    // `commits` counts c6, c5 and c3, and e.py is out of range.
    let s = &rpc_answer(&records, "n")["data"];
    assert_eq!(s["commits"].as_array().unwrap().len(), 2, "{s}");
    assert_eq!(s["commits"][1]["commit"], c("c5"), "{s}");
    assert_eq!(s["tags"].as_array().unwrap().len(), 2, "{s}");
    assert_eq!(
        s["churn"],
        serde_json::json!([
            row("a.py", [1, 1, 2], n(1), "c6", "2025-02-20T01:00:00+01:00"),
            row("b.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("c.py", [0, 1, 1], n(1), "c3", "2025-01-15T00:00:00+00:00"),
            row("d.py", [1, 1, 1], n(1), "c5", "2025-02-10T00:00:00+00:00"),
        ]),
        "{s}"
    );

    // No tag asked for: there is no tag to count from, so `commits` is
    // null, never 0, and the year still sets the rows.
    let z = &rpc_answer(&records, "z")["data"];
    assert_eq!(z["commits"], serde_json::json!([]), "{z}");
    assert_eq!(z["tags"], serde_json::json!([]), "{z}");
    let null = serde_json::Value::Null;
    assert_eq!(
        z["churn"],
        serde_json::json!([
            row(
                "a.py",
                [1, 1, 2],
                null.clone(),
                "c6",
                "2025-02-20T01:00:00+01:00"
            ),
            row(
                "b.py",
                [0, 1, 1],
                null.clone(),
                "c3",
                "2025-01-15T00:00:00+00:00"
            ),
            row(
                "c.py",
                [0, 1, 1],
                null.clone(),
                "c3",
                "2025-01-15T00:00:00+00:00"
            ),
            row("d.py", [1, 1, 1], null, "c5", "2025-02-10T00:00:00+00:00"),
        ]),
        "{z}"
    );
    let _ = std::fs::remove_dir_all(&h.root);
}

#[test]
fn rpc_atlas_get_history_of_no_commit_is_empty_and_bad_counts_and_no_git_are_refused() {
    // The risks: a repository with no commit answered with an error the
    // Atlas shows as a failure, when the truth is "no history yet"; a
    // count that is not one taken as the default; and #559's refusal
    // skipped by a new read.
    let d = empty_repo("rpc-atlas-history-unborn");
    std::fs::write(d.join("a.py"), "A = 0\n").unwrap();
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "u", "type": "get_history"}),
            serde_json::json!({"id": "a", "type": "get_history", "commits": "20"}),
            serde_json::json!({"id": "b", "type": "get_history", "tags": -1}),
            serde_json::json!({"id": "c", "type": "get_history", "commits": 2.5}),
        ],
    );
    let u = rpc_answer(&records, "u");
    assert_eq!(u["success"], true, "{u}");
    assert_eq!(
        u["data"],
        serde_json::json!({"head": null, "windows_days": [30, 90, 365],
                           "churn": [], "commits": [], "tags": []}),
        "{u}"
    );
    for (id, field) in [("a", "`commits`"), ("b", "`tags`"), ("c", "`commits`")] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], false, "{r}");
        assert!(r["error"].as_str().unwrap().contains(field), "{r}");
    }

    let bare = scratch("rpc-atlas-history-no-git");
    let records = rpc_records(
        &bare,
        Some(&bare),
        &[serde_json::json!({"id": "h", "type": "get_history"})],
    );
    let r = rpc_answer(&records, "h");
    assert_eq!(r["success"], false, "{r}");
    let e = r["error"].as_str().unwrap();
    assert!(e.contains("get_history") && e.contains("no .git"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bare);
}

/// Every dotted path in `v` as the contract writes them: a key, `a.b`
/// under it, and `a[].b` under each element of an array.
fn answered_paths(v: &serde_json::Value, at: &str, out: &mut std::collections::BTreeSet<String>) {
    match v {
        serde_json::Value::Object(o) => {
            for (k, child) in o {
                let p = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                out.insert(p.clone());
                answered_paths(child, &p, out);
            }
        }
        serde_json::Value::Array(a) => {
            for e in a {
                if e.is_object() {
                    answered_paths(e, &format!("{at}[]"), out);
                }
            }
        }
        _ => {}
    }
}

#[test]
fn rpc_atlas_contract_lists_every_field_the_atlas_commands_answer() {
    // The risk (ADR 0048 §6): the desktop's Atlas reads these answers by
    // name, and a field renamed here, or answered and never promised,
    // passes every other test. Both directions: each field the contract
    // lists is answered, and each field answered is listed.
    let (d, _) = atlas_repo("rpc-atlas-contract");
    // An unbound call, so the radius's doubt lists carry entries.
    std::fs::write(
        d.join("app/core.py"),
        "def greet(who):\n    return who.strip()\n",
    )
    .unwrap();
    git(&d, &["commit", "-qam", "doubt"]);
    let records = rpc_records(
        &d,
        None,
        &[
            serde_json::json!({"id": "g", "type": "get_codebase", "limit": 1000, "edges": "all"}),
            serde_json::json!({"id": "r", "type": "get_codebase", "ref": "v1"}),
            serde_json::json!({"id": "s", "type": "get_symbols"}),
            serde_json::json!({"id": "x", "type": "get_radius", "symbol": "greet"}),
            serde_json::json!({"id": "h", "type": "get_history"}),
        ],
    );
    let c = client_contract();
    let responses = &c["rpc"]["responses"];
    for (id, command, lists) in [
        ("g", "get_codebase", &["always", "saved_only"][..]),
        (
            "r",
            "get_codebase",
            &["always", "ref_only", "not_saved_only"][..],
        ),
        ("s", "get_symbols", &["always"][..]),
        ("x", "get_radius", &["always"][..]),
        ("h", "get_history", &["always", "head_only"][..]),
    ] {
        let r = rpc_answer(&records, id);
        assert_eq!(r["success"], true, "{r}");
        let data = &r["data"];
        let entry = &responses[command];
        let promised: Vec<&str> = lists
            .iter()
            .flat_map(|l| contract_paths(&entry[*l]))
            .collect();
        let gaps: Vec<&&str> = promised.iter().filter(|p| !carries_each(data, p)).collect();
        assert!(
            gaps.is_empty(),
            "{command} ({id}) lacks what tests/client_contract.json lists: {gaps:?}\n{data}"
        );
        let mut listed: std::collections::BTreeSet<String> = Default::default();
        for (k, l) in entry.as_object().unwrap() {
            if k == "always" || k.ends_with("_only") {
                listed.extend(contract_paths(l).into_iter().map(str::to_string));
            }
        }
        let mut answered = Default::default();
        answered_paths(data, "", &mut answered);
        let unlisted: Vec<&String> = answered.difference(&listed).collect();
        assert!(
            unlisted.is_empty(),
            "{command} ({id}) answers what tests/client_contract.json does not list: {unlisted:?}"
        );
    }
    // The fixture gives every list something to carry, so an element's
    // fields are checked rather than passed by an empty array.
    let x = &rpc_answer(&records, "x")["data"];
    for list in ["reached", "sites", "doubt.reasons", "doubt.files"] {
        let v = list.split('.').fold(x, |v, k| &v[k]);
        assert!(v.as_array().is_some_and(|a| !a.is_empty()), "{list}: {x}");
    }
    let h = &rpc_answer(&records, "h")["data"];
    for list in ["churn", "commits", "tags"] {
        assert!(
            h[list].as_array().is_some_and(|a| !a.is_empty()),
            "{list}: {h}"
        );
    }
    assert!(h["churn"][0]["commits"].is_u64(), "a tag is listed: {h}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rpc_atlas_get_codebase_answers_its_modules_when_head_cannot_be_read() {
    // The risk: `commit` and `date`, metadata the Atlas added, failing a
    // read that needs no git: the desktop's own codebase test gives the
    // root a hand-made `.git`, which git refuses, and the index never
    // asks git anything. A git error is said in `head_error`; a
    // repository with no commit yet is not an error, so it has none.
    // Discovery is held to the scratch directory, so a `.git` git refuses
    // is not passed over for a repository above it.
    let ceiling = std::env::temp_dir();
    let env: &[(&str, &Path)] = &[("GIT_CEILING_DIRECTORIES", &ceiling)];
    let refused = scratch("rpc-atlas-head-refused");
    std::fs::create_dir_all(refused.join(".git")).unwrap();
    let unborn = scratch("rpc-atlas-head-unborn");
    for d in [".git/objects", ".git/refs"] {
        std::fs::create_dir_all(unborn.join(d)).unwrap();
    }
    std::fs::write(unborn.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    for root in [&refused, &unborn] {
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/a.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(
            root.join("app/b.py"),
            "from app.a import f\n\ndef g():\n    return f()\n",
        )
        .unwrap();
    }
    let ask = [serde_json::json!({"id": "g", "type": "get_codebase"})];

    let r = rpc_answer(&rpc_records_env(&refused, Some(&refused), env, &ask), "g").clone();
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(data["modules_indexed"], 2, "{data}");
    assert!(module_in(data, "app.b").is_some(), "{data}");
    assert_eq!(data["commit"], serde_json::Value::Null, "{data}");
    assert_eq!(data["date"], serde_json::Value::Null, "{data}");
    let e = data["head_error"]
        .as_str()
        .unwrap_or_else(|| panic!("git's refusal is said: {data}"));
    assert!(e.contains("not a git repository"), "{e}");
    assert_eq!(e, e.trim(), "trimmed");

    let r = rpc_answer(&rpc_records_env(&unborn, Some(&unborn), env, &ask), "g").clone();
    assert_eq!(r["success"], true, "{r}");
    let data = &r["data"];
    assert_eq!(data["modules_indexed"], 2, "{data}");
    assert_eq!(data["commit"], serde_json::Value::Null, "{data}");
    assert!(
        data.get("head_error").is_none(),
        "no commit is not an error: {data}"
    );

    // A ref still needs git, and a ref git cannot resolve is still refused.
    let r = rpc_records_env(
        &refused,
        Some(&refused),
        env,
        &[serde_json::json!({"id": "r", "type": "get_codebase", "ref": "HEAD"})],
    );
    assert_eq!(rpc_answer(&r, "r")["success"], false, "{r:?}");
    let _ = std::fs::remove_dir_all(&refused);
    let _ = std::fs::remove_dir_all(&unborn);
}
