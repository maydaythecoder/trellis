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
