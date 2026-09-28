//! The TypeScript twin's sixteen tests (`ctx/validate.test.ts`,
//! `ctx/compile.test.ts`), ported by behaviour, plus the three golden files
//! both implementations are held to.
//!
//! Bad vaults are built by copying the shared fixture into a temporary
//! directory and breaking one thing, so each test names exactly one rule.

use std::path::{Path, PathBuf};
use trellis_vault::{Compiled, DEFAULT_CAP, Stats, VaultError, compile, validate};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture() -> PathBuf {
    fixtures().join("vault")
}

fn copy_vault() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for e in std::fs::read_dir(fixture()).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), d.path().join(e.file_name())).unwrap();
    }
    d
}

fn edit(path: &Path, f: impl FnOnce(&str) -> String) {
    let text = std::fs::read_to_string(path).unwrap();
    std::fs::write(path, f(&text)).unwrap();
}

fn to_crlf(dir: &Path) {
    for e in std::fs::read_dir(dir).unwrap() {
        edit(&e.unwrap().path(), |t| {
            t.replace("\r\n", "\n").replace('\n', "\r\n")
        });
    }
}

/// The twin's `setParent`: replaces the first `parent:` line.
fn set_parent(dir: &Path, file: &str, parent: &str) {
    edit(&dir.join(file), |t| {
        t.lines()
            .map(|l| {
                if l.starts_with("parent: ") {
                    format!("parent: {parent}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    });
}

fn add_extra_key(dir: &Path) {
    edit(&dir.join("task-a.md"), |t| {
        t.replacen("title: ", "tags: x\ntitle: ", 1)
    });
}

/// The violations and stats of a vault that must be invalid.
fn invalid(dir: &Path) -> (Vec<String>, Stats) {
    match validate(dir) {
        Err(VaultError::Invalid { violations, stats }) => (violations, stats),
        other => panic!("expected an invalid vault, got {other:?}"),
    }
}

fn has(violations: &[String], needle: &str) -> bool {
    violations.iter().any(|v| v.contains(needle))
}

fn ok(dir: &Path, stem: &str) -> Compiled {
    compile(dir, stem, DEFAULT_CAP).unwrap_or_else(|e| panic!("{e}"))
}

const FIXTURE_STATS: &str = "notes=5 parentless=2 connected=4 manifest_missing=0 manifest_extra=0";

// ---------------------------------------------------------------------------
// validate.test.ts
// ---------------------------------------------------------------------------

#[test]
fn fixture_vault_is_valid_and_reports_the_expected_stats() {
    let s = validate(&fixture()).unwrap();
    assert_eq!(s.to_string(), FIXTURE_STATS);
}

#[test]
fn crlf_line_endings_are_accepted() {
    // git's autocrlf rewrites every file on a Windows checkout. A vault that
    // validates on one machine and not another is not a format.
    let d = copy_vault();
    to_crlf(d.path());
    assert_eq!(validate(d.path()).unwrap().to_string(), FIXTURE_STATS);
}

#[test]
fn extra_frontmatter_key_is_rejected() {
    // Two keys, exactly. A third is where tags, status and dates start, and
    // each is a field the compiler would then be asked to interpret.
    let d = copy_vault();
    add_extra_key(d.path());
    let (v, _) = invalid(d.path());
    assert!(has(&v, "unexpected frontmatter key \"tags\""), "{v:?}");
}

#[test]
fn unresolved_parent_is_rejected() {
    let d = copy_vault();
    set_parent(d.path(), "task-b.md", "\"[[nowhere]]\"");
    let (v, _) = invalid(d.path());
    assert!(has(&v, "parent [[nowhere]] does not resolve"), "{v:?}");
}

#[test]
fn parent_cycle_is_rejected() {
    // A cycle would make lineage walking loop forever; compile relies on
    // validation to have ruled it out.
    let d = copy_vault();
    set_parent(d.path(), "root.md", "\"[[task-a]]\"");
    let (v, _) = invalid(d.path());
    assert!(
        has(&v, "parent cycle: project.md -> root.md -> task-a.md"),
        "{v:?}"
    );
    assert_eq!(
        v.iter().filter(|x| x.starts_with("parent cycle")).count(),
        1,
        "one cycle, reported once rather than once per member: {v:?}"
    );
}

#[test]
fn note_missing_from_manifest_is_rejected() {
    let d = copy_vault();
    std::fs::write(
        d.path().join("extra.md"),
        "---\ntitle: \"Extra\"\nparent: null\n---\nbody\n",
    )
    .unwrap();
    let (v, s) = invalid(d.path());
    assert_eq!((s.manifest_missing, s.manifest_extra), (1, 0));
    assert!(has(&v, "manifest.json: missing extra.md"), "{v:?}");
}

#[test]
fn manifest_entry_with_no_file_is_rejected() {
    let d = copy_vault();
    std::fs::remove_file(d.path().join("task-b.md")).unwrap();
    let (v, s) = invalid(d.path());
    assert_eq!((s.manifest_missing, s.manifest_extra), (0, 1));
    assert!(has(&v, "lists task-b.md, which does not exist"), "{v:?}");
}

#[test]
fn unsorted_manifest_is_rejected() {
    let d = copy_vault();
    std::fs::write(
        d.path().join("manifest.json"),
        r#"["project.md","glossary.md","root.md","task-a.md","task-b.md"]"#,
    )
    .unwrap();
    let (v, s) = invalid(d.path());
    assert_eq!((s.manifest_missing, s.manifest_extra), (0, 0));
    assert!(has(&v, "not sorted"), "{v:?}");
}

// ---------------------------------------------------------------------------
// compile.test.ts
// ---------------------------------------------------------------------------

#[test]
fn lineage_runs_root_to_target_with_bodies_target_last() {
    let t = ok(&fixture(), "task-a").text;
    let root = t.find("### Root (root.md)").unwrap();
    let project = t.find("### Project (project.md)").unwrap();
    let target = t.find("### Task A (task-a.md) [target]").unwrap();
    assert!(root < project && project < target, "{t}");
    for body in [
        "Top of the fixture hierarchy.",
        "The fixture project.",
        "MARKER-A",
    ] {
        assert!(t.contains(body), "{body} missing:\n{t}");
    }
    assert!(t.starts_with("# Context: Task A\n"), "{t}");
}

#[test]
fn children_appear_as_titles_only_sorted_by_stem() {
    let t = ok(&fixture(), "project").text;
    let a = t.find("- Task A (task-a.md)").unwrap();
    let b = t.find("- Task B (task-b.md)").unwrap();
    assert!(a < b, "{t}");
    assert!(!t.contains("MARKER-A"), "child body must not be included");
    assert!(!t.contains("MARKER-B"), "child body must not be included");
}

#[test]
fn linked_note_body_is_included_one_hop() {
    let t = ok(&fixture(), "task-a").text;
    assert!(
        t.contains("### Glossary (glossary.md)\n\nGLOSSARY-BODY"),
        "{t}"
    );
}

#[test]
fn unresolved_link_is_listed_on_its_own_line() {
    let t = ok(&fixture(), "task-a").text;
    assert!(t.contains("## Unresolved\n\n- [[missing-note]]\n"), "{t}");
}

#[test]
fn low_cap_drops_the_reference_body_but_keeps_its_heading_and_lower_cap_fails() {
    let full = ok(&fixture(), "task-a");
    assert_eq!(full.dropped_reference_bodies, 0);

    let dropped = compile(&fixture(), "task-a", full.bytes - 1).unwrap();
    assert!(!dropped.text.contains("GLOSSARY-BODY"), "{}", dropped.text);
    assert!(dropped.text.contains("### Glossary (glossary.md)"));
    assert!(
        dropped.text.contains("MARKER-A"),
        "target is never truncated"
    );
    assert!(dropped.bytes < full.bytes);
    assert_eq!(dropped.dropped_reference_bodies, 1);

    match compile(&fixture(), "task-a", dropped.bytes - 1) {
        Err(e @ VaultError::OverCap { .. }) => {
            assert!(e.to_string().contains("cap"), "{e}");
        }
        other => panic!("expected OverCap, got {other:?}"),
    }
}

#[test]
fn unknown_stem_is_its_own_error() {
    assert!(matches!(
        compile(&fixture(), "no-such-note", DEFAULT_CAP),
        Err(VaultError::UnknownStem(s)) if s == "no-such-note"
    ));
}

#[test]
fn invalid_vault_is_refused_before_anything_is_compiled() {
    let d = copy_vault();
    add_extra_key(d.path());
    assert!(matches!(
        compile(d.path(), "task-a", DEFAULT_CAP),
        Err(VaultError::Invalid { .. })
    ));
}

#[test]
fn two_runs_are_byte_identical_and_crlf_input_yields_the_same_lf_output() {
    let one = ok(&fixture(), "task-a");
    let two = ok(&fixture(), "task-a");
    assert_eq!(one, two);
    assert!(!one.text.contains('\r'));

    let d = copy_vault();
    to_crlf(d.path());
    assert_eq!(ok(d.path(), "task-a"), one);
}

// ---------------------------------------------------------------------------
// Shared goldens. The TypeScript twin asserts the same three files.
// ---------------------------------------------------------------------------

fn golden(stem: &str) {
    let expected = std::fs::read(fixtures().join(format!("expected-{stem}.md"))).unwrap();
    let got = ok(&fixture(), stem);
    assert_eq!(
        got.text.as_bytes(),
        expected.as_slice(),
        "compiled {stem} differs from its golden:\n{}",
        got.text
    );
    assert_eq!(got.bytes, expected.len());
}

#[test]
fn golden_task_a() {
    golden("task-a");
}

#[test]
fn golden_project() {
    golden("project");
}

#[test]
fn golden_task_b() {
    golden("task-b");
}

// ---------------------------------------------------------------------------
// `[context] dates` (ADR 0022 §7, integrated). The goldens are the
// specification of the selection rule, so the annotation must leave them
// byte-identical whenever it is off or has nothing to say.
// ---------------------------------------------------------------------------

fn dated(entries: &[(&str, Option<&str>, bool)]) -> trellis_vault::dates::Document {
    let mut doc = trellis_vault::dates::Document::default();
    for (path, day, estimated) in entries {
        doc.notes.insert(
            path.to_string(),
            trellis_vault::dates::NoteDates {
                created: trellis_vault::dates::Stamp {
                    day: day.map(str::to_string),
                    estimated: *estimated,
                },
                lines: Vec::new(),
            },
        );
    }
    doc
}

#[test]
fn a_dates_json_beside_the_vault_does_not_change_the_goldens_when_the_flag_is_off() {
    // The risk: the flag's plumbing reading dates.json whenever it exists,
    // so a vault Apex has opened compiles differently from its golden.
    let d = copy_vault();
    let doc = dated(&[("task-a.md", Some("2026-03-01"), false)]);
    std::fs::write(
        d.path().join("dates.json"),
        trellis_vault::dates::to_bytes(&doc),
    )
    .unwrap();
    for stem in ["task-a", "project", "task-b"] {
        let expected = std::fs::read(fixtures().join(format!("expected-{stem}.md"))).unwrap();
        let off = trellis_vault::compile_with_dates(d.path(), stem, DEFAULT_CAP, None).unwrap();
        assert_eq!(off.text.as_bytes(), expected.as_slice(), "{stem}");
        assert_eq!(ok(d.path(), stem).text.as_bytes(), expected.as_slice());
        // On, with a document that dates none of these notes: still the
        // golden. Nothing is guessed for a note with no entry.
        let none = dated(&[]);
        let on =
            trellis_vault::compile_with_dates(d.path(), stem, DEFAULT_CAP, Some(&none)).unwrap();
        assert_eq!(on.text.as_bytes(), expected.as_slice(), "{stem}");
    }
}

#[test]
fn with_dates_on_lineage_and_reference_headings_carry_the_created_day() {
    let doc = dated(&[
        ("root.md", Some("2025-01-02"), true),
        ("task-a.md", Some("2026-03-01"), false),
        ("glossary.md", Some("2025-06-30"), false),
        // A null day is written by Apex for a note it could not date.
        ("project.md", None, false),
    ]);
    let c =
        trellis_vault::compile_with_dates(&fixture(), "task-a", DEFAULT_CAP, Some(&doc)).unwrap();
    assert!(
        c.text
            .contains("### Root (root.md, created 2025-01-02, estimated)\n"),
        "{}",
        c.text
    );
    assert!(c.text.contains("### Project (project.md)\n"), "{}", c.text);
    assert!(
        c.text
            .contains("### Task A (task-a.md, created 2026-03-01) [target]\n"),
        "{}",
        c.text
    );
    assert!(
        c.text
            .contains("### Glossary (glossary.md, created 2025-06-30)\n"),
        "{}",
        c.text
    );
    assert_eq!(c.bytes, c.text.len());
}

// ---------------------------------------------------------------------------
// Beyond the twin's suite: the error variants a caller branches on.
// ---------------------------------------------------------------------------

#[test]
fn an_ambiguous_stem_names_every_candidate() {
    let d = copy_vault();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    std::fs::write(
        d.path().join("sub/task-b.md"),
        "---\ntitle: \"Other B\"\nparent: null\n---\nx\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("manifest.json"),
        r#"["glossary.md","project.md","root.md","sub/task-b.md","task-a.md","task-b.md"]"#,
    )
    .unwrap();
    match compile(d.path(), "task-b", DEFAULT_CAP) {
        Err(VaultError::AmbiguousStem { paths, .. }) => {
            assert_eq!(paths, ["sub/task-b.md", "task-b.md"]);
        }
        other => panic!("expected AmbiguousStem, got {other:?}"),
    }
}

#[test]
fn a_path_that_is_not_a_directory_is_a_violation_not_a_panic() {
    let d = tempfile::tempdir().unwrap();
    let (v, _) = invalid(&d.path().join("absent"));
    assert!(has(&v, "not a directory"), "{v:?}");
}
