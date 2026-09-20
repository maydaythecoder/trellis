//! Tool schema to grammar, with llguidance as the single grammar authority.
//!
//! No hand-written BNF anywhere: a tool's argument struct derives
//! `schemars::JsonSchema` and schemars emits JSON Schema.
//!
//! **What travels is the schema, not a compiled grammar.** The serving
//! runtimes take a JSON Schema and compile it themselves — llama.cpp
//! (`json_schema`), vLLM (`guided_json`), SGLang (`json_schema`) — under
//! three different field names, and llama.cpp's `grammar` field wants GBNF
//! rather than llguidance's Lark. JSON Schema is the one format all of them
//! accept, so it is the wire format for tier 2.
//!
//! llguidance's role here is therefore **validation, not delivery**: a schema
//! it cannot compile is one no runtime should be asked to compile either, and
//! finding that out at startup beats finding out mid-run. It becomes a
//! delivery mechanism only for tier 4 (validating returned text) and for
//! tier 3, which ADR 0013 defers.

use llguidance::api::ParserLimits;
use llguidance::{GrammarBuilder, JsonCompileOptions};

#[derive(Debug, thiserror::Error)]
pub enum GrammarError {
    #[error("schema is not valid JSON Schema for grammar compilation: {0}")]
    Schema(String),
}

/// A compiled grammar for one tool's arguments.
///
/// Holds the serialized grammar rather than a live parser: the same bytes
/// go to a local sampler or to a runtime parameter, and neither needs the
/// builder that produced them.
#[derive(Debug, Clone)]
pub struct ToolGrammar {
    pub tool_name: String,
    /// The JSON Schema the grammar was compiled from. Kept so `trellis
    /// doctor` and the trail can show what the model was constrained to.
    pub schema: serde_json::Value,
}

impl ToolGrammar {
    /// Compiles `schema` and returns it on success.
    ///
    /// Compilation is the validation: a schema llguidance cannot compile is
    /// a schema we must not ship to a runtime either, and finding that out
    /// at startup beats finding out mid-run.
    pub fn compile(tool_name: &str, schema: serde_json::Value) -> Result<Self, GrammarError> {
        let builder = GrammarBuilder::new(None, ParserLimits::default());
        JsonCompileOptions::default()
            .json_to_llg(builder, schema.clone())
            .map_err(|e| GrammarError::Schema(e.to_string()))?;

        Ok(Self {
            tool_name: tool_name.to_string(),
            schema,
        })
    }

    /// Compiles from a type's derived schema.
    pub fn for_args<T: schemars::JsonSchema>(tool_name: &str) -> Result<Self, GrammarError> {
        let schema = serde_json::to_value(schemars::schema_for!(T))
            .map_err(|e| GrammarError::Schema(e.to_string()))?;
        Self::compile(tool_name, schema)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct BlastRadiusArgs {
        /// Fully qualified symbol, e.g. `poetry.installation.installer.Installer.run`.
        symbol: String,
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct EditArgs {
        path: String,
        old: String,
        new: String,
        line_hint: Option<u32>,
    }

    #[test]
    fn derived_tool_schemas_compile_to_grammars() {
        // The end-to-end path the loop depends on: derive -> JSON Schema ->
        // llguidance. If llguidance ever rejects what schemars emits, this
        // fails here rather than mid-run against a user's repo.
        let g = ToolGrammar::for_args::<BlastRadiusArgs>("blast_radius").unwrap();
        assert_eq!(g.tool_name, "blast_radius");
        assert!(g.schema["properties"]["symbol"].is_object());

        ToolGrammar::for_args::<EditArgs>("edit").unwrap();
    }

    #[test]
    fn an_uncompilable_schema_fails_at_compile_time_not_mid_run() {
        let bad = serde_json::json!({ "type": "not-a-json-schema-type" });
        assert!(ToolGrammar::compile("bogus", bad).is_err());
    }
}
