//! The JSON a model emits to call a tool.
//!
//! One wire format, used by both the scripted model and the runtime adapter.
//! Two parsers would drift, and the one that drifts is the one under test —
//! so a script that passes would stop proving anything about what a real model
//! must produce.
//!
//! The `JsonSchema` derive here is every built-in tool there is. What tier 2
//! ships to the runtime is this schema with its branches filtered to the
//! tools a run enables and the repository's own tools appended
//! (`registry::ToolSet::schema`, ADR 0023). Widening this type widens what a
//! run *may* enable; the tool set decides what one does.

use crate::agent::{Step, ToolCall};
use schemars::JsonSchema;
use serde::Deserialize;
use trellis_checks::CheckKind;

/// One move: a tool call, or a declaration that the work is finished.
///
/// `done` is in the same union as the tools deliberately. A model that can
/// only ever call tools cannot stop, and a stop expressed out-of-band would
/// escape the grammar — the one place the format is guaranteed.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCallWire {
    /// Which first-party call sites reach a symbol, with coverage.
    BlastRadius { symbol: String },
    /// Read a file, relative to the repository root.
    ReadFile { path: String },
    /// Replace `old` with `new`. `old` must occur exactly once in the file.
    Edit {
        path: String,
        old: String,
        new: String,
    },
    /// Run the repository's own checks. The only way a correctness claim
    /// enters the loop.
    RunChecks {
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default = "default_kinds")]
        kinds: Vec<String>,
    },
    /// Read one of the repository's skills, by the name it is listed under.
    /// Reference material; it informs and cannot stand in for a check.
    ReadSkill { name: String },
    /// Create a new file. Refused if the path exists; use `edit` for that.
    WriteFile { path: String, content: String },
    /// Search file contents with `git grep`, optionally under one path.
    Grep {
        pattern: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// Repository files matching a glob: `*`, `**` and `?`.
    Find { glob: String },
    /// List one directory, relative to the repository root.
    Ls {
        #[serde(default = "default_dir")]
        path: String,
    },
    /// Run a shell command in the repository root. Its output is not a check.
    Bash { command: String },
    /// Stop. Not a claim of success: the loop decides that from the checks.
    Done {
        #[serde(default)]
        summary: String,
    },
}

fn default_kinds() -> Vec<String> {
    vec!["typecheck".into()]
}

fn default_dir() -> String {
    ".".into()
}

impl ToolCallWire {
    /// Converts to the loop's own type, rejecting anything the loop cannot
    /// act on.
    ///
    /// An unknown check kind is an error rather than a silent drop: a run that
    /// quietly skipped the tests a model asked for would report a passing
    /// typecheck as though the suite had been consulted.
    pub fn into_step(self) -> Result<Step, String> {
        Ok(match self {
            ToolCallWire::BlastRadius { symbol } => Step::Call(ToolCall::BlastRadius { symbol }),
            ToolCallWire::ReadFile { path } => Step::Call(ToolCall::ReadFile { path }),
            ToolCallWire::Edit { path, old, new } => Step::Call(ToolCall::Edit { path, old, new }),
            ToolCallWire::RunChecks { paths, kinds } => {
                let kinds = kinds
                    .iter()
                    .map(|k| match k.to_ascii_lowercase().as_str() {
                        "typecheck" => Ok(CheckKind::Typecheck),
                        "tests" => Ok(CheckKind::Tests),
                        "import" => Ok(CheckKind::Import),
                        "governance" => Ok(CheckKind::Governance),
                        other => Err(format!(
                            "unknown check kind `{other}`; expected typecheck, tests, import or governance"
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Step::Call(ToolCall::RunChecks { paths, kinds })
            }
            ToolCallWire::ReadSkill { name } => Step::Call(ToolCall::ReadSkill { name }),
            ToolCallWire::WriteFile { path, content } => {
                Step::Call(ToolCall::WriteFile { path, content })
            }
            ToolCallWire::Grep { pattern, path } => Step::Call(ToolCall::Grep { pattern, path }),
            ToolCallWire::Find { glob } => Step::Call(ToolCall::Find { glob }),
            ToolCallWire::Ls { path } => Step::Call(ToolCall::Ls { path }),
            ToolCallWire::Bash { command } => Step::Call(ToolCall::Bash { command }),
            ToolCallWire::Done { summary } => Step::Done { summary },
        })
    }

    /// The JSON Schema a constrained runtime is given.
    ///
    /// Carries `x-guidance: { whitespace_flexible: false }`, which is not
    /// cosmetic. JSON permits arbitrary whitespace between tokens, so a
    /// schema-constrained model is free to emit spaces forever and still be
    /// producing valid JSON — the grammar has no reason to stop it.
    ///
    /// Measured on a 0.5B under greedy sampling: asked for one boolean field,
    /// it opened the object, emitted the key, and then produced 35 consecutive
    /// spaces until the token budget ran out. With flexible whitespace off it
    /// produced a complete object in 13 tokens. The difference is not
    /// marginal, and the failure is the kind that looks like a bad model
    /// rather than a bad grammar.
    ///
    /// `x-guidance` is read by llguidance itself, which is the grammar engine
    /// inside llama.cpp, vLLM and SGLang — so this travels with the schema and
    /// applies on tier 2, not only where TRELLIS owns the sampler.
    pub fn schema() -> serde_json::Value {
        let mut s = serde_json::to_value(schemars::schema_for!(ToolCallWire))
            .expect("a derived schema is always serializable");
        if let Some(obj) = s.as_object_mut() {
            obj.insert(
                "x-guidance".into(),
                serde_json::json!({ "whitespace_flexible": false }),
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_names_every_tool() {
        // This schema is what constrains a real model, so a tool missing here
        // is a tool the model can never call.
        let s = ToolCallWire::schema().to_string();
        for tool in [
            "blast_radius",
            "read_file",
            "edit",
            "run_checks",
            "read_skill",
            "done",
        ] {
            assert!(s.contains(tool), "schema omits `{tool}`: {s}");
        }
    }

    #[test]
    fn the_schema_forbids_flexible_whitespace() {
        // Without this a constrained weak model can emit spaces until its
        // token budget runs out and never reach a value, which reads as a
        // model failure and is a grammar one.
        let s = ToolCallWire::schema();
        assert_eq!(
            s["x-guidance"]["whitespace_flexible"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn the_schema_compiles_to_a_grammar() {
        // If llguidance cannot compile it, no runtime can either, and finding
        // that out at startup beats finding it out mid-run.
        trellis_model::ToolGrammar::compile("tool_call", ToolCallWire::schema())
            .expect("the tool-call schema must compile");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // `deny_unknown_fields` keeps a model from smuggling parameters the
        // loop does not implement and appearing to have used them.
        let e = serde_json::from_str::<ToolCallWire>(
            r#"{"tool":"read_file","path":"a.py","sudo":true}"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("sudo"), "{e}");
    }

    #[test]
    fn an_unknown_check_kind_is_refused_rather_than_dropped() {
        let w: ToolCallWire =
            serde_json::from_str(r#"{"tool":"run_checks","kinds":["lint"]}"#).unwrap();
        assert!(w.into_step().unwrap_err().contains("unknown check kind"));
    }

    #[test]
    fn done_is_part_of_the_constrained_vocabulary() {
        // A model that can only call tools cannot stop.
        let w: ToolCallWire = serde_json::from_str(r#"{"tool":"done","summary":"x"}"#).unwrap();
        assert!(matches!(w.into_step().unwrap(), Step::Done { .. }));
    }
}
