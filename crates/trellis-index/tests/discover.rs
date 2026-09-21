//! What the index decides is first-party.
//!
//! `discover` walks the repository and returns the `.py` files the graph will
//! be built from. Everything downstream inherits its answer: a file it skips
//! has no symbols, no edges, and contributes nothing to the recall
//! denominator — so getting it wrong does not produce an error, it produces a
//! smaller graph that still looks complete.
//!
//! It was reached by no test. The suite builds indexes with
//! `build_from_sources`, which takes `(path, source)` pairs already in
//! memory, so the whole filesystem half was exercised only by the CLI and by
//! `examples/audit_sample.rs`. Mutation testing reported six survivors here,
//! including `discover -> vec![]` — the graph being empty — and the mutants
//! on the skip-list and the extension filter.

use std::path::{Path, PathBuf};
use trellis_index::discover;

/// A repository tree, written from a list of relative paths.
fn tree(name: &str, files: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("trellis-discover-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x = 1\n").unwrap();
    }
    root
}

/// Paths relative to the root, with forward slashes, so assertions read like
/// the tree they describe rather than like this machine's filesystem.
fn found(root: &Path) -> Vec<String> {
    discover(root)
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn python_files_are_found_and_nothing_else_is() {
    let root = tree(
        "extensions",
        &[
            "app/__init__.py",
            "app/core.py",
            "app/nested/deep.py",
            "README.md",
            "app/data.json",
            "app/notes.txt",
            "Makefile",
            // A directory whose *name* ends in .py is not a Python file.
            "app/weird.py/inner.txt",
        ],
    );
    assert_eq!(
        found(&root),
        ["app/__init__.py", "app/core.py", "app/nested/deep.py"]
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn every_skipped_directory_is_actually_skipped() {
    // Listed one by one rather than as a loop over the constant, because the
    // constant is what is under test. A test that reads `SKIP_DIRS` would
    // agree with it however it changed.
    let skipped = [
        ".git",
        ".trellis",
        ".venv",
        "venv",
        "node_modules",
        "__pycache__",
        ".tox",
        ".nox",
        ".mypy_cache",
        ".pytest_cache",
        "site-packages",
        ".eggs",
        "build",
        "dist",
    ];

    let mut files = vec!["app/core.py".to_string()];
    for d in skipped {
        files.push(format!("{d}/hidden.py"));
        // And nested, because a vendored tree is rarely at the root.
        files.push(format!("app/{d}/hidden.py"));
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let root = tree("skipdirs", &refs);

    assert_eq!(
        found(&root),
        ["app/core.py"],
        "something under a skipped directory was indexed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_skipped_name_only_skips_a_directory_not_a_file() {
    // `build` is skipped as a directory. A *file* called `build.py` is
    // ordinary first-party code and must not disappear with it.
    let root = tree(
        "skipname",
        &["app/build.py", "app/dist.py", "build/generated.py"],
    );
    assert_eq!(found(&root), ["app/build.py", "app/dist.py"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_result_is_sorted_and_that_is_load_bearing() {
    // Ids are assigned by position in this vector, so the order *is* the id
    // space. `tests/determinism.rs` asserts that the graph is a function of
    // its sources and says in a comment that `discover` sorting is what makes
    // that true — and then builds its indexes with `build_from_sources`,
    // which never calls this. This is the missing half of that claim.
    let root = tree(
        "sorted",
        &[
            "z/last.py",
            "a/first.py",
            "m/middle.py",
            "a/aardvark.py",
            "app/core.py",
        ],
    );

    let once = found(&root);
    let mut expected = once.clone();
    expected.sort();
    assert_eq!(once, expected, "discover must return sorted paths");

    // And stable across calls, since a walk offers no ordering guarantee of
    // its own.
    for _ in 0..3 {
        assert_eq!(found(&root), once);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_tree_with_no_python_is_empty_rather_than_an_error() {
    // The state a user is in before they have written anything, and the one
    // `trellis index` has to report honestly rather than crash on.
    let root = tree("nopython", &["README.md", "src/main.rs"]);
    assert!(found(&root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
