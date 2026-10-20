//! Source-level guards, as tests rather than as a review habit.
//!
//! The rule here was applied by hand twice and missed sites both times. A rule
//! that depends on whoever is editing remembering it is not a rule; this makes
//! the next omission fail on `cargo test`, next to the change that caused it.

/// Every line of every `.rs` file under `src/`, as (file, 1-based line, text).
///
/// It read `main.rs` alone until #122 began splitting that file by command:
/// `cmd_log` and `cmd_why`, the commands that print trails, were the first to
/// move, and a guard that kept reading only `main.rs` would have stopped
/// seeing them while still passing.
fn src_lines() -> Vec<(String, usize, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    rust_files(&dir, &mut files);
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let name = path
            .strip_prefix(&dir)
            .unwrap_or(&path)
            .display()
            .to_string();
        let text = std::fs::read_to_string(&path).expect("a readable source file");
        for (i, l) in text.lines().enumerate() {
            out.push((name.clone(), i + 1, l.to_string()));
        }
    }
    out
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
    let lines = src_lines();
    assert!(
        lines.iter().any(|(_, _, l)| l.contains("fn cmd_log(")),
        "the scan no longer reaches `cmd_log`, the command it exists for"
    );
    for (file, n, line) in lines {
        let code = line.split("//").next().unwrap_or("");
        if !code.contains("println!") && !code.contains("writeln!") {
            continue;
        }
        for needle in FROM_A_TRAIL {
            if code.contains(needle) && !code.contains("plain(") {
                bad.push(format!("{file}:{n}: {}", line.trim()));
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
const CONVERSATIONAL: &[&str] = &["rpc.rs", "session.rs"];

#[test]
fn every_conversational_surface_asks_whether_the_input_is_a_task() {
    // Typing `hello` used to take a git snapshot, drive the loop, run the
    // repository's checks, roll back and write a trail. `ask::classify` fixed
    // that — in `trellis tui`, and nowhere else, because the fix was applied
    // where the failure was noticed. `trellis session` kept the old behaviour
    // for days with nothing to catch it.
    //
    // A third surface is the likely future mistake, so the rule is a test
    // rather than a thing to remember. The TUI's surface is `rpc.rs` since
    // the Rust TUI was retired (#516): `trellis-tui` sends what is typed
    // to `trellis rpc`, whose `classify` command (R1, #501) asks.
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
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with("#[") {
                j += 1;
            }
            // An item with no body (`mod extension_tests;`, a `use`) ends
            // at its `;`; counting braces past it hid the production code
            // that follows.
            let item = lines.get(j).copied().unwrap_or("");
            if let Some(semi) = item.find(';')
                && item.find('{').is_none_or(|brace| semi < brace)
            {
                i = j + 1;
                continue;
            }
            // Otherwise skip the item it marks, to the end of its first
            // balanced brace block.
            let mut depth = 0i64;
            let mut opened = false;
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

/// Where shipped code may name `git` itself, how many lines do, and why.
/// Counted exactly, so a new site in an allowed file is a deliberate edit
/// here rather than a pass.
const GIT_ALLOWED: &[(&str, usize, &str)] = &[
    (
        "trellis-vcs/src/lib.rs",
        13,
        "`command`, `command_with_hooks`, `git_argv`: the constructors; the other 10 are \
         `GitFailed` messages naming the command that failed (`\"git {}\"`, \
         `\"git hash-object ...\"`), which the shell-line rule reads as a spawn",
    ),
    (
        "trellis-cli/src/doctor_cmd.rs",
        1,
        "`trellis init`'s refusal prints `git init && ...` for the person to type",
    ),
    (
        "trellis-cli/src/process_agent.rs",
        2,
        "messages naming the `git` subcommand that failed; the spawn is `trellis_vcs::command`",
    ),
    (
        "trellis-loop/src/agent.rs",
        1,
        "prompt prose that begins a line with `git runs code ...`",
    ),
    (
        "trellis-loop/src/exec.rs",
        1,
        "`Program Files\\Git\\bin\\bash.exe`: `Git` is a directory name there",
    ),
    (
        "trellis-loop/src/orchestrator.rs",
        3,
        "messages about `git check-ignore`, which runs through `trellis_vcs`",
    ),
    (
        "trellis-tickets/src/new.rs",
        1,
        "G6's allowlist of Verify-line prefixes (`\"git \"` among them), compared \
         against a ticket's text; `new` never spawns git",
    ),
    (
        "trellis-vcs/src/git_guard.rs",
        2,
        "`GuardError` labels naming the command (`what: \"git rev-parse\"`)",
    ),
    (
        "trellis-vcs/src/ignored.rs",
        2,
        "`GuardError` labels naming the command (`what: \"git ls-files\"`)",
    ),
    (
        "trellis-vcs/src/history.rs",
        1,
        "the `GitFailed` message naming the command that failed (`\"git {}\"`); its \
         spawns are `command()` (the Atlas's reads, owner request of 2026-10-11)",
    ),
];

/// Whether `line` names `git` as a program in a string literal: `"git"`
/// in any case (Windows and macOS resolve `GIT` too), `"git.exe"` and the
/// other Windows spellings, a path ending in one (`/usr/bin/git`), or a
/// shell command line that runs one (`sh -c "git status"`, `cmd /C git`,
/// `"a && git add ."`): the first word of the literal or of any part after
/// `; & | (`, or the word after a flag that hands the rest to a shell.
///
/// Best effort, line by line, and its limits are stated rather than
/// assumed away. It does not see: a name built at run time
/// (`concat!("gi", "t")`, `format!`, bytes), a literal spanning lines, a
/// raw string holding a `"` (the line is misread from there), a command
/// after a newline escape inside one literal (`"cd x\ngit add ."`), a
/// program named in a file or the environment, `git` reached through
/// another program's own spawn (`xargs` with the name in a variable), or a
/// literal inside a `//` comment's tail after code that contains `//` in a
/// string. A `const` or `static` holding such a literal is caught at its
/// definition and at every use ([`git_consts`]).
fn names_git(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    string_literals(code).iter().any(|l| runs_git(l))
}

/// The string literals on one line of Rust, their contents as written.
/// A `'"'` char literal is not a string's start.
fn string_literals(code: &str) -> Vec<String> {
    let code = code.replace("'\\\"'", "").replace("'\"'", "");
    let mut out = Vec::new();
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut lit = String::new();
        let mut escaped = false;
        for c in chars.by_ref() {
            if escaped {
                lit.push(c);
                escaped = false;
            } else if c == '\\' {
                lit.push(c);
                escaped = true;
            } else if c == '"' {
                break;
            } else {
                lit.push(c);
            }
        }
        out.push(lit);
    }
    out
}

/// Whether one word is the git program: `git`, `git.exe`, `git.cmd`,
/// `git.bat`, `git.com` in any case, bare or at the end of a path.
fn is_git_program(word: &str) -> bool {
    let w = word
        .trim_matches(|c| matches!(c, '\'' | '`' | '\\'))
        .to_ascii_lowercase();
    let w = w.rsplit(['/', '\\']).next().unwrap_or("");
    matches!(w, "git" | "git.exe" | "git.cmd" | "git.bat" | "git.com")
}

/// Whether a string literal starts git: the whole literal, the first word
/// of it or of any command after a shell separator, or the word after a
/// flag that runs the rest (`/C`, `-c`, `-lc`, `exec`, `env`, `start`).
fn runs_git(lit: &str) -> bool {
    lit.split([';', '&', '|', '(']).any(|cmd| {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        words.first().is_some_and(|w| is_git_program(w))
            || words.windows(2).any(|w| {
                matches!(
                    w[0].to_ascii_lowercase().as_str(),
                    "/c" | "/k" | "-c" | "-lc" | "-ec" | "-command" | "exec" | "env" | "start"
                ) && is_git_program(w[1])
            })
    })
}

/// The name a `const` or `static` line defines, when its value is a
/// literal [`names_git`] matches: `const GIT: &str = "git";`. Its uses name
/// git without the literal, so they are scanned for too.
fn git_consts(line: &str) -> Option<String> {
    let code = line.split("//").next().unwrap_or("");
    if !names_git(code) {
        return None;
    }
    let after = code
        .split_once("const ")
        .or_else(|| code.split_once("static "))?
        .1;
    let name: String = after
        .trim_start_matches("mut ")
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Whether `line` uses the identifier `name` (a whole word, outside a
/// `//` comment).
fn uses_ident(line: &str, name: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    code.match_indices(name).any(|(i, _)| {
        !code[..i].chars().next_back().is_some_and(ident)
            && !code[i + name.len()..].chars().next().is_some_and(ident)
    })
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
        // `examples/` too: the measurement tooling is run against real
        // repositories.
        for d in ["src", "examples"] {
            let dir = c.path().join(d);
            if dir.is_dir() {
                rust_files(&dir, &mut files);
            }
        }
    }
    // Test-only modules kept in files of their own.
    let test_file = |p: &std::path::Path| {
        let n = p.file_name().unwrap().to_string_lossy();
        n == "tests.rs" || n.ends_with("_tests.rs") || n == "testutil.rs"
    };
    let mut allowed_seen: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut bare = Vec::new();
    let sources: Vec<(String, String)> = files
        .iter()
        .filter(|f| !test_file(f))
        .map(|f| {
            let rel = f
                .strip_prefix(crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(f).unwrap())
        })
        .collect();
    // A const holding git names it wherever it is used, in any file.
    let consts: Vec<String> = sources
        .iter()
        .flat_map(|(_, text)| {
            non_test_lines(text)
                .into_iter()
                .filter_map(|(_, l)| git_consts(l))
        })
        .collect();
    for (rel, text) in &sources {
        let rel = rel.as_str();
        for (i, line) in non_test_lines(text) {
            if !names_git(line) && !consts.iter().any(|c| uses_ident(line, c)) {
                continue;
            }
            if let Some((p, _, _)) = GIT_ALLOWED.iter().find(|(p, _, _)| *p == rel) {
                *allowed_seen.entry(p).or_default() += 1;
                continue;
            }
            bare.push(format!("{rel}:{}: {}", i + 1, line.trim()));
        }
    }
    let expected: std::collections::BTreeMap<&str, usize> =
        GIT_ALLOWED.iter().map(|(p, n, _)| (*p, *n)).collect();
    assert_eq!(
        allowed_seen, expected,
        "the allowed files name git on a different number of lines than listed: a \
         new site there, or the scan is not reading the source"
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
    // Review round 11: the other spellings, and git inside a shell line.
    assert!(names_git(r#"    Command::new("git.exe")"#));
    assert!(names_git(r#"    Command::new("GIT")"#));
    assert!(names_git(r#"    Command::new("/usr/bin/git")"#));
    assert!(names_git(
        r#"    Command::new("sh").args(["-c", "git status"])"#
    ));
    assert!(names_git(
        r#"    Command::new("sh").args(["-c", "cd x && git add ."])"#
    ));
    assert!(names_git(
        r#"    Command::new("cmd").args(["/C", "git", "status"])"#
    ));
    assert!(names_git(r#"    Command::new("cmd").arg("/C git status")"#));
    assert!(names_git(
        r#"    Command::new("bash").args(["-lc", "exec git fetch"])"#
    ));
    assert!(!names_git(r#"    bail!("not a git repository: {0}")"#));
    assert!(!names_git(r#"    let quote = '"'; let s = "a git b";"#));
    // A const: caught at its definition, and its uses are found by name.
    let def = r#"const PROGRAM: &str = "git";"#;
    assert!(names_git(def));
    assert_eq!(git_consts(def).as_deref(), Some("PROGRAM"));
    assert_eq!(
        git_consts(r#"static GIT_BIN: &str = "git.exe";"#).as_deref(),
        Some("GIT_BIN")
    );
    assert!(uses_ident("    let c = Command::new(PROGRAM);", "PROGRAM"));
    assert!(!uses_ident(
        "    let c = Command::new(PROGRAM_NAME);",
        "PROGRAM"
    ));
    let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { x(\"git\"); }\n}\nfn c() {}\n";
    let kept: Vec<&str> = non_test_lines(src).into_iter().map(|(_, l)| l).collect();
    assert_eq!(kept, ["fn a() {}", "fn c() {}"]);
    // A bodiless test item ends at its `;`: what follows is shipped code.
    let src = "fn a() {}\n#[cfg(test)]\nmod t;\nfn c() { x(\"git\"); }\n";
    let kept: Vec<&str> = non_test_lines(src).into_iter().map(|(_, l)| l).collect();
    assert_eq!(kept, ["fn a() {}", "fn c() { x(\"git\"); }"]);
}
