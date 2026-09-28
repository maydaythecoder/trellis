//! Apex parity (ADR 0022): the lenient reader, the graph index and its
//! issues, the edits, and what each does to the sidecar files.
//!
//! Every vault here starts as a copy of the shared fixture, so the goldens'
//! vault is also the one the edits are exercised on.

use std::path::{Path, PathBuf};
use trellis_vault::dates::{self, Touched};
use trellis_vault::edit;
use trellis_vault::{DEFAULT_CAP, Graph, Issue, Vault, VaultError, compile, validate};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vault")
}

fn copy_vault() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for e in std::fs::read_dir(fixture()).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), d.path().join(e.file_name())).unwrap();
    }
    d
}

fn read(dir: &Path, f: &str) -> String {
    std::fs::read_to_string(dir.join(f)).unwrap()
}

fn write(dir: &Path, f: &str, s: &str) {
    if let Some(p) = dir.join(f).parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(dir.join(f), s).unwrap();
}

fn manifest(dir: &Path, paths: &[&str]) {
    write(dir, "manifest.json", &serde_json::to_string(paths).unwrap());
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

fn idx(v: &Vault, path: &str) -> usize {
    v.index_of(path).unwrap()
}

const FIXTURE_LINE: &str =
    "notes=5 parentless=2 connected=4 manifest_missing=0 manifest_extra=0 fixes=[]";

// ---------------------------------------------------------------------------
// The report line and the gate
// ---------------------------------------------------------------------------

#[test]
fn check_prints_apex_s_report_line_for_the_fixture_and_the_gate_accepts_it() {
    // An agent following Apex's SKILL.md ends with exactly this line; trellis
    // printing the same one is what lets that agent verify with trellis.
    let r = edit::check(&fixture(), false).unwrap();
    assert_eq!(r.line, FIXTURE_LINE);
    assert!(r.gate_ok());
}

#[test]
fn fix_on_a_correct_vault_writes_nothing() {
    // A check that rewrote canonical notes would dirty the worktree a run
    // requires clean (ADR 0009) — every time it was run.
    let d = copy_vault();
    let before = snapshot(d.path());
    let r = edit::check(d.path(), true).unwrap();
    assert_eq!(r.line, FIXTURE_LINE);
    assert_eq!(snapshot(d.path()), before);
}

#[test]
fn fix_strips_an_extra_key_the_way_compose_raw_does_and_the_gate_then_accepts() {
    let d = copy_vault();
    let p = d.path().join("task-a.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replacen("title: ", "tags: x\ntitle: ", 1)).unwrap();
    // The run gate stays strict about the schema.
    assert!(validate(d.path()).is_err());
    let r = edit::check(d.path(), true).unwrap();
    assert!(
        r.line.ends_with("fixes=[frontmatter:task-a.md]"),
        "{}",
        r.line
    );
    assert!(r.gate_ok(), "{:?}", r.gate);
    assert_eq!(
        read(d.path(), "task-a.md"),
        "---\ntitle: \"Task A\"\nparent: \"[[project]]\"\n---\n\nMARKER-A\n\nSee [[glossary]] and [[missing-note]].\n"
    );
}

#[test]
fn fix_rewrites_a_wrong_manifest_in_apex_s_format() {
    let d = copy_vault();
    write(d.path(), "manifest.json", r#"["task-b.md","glossary.md"]"#);
    let r = edit::check(d.path(), true).unwrap();
    assert!(r.line.contains("fixes=[manifest.json]"), "{}", r.line);
    assert_eq!(
        read(d.path(), "manifest.json"),
        "[\n  \"glossary.md\",\n  \"project.md\",\n  \"root.md\",\n  \"task-a.md\",\n  \"task-b.md\"\n]\n"
    );
    assert!(r.gate_ok());
}

// ---------------------------------------------------------------------------
// Reading and resolution
// ---------------------------------------------------------------------------

#[test]
fn every_apex_null_form_is_a_null_parent_at_the_gate() {
    // Apex writes and reads these as "no parent"; a gate that refused them
    // would refuse a vault Apex considers well-formed.
    for form in ["~", "\"\"", "undefined", "NULL"] {
        let d = copy_vault();
        let p = d.path().join("task-b.md");
        let t = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, t.replace("\"[[project]]\"", form)).unwrap();
        let s = validate(d.path()).unwrap_or_else(|e| panic!("{form}: {e}"));
        assert_eq!(s.parentless, 3, "{form}");
    }
}

#[test]
fn a_title_form_parent_resolves_at_the_gate_and_in_compile() {
    let d = copy_vault();
    let p = d.path().join("task-b.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("[[project]]", "[[Project]]")).unwrap();
    validate(d.path()).unwrap();
    let c = compile(d.path(), "project", DEFAULT_CAP).unwrap();
    assert!(c.text.contains("- Task B (task-b.md)"), "{}", c.text);
}

#[test]
fn a_path_shortcut_wins_over_a_shared_basename() {
    // Apex's resolveNotePath tries `ref.md` before the alias map, so
    // [[task-b]] is the root-level note even when sub/task-b.md exists.
    let d = copy_vault();
    write(
        d.path(),
        "sub/task-b.md",
        "---\ntitle: \"Other B\"\nparent: \"[[task-b]]\"\n---\nx\n",
    );
    manifest(
        d.path(),
        &[
            "glossary.md",
            "project.md",
            "root.md",
            "sub/task-b.md",
            "task-a.md",
            "task-b.md",
        ],
    );
    validate(d.path()).unwrap();
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    assert_eq!(
        g.parents[idx(&v, "sub/task-b.md")],
        Some(idx(&v, "task-b.md"))
    );
    // The basename is shared, so it is a duplicate alias.
    assert!(g.issues.iter().any(|i| matches!(i,
        Issue::DuplicateAlias { paths, keys } if paths.len() == 2 && keys.contains(&"task-b".to_string()))));
}

#[test]
fn a_link_by_title_to_a_lineage_note_is_not_repeated_as_a_reference() {
    let d = copy_vault();
    let p = d.path().join("task-a.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{t}\n[[Root]] [[GLOSSARY]]\n")).unwrap();
    let c = compile(d.path(), "task-a", DEFAULT_CAP).unwrap();
    assert_eq!(
        c.text.matches("### Root (root.md)").count(),
        1,
        "{}",
        c.text
    );
    assert_eq!(
        c.text.matches("### Glossary (glossary.md)").count(),
        1,
        "{}",
        c.text
    );
}

#[test]
fn notes_subfolder_is_the_root_and_dotfiles_are_skipped() {
    let d = tempfile::tempdir().unwrap();
    for e in std::fs::read_dir(fixture()).unwrap() {
        let e = e.unwrap();
        std::fs::create_dir_all(d.path().join("notes")).unwrap();
        std::fs::copy(e.path(), d.path().join("notes").join(e.file_name())).unwrap();
    }
    write(d.path(), "notes/.hidden.md", "not a note");
    write(d.path(), "notes/.obsidian/x.md", "not a note");
    assert_eq!(validate(d.path()).unwrap().notes, 5);
    assert_eq!(edit::check(d.path(), false).unwrap().line, FIXTURE_LINE);
}

#[test]
fn graph_has_apex_s_roots_depth_refs_and_issues() {
    let v = Vault::open(&fixture()).unwrap();
    let g = Graph::build(&v);
    let root = idx(&v, "root.md");
    let task_a = idx(&v, "task-a.md");
    let glossary = idx(&v, "glossary.md");
    assert_eq!(g.roots, [glossary, root]);
    assert_eq!(g.depth[task_a], Some(2));
    assert_eq!(g.refs_out[task_a], [(glossary, 1)]);
    assert_eq!(g.refs_in[glossary], [(task_a, 1)]);
    // Glossary is a loose note: informational, not a failure.
    assert_eq!(
        g.issues,
        [Issue::Disconnected {
            path: "glossary.md".into()
        }]
    );
}

#[test]
fn a_repeated_link_adds_weight_and_a_parent_link_adds_no_edge() {
    let d = copy_vault();
    let p = d.path().join("task-a.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{t}\n[[glossary]] [[project]] [[task-a]]\n")).unwrap();
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    let (a, gl) = (idx(&v, "task-a.md"), idx(&v, "glossary.md"));
    assert_eq!(g.refs_out[a], [(gl, 2)]);
}

/// Vectors from Apex's `web/graphModel.ts` at f449644a under Node v25.6.0:
/// the fixture plus `sub/task-b.md` (parent `[[task-b]]`) and a second line
/// in task-a (`[[glossary]] [[project]] [[task-a]]`), through `parseNote`
/// and `createGraphIndex`:
///
/// ```text
/// parents.get("sub/task-b.md")   = "task-b.md"
/// resolvePath("Task B")          = "task-b.md"
/// resolvePath("Other B")         = "sub/task-b.md"
/// refsOut.get("task-a.md")       = [["glossary.md", 2]]
/// roots                          = ["glossary.md", "root.md"]
/// derivedLevels                  = glossary 0, root 0, project 1,
///                                  task-a 2, task-b 2, sub/task-b 3
/// duplicateAliases               = [["task-b", ["sub/task-b.md", "task-b.md"]]]
/// issues                         = []   (Apex never raises duplicate-alias)
/// ```
#[test]
fn graph_matches_apex_vectors() {
    let d = copy_vault();
    let p = d.path().join("task-a.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{t}[[glossary]] [[project]] [[task-a]]\n")).unwrap();
    write(
        d.path(),
        "sub/task-b.md",
        "---\ntitle: \"Other B\"\nparent: \"[[task-b]]\"\n---\nx\n",
    );
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    let i = |p: &str| idx(&v, p);
    assert_eq!(g.parents[i("sub/task-b.md")], Some(i("task-b.md")));
    assert_eq!(g.resolver.resolve("Task B").note(), Some(i("task-b.md")));
    assert_eq!(
        g.resolver.resolve("Other B").note(),
        Some(i("sub/task-b.md"))
    );
    assert_eq!(g.refs_out[i("task-a.md")], [(i("glossary.md"), 2)]);
    assert_eq!(g.roots, [i("glossary.md"), i("root.md")]);
    let depth: Vec<_> = [
        "glossary.md",
        "root.md",
        "project.md",
        "task-a.md",
        "task-b.md",
        "sub/task-b.md",
    ]
    .iter()
    .map(|p| g.depth[i(p)])
    .collect();
    assert_eq!(
        depth,
        [Some(0), Some(0), Some(1), Some(2), Some(2), Some(3)]
    );
    // Trellis raises what Apex only defines.
    let dup: Vec<_> = g
        .issues
        .iter()
        .filter_map(|x| match x {
            Issue::DuplicateAlias { paths, keys } => Some((paths.clone(), keys.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        dup,
        [(
            vec!["sub/task-b.md".to_string(), "task-b.md".to_string()],
            vec!["task-b".to_string()]
        )]
    );
}

#[test]
fn a_missing_parent_is_neither_a_root_nor_given_a_depth() {
    // Depth 0 would say "top of the hierarchy" about a note whose place in
    // it is unknown.
    let d = copy_vault();
    let p = d.path().join("task-b.md");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("[[project]]", "[[nowhere]]")).unwrap();
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    let b = idx(&v, "task-b.md");
    assert!(!g.roots.contains(&b));
    assert_eq!(g.depth[b], None);
    assert!(g.issues.iter().any(|i| {
        i.kind() == "missing-parent"
            && i.to_string()
                .contains("Task B has a missing parent [[nowhere]]")
    }));
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

#[test]
fn new_slugifies_deduplicates_and_keeps_the_manifest_correct() {
    let d = copy_vault();
    let (p, out) = edit::create(d.path(), "Task A", Some("project")).unwrap();
    // "Task A" is taken, so the title gets " 2" and the file its slug.
    assert_eq!(p, "task-a-2.md");
    assert_eq!(
        read(d.path(), &p),
        "---\ntitle: \"Task A 2\"\nparent: \"[[project]]\"\n---\n\n"
    );
    assert_eq!(out.touched.created, std::slice::from_ref(&p));
    // A taken title is suffixed again, as getAvailableDisplayTitle does.
    let (p2, _) = edit::create(d.path(), "Task A 2", None).unwrap();
    assert_eq!(p2, "task-a-2-2.md");
    // A free title whose slug is taken gets the next free `-N` path.
    let (p3, _) = edit::create(d.path(), "task a!", None).unwrap();
    assert_eq!(p3, "task-a-3.md");
    assert!(read(d.path(), &p3).contains("title: \"task a!\""));
    assert!(read(d.path(), "manifest.json").contains("\"task-a-2.md\""));
    validate(d.path()).unwrap();
}

#[test]
fn reparent_refuses_a_descendant_and_rewrites_only_the_child() {
    let d = copy_vault();
    match edit::reparent(d.path(), "root", Some("task-a")) {
        Err(VaultError::Refused(m)) => assert!(m.contains("descendants"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let out = edit::reparent(d.path(), "glossary", Some("Root")).unwrap();
    assert_eq!(out.touched.modified, ["glossary.md"]);
    assert!(read(d.path(), "glossary.md").contains("parent: \"[[root]]\""));
    edit::reparent(d.path(), "glossary", None).unwrap();
    assert!(read(d.path(), "glossary.md").contains("parent: null"));
    validate(d.path()).unwrap();
}

#[test]
fn link_appends_once() {
    let d = copy_vault();
    edit::link(d.path(), "task-b", "Glossary").unwrap();
    assert!(read(d.path(), "task-b.md").ends_with("MARKER-B\n\n[[glossary]]\n"));
    let again = edit::link(d.path(), "task-b", "glossary").unwrap();
    assert!(again.touched.modified.is_empty());
    assert!(again.messages[0].contains("already links"));
}

#[test]
fn retitle_rewrites_every_link_that_resolved_to_the_note_keeping_labels() {
    let d = copy_vault();
    write(
        d.path(),
        "task-b.md",
        "---\ntitle: \"Task B\"\nparent: \"[[project]]\"\n---\nSee [[glossary|the terms]] and [[Glossary]].\n```\n[[glossary]]\n```\n",
    );
    edit::retitle(d.path(), "glossary", "Terms").unwrap();
    let b = read(d.path(), "task-b.md");
    assert!(b.contains("See [[Terms|the terms]] and [[Terms]]."), "{b}");
    assert!(
        b.contains("```\n[[glossary]]\n```"),
        "a fenced link is not rewritten: {b}"
    );
    assert!(read(d.path(), "task-a.md").contains("See [[Terms]] and"));
    assert!(read(d.path(), "glossary.md").contains("title: \"Terms\""));
    // The file keeps its name, and [[Terms]] now resolves by title.
    validate(d.path()).unwrap();
    assert!(
        compile(d.path(), "task-a", DEFAULT_CAP)
            .unwrap()
            .text
            .contains("### Terms (glossary.md)")
    );
}

#[test]
fn retitle_keeps_a_child_whose_parent_named_the_old_title() {
    // Only body links were rewritten, and a child with none was skipped, so
    // `parent: "[[Big Plan]]"` pointed at a title that no longer existed:
    // the child fell out of the hierarchy with nothing said.
    let d = copy_vault();
    write(
        d.path(),
        "project.md",
        "---\ntitle: \"Big Plan\"\nparent: \"[[root]]\"\n---\nThe fixture project.\n",
    );
    write(
        d.path(),
        "task-b.md",
        "---\ntitle: \"Task B\"\nparent: \"[[Big Plan]]\"\n---\nMARKER-B\n",
    );
    validate(d.path()).unwrap();
    let a_before = read(d.path(), "task-a.md");
    let out = edit::retitle(d.path(), "project", "Delivery").unwrap();
    let b = read(d.path(), "task-b.md");
    assert!(b.contains("parent: \"[[Delivery]]\""), "{b}");
    assert!(out.touched.modified.contains(&"task-b.md".to_string()));
    // Written as the basename, it still resolves: left byte for byte.
    assert_eq!(read(d.path(), "task-a.md"), a_before);
    validate(d.path()).unwrap();
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    assert_eq!(
        g.parents[idx(&v, "task-b.md")],
        Some(idx(&v, "project.md")),
        "task-b is still project's child"
    );
}

#[test]
fn rm_turns_links_to_text_orphans_children_and_prunes_sidecars() {
    let d = copy_vault();
    write(
        d.path(),
        "layout.json",
        "{\"project.md\":{\"x\":1},\"root.md\":{\"x\":2}}",
    );
    write(
        d.path(),
        "annotations.json",
        "{\"version\":1,\"items\":[]} ",
    );
    let out = edit::remove(d.path(), "project").unwrap();
    assert!(!d.path().join("project.md").exists());
    assert_eq!(out.touched.deleted, ["project.md"]);
    // Children get parent null: Apex's code, not its docs.
    assert!(read(d.path(), "task-a.md").contains("parent: null"));
    assert!(read(d.path(), "task-b.md").contains("parent: null"));
    assert!(!read(d.path(), "manifest.json").contains("project.md"));
    assert_eq!(
        read(d.path(), "layout.json"),
        "{\n  \"root.md\": {\n    \"x\": 2\n  }\n}\n"
    );
    assert_eq!(
        read(d.path(), "annotations.json"),
        "{\"version\":1,\"items\":[]} "
    );
    validate(d.path()).unwrap();

    let d = copy_vault();
    edit::remove(d.path(), "glossary").unwrap();
    assert!(read(d.path(), "task-a.md").contains("See glossary and [[missing-note]]."));
}

#[test]
fn init_makes_an_empty_vault_once() {
    let d = tempfile::tempdir().unwrap();
    let v = d.path().join("new");
    edit::init(&v).unwrap();
    assert_eq!(read(&v, "manifest.json"), "[]\n");
    assert_eq!(read(&v, "layout.json"), "{}\n");
    assert!(matches!(edit::init(&v), Err(VaultError::Refused(_))));
    validate(&v).unwrap();
}

// ---------------------------------------------------------------------------
// dates.json alongside edits
// ---------------------------------------------------------------------------

#[test]
fn edits_keep_an_existing_dates_json_in_step() {
    let d = copy_vault();
    write(d.path(), "dates.json", "{\"version\":1,\"notes\":{}}");
    let (p, out) = edit::create(d.path(), "Fresh", None).unwrap();
    dates::after_edit(d.path(), &out.touched, "2026-09-27").unwrap();
    let dates::State::Valid(f) = dates::load(d.path()) else {
        panic!("dates.json should validate")
    };
    let nd = &f.doc.notes[&p];
    assert_eq!(nd.created.day.as_deref(), Some("2026-09-27"));
    assert!(!nd.created.estimated);

    let out = edit::remove(d.path(), "fresh").unwrap();
    dates::after_edit(d.path(), &out.touched, "2026-09-28").unwrap();
    let dates::State::Valid(f) = dates::load(d.path()) else {
        panic!("dates.json should validate")
    };
    assert!(!f.doc.notes.contains_key(&p));
}

#[test]
fn nothing_date_related_happens_without_dates_json() {
    let d = copy_vault();
    let (_, out) = edit::create(d.path(), "Fresh", None).unwrap();
    assert_eq!(
        dates::after_edit(d.path(), &out.touched, "2026-09-27").unwrap(),
        None
    );
    assert!(!d.path().join("dates.json").exists());
    assert_eq!(
        dates::after_edit(d.path(), &Touched::default(), "2026-09-27").unwrap(),
        None
    );
}
