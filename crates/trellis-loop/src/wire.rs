//! The JSON a model emits to call a tool.
//!
//! One wire format, used by both the scripted model and the runtime adapter.
//! Two parsers would drift, and the one that drifts is the one under test —
//! so a script that passes would stop proving anything about what a real model
//! must produce.
//!
//! The `JsonSchema` derive here is the artefact tier 2 ships to the serving
//! runtime, so this type *is* the constraint. Widening it widens what a model
//! is allowed to emit.

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
    /// Stop. Not a claim of success: the loop decides that from the checks.
    Done {
        #[serde(default)]
        summary: String,
    },
}

fn default_kinds() -> Vec<String> {
    vec!["typecheck".into()]
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
                        other => Err(format!(
                            "unknown check kind `{other}`; expected typecheck, tests or import"
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Step::Call(ToolCall::RunChecks { paths, kinds })
            }
            ToolCallWire::Done { summary } => Step::Done { summary },
        })
    }

    /// The JSON Schema a constrained runtime is given.
    pub fn schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ToolCallWire))
            .expect("a derived schema is always serializable")
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
        for tool in ["blast_radius", "read_file", "edit", "run_checks", "done"] {
            assert!(s.contains(tool), "schema omits `{tool}`: {s}");
        }
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
