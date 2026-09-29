//! Source-level guards, as tests rather than as a CI grep.
//!
//! Chat has the same guard over its own `src/tui/`. This crate's TUI is a
//! separate tree, and a guard in one crate does not read the other's files.

use std::path::Path;

const STDOUT_MACROS: [&str; 5] = ["println!", "eprintln!", "print!", "eprint!", "dbg!"];

/// What one scan saw: how many `.rs` files it read, and a `path: line` entry
/// for each use of a stdout macro. The count is returned so the caller can
/// refuse an empty scan, which would otherwise pass for a clean one.
struct Scan {
    files: usize,
    found: Vec<String>,
}

/// The guard and its self-test both call this, so the self-test exercises
/// the code the guard relies on rather than a copy of it.
fn scan_for_stdout_macros(dir: &Path) -> Scan {
    let mut files = Vec::new();
    read_dir_recursive(dir, &mut files);
    let mut found = Vec::new();
    for (path, text) in &files {
        for macro_name in STDOUT_MACROS {
            for line in offending_lines(text, macro_name) {
                found.push(format!("{path}: {}", line.trim()));
            }
        }
    }
    Scan {
        files: files.len(),
        found,
    }
}

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
fn stdout_hygiene_nothing_under_src_tui_writes_to_stdout() {
    // **Stdout belongs to the display.** A stray `println!` lands in the
    // alternate screen buffer, is not cleared by the next frame's diff, and
    // shows up as a line of text wedged between two borders, which reads as
    // a rendering bug and sends whoever hits it looking in the wrong file.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tui");
    let scan = scan_for_stdout_macros(&dir);
    // A guard over an empty set asserts nothing: a moved or renamed
    // directory would otherwise turn this test into a silent pass.
    assert!(
        scan.files > 0,
        "the directory being guarded must hold .rs files: {}",
        dir.display()
    );
    assert!(
        scan.found.is_empty(),
        "stdout is the display's:\n{}",
        scan.found.join("\n")
    );
}

#[test]
fn stdout_hygiene_guard_fires_on_a_planted_println() {
    // Defends against a guard that can never fail: a scanner that reads no
    // files, or matches nothing, passes the test above just as a clean tree
    // does. Planting one real use and one commented mention pins both
    // halves: the use is reported, the comment is not.
    let tmp = tempfile::tempdir().expect("tempdir");
    let nested = tmp.path().join("nested");
    std::fs::create_dir(&nested).expect("nested dir");
    std::fs::write(
        nested.join("planted.rs"),
        "fn f() {\n    println!(\"leak\");\n}\n",
    )
    .expect("write planted.rs");
    std::fs::write(
        tmp.path().join("commented.rs"),
        "fn g() {\n    // println!(\"only a mention\");\n}\n",
    )
    .expect("write commented.rs");

    let scan = scan_for_stdout_macros(tmp.path());
    assert_eq!(scan.files, 2, "both planted files must be read");
    assert_eq!(scan.found.len(), 1, "exactly one report: {:?}", scan.found);
    assert!(
        scan.found[0].contains("planted.rs") && scan.found[0].ends_with(": println!(\"leak\");"),
        "the report must name the planted file and line: {}",
        scan.found[0]
    );
}
