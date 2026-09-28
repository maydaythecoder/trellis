//! Source-level guards, as tests rather than as a review habit.
//!
//! The rule here was applied by hand twice and missed sites both times. A rule
//! that depends on whoever is editing remembering it is not a rule; this makes
//! the next omission fail on `cargo test`, next to the change that caused it.

/// Every line of `main.rs`, with its 1-based number.
fn main_rs() -> Vec<(usize, String)> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("main.rs is next to its own manifest");
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.to_string()))
        .collect()
}

/// Expressions that read content out of a trail.
///
/// A trail lives in a repository that takes contributions, so everything read
/// back from one is untrusted (S10) — including the parts that look like
/// metadata. `trellis_version` and `model` are recorded by whoever wrote the
/// file, not by this binary.
const FROM_A_TRAIL: &[&str] = &[
    "e.trail.task",
    "e.trail.base_commit",
    "a.tool",
    "a.target",
    "a.outcome",
    "trail.context_lines()",
    "e.run_id()",
    "p.trellis_version",
];

#[test]
fn nothing_read_from_a_trail_is_printed_to_a_terminal_unsanitised() {
    // A terminal interprets what it is given. An escape sequence in a task
    // description can move the cursor and repaint a line, so a run that did
    // not converge can be made to *look* like one that did — in the commands
    // a user reads to find out which.
    //
    // That matters more here than in most tools: this project's claim is that
    // work comes back verified rather than merely written, and a forged
    // terminal defeats the reading of the evidence rather than the evidence.
    let mut bad: Vec<String> = Vec::new();
    for (n, line) in main_rs() {
        let code = line.split("//").next().unwrap_or("");
        if !code.contains("println!") && !code.contains("writeln!") {
            continue;
        }
        for needle in FROM_A_TRAIL {
            if code.contains(needle) && !code.contains("plain(") {
                bad.push(format!("main.rs:{n}: {}", line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "trail content reaches a terminal without `plain()`:\n  {}\n\n\
         Wrap it: `println!(\"{{}}\", plain(&x))`. If this is genuinely not \
         trail content, the needle list in this test is what to correct.",
        bad.join("\n  ")
    );
}

/// Surfaces where a person types prose and presses enter.
///
/// Not `run`: naming a task on the command line says it is one. These are the
/// ones where the next thing typed might be a question.
const CONVERSATIONAL: &[&str] = &["tui_run.rs", "session.rs"];

#[test]
fn every_conversational_surface_asks_whether_the_input_is_a_task() {
    // Typing `hello` used to take a git snapshot, drive the loop, run the
    // repository's checks, roll back and write a trail. `ask::classify` fixed
    // that — in `trellis tui`, and nowhere else, because the fix was applied
    // where the failure was noticed. `trellis session` kept the old behaviour
    // for days with nothing to catch it.
    //
    // A third surface is the likely future mistake, so the rule is a test
    // rather than a thing to remember.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = Vec::new();
    for name in CONVERSATIONAL {
        let text = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name} is a surface this test names: {e}"));
        if !text.contains("ask::classify") {
            missing.push(*name);
        }
    }
    assert!(
        missing.is_empty(),
        "these take typed prose and never ask whether it is a task: {missing:?}\n\
         Typing a question into one of them starts an agent run — a snapshot, \
         the loop, the checks, a rollback and a trail — to answer it."
    );
}

#[test]
fn the_guard_would_actually_catch_something() {
    // A hygiene test that cannot fail is decoration. This exercises the same
    // predicate the test above applies, on a line known to violate it.
    let offending = r#"        println!("    task: {}", e.trail.task);"#;
    let code = offending.split("//").next().unwrap_or("");
    assert!(
        FROM_A_TRAIL.iter().any(|n| code.contains(n)) && !code.contains("plain("),
        "the predicate no longer recognises the shape it was written for"
    );

    let fixed = r#"        println!("    task: {}", plain(&e.trail.task));"#;
    let code = fixed.split("//").next().unwrap_or("");
    assert!(code.contains("plain("), "and recognises the corrected form");
}

/// Non-test source lines of `text`, 0-based: everything outside an item
/// marked `#[cfg(test)]` (a module or a function). The same naive brace
/// count as `child_env.rs`'s scan, which holds for this workspace's test
/// code.
fn non_test_lines(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]" {
            // Skip the item it marks, to the end of its first balanced
            // brace block.
            let mut depth = 0i64;
            let mut opened = false;
            let mut j = i + 1;
            while j < lines.len() {
                depth += lines[j].matches('{').count() as i64;
                depth -= lines[j].matches('}').count() as i64;
                opened |= lines[j].contains('{');
                j += 1;
                if opened && depth <= 0 {
                    break;
                }
            }
            i = j;
            continue;
        }
        out.push((i, lines[i]));
        i += 1;
    }
    out
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Where shipped code may name `git` itself, and why.
const GIT_ALLOWED: &[(&str, &str)] = &[
    (
        "trellis-vcs/src/lib.rs",
        "`command`, `command_with_hooks`, `git_argv`: the constructors",
    ),
    (
        "chat/src/lib.rs",
        "`git_command`: chat depends on nothing in the workspace, so it carries a copy",
    ),
    (
        "chat/src/commands/mod.rs",
        "`/git` is a slash command's name there, not a spawn",
    ),
];

/// Whether `line` names `git` as a program: `Command::new("git")` of
/// either kind, or `"git"` as an argv's first element, both of which
/// contain the literal.
fn names_git(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    code.contains("\"git\"")
}

#[test]
fn every_git_spawn_goes_through_trellis_vcs() {
    // The risk (review round 10, finding 2): `grep` and `find` started
    // plain `git`, without `trellis_vcs::OVERRIDES`, so a `core.fsmonitor`
    // that `bash` planted ran at the model's next listing. Every `git`
    // shipped code starts comes from `trellis_vcs::command`,
    // `command_with_hooks` or `git_argv`, which carry the overrides; a new
    // one that names `git` itself fails here.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut files = Vec::new();
    for c in std::fs::read_dir(crates).unwrap().flatten() {
        let src = c.path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    // Test-only modules kept in files of their own.
    let test_file = |p: &std::path::Path| {
        let n = p.file_name().unwrap().to_string_lossy();
        n == "tests.rs" || n.ends_with("_tests.rs") || n == "testutil.rs"
    };
    let mut allowed_seen = 0;
    let mut bare = Vec::new();
    for f in files.iter().filter(|f| !test_file(f)) {
        let rel = f
            .strip_prefix(crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(f).unwrap();
        for (i, line) in non_test_lines(&text) {
            if !names_git(line) {
                continue;
            }
            if GIT_ALLOWED.iter().any(|(p, _)| *p == rel) {
                allowed_seen += 1;
                continue;
            }
            bare.push(format!("{rel}:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        allowed_seen >= 4,
        "the scan saw {allowed_seen} allowed sites; it is not reading the source"
    );
    assert!(
        bare.is_empty(),
        "git started without trellis_vcs::command or git_argv, so a planted \
         core.fsmonitor or hook would run (ADR 0023 §5):\n{}",
        bare.join("\n")
    );
}

#[test]
fn the_git_scan_would_catch_a_bare_spawn() {
    // A hygiene test that cannot fail is decoration.
    assert!(names_git(
        r#"    let mut c = tokio::process::Command::new("git");"#
    ));
    assert!(names_git(r#"    let argv = ["git", "grep", "-n"];"#));
    assert!(!names_git(r#"    let mut git = trellis_vcs::command();"#));
    assert!(!names_git(r#"    // Command::new("git") in a comment"#));
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { x(\"git\"); }\n}\nfn c() {}\n";
    let kept: Vec<&str> = non_test_lines(src).into_iter().map(|(_, l)| l).collect();
    assert_eq!(kept, ["fn a() {}", "fn c() {}"]);
}
