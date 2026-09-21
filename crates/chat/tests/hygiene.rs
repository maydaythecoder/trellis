//! Source-level guards, as tests rather than as a CI grep.
//!
//! A grep in CI catches these too, but only after a push, and only for
//! whoever reads the log. As a test they fail on `cargo test`, next to the
//! change that caused them.

use std::path::Path;

fn read_dir_recursive(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            read_dir_recursive(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs")
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            out.push((p.display().to_string(), text));
        }
    }
}

/// Lines with `//` before the needle are comments about it, not uses of it.
fn offending_lines<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|l| {
            let code = l.split("//").next().unwrap_or("");
            code.contains(needle)
        })
        .collect()
}

#[test]
fn nothing_under_src_tui_writes_to_stdout() {
    // **Stdout belongs to the display.** A stray `println!` lands in the
    // alternate screen buffer, is not cleared by the next frame's diff, and
    // shows up as a line of text wedged between two borders — which reads
    // as a rendering bug and sends whoever hits it looking in the wrong
    // file. `tracing` goes to a file subscriber and to the log pane.
    let mut files = Vec::new();
    read_dir_recursive(Path::new("src/tui"), &mut files);
    assert!(!files.is_empty(), "the directory being guarded must exist");

    let mut found = Vec::new();
    for (path, text) in &files {
        for macro_name in ["println!", "eprintln!", "print!", "eprint!", "dbg!"] {
            for line in offending_lines(text, macro_name) {
                found.push(format!("{path}: {}", line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "stdout is the display's:\n{}",
        found.join("\n")
    );
}

#[test]
fn update_contains_no_io_and_no_await() {
    // The property the whole Model/Message/Update/View split exists for. An
    // `.await` here would make the state transition suspend mid-frame; a
    // file read would make it depend on a disk. Either turns a function
    // that is testable with no terminal into one that is not.
    let text = std::fs::read_to_string("src/tui/app.rs").expect("app.rs");
    let start = text
        .find("impl AppState {")
        .expect("the impl block must be findable");
    let tests = text.find("#[cfg(test)]").unwrap_or(text.len());
    let body = &text[start..tests];

    for banned in [
        ".await",
        "std::fs::",
        "reqwest",
        "Command::new",
        "std::process",
        "tokio::",
    ] {
        let hits = offending_lines(body, banned);
        assert!(
            hits.is_empty(),
            "`{banned}` in AppState's impl block — update must stay pure:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn the_interface_reaches_crossterm_only_through_ratatui() {
    // `crossterm` is a direct dependency solely to turn on `event-stream`.
    // Importing it directly risks binding a *different* version than the
    // one ratatui re-exports, and the symptom is a type error about two
    // `KeyEvent`s that print identically.
    let mut files = Vec::new();
    read_dir_recursive(Path::new("src"), &mut files);

    let mut found = Vec::new();
    for (path, text) in &files {
        for line in offending_lines(text, "use crossterm") {
            found.push(format!("{path}: {}", line.trim()));
        }
    }
    assert!(
        found.is_empty(),
        "reach it through `ratatui::crossterm::...`:\n{}",
        found.join("\n")
    );
}

#[test]
fn no_ratatui_sub_crate_is_imported_directly() {
    // Depending on both `ratatui` and `ratatui-core` puts two copies of
    // `Style` in one build, and the error message does not say so.
    let manifest = std::fs::read_to_string("Cargo.toml").expect("Cargo.toml");
    for banned in ["ratatui-core", "ratatui-widgets", "ratatui-macros"] {
        assert!(
            !manifest.contains(&format!("\n{banned} ")),
            "`{banned}` must not be a direct dependency"
        );
    }
}
