use super::*;

// ---------------------------------------------------------------------------
// read_file (ADR 0058 §4, #809)
// ---------------------------------------------------------------------------

#[test]
fn rpc_read_file_refuses_git_and_outside_paths() {
    // The risks: a client's whole-file view becomes a way to read what
    // `@` completion was built never to offer (an ignored `.env`), git's
    // own directory (credentials in `.git/config`), or a file outside the
    // repository through a link a hostile clone commits. Each is refused by
    // the rule it breaks, and no answer carries the outside file's text.
    let d = repo("rpc-read-file-refused");
    let outside = scratch("rpc-read-file-refused-outside");
    // Built at run time, so no secret-shaped literal sits in the source.
    let canary = format!("canary-{}-outside", std::process::id());
    std::fs::write(outside.join("secret.txt"), &canary).unwrap();
    std::fs::write(d.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(d.join(".env"), &canary).unwrap();
    std::fs::write(d.join("blob.bin"), [0x66u8, 0x6f, 0xff, 0xfe, 0x00]).unwrap();
    let linked = link_or_skip(&outside.join("secret.txt"), &d.join("leak.txt"))
        && link_or_skip(Path::new(".git/config"), &d.join("gitcfg.txt"));
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "files read_file must refuse"]);
    assert!(
        git(&d, &["ls-files"]).lines().any(|l| l == "blob.bin"),
        "the non-UTF-8 file is one list_paths offers, so its refusal is its own"
    );

    let mut cases: Vec<(serde_json::Value, &str)> = vec![
        (serde_json::json!(null), "is required"),
        (serde_json::json!(7), "is required"),
        (serde_json::json!(""), "empty"),
        (serde_json::json!(".git/config"), "list_paths"),
        (serde_json::json!(".git"), "list_paths"),
        (serde_json::json!(".GIT/config"), "list_paths"),
        (serde_json::json!(".env"), "list_paths"),
        (serde_json::json!("app"), "list_paths"),
        (serde_json::json!("app\\core.py"), "list_paths"),
        (serde_json::json!("../x"), "`..`"),
        (serde_json::json!("app/../.git/config"), "`..`"),
        (serde_json::json!("/etc/passwd"), "absolute"),
        (serde_json::json!("C:/x"), "absolute"),
        (serde_json::json!("a\u{7}b"), "control character"),
        (serde_json::json!("blob.bin"), "UTF-8"),
    ];
    if linked {
        cases.push((serde_json::json!("leak.txt"), "outside the repository"));
        cases.push((serde_json::json!("gitcfg.txt"), "into .git"));
    }
    let commands: Vec<serde_json::Value> = cases
        .iter()
        .enumerate()
        .map(|(n, (path, _))| {
            let mut c = serde_json::json!({"id": n.to_string(), "type": "read_file"});
            if !path.is_null() {
                c["path"] = path.clone();
            }
            c
        })
        .collect();
    let r = rpc_records(&d, Some(&d), &commands);
    for (n, (path, rule)) in cases.iter().enumerate() {
        let got = rpc_answer(&r, &n.to_string());
        assert_eq!(got["command"], "read_file", "{got}");
        assert_eq!(got["success"], false, "{path} was read: {got}");
        assert!(got.get("data").is_none(), "{path}: {got}");
        let e = got["error"].as_str().unwrap();
        assert!(e.contains("`path`") && e.contains(rule), "{path}: {e}");
    }
    for record in &r {
        assert!(
            !record.to_string().contains(&canary),
            "a refused file's text reached the client: {record}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn rpc_read_file_cuts_a_file_past_the_cap_and_says_so() {
    // The risks: a large file sent whole to a client's view, or cut
    // without a word, so the view passes part of a file off as all of it;
    // or cut mid-character, so the text is not UTF-8. The cap (256 KiB) is
    // not a multiple of 3, so a file of three-byte characters puts it
    // inside one.
    let d = repo("rpc-read-file-cut");
    let big = "\u{20ac}".repeat(100_000);
    std::fs::write(d.join("big.txt"), &big).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "a file past the cap"]);
    // Untracked and not ignored: `list_paths` offers it, so it is read too.
    std::fs::write(d.join("draft.txt"), "caf\u{e9}\n").unwrap();

    let r = rpc_records(
        &d,
        Some(&d),
        &[
            serde_json::json!({"id": "big", "type": "read_file", "path": "big.txt"}),
            serde_json::json!({"id": "small", "type": "read_file", "path": "app/core.py"}),
            serde_json::json!({"id": "draft", "type": "read_file", "path": "draft.txt"}),
        ],
    );
    let got = rpc_answer(&r, "big");
    assert_eq!(got["success"], true, "{got}");
    let data = &got["data"];
    assert_eq!(data["path"], "big.txt", "{data}");
    assert_eq!(data["cut"], true, "a cut file must say so");
    assert_eq!(data["bytes"], big.len(), "bytes is the file's whole size");
    let text = data["text"].as_str().unwrap();
    assert_eq!(
        text.len(),
        262_143,
        "cut at the last whole character before the cap"
    );
    assert!(big.starts_with(text), "the cut text is the file's start");

    let core = std::fs::read_to_string(d.join("app/core.py")).unwrap();
    let small = &rpc_answer(&r, "small")["data"];
    assert_eq!(small["text"], core.as_str(), "{small}");
    assert_eq!(small["cut"], false, "{small}");
    assert_eq!(small["bytes"], core.len(), "{small}");
    let draft = &rpc_answer(&r, "draft")["data"];
    assert_eq!(draft["text"], "caf\u{e9}\n", "{draft}");
    assert_eq!(draft["cut"], false, "{draft}");

    // A client reads these by name; a rename fails here, not in a client.
    let c = client_contract();
    for answer in [data, small, draft] {
        for p in contract_paths(&c["rpc"]["responses"]["read_file"]["always"]) {
            assert!(carries_each(answer, p), "{p} missing: {answer}");
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}
