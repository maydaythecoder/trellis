//! `trellis-tickets scan` over a tree built at run time. `scan` takes no
//! `gh` closure at all, so there is nothing to stub: it cannot file.

use std::path::{Path, PathBuf};

use trellis_tickets::{Proposal, g4_markers, scan};

fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "trellis-tickets-scan-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    }
    root
}

fn found(root: &Path) -> Vec<(String, usize, String)> {
    scan(root)
        .unwrap()
        .into_iter()
        .map(|p| (p.path, p.line, p.marker))
        .collect()
}

#[test]
fn scan_prints_a_file_line_proposal_for_each_marker_in_production_source() {
    let root = tree(
        "prints",
        &[
            (
                "crates/a/src/lib.rs",
                "fn a() {}\r\n// TODO: resolve re-exports deeper than one hop\r\nfn b() { todo!() }\n",
            ),
            ("crates/a/src/clean.rs", "fn c() {}\n"),
        ],
    );
    let got = scan(&root).unwrap();
    assert_eq!(
        got.iter()
            .map(|p| (p.path.as_str(), p.line, p.marker.as_str()))
            .collect::<Vec<_>>(),
        [
            ("crates/a/src/lib.rs", 2, "TODO"),
            ("crates/a/src/lib.rs", 3, "todo!("),
        ]
    );
    assert_eq!(
        got[0].text(),
        "crates/a/src/lib.rs:2: proposed origin:code ticket (marker `TODO`): // TODO: resolve re-exports deeper than one hop\n"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn scan_skips_what_g4_skips_tests_examples_test_modules_and_non_rust() {
    // G4 lets a test's `panic!("not ...")` and a fixture's marker through;
    // proposing tickets for them would file work nobody needs.
    let marker = "// FIXME later\n";
    let root = tree(
        "skips",
        &[
            ("crates/a/tests/it.rs", marker),
            ("crates/a/examples/ex.rs", marker),
            ("crates/a/benches/b.rs", marker),
            ("crates/a/tests/fixtures/f.rs", marker),
            ("crates/a/src/parse_test.rs", marker),
            ("crates/a/src/notes.py", marker),
            ("target/debug/build/gen.rs", marker),
            (".claude/worktrees/x/src/lib.rs", marker),
            (
                "crates/a/src/lib.rs",
                "fn a() {}\n// XXX above the tests\n#[cfg(test)]\nmod tests {\n    // TODO in a test module\n}\n",
            ),
        ],
    );
    assert_eq!(
        found(&root),
        [("crates/a/src/lib.rs".to_string(), 2, "XXX".to_string())]
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn scan_markers_are_the_ones_g4_greps_for() {
    // Read from scripts/gates/no-stubs.sh, not copied: a marker added to G4
    // and not here would be a stub G4 blocks and scan never proposes.
    assert_eq!(
        g4_markers().unwrap(),
        [
            "TODO",
            "FIXME",
            "XXX",
            "unimplemented!",
            "todo!(",
            "panic!(\"not ",
            "NotImplemented",
            "mock_data",
            "MOCK_",
        ]
    );
    let root = tree(
        "markers",
        &[(
            "src/m.rs",
            "let x = mock_data();\nlet y = MOCK_Y;\nunimplemented!()\npanic!(\"not built\")\nNotImplemented\n",
        )],
    );
    let lines: Vec<usize> = found(&root).into_iter().map(|(_, l, _)| l).collect();
    assert_eq!(lines, [1, 2, 3, 4, 5]);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn scan_escapes_control_characters_in_the_line_it_prints() {
    // A source line is anyone's text; printed raw it could move the cursor
    // or recolour the terminal of whoever reads the proposals.
    let p = Proposal {
        path: "src/a.rs".to_string(),
        line: 1,
        marker: "TODO".to_string(),
        text: "// TODO \u{1b}[2J".to_string(),
    };
    assert!(!p.text().contains('\u{1b}'));
    assert!(p.text().contains("\\u{1b}[2J"));
}
