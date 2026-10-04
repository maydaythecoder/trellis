//! `trellis-tickets index` over issue JSON recorded from `gh issue view <n>
//! --json number,title,body,labels,state` (`tests/fixtures/triage/`). The
//! open-issue list is built from the recordings and the blocker reads are a
//! closure over them, so no test spawns `gh` or reaches GitHub.
//!
//! The golden (`tests/fixtures/index/golden.json`) is compared byte for
//! byte. When a rule change moves it on purpose, the failure prints the new
//! output to paste in; the diff of that file is the review of the change.

use std::cell::RefCell;
use std::path::PathBuf;

use serde_json::{Value, json};
use trellis_tickets::{Index, IndexError, Line, build_index, index_with};

fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// A recording, or a fixed message when there is none: the OS's own error
/// text differs between platforms, and it would reach the golden through a
/// blocker's note.
fn recorded_text(n: u64) -> Result<String, String> {
    std::fs::read_to_string(fixture_path(&format!("triage/issue-{n}.json")))
        .map_err(|_| format!("no recording for #{n}"))
}

fn recorded(n: u64) -> Value {
    serde_json::from_str(&recorded_text(n).unwrap()).unwrap()
}

/// Every open recording: two tickets in S13's and S07's and S09's stories,
/// S13's story issue itself, one parked issue, and two with no triage block.
const OPEN: [u64; 6] = [202, 77, 198, 60, 201, 94];

/// The index over `open`, reading blockers from the recordings, and every
/// issue that was read.
fn index_of(open: &[Value]) -> (Result<Index, IndexError>, Vec<u64>) {
    let asked = RefCell::new(Vec::new());
    let list = Value::Array(open.to_vec()).to_string();
    let out = index_with(
        || Ok(list),
        |n| {
            asked.borrow_mut().push(n);
            recorded_text(n)
        },
    );
    (out, asked.into_inner())
}

fn golden_open() -> Vec<Value> {
    OPEN.iter().map(|n| recorded(*n)).collect()
}

fn line<'a>(v: &'a Value, number: u64, field: &str) -> &'a Value {
    v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["number"] == number)
        .unwrap_or_else(|| panic!("#{number} is not in the index"))
        .get(field)
        .unwrap_or_else(|| panic!("#{number} has no `{field}`"))
}

#[test]
fn index_golden_matches_the_recorded_open_issues_byte_for_byte() {
    let (index, _) = index_of(&golden_open());
    let actual = index.unwrap().json();
    let golden = std::fs::read_to_string(fixture_path("index/golden.json")).unwrap();
    assert!(
        actual == golden,
        "the index moved from tests/fixtures/index/golden.json; if the change is intended, this is the new golden:\n{actual}"
    );
}

#[test]
fn index_golden_is_byte_identical_on_a_rerun_in_any_list_order() {
    // The index exists to be diffed between runs. A re-run over the same
    // issues that reorders anything shows a diff for a change that did not
    // happen: `gh` lists newest first today, but promises no order, and
    // label order on GitHub is not the order they were added.
    let first = index_of(&golden_open()).0.unwrap().json();
    let again = index_of(&golden_open()).0.unwrap().json();
    assert_eq!(first, again, "two runs over the same issues differ");

    let mut shuffled = golden_open();
    shuffled.reverse();
    for v in &mut shuffled {
        let labels = v["labels"].as_array_mut().unwrap();
        labels.reverse();
    }
    let reordered = index_of(&shuffled).0.unwrap().json();
    assert_eq!(
        first, reordered,
        "the list or label order leaked into the index"
    );
}

#[test]
fn index_golden_out_file_is_byte_identical_when_written_again() {
    // `--out` is the file two runs are diffed through: a second write over
    // the same issues must leave the same bytes, and the same bytes stdout
    // gets.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("index_golden_out");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("index.json");
    let index = index_of(&golden_open()).0.unwrap();
    index.write(&path).unwrap();
    let first = std::fs::read(&path).unwrap();
    let mut shuffled = golden_open();
    shuffled.reverse();
    index_of(&shuffled).0.unwrap().write(&path).unwrap();
    let second = std::fs::read(&path).unwrap();
    assert_eq!(first, second, "a re-run changed the written file");
    assert_eq!(first, index.json().into_bytes());
    let golden = std::fs::read(fixture_path("index/golden.json")).unwrap();
    assert_eq!(first, golden);
}

#[test]
fn index_golden_is_version_2_and_sorted_by_story_then_number() {
    let (index, _) = index_of(&golden_open());
    let v: Value = serde_json::from_str(&index.unwrap().json()).unwrap();
    assert_eq!(v["triage_version"], 2);
    let order: Vec<(Value, u64)> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["story"].clone(), e["number"].as_u64().unwrap()))
        .collect();
    // S07 < S09 < S13 by number; the S13 story issue (#94) sorts among its
    // tickets by number; issues with no story come last.
    assert_eq!(
        order,
        [
            (json!("S07"), 201),
            (json!("S09"), 60),
            (json!("S13"), 77),
            (json!("S13"), 94),
            (Value::Null, 198),
            (Value::Null, 202),
        ]
    );
}

#[test]
fn index_golden_says_abstained_for_agent_ok_on_every_issue_195() {
    // The rules never set !agent-ok (#195). An index whose flags merely
    // omit it would read as "decided: not agent-ok" on #77, which carries
    // the label.
    let v: Value = serde_json::from_str(&index_of(&golden_open()).0.unwrap().json()).unwrap();
    for e in v["issues"].as_array().unwrap() {
        assert_eq!(e["agent_ok"]["outcome"], "abstained", "{e}");
        assert!(e["agent_ok"]["reason"].as_str().unwrap().contains("#195"));
    }
    assert!(
        line(&v, 77, "labels")
            .as_array()
            .unwrap()
            .contains(&json!("!agent-ok"))
    );
}

#[test]
fn index_golden_maps_a_crate_the_standard_omits_194() {
    // #201 names only crates/trellis-loop, which §5's table leaves out and
    // the owner mapped to area:runtime (#194). Before the mapping this
    // abstained; an index that still did would route the issue to a person
    // on a question already answered.
    let v: Value = serde_json::from_str(&index_of(&golden_open()).0.unwrap().json()).unwrap();
    assert_eq!(line(&v, 201, "area")["outcome"], "decided");
    assert_eq!(line(&v, 201, "area")["value"], "area:runtime");
}

/// #77 with a `**Files:**` line every rule decides on (risk computes
/// `risk:medium`), recorded at `risk`.
fn files_77(risk: &str) -> Value {
    let mut v = recorded(77);
    let b = v["body"].as_str().unwrap().replacen(
        "\r\r\n",
        "\r\r\n**Files:** crates/trellis-cli/src/main.rs\r\r\n",
        1,
    );
    v["body"] = Value::String(b);
    v["labels"] = Value::Array(
        [
            "area:cli",
            "kind:feature",
            risk,
            "surface:cli",
            "effort:s",
            "origin:manual",
            "triage:auto",
        ]
        .iter()
        .map(|l| json!({ "name": l }))
        .collect(),
    );
    v
}

#[test]
fn index_abstains_on_a_risk_below_the_recorded_label_and_on_what_reads_it_196() {
    // The risk rule cannot see rollback, worktree teardown or session
    // persistence (#196). Printing its lower value as decided, or a route
    // computed from it, would state what the rules cannot know.
    let v: Value =
        serde_json::from_str(&index_of(&[files_77("risk:high")]).0.unwrap().json()).unwrap();
    for field in ["risk", "triage", "flags", "ready"] {
        let l = line(&v, 77, field);
        assert_eq!(l["outcome"], "abstained", "{field}: {l}");
        assert!(
            l["reason"].as_str().unwrap().contains("#196"),
            "{field}: {l}"
        );
    }

    // Recorded at medium, the rule's medium is decided: nothing is lowered.
    let v: Value =
        serde_json::from_str(&index_of(&[files_77("risk:medium")]).0.unwrap().json()).unwrap();
    assert_eq!(line(&v, 77, "risk")["outcome"], "decided");
    assert_eq!(line(&v, 77, "risk")["value"], "risk:medium");
}

#[test]
fn index_reads_only_blockers_outside_the_list_and_abstains_on_one_it_cannot_read() {
    // #198's blocker #60 is in the open list and is not read. The rest
    // are not in it, and each is read once: #77's #75 and #76, #60's #98
    // and #21 (not recorded, so their notes say so), #201's #120.
    let (_, asked) = index_of(&golden_open());
    assert_eq!(asked, [75, 76, 98, 21, 120]);

    let list = Value::Array(vec![recorded(77)]).to_string();
    let v: Value = serde_json::from_str(
        &index_with(
            || Ok(list),
            |n| {
                if n == 76 {
                    Err("gh: not found".to_string())
                } else {
                    recorded_text(n)
                }
            },
        )
        .unwrap()
        .json(),
    )
    .unwrap();
    assert!(
        line(&v, 77, "notes").to_string().contains("blocker #76"),
        "{v}"
    );
    assert_eq!(line(&v, 77, "ready")["outcome"], "abstained");
    assert_eq!(line(&v, 77, "triage")["outcome"], "abstained");
}

#[test]
fn index_refuses_a_list_it_cannot_trust_as_every_open_issue() {
    // An index built from a partial or wrong list would read as a
    // statement about every open issue.
    let dup = Value::Array(vec![recorded(202), recorded(202)]).to_string();
    assert!(matches!(
        build_index(&dup, recorded_text),
        Err(IndexError::Duplicate { number: 202 })
    ));
    let closed = Value::Array(vec![recorded(75)]).to_string();
    assert!(matches!(
        build_index(&closed, recorded_text),
        Err(IndexError::NotOpen { number: 75, .. })
    ));
    assert!(matches!(
        build_index("{}", recorded_text),
        Err(IndexError::ListJson(_))
    ));
    assert!(matches!(
        index_with(|| Err("gh: offline".to_string()), recorded_text),
        Err(IndexError::List { .. })
    ));
}

#[test]
fn index_escapes_every_terminal_control_and_still_parses_to_the_same_title() {
    // serde_json leaves DEL and the C1 range raw, and U+009B alone starts a
    // terminal control sequence when the index is printed to stdout.
    let mut v = recorded(202);
    v["title"] = Value::String("csi \u{9b}31m del \u{7f} esc \u{1b}[0m".to_string());
    let out = index_of(&[v.clone()]).0.unwrap().json();
    assert!(
        !out.chars().any(|c| c.is_control() && c != '\n'),
        "a raw control reached the output: {out:?}"
    );
    let back: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(back["issues"][0]["title"], v["title"]);
}

#[test]
fn index_story_that_cannot_be_read_is_null_with_a_note_not_a_guess() {
    // A `story:` that is not `S<n>` would sort the issue under a story that
    // may not exist.
    let mut v = recorded(202);
    let b = format!(
        "{}
story: thirteen",
        v["body"].as_str().unwrap()
    );
    v["body"] = Value::String(b);
    let (index, _) = index_of(&[v]);
    let e = &index.unwrap().issues[0];
    assert_eq!(e.story, None);
    assert!(
        e.notes
            .iter()
            .any(|n| n.contains("story could not be read")),
        "{:?}",
        e.notes
    );
    assert!(matches!(e.agent_ok, Line::Abstained { .. }));
}
