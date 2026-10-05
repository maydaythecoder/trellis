//! `search_notes` and `read_note` wired through a real run (ADR 0024 §6).
//!
//! The registry's own tests attach a vault by hand; these prove the binary
//! does, so a tool the prompt advertises is a tool that works.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("trellis-notes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

fn repo(name: &str, config: &str) -> PathBuf {
    let d = scratch(name).join("repo");
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
