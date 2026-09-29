//! ADR 0007 on a real merge: the index is regenerated at the merge and at
//! its first parent, never merged, and the symbol delta between the two is
//! paired with the committed trails that touched each changed file.
//!
//! Feeding the result into a run's prompt is a separate change to what the
//! model is shown; this holds the delta and the pairing to a fixture.

use std::path::Path;

/// Every spawn goes through `trellis_vcs::command()`, as shipped code must:
/// a test that ran plain `git` would not exercise the overrides the
/// regeneration path runs under.
fn git(root: &Path, args: &[&str]) -> String {
    let out = trellis_vcs::command()
        .args([
            "-c",
            "user.name=trellis-test",
            "-c",
            "user.email=trellis-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .expect("the git program must be on PATH to run these tests");
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The sources at `rev`, read by the same walker `trellis index` uses, so
/// the fixture cannot pass on a discovery rule the product does not have.
fn sources_at(root: &Path, rev: &str) -> Vec<(String, String)> {
    git(root, &["checkout", "-q", "--detach", rev]);
    trellis_index::read_sources(root)
}

/// The risk: a merge brings in a change the current branch never saw, and
/// the loop is either not told (the delta misses it) or told about work
/// that was already on this side (the delta is taken against the wrong
/// parent). Diffing against the first parent lists exactly what the merge
/// brought in, and the trail that proposed it comes with it.
#[test]
fn merge_delta_lists_the_merged_change_with_its_trail_and_not_the_first_parents() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    git(root, &["init", "-q"]);

    write(root, "a.py", "def f():\n    return 1\n");
    write(root, "b.py", "def g():\n    return 1\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "feature"]);

    // The first-parent side: `g` changes here, before the merge.
    write(root, "b.py", "def g():\n    return 2\n");
    git(root, &["commit", "-q", "-am", "edit g"]);

    // The merged branch: `f` changes, and a run's trail records it.
    git(root, &["checkout", "-q", "feature"]);
    write(root, "a.py", "def f():\n    return 42\n");
    write(
        root,
        ".trellis/runs/20260928-120000-000-edit-f.json",
        r#"{"run_id":"20260928-120000-000-edit-f","task":"make f return 42","factual":{"proposed":["a.py"],"actions":[{"turn":1,"tool":"edit","target":"edit a.py","outcome":"applied","wrote":true}]}}"#,
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "edit f"]);

    git(root, &["checkout", "-q", "-"]);
    git(
        root,
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );
    let merge = git(root, &["rev-parse", "HEAD"]);

    // A merge has a second parent; the delta is taken against the first.
    git(root, &["rev-parse", "--verify", "-q", "HEAD^2"]);
    let first_parent = git(root, &["rev-parse", "HEAD^1"]);

    // Regenerate both sides from their sources: derived state, never merged.
    let before_src = sources_at(root, &first_parent);
    let after_src = sources_at(root, &merge);
    let before = trellis_index::build_from_sources(&before_src);
    let after = trellis_index::build_from_sources(&after_src);
    let delta = trellis_index::symbol_delta(
        trellis_index::IndexedSources {
            index: &before,
            sources: &before_src,
        },
        trellis_index::IndexedSources {
            index: &after,
            sources: &after_src,
        },
    );

    // Pair each change with the trails, read at the merge, that touched its file.
    let entries = trellis_trail::query::all(root);
    let paired: Vec<(String, trellis_index::ChangeKind, Vec<String>)> = delta
        .iter()
        .map(|c| {
            let runs = trellis_trail::query::touching(&entries, &c.file)
                .iter()
                .map(|t| t.entry.run_id())
                .collect();
            (c.qualified.clone(), c.kind, runs)
        })
        .collect();

    assert_eq!(
        paired,
        vec![(
            "a.f".to_string(),
            trellis_index::ChangeKind::Changed,
            vec!["20260928-120000-000-edit-f".to_string()],
        )],
        "f comes with the trail that proposed a.py; g was already on the first parent"
    );
}
