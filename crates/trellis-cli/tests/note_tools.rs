//! `search_notes` and `read_note` wired through a real run (ADR 0024 §6).
//!
//! The registry's own tests attach a vault by hand; these prove the binary
//! does, so a tool the prompt advertises is a tool that works.

use std::path::Path;
use std::process::Command;

mod common;
use common::Scratch;

fn scratch(name: &str) -> Scratch {
    common::scratch_dir(&format!("trellis-notes-{name}"))
}

fn git(root: &Path, args: &[&str]) {
    let out = trellis_vcs::command()
        .args(args)
        .current_dir(root)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The repository is the scratch directory itself; the script and the user
/// config go beside it, in its parent, which the guard removes too.
fn repo(name: &str, config: &str) -> Scratch {
    let d = scratch(name);
    std::fs::create_dir_all(d.join("vault")).unwrap();
    std::fs::create_dir_all(d.join(".trellis")).unwrap();
    std::fs::create_dir_all(d.join("app")).unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["config", "user.email", "t@example.invalid"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["config", "core.autocrlf", "false"]);
    std::fs::write(d.join("app/__init__.py"), "").unwrap();
    std::fs::write(
        d.join("vault/retries.md"),
        "---\ntitle: \"Retry policy\"\nparent: null\n---\n\nBackoff doubles.\n",
    )
    .unwrap();
    std::fs::write(d.join(".trellis/config.toml"), config).unwrap();
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

/// Outside the repository, so writing it does not dirty the worktree.
fn script(repo: &Path, steps: &str) -> String {
    let p = repo.parent().unwrap().join("script.json");
    std::fs::write(&p, format!(r#"{{"steps":[{steps}]}}"#)).unwrap();
    format!("scripted:{}", p.display())
}

fn trellis(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_trellis"))
        .arg("--root")
        .arg(root)
        .args(args)
        // Hermetic: the user-level config and context files of whoever runs
        // the suite must not change what it sees.
        .env("TRELLIS_CONFIG_DIR", root.parent().unwrap().join("user"))
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn an_enabled_note_tool_reads_the_configured_vault_in_a_real_run() {
    let d = repo(
        "on",
        "[context]\nvault = \"vault\"\n\n[tools]\nenable = [\"read_note\"]\n",
    );
    let spec = script(
        &d,
        r#"{"tool":"read_note","note":"retries"},{"tool":"done","summary":"read it"}"#,
    );
    let (_, all) = trellis(&d, &["run", "read the note", "--model", &spec]);
    assert!(all.contains("snapshot at"), "{all}");
    let runs = d.join(".trellis/runs");
    let trail = std::fs::read_dir(&runs)
        .unwrap_or_else(|_| panic!("no trail: {all}"))
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .next()
        .expect("a trail");
    assert!(trail.contains("\"read_note\""), "{trail}");
    assert!(
        trail.contains("note `retries.md` (Retry policy), from the vault at vault"),
        "the call reached the vault rather than failing for want of one: {trail}"
    );
}

#[test]
fn a_note_tool_with_no_vault_refuses_before_the_snapshot() {
    let d = repo("off", "[tools]\nenable = [\"search_notes\"]\n");
    let spec = script(&d, r#"{"tool":"done","summary":"x"}"#);
    let (ok, all) = trellis(&d, &["run", "x", "--model", &spec]);
    assert!(!ok, "{all}");
    assert!(all.contains("[context] vault"), "{all}");
    assert!(!all.contains("snapshot at"), "{all}");
}

/// The edit the vault tests script: a note's body rewritten, then done.
const EDITS_A_NOTE: &str = r#"{"tool":"edit","path":"vault/retries.md","old":"Backoff doubles.","new":"Backoff triples."},{"tool":"done","summary":"edited the note"}"#;

/// The trail of the one run in `d`.
fn only_trail(d: &Path, all: &str) -> String {
    std::fs::read_dir(d.join(".trellis/runs"))
        .unwrap_or_else(|_| panic!("no trail: {all}"))
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .next()
        .unwrap_or_else(|| panic!("no trail: {all}"))
}

#[test]
fn a_model_edit_into_the_vault_is_refused_and_named() {
    // The risk (ADR 0049): a note is prompt input to later runs, so a model
    // that rewrites one writes its own future context with no check behind
    // it. `edit` reaches the vault unless the run refuses it, and a
    // refusal that does not name `[context] vault` sends the reader looking
    // for a `deny_write` rule that does not exist.
    let d = repo("vault-edit", "[context]\nvault = \"vault\"\n");
    let before = std::fs::read(d.join("vault/retries.md")).unwrap();
    let spec = script(&d, EDITS_A_NOTE);
    let (_, all) = trellis(&d, &["run", "edit the note", "--model", &spec]);
    assert!(all.contains("snapshot at"), "{all}");
    let trail = only_trail(&d, &all);
    assert!(trail.contains("edit refused"), "{trail}");
    assert!(trail.contains("[context] vault"), "{trail}");
    assert_eq!(
        std::fs::read(d.join("vault/retries.md")).unwrap(),
        before,
        "the note is byte-identical after the run: {all}"
    );
}

#[test]
fn without_a_vault_a_vault_directory_is_an_ordinary_path() {
    // The refusal follows the setting, not a directory's name: with no
    // `[context] vault`, `vault/` is any other path and nothing about the
    // run mentions a vault.
    let d = repo("vault-none", "");
    let spec = script(&d, EDITS_A_NOTE);
    let (_, all) = trellis(&d, &["run", "edit the note", "--model", &spec]);
    assert!(all.contains("snapshot at"), "{all}");
    let trail = only_trail(&d, &all);
    assert!(!trail.contains("[context] vault"), "{trail}");
    assert!(!trail.contains("edit refused"), "{trail}");
}

#[test]
fn a_vault_at_the_repository_root_stops_the_run_before_the_snapshot() {
    // `[context] vault = "."` with no `notes/` at the root makes the notes
    // root the whole repository, so the refusal would refuse every write
    // the task needs (ADR 0049 §5). Said before the snapshot, naming the
    // setting, rather than a run that can change nothing.
    let d = repo("vault-root", "[context]\nvault = \".\"\n");
    let spec = script(&d, EDITS_A_NOTE);
    let (ok, all) = trellis(&d, &["run", "edit the note", "--model", &spec]);
    assert!(!ok, "{all}");
    assert!(all.contains("[context] vault"), "{all}");
    assert!(all.contains("is the repository root"), "{all}");
    assert!(!all.contains("snapshot at"), "{all}");
}
