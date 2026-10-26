//! What a turn did, in the words the trail records it with.
//!
//! One function for the trail's action (`action_from` in trellis-cli's
//! `run_cmd.rs`) and the `turn` event (`agent.rs`), so a client reading a
//! run live and a reader of its trail afterwards are told the same tool,
//! target and write. Before #800 the clients split the event's `call` at a
//! space and searched the observation themselves, a copy of this that could
//! drift from it.

use crate::agent::ToolCall;

/// A turn's tool, what it was aimed at, and whether it wrote through the
/// orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAction {
    /// The tool, as the model named it; a repository's own tool by its name.
    pub tool: String,
    /// What it was aimed at: a path, a symbol, the checks requested.
    pub target: String,
    /// Whether an edit applied or a file was created. Only those change the
    /// tree through the orchestrator; what `bash` or a custom tool changed
    /// is recorded apart (ADR 0023 §7), so `wrote` keeps the meaning older
    /// trails gave it.
    pub wrote: bool,
}

/// The [`TurnAction`] of a call and the observation it came back with.
pub fn turn_action(call: &ToolCall, observation: &str) -> TurnAction {
    let (tool, target) = match call {
        ToolCall::BlastRadius { symbol } => ("blast_radius", symbol.clone()),
        ToolCall::ReadFile { path } => ("read_file", path.clone()),
        ToolCall::Edit { path, .. } => ("edit", path.clone()),
        ToolCall::RunChecks { paths, kinds } => {
            let kinds: Vec<String> = kinds.iter().map(|k| format!("{k:?}")).collect();
            // The checks requested, then what they were pointed at. An empty
            // `paths` is the whole repository and says so, rather than
            // rendering as a blank the reader has to interpret.
            let where_ = if paths.is_empty() {
                "whole repository".to_string()
            } else {
                paths.join(", ")
            };
            ("run_checks", format!("{} on {where_}", kinds.join("+")))
        }
        ToolCall::ReadSkill { name } => ("read_skill", name.clone()),
        // ADR 0023's tools. A repository's own tool is recorded under its
        // own name, with the arguments it was sent.
        ToolCall::WriteFile { path, .. } => ("write_file", path.clone()),
        ToolCall::Grep { pattern, path } => (
            "grep",
            match path {
                Some(p) => format!("{pattern} in {p}"),
                None => pattern.clone(),
            },
        ),
        ToolCall::Find { glob } => ("find", glob.clone()),
        ToolCall::Ls { path } => ("ls", path.clone()),
        ToolCall::Bash { command } => ("bash", summarise(command)),
        ToolCall::SearchNotes { query, .. } => ("search_notes", summarise(query)),
        ToolCall::ReadNote { note } => ("read_note", note.clone()),
        ToolCall::CallersOf { symbol } => ("callers_of", symbol.clone()),
        ToolCall::CalleesOf { symbol } => ("callees_of", symbol.clone()),
        ToolCall::CoverageReport { paths } => ("coverage_report", summarise(&paths.join(" "))),
        ToolCall::Custom { name, args } => (name.as_str(), summarise(&args.to_string())),
        // Quoted: model output, and a trail is read back into prompts.
        ToolCall::AskHuman { question, .. } => ("ask_human", summarise(&format!("{question:?}"))),
    };
    let wrote = match call {
        ToolCall::Edit { .. } => observation.contains("applied: true"),
        ToolCall::WriteFile { path, .. } => observation.starts_with(&format!("created {path} ")),
        _ => false,
    };
    TurnAction {
        tool: tool.to_string(),
        target,
        wrote,
    }
}

/// An observation reduced to one line a person can read in a list.
pub fn summarise(observation: &str) -> String {
    const MAX: usize = 120;
    let first = observation
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.chars().count() <= MAX {
        return first.to_string();
    }
    // Cut on a character boundary, and say it was cut — a silently truncated
    // line reads as the whole outcome.
    let head: String = first.chars().take(MAX - 1).collect();
    format!("{head}…")
}
