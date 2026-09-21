//! `/eng <name>` — generate a self-verifying loop and register it as a
//! permanent slash command.
//!
//! Three parts, always: the loop orchestration, the state and artifact
//! contracts, and the command definition written to
//! `.harness/commands/<name>.md`.
//!
//! The generator produces *text*. It writes two files and does nothing else
//! — it does not run the loop, and the loop it describes is executed by
//! `runner.rs`, which enforces the cycle cap itself. That division is the
//! point: a cap written into a prompt is a suggestion, and the file this
//! module writes is a prompt.

use std::path::{Path, PathBuf};

/// The default cycle cap. Five is enough for a compile-fix loop to converge
/// and few enough that a loop which is not converging is noticed by a human
/// rather than by a bill.
pub const DEFAULT_HALT_THRESHOLD: u32 = 5;

/// Whether `name` can be both a filename and a command name.
///
/// Checked at parse time (see `Registry::parse`) so `/eng ../../etc/passwd`
/// fails before anything is described, let alone written. Rejecting `.` as
/// well as the separators means no traversal component can be constructed
/// from a single argument.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where the harness keeps its state and its generated commands.
#[derive(Debug, Clone)]
pub struct Paths {
    pub commands_dir: PathBuf,
    pub loops_dir: PathBuf,
}

impl Paths {
    pub fn under(root: &Path) -> Self {
        let h = root.join(".harness");
        Paths {
            commands_dir: h.join("commands"),
            loops_dir: h.join("loops"),
        }
    }

    pub fn command_file(&self, name: &str) -> PathBuf {
        self.commands_dir.join(format!("{name}.md"))
    }

    pub fn state_file(&self, name: &str) -> PathBuf {
        self.loops_dir.join(format!("{name}.json"))
    }
}

/// What a generated loop is contracted to do. Produced as data so the
/// rendered files and the runtime's own enforcement read the same numbers.
#[derive(Debug, Clone)]
pub struct LoopSpec {
    pub name: String,
    /// Target directory or files the loop works over.
    pub input_context: String,
    /// The tracking artifact, so the loop remembers failures across cycles.
    pub memory_file: PathBuf,
    /// The cycle cap.
    pub halt_threshold: u32,
    /// The exact shell commands the loop runs to check its own work.
    pub verification: Vec<String>,
}

impl LoopSpec {
    /// The default shape: a Rust project checking itself.
    ///
    /// Deliberately the project's own commands rather than something
    /// generic. A self-verifying loop whose verification step is a
    /// placeholder verifies nothing and reports success.
    pub fn rust_default(name: &str, paths: &Paths) -> Self {
        LoopSpec {
            name: name.to_string(),
            input_context: "{{dir}}".to_string(),
            memory_file: paths.state_file(name),
            halt_threshold: DEFAULT_HALT_THRESHOLD,
            verification: vec![
                "cargo fmt --all -- --check".to_string(),
                "cargo clippy --all-targets -- -D warnings".to_string(),
                "cargo test --workspace".to_string(),
            ],
        }
    }

    /// The `.harness/commands/<name>.md` body: YAML frontmatter plus the
    /// numbered steps. This is what `loader.rs` reads back.
    pub fn render_command_file(&self) -> String {
        let mut s = String::new();
        s.push_str("---\n");
        s.push_str(&format!("command: {}\n", self.name));
        s.push_str(&format!(
            "description: Autonomous self-verifying loop over the target directory ({} cycles max)\n",
            self.halt_threshold
        ));
        s.push_str("parameters:\n");
        s.push_str("  dir: \"target path to check\"\n");
        s.push_str("---\n");
        s.push_str(&format!(
            "1. Read the file layout in {}\n",
            self.input_context
        ));
        s.push_str("2. Run the project's check command:\n");
        for v in &self.verification {
            s.push_str(&format!("   - `{v}`\n"));
        }
        s.push_str("3. For each error: generate a fix, re-verify with local tests\n");
        s.push_str(&format!(
            "4. Append the cycle's error set to `{}` before starting the next\n",
            self.memory_file.display()
        ));
        s.push_str(&format!(
            "5. Halt at zero errors, at {} cycles, or when two cycles produce an\n   identical error set\n",
            self.halt_threshold
        ));
        s.push_str("6. Stop at the human gate and report; nothing lands without it\n");
        s
    }

    /// The initial `.harness/loops/<name>.json`.
    pub fn initial_state(&self) -> String {
        "{\"cycles\": 0, \"errors\": []}".to_string()
    }

    /// What the user is shown when the loop is generated. Three parts,
    /// because a loop whose contracts are invisible is one nobody can
    /// predict the cost of.
    pub fn summary(&self) -> Vec<String> {
        let mut out = vec![
            format!("/{} generated. Three parts:", self.name),
            String::new(),
            "  loop orchestration".into(),
            format!("    state hook       {}", self.memory_file.display()),
            format!(
                "    halt at          {} cycles, checked before each one",
                self.halt_threshold
            ),
            "    self-verification".into(),
        ];
        for v in &self.verification {
            out.push(format!("                     {v}"));
        }
        out.push("    human gate       before anything lands".into());
        out.push(String::new());
        out.push("  contracts".into());
        out.push(format!("    input_context    {}", self.input_context));
        out.push(format!(
            "    memory_file      {}",
            self.memory_file.display()
        ));
        out.push(format!("    halt_threshold   {}", self.halt_threshold));
        out.push(String::new());
        out.push("  command injection".into());
        out.push(format!("    .harness/commands/{}.md", self.name));
        out.push("    /reload to register it, then it appears in the popup".into());
        out
    }
}

/// Writes both files. Returns the paths written, in the order written.
///
/// The state file is written first and never overwritten: a loop that has
/// already run holds its own history, and regenerating the command
/// definition is not a reason to forget it.
pub fn generate(root: &Path, name: &str) -> std::io::Result<(LoopSpec, Vec<PathBuf>)> {
    debug_assert!(is_valid_name(name), "callers check this at parse time");
    let paths = Paths::under(root);
    let spec = LoopSpec::rust_default(name, &paths);

    std::fs::create_dir_all(&paths.loops_dir)?;
    std::fs::create_dir_all(&paths.commands_dir)?;

    let mut written = Vec::new();
    let state = paths.state_file(name);
    if !state.exists() {
        std::fs::write(&state, spec.initial_state())?;
        written.push(state);
    }
    let cmd = paths.command_file(name);
    std::fs::write(&cmd, spec.render_command_file())?;
    written.push(cmd);

    Ok((spec, written))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chat-eng-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_name_that_could_escape_the_directory_is_rejected() {
        for bad in ["..", ".", "a/b", "a\\b", "", " ", "a b", &"x".repeat(41)] {
            assert!(!is_valid_name(bad), "`{bad}` was accepted");
        }
        for good in ["test-loop", "fix_types", "a", "x9"] {
            assert!(is_valid_name(good), "`{good}` was rejected");
        }
    }

    #[test]
    fn generate_writes_both_files_and_the_command_parses_back() {
        let root = tmp("generate");
        let (spec, written) = generate(&root, "test-loop").unwrap();
        assert_eq!(written.len(), 2);
        assert!(root.join(".harness/commands/test-loop.md").exists());
        assert!(root.join(".harness/loops/test-loop.json").exists());

        // The definition this writes is the definition the loader reads.
        // Rendering something the loader cannot parse would leave `/reload`
        // silently registering nothing.
        let text = std::fs::read_to_string(root.join(".harness/commands/test-loop.md")).unwrap();
        let parsed = super::super::loader::parse_command_file(&text, "test-loop.md")
            .expect("what /eng writes must be what /reload reads");
        assert_eq!(parsed.name, "test-loop");
        assert!(parsed.custom);
        assert!(text.contains(&spec.verification[0]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_initial_state_is_the_shape_the_brief_names() {
        let paths = Paths::under(Path::new("/x"));
        let spec = LoopSpec::rust_default("l", &paths);
        let v: serde_json::Value = serde_json::from_str(&spec.initial_state()).unwrap();
        assert_eq!(v["cycles"], 0);
        assert!(v["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn regenerating_does_not_forget_what_a_loop_already_did() {
        // The command definition is a template and may be rewritten. The
        // state file is history: a loop that failed three cycles yesterday
        // must still know that, or the no-progress halt cannot fire.
        let root = tmp("regen");
        generate(&root, "l").unwrap();
        let state = root.join(".harness/loops/l.json");
        std::fs::write(&state, "{\"cycles\": 3, \"errors\": [\"E0308\"]}").unwrap();

        let (_, written) = generate(&root, "l").unwrap();
        assert_eq!(written.len(), 1, "only the command file is rewritten");
        let after = std::fs::read_to_string(&state).unwrap();
        assert!(after.contains("E0308"), "history was clobbered: {after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_verification_step_names_real_commands() {
        // A self-verifying loop whose verification is a placeholder
        // verifies nothing and reports success, which is worse than a loop
        // that does not verify at all — it carries a claim.
        let paths = Paths::under(Path::new("/x"));
        let spec = LoopSpec::rust_default("l", &paths);
        assert!(spec.verification.iter().any(|v| v.contains("clippy")));
        assert!(spec.verification.iter().any(|v| v.contains("test")));
        assert!(!spec.verification.is_empty());
    }

    #[test]
    fn the_summary_names_all_three_parts() {
        let paths = Paths::under(Path::new("/x"));
        let text = LoopSpec::rust_default("l", &paths).summary().join("\n");
        assert!(text.contains("loop orchestration"));
        assert!(text.contains("contracts"));
        assert!(text.contains("command injection"));
        assert!(text.contains("human gate"));
        assert!(text.contains("halt_threshold"));
    }
}
