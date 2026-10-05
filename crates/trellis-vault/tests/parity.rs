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

// Apex's canvas owns these two files and trellis does not interpret them
// (ADR 0021 rows 9 and 12, ADR 0022). The content is deliberately not what
// serde_json would write: keys out of order, odd whitespace, CRLF, no final
// newline. A create or retitle that parsed and re-serialised either file
// would reorder or reformat it and fail here, while the round-trip through
// Apex's own reader would still look valid.
const ODD_LAYOUT: &str =
    "{ \"task-a.md\" :{\"y\":3 ,\"x\":2},\r\n\t\"project.md\":{ \"dy\":-1,\"x\":1.50}  }";
const ODD_ANNOTATIONS: &str =
    "{\"items\":[ {\"type\":\"text\",\"text\":\"note\"} ] ,\r\n  \"version\":1 }";

fn plant_odd_sidecars(dir: &Path) {
    write(dir, "layout.json", ODD_LAYOUT);
    write(dir, "annotations.json", ODD_ANNOTATIONS);
}

fn assert_odd_sidecars_unchanged(dir: &Path) {
    assert_eq!(
        std::fs::read(dir.join("layout.json")).unwrap(),
        ODD_LAYOUT.as_bytes(),
        "layout.json was rewritten"
    );
    assert_eq!(
        std::fs::read(dir.join("annotations.json")).unwrap(),
        ODD_ANNOTATIONS.as_bytes(),
        "annotations.json was rewritten"
    );
}

#[test]
fn sidecars_survive_create_byte_for_byte() {
    // `vault new` must not add a layout entry or normalise either file: a
    // new note has no position until Apex's canvas gives it one.
    let d = copy_vault();
    plant_odd_sidecars(d.path());
    let (p, out) = edit::create(d.path(), "Fresh", Some("project")).unwrap();
    assert_eq!(out.touched.created, std::slice::from_ref(&p));
    assert_odd_sidecars_unchanged(d.path());
    validate(d.path()).unwrap();
}

#[test]
fn sidecars_survive_retitle_byte_for_byte() {
    // Retitle rewrites links across the vault but never renames a file, so
    // layout.json's path keys stay valid and there is nothing to prune.
    let d = copy_vault();
    plant_odd_sidecars(d.path());
    let out = edit::retitle(d.path(), "project", "Delivery").unwrap();
    assert!(out.touched.modified.contains(&"project.md".to_string()));
    assert_odd_sidecars_unchanged(d.path());
    validate(d.path()).unwrap();
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

// ---------------------------------------------------------------------------
// What the mutation survivors of #246 showed was unasserted
// ---------------------------------------------------------------------------

#[test]
fn a_layout_json_that_cannot_be_read_fails_the_edit_rather_than_reading_as_absent() {
    let d = copy_vault();
    std::fs::create_dir(d.path().join("layout.json")).unwrap();
    match edit::remove(d.path(), "glossary") {
        Err(VaultError::Io { .. }) => {}
        other => panic!("expected an io error, got {other:?}"),
    }
}

#[test]
fn rm_says_what_it_did_to_the_layout() {
    // Not an object: left alone, and said so.
    let d = copy_vault();
    write(d.path(), "layout.json", "[]");
    let out = edit::remove(d.path(), "glossary").unwrap();
    assert!(
        out.messages
            .contains(&"layout.json is not a JSON object; left untouched".to_string()),
        "{:?}",
        out.messages
    );
    assert_eq!(read(d.path(), "layout.json"), "[]");

    // A stale key: removed and counted.
    let d = copy_vault();
    write(
        d.path(),
        "layout.json",
        "{\"project.md\":{},\"root.md\":{}}",
    );
    let out = edit::remove(d.path(), "project").unwrap();
    assert!(
        out.messages
            .contains(&"layout.json: 1 stale key(s) removed".to_string()),
        "{:?}",
        out.messages
    );

    // Nothing stale: nothing said about the layout.
    let d = copy_vault();
    write(d.path(), "layout.json", "{\"root.md\":{}}");
    let out = edit::remove(d.path(), "glossary").unwrap();
    assert!(
        !out.messages.iter().any(|m| m.starts_with("layout.json")),
        "{:?}",
        out.messages
    );
}

#[test]
fn rm_counts_links_and_orphans_and_is_silent_when_there_are_none() {
    let summary = |stem: &str| {
        let d = copy_vault();
        let out = edit::remove(d.path(), stem).unwrap();
        out.messages
            .into_iter()
            .find(|m| m.contains("turned to text"))
    };
    assert_eq!(
        summary("glossary").as_deref(),
        Some("1 link(s) turned to text; 0 child note(s) now have parent null")
    );
    assert_eq!(
        summary("root").as_deref(),
        Some("0 link(s) turned to text; 1 child note(s) now have parent null")
    );
    assert_eq!(summary("task-b"), None);
}

#[test]
fn rm_leaves_the_report_line_counting_a_parent_with_children_as_connected() {
    // Without glossary every note left is in the hierarchy, root because it
    // has a child.
    let d = copy_vault();
    edit::remove(d.path(), "glossary").unwrap();
    let r = edit::check(d.path(), false).unwrap();
    assert!(r.line.contains(" connected=4 "), "{}", r.line);
}

#[test]
fn a_third_note_with_a_taken_title_gets_the_next_suffix() {
    let d = copy_vault();
    let (_, out) = edit::create(d.path(), "Task A", None).unwrap();
    assert!(
        out.messages[0].contains("\"Task A 2\""),
        "{:?}",
        out.messages
    );
    let (_, out) = edit::create(d.path(), "Task A", None).unwrap();
    assert!(
        out.messages[0].contains("\"Task A 3\""),
        "{:?}",
        out.messages
    );
}

#[test]
fn a_title_with_no_slug_gets_the_untitled_path() {
    let d = copy_vault();
    let (p, _) = edit::create(d.path(), "!!!", None).unwrap();
    assert_eq!(p, "untitled.md");
}

#[test]
fn retitle_warns_only_when_the_title_is_another_note_s_key() {
    let warned = |title: &str| {
        let d = copy_vault();
        let out = edit::retitle(d.path(), "glossary", title).unwrap();
        out.messages.into_iter().find(|m| m.starts_with("warning:"))
    };
    let w = warned("Root").unwrap();
    assert!(w.contains("is also a key of root.md"), "{w}");
    assert_eq!(warned("Terms"), None);
}

#[test]
fn retitle_counts_the_links_and_parents_it_rewrote() {
    let d = copy_vault();
    write(
        d.path(),
        "project.md",
        "---\ntitle: \"Big Plan\"\nparent: \"[[root]]\"\n---\nThe fixture project.\n",
    );
    write(
        d.path(),
        "task-b.md",
        "---\ntitle: \"Task B\"\nparent: \"[[Big Plan]]\"\n---\nSee [[Big Plan]] and [[project]].\n",
    );
    let out = edit::retitle(d.path(), "project", "Delivery").unwrap();
    let last = out.messages.last().unwrap();
    assert!(
        last.ends_with("2 link(s) and 1 parent reference(s) rewritten"),
        "{last}"
    );
}

#[test]
fn check_does_not_warn_below_apex_s_size_thresholds() {
    // 1.5 MiB in one note: under the 2 MiB note warning and far under the
    // 50 MiB vault warning, in a vault of far fewer than 1000 notes.
    let d = copy_vault();
    let body = "x".repeat(3 * 1024 * 1024 / 2);
    write(
        d.path(),
        "glossary.md",
        &format!("---\ntitle: \"Glossary\"\nparent: null\n---\n{body}"),
    );
    let r = edit::check(d.path(), false).unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn the_gate_answer_is_no_for_an_invalid_vault() {
    let d = copy_vault();
    manifest(d.path(), &["root.md"]);
    assert!(!edit::check(d.path(), false).unwrap().gate_ok());
}

#[test]
fn fix_lists_the_layout_only_when_it_removed_a_key() {
    let d = copy_vault();
    write(d.path(), "layout.json", "{\"root.md\":{}}");
    assert_eq!(edit::check(d.path(), true).unwrap().line, FIXTURE_LINE);
    write(d.path(), "layout.json", "{\"gone.md\":{},\"root.md\":{}}");
    let r = edit::check(d.path(), true).unwrap();
    assert!(r.line.ends_with("fixes=[layout.json]"), "{}", r.line);
}

#[test]
fn notes_root_follows_the_notes_folder_rule() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(trellis_vault::notes_root(d.path()), d.path());
    std::fs::create_dir(d.path().join("notes")).unwrap();
    assert_eq!(trellis_vault::notes_root(d.path()), d.path().join("notes"));
}

// ---------------------------------------------------------------------------
// Links Apex refuses (APX-43, APX-07)
// ---------------------------------------------------------------------------

/// A symbolic link at `link` to `target`, or `Err` with a line starting
/// `skipped:` where this host cannot make one (Windows without Developer
/// Mode or elevation). The same logic as `trellis_core::paths::
/// symlink_for_test`, copied because trellis-vault takes no dependency on
/// trellis-core.
fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let r = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    r.map_err(|e| {
        format!(
            "skipped: a symbolic link could not be created on this host ({e}); on Windows \
             this needs Developer Mode or an elevated process"
        )
    })
}

/// A file outside the vault holding `body`, and its bytes, for asserting a
/// link's target was never written through.
fn outside_file(name: &str, body: &str) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(name);
    std::fs::write(&p, body).unwrap();
    (d, p)
}

fn assert_notes_link_refused(vault: &Path) {
    let msg = |r: Result<(), VaultError>| match r {
        Err(e) => e.to_string(),
        Ok(()) => panic!("a linked notes/ was read as the vault"),
    };
    let open = msg(Vault::open(vault).map(drop));
    assert!(open.contains("notes folder cannot be a symlink"), "{open}");
    let gate = msg(validate(vault).map(drop));
    assert!(gate.contains("notes folder cannot be a symlink"), "{gate}");
}

#[test]
fn a_symlinked_notes_folder_is_refused_not_read_from_the_parent() {
    // Apex refuses the folder; trellis fell back to the vault directory
    // itself, so the two tools read different files for the same vault and
    // the run gate passed on notes Apex would never show.
    let elsewhere = copy_vault();
    let d = copy_vault();
    let link = d.path().join("notes");
    // A junction needs no privilege, so a Windows host without Developer
    // Mode still checks that one is treated as a link.
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(elsewhere.path())
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(made, "mklink /J needs no privilege");
        assert_notes_link_refused(d.path());
        std::fs::remove_dir(&link).unwrap();
    }
    if let Err(why) = symlink(elsewhere.path(), &link) {
        println!("{why}");
        return;
    }
    assert_notes_link_refused(d.path());
}

#[test]
fn a_symlinked_manifest_is_refused_before_any_write() {
    // `vault new` rewrote whatever file the link named, outside the vault.
    let d = copy_vault();
    let manifest_text = read(d.path(), "manifest.json");
    let (_out, target) = outside_file("manifest.json", &manifest_text);
    std::fs::remove_file(d.path().join("manifest.json")).unwrap();
    if let Err(why) = symlink(&target, &d.path().join("manifest.json")) {
        println!("{why}");
        return;
    }
    let notes_before: Vec<_> = snapshot(d.path()).into_iter().map(|(n, _)| n).collect();

    let err = edit::create(d.path(), "Fresh", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("cannot be a symlink"), "{err}");
    let notes_after: Vec<_> = snapshot(d.path()).into_iter().map(|(n, _)| n).collect();
    assert_eq!(notes_after, notes_before, "no note was created");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), manifest_text);

    let gate = validate(d.path()).unwrap_err().to_string();
    assert!(
        gate.contains("manifest.json") && gate.contains("cannot be a symlink"),
        "{gate}"
    );
}

#[test]
fn a_symlinked_layout_is_refused_before_any_write() {
    // `rm` pruned the removed note's key from whatever file the link named.
    let d = copy_vault();
    let layout = "{\"glossary.md\":{},\"root.md\":{}}";
    let (_out, target) = outside_file("layout.json", layout);
    if let Err(why) = symlink(&target, &d.path().join("layout.json")) {
        println!("{why}");
        return;
    }
    let err = edit::remove(d.path(), "glossary").unwrap_err().to_string();
    assert!(err.contains("cannot be a symlink"), "{err}");
    assert!(
        d.path().join("glossary.md").is_file(),
        "the note was removed"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), layout);
}

#[test]
fn a_symlinked_dates_json_is_refused_and_left_unchanged() {
    // The refusal existed with nothing asserting it: #246 found
    // `is_symlink -> false` survived. A valid document behind the link is
    // the case that matters, since an invalid one is refused anyway.
    let d = copy_vault();
    let doc = "{\"version\":1,\"notes\":{}}";
    let (_out, target) = outside_file("dates.json", doc);
    if let Err(why) = symlink(&target, &d.path().join("dates.json")) {
        println!("{why}");
        return;
    }
    match dates::load(d.path()) {
        dates::State::Invalid(e) => assert!(e.contains("cannot be a symlink"), "{e}"),
        other => panic!("a linked dates.json was read: {other:?}"),
    }
    let touched = Touched {
        created: vec!["root.md".into()],
        ..Touched::default()
    };
    dates::after_edit(d.path(), &touched, "2026-10-04").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), doc);
}

/// `edit::init` on `vault` refused naming `name`, with neither metadata file
/// written: the refusal comes before the first write, not between the two.
fn assert_init_refused(vault: &Path, name: &str) {
    let err = edit::init(vault).unwrap_err().to_string();
    assert!(
        err.contains(name) && err.contains("cannot be a symlink"),
        "{err}"
    );
    for f in ["manifest.json", "layout.json"] {
        let p = vault.join(f);
        assert!(
            std::fs::symlink_metadata(&p).is_err() || f == name,
            "{f} was written"
        );
    }
}

/// A broken link: `exists()` follows it and reads it as absent, which is
/// how `init` came to write a new vault's metadata through it, outside the
/// vault, to a file the link's target named.
fn assert_init_refuses_broken_link(name: &str) {
    let d = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join(name);
    if let Err(why) = symlink(&target, &d.path().join(name)) {
        println!("{why}");
        return;
    }
    assert_init_refused(d.path(), name);
    assert!(
        !target.exists(),
        "init wrote through the link to {target:?}"
    );
}

#[test]
fn init_refuses_a_linked_manifest_json() {
    assert_init_refuses_broken_link("manifest.json");
}

#[test]
fn init_refuses_a_linked_layout_json() {
    // Checked before `manifest.json` is written, so the refusal leaves no
    // half-initialised vault behind.
    assert_init_refuses_broken_link("layout.json");
}

#[test]
fn init_refuses_a_linked_notes_folder() {
    // Every later read refuses a linked `notes/`, so a vault `init` accepted
    // over one could never be opened.
    let outside = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    {
        // A junction needs no privilege, so this runs on every Windows host.
        let d = tempfile::tempdir().unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(d.path().join("notes"))
            .arg(outside.path())
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(made, "mklink /J needs no privilege");
        assert_init_refused(d.path(), "notes");
    }
    let d = tempfile::tempdir().unwrap();
    if let Err(why) = symlink(&outside.path().join("gone"), &d.path().join("notes")) {
        println!("{why}");
        return;
    }
    assert_init_refused(d.path(), "notes");
}

#[test]
fn a_shared_title_resolves_as_ambiguous_naming_both() {
    let r = trellis_vault::Resolver::new([("a/x.md", "Same"), ("b/y.md", "Same")]);
    assert_eq!(
        r.resolve("Same"),
        trellis_vault::Resolved::Ambiguous(vec![0, 1])
    );
}

#[test]
fn key_for_is_the_basename_only_when_it_is_unique() {
    let d = tempfile::tempdir().unwrap();
    for f in ["a/x.md", "b/x.md", "c.md"] {
        write(d.path(), f, "body");
    }
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    assert_eq!(g.resolver.key_for(&v, idx(&v, "a/x.md")), "a/x");
    assert_eq!(g.resolver.key_for(&v, idx(&v, "c.md")), "c");
}

#[test]
fn a_missing_parent_names_candidates_only_when_it_is_ambiguous() {
    let d = copy_vault();
    write(
        d.path(),
        "glossary.md",
        "---\ntitle: \"Twin\"\nparent: null\n---\n",
    );
    write(
        d.path(),
        "root.md",
        "---\ntitle: \"Twin\"\nparent: null\n---\n",
    );
    write(
        d.path(),
        "task-a.md",
        "---\ntitle: \"Task A\"\nparent: \"[[Twin]]\"\n---\n",
    );
    write(
        d.path(),
        "task-b.md",
        "---\ntitle: \"Task B\"\nparent: \"[[nowhere]]\"\n---\n",
    );
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    let shown = |path: &str| {
        g.issues
            .iter()
            .find(|i| matches!(i, Issue::MissingParent { path: p, .. } if p == path))
            .map(|i| i.to_string())
            .unwrap()
    };
    let a = shown("task-a.md");
    assert!(a.ends_with(" (ambiguous: glossary.md, root.md)"), "{a}");
    let b = shown("task-b.md");
    assert!(!b.contains("ambiguous"), "{b}");
}

#[test]
fn lineage_runs_from_the_root_to_the_note() {
    let v = Vault::open(&fixture()).unwrap();
    let g = Graph::build(&v);
    assert_eq!(
        g.lineage(idx(&v, "task-a.md")),
        [
            idx(&v, "root.md"),
            idx(&v, "project.md"),
            idx(&v, "task-a.md")
        ]
    );
}

#[test]
fn the_graph_reports_a_parent_cycle() {
    let d = copy_vault();
    write(
        d.path(),
        "root.md",
        "---\ntitle: \"Root\"\nparent: \"[[task-a]]\"\n---\n",
    );
    let v = Vault::open(d.path()).unwrap();
    let g = Graph::build(&v);
    assert!(
        g.issues.iter().any(|i| matches!(i, Issue::Cycle { .. })),
        "{:?}",
        g.issues
    );
}
