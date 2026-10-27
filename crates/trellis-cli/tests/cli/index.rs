use super::*;

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

#[test]
fn index_reports_coverage_and_never_claims_to_be_complete() {
    // Constraint 1: complete call-graph recall is not achievable in Python,
    // so the graph is high-precision and explicitly incomplete. A bare
    // "complete" in this output is the failure, whatever the numbers say.
    let d = repo("index");
    let out = run(&d, &["index"]);
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);

    assert!(
        s.contains("definitions"),
        "it must report what it found: {s}"
    );
    let lower = s.to_lowercase();
    for banned in [
        "complete call graph",
        "fully resolved",
        "all call sites resolved",
    ] {
        assert!(!lower.contains(banned), "index claimed completeness: {s}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn index_says_its_percentage_is_a_lower_bound_and_not_recall() {
    // The number in this output is what the resolver *bound*. Comparing it to
    // the gate thresholds in BENCHMARK_AND_EVAL.md §3 is wrong, because an
    // `obj.method()` with an unknown receiver counts as a miss here even when
    // its true callee is third-party and excluded from the gate's denominator
    // outright. A-30, and the standing warning at the top of CLAUDE.md.
    //
    // The caveat travelling with the number is the entire defence. Without it
    // the figure reads as recall to every person who sees it once.
    let d = repo("index-caveat");
    // The caveat is hand-wrapped for the terminal, so match on the words
    // rather than on where the lines happen to break.
    let s = stdout(&run(&d, &["index"]))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        s.contains("not a verified recall figure"),
        "the figure must carry its own caveat: {s}"
    );
    assert!(
        s.contains("counts agreement with itself"),
        "it must say *why* it is not recall, or the caveat is a disclaimer: {s}"
    );
    assert!(
        s.contains("hand-audited sample"),
        "it must name what *is* comparable to the gate: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn index_with_the_pyright_backstop_absent_completes_and_says_it_did_not_run() {
    // ADR 0031: with Node or Pyright missing, the index does not fail, every
    // site keeps the resolver's verdict, and the output says the backstop did
    // not run, so it cannot be read as "Pyright ran and found nothing". PATH
    // points at an empty directory so the result does not depend on whether
    // the machine running the suite has Pyright.
    let d = repo("index-backstop");
    let off = stdout(&run(&d, &["index"]));
    assert!(off.contains("backstop: off"), "{off}");

    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[index]
backstop = \"pyright\"
",
    )
    .unwrap();
    let empty = scratch("index-backstop-path");
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("index")
        .current_dir(&d)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("PATH", &empty)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(s.contains("backstop: pyright did not run"), "{s}");
    assert!(s.contains("keep the resolver's verdict"), "{s}");
    assert!(
        s.contains("first-party call sites resolved"),
        "the resolver's figures still print: {s}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn index_does_not_wait_two_minutes_on_a_backstop_server_that_never_answers_initialize() {
    // #445: `trellis index --graph` went quiet for over two minutes and was
    // stopped by hand. With `[index] backstop = "pyright"` and a server that
    // starts but never answers `initialize`, the handshake was held to the
    // 120 s meant for a first query's import analysis, then `finish` added
    // up to 7 s: 125 s measured, with nothing printed after the coverage.
    // The handshake now has its own bound, and the output names it.
    let d = repo("index-silent-backstop");
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::write(
        d.join(".trellis/config.toml"),
        "[index]\nbackstop = \"pyright\"\n",
    )
    .unwrap();
    // A `pyright-langserver` that reads its input to the end and writes
    // nothing. It exits when trellis closes its stdin, so nothing outlives
    // the test.
    let bin = scratch("index-silent-backstop-bin");
    if cfg!(windows) {
        std::fs::write(
            bin.join("pyright-langserver.cmd"),
            "@\"%SystemRoot%\\System32\\sort.exe\" > nul\r\n",
        )
        .unwrap();
    } else {
        // No `exec`: `sh` must stay alive holding the stdout pipe, as
        // cmd.exe does on Windows. With `exec cat > /dev/null` no process
        // holds it, trellis reads end of stream at once and reports that the
        // server closed its output, which is not the silent server this test
        // is about.
        let stub = bin.join("pyright-langserver");
        std::fs::write(&stub, "#!/bin/sh\ncat > /dev/null\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();

    let started = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .args(["index", "--graph"])
        .current_dir(&d)
        .env("NO_COLOR", "1")
        .env(
            "TRELLIS_CONFIG_DIR",
            std::env::temp_dir().join("trellis-tests-no-user-config"),
        )
        .env("PATH", path)
        .output()
        .unwrap();
    let took = started.elapsed();

    assert!(out.status.success(), "{}", combined(&out));
    let s = stdout(&out);
    assert!(
        s.contains("backstop: pyright did not run (no answer to initialize within 20s)"),
        "{s}"
    );
    assert!(s.contains("codebase graph"), "{s}");
    // 20 s for the handshake and at most 7 s for `finish`; well under the
    // 125 s it took before.
    assert!(
        took < std::time::Duration::from_secs(60),
        "index waited {took:?} on a silent server"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&bin);
}

// ---------------------------------------------------------------------------
// index.bin behind a link (#721)
// ---------------------------------------------------------------------------

/// A committed repository whose `.trellis/<name>` is a link to `target`,
/// as a clone of a hostile repository arrives. `None` (the test skips)
/// where the platform will not make a file link.
fn repo_with_linked(test: &str, name: &str, target: &Path) -> Option<PathBuf> {
    let d = repo(test);
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    let link = d.join(".trellis").join(name);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, &link);
    if let Err(e) = made {
        eprintln!("skipped: cannot create a file symlink here: {e}");
        let _ = std::fs::remove_dir_all(&d);
        return None;
    }
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-qm", "a linked .trellis entry"]);
    Some(d)
}

#[test]
fn cache_symlink_trellis_index_leaves_the_linked_outside_file_untouched() {
    // #721's reproduction: `.trellis/index.bin` committed as a link to
    // `outside/rc.sh`. `trellis index` overwrote rc.sh with the blob, and a
    // file name holding a shell command made the blob a runnable script.
    let outside = scratch("cache-symlink-index-outside");
    let target = outside.join("rc.sh");
    std::fs::write(&target, "echo original\n").unwrap();
    let Some(d) = repo_with_linked("cache-symlink-index", "index.bin", &target) else {
        return;
    };

    let o = run(&d, &["index"]);
    let all = combined(&o);
    assert!(all.contains("not written"), "{all}");
    assert!(
        all.contains("symbolic link"),
        "the refusal names why: {all}"
    );
    assert!(all.contains("index.bin"), "and names the path: {all}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "echo original\n",
        "the file outside the repository was written through the link"
    );
    assert!(is_link(&d.join(".trellis/index.bin")));
    assert!(
        !d.join(".trellis/.gitignore").exists(),
        "a refusal writes nothing, the .gitignore included"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn cache_symlink_dangling_index_bin_creates_nothing_through_index_or_radius() {
    // A dangling link: `fs::write` would create its target. `radius` writes
    // through `build_cached`, the second of the three writers.
    let outside = scratch("cache-symlink-dangling-outside");
    let target = outside.join("planted");
    let Some(d) = repo_with_linked("cache-symlink-dangling", "index.bin", &target) else {
        return;
    };

    let o = run(&d, &["index"]);
    assert!(combined(&o).contains("symbolic link"), "{}", combined(&o));
    let o = run(&d, &["radius", "app.core.greet"]);
    assert!(o.status.success(), "{}", combined(&o));
    assert!(
        std::fs::symlink_metadata(&target).is_err(),
        "the dangling link was followed and its target created"
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(is_link(&d.join(".trellis/index.bin")));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn cache_symlink_dangling_gitignore_creates_nothing() {
    // `ignore_the_blob` checked `.trellis/.gitignore` with `exists()`, which
    // follows the link, so a dangling one created a file at its target.
    let outside = scratch("cache-symlink-ignore-outside");
    let target = outside.join("planted");
    let Some(d) = repo_with_linked("cache-symlink-ignore", ".gitignore", &target) else {
        return;
    };

    let o = run(&d, &["index"]);
    assert!(combined(&o).contains("symbolic link"), "{}", combined(&o));
    assert!(std::fs::symlink_metadata(&target).is_err());
    assert!(
        std::fs::symlink_metadata(d.join(".trellis/index.bin")).is_err(),
        "nothing is written beside a refused .gitignore"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn cache_symlink_get_codebase_refuses_the_link_and_says_so() {
    // The desktop's Codebase view saves the index through rpc's
    // `get_codebase` (#654), the third writer. It must refuse the link as
    // `trellis index` does, and still answer with the graph.
    let outside = scratch("cache-symlink-rpc-outside");
    let target = outside.join("rc.sh");
    std::fs::write(&target, "echo original\n").unwrap();
    let Some(d) = repo_with_linked("cache-symlink-rpc", "index.bin", &target) else {
        return;
    };

    let r = get_codebase_in(&d, serde_json::json!({}));
    assert_eq!(r["success"], true, "the graph is still drawn: {r}");
    let data = &r["data"];
    assert_eq!(data["index_saved"], false, "{data}");
    let why = data["index_not_saved"].as_str().unwrap_or_default();
    assert!(why.contains("symbolic link"), "{data}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "echo original\n");
    assert!(is_link(&d.join(".trellis/index.bin")));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&outside);
}
