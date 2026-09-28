//! A model that plays a fixed script.
//!
//! Not a stub standing in for a real adapter. It is a real `Model` whose
//! decisions happen to be written down in advance, and it exists because the
//! loop's behaviour — serialized writes, budget escalation, rollback, and
//! above all the refusal to converge without an executed check — is worth
//! testing end to end *before* inference is in the picture.
//!
//! It also gives v0.2's exit condition a path that does not wait on weights:
//! "one real change runs end to end" can be demonstrated with real files, a
//! real type checker and a real rollback, with only the judgement stubbed.
//! What that does **not** demonstrate is judgement, and no result obtained
//! this way may be reported as a model floor (S7).

use crate::agent::{Model, Step, Transcript};
use crate::registry::{ToolCallError, ToolSet};
use serde::Deserialize;

/// Why a script was refused at load. Each message is the string
/// [`ScriptedModel::from_json_with`] returned before (review round 11).
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("script is not valid JSON: {0}")]
    NotJson(serde_json::Error),
    /// Numbered from 1, as a person counts the steps in the file.
    #[error("step {step}: {source}")]
    Step { step: usize, source: ToolCallError },
    #[error("script has no steps")]
    NoSteps,
}

#[derive(Debug, Deserialize)]
struct Script {
    steps: Vec<serde_json::Value>,
}

#[derive(Debug)]
pub struct ScriptedModel {
    steps: Vec<Step>,
    at: usize,
}

impl ScriptedModel {
    /// Parses a script. An unknown tool or check kind is an error rather than
    /// a skipped step: a script that silently does less than it says would
    /// make the loop look better behaved than it is.
    ///
    /// Every built-in parses; a repository's own tools do not, since there
    /// is no tool set to find them in. [`from_json_with`](Self::from_json_with)
    /// parses against the run's own set.
    pub fn from_json(text: &str) -> Result<Self, ScriptError> {
        Self::from_json_with(text, &ToolSet::every_builtin())
    }

    /// Parses a script against the tools a run actually offers, through the
    /// same parser the runtime adapter uses (ADR 0023). A step calling a
    /// tool the run does not have is an error at load, for the reason an
    /// unknown tool is: a script that does less than it says makes the loop
    /// look better behaved than it is.
    pub fn from_json_with(text: &str, tools: &ToolSet) -> Result<Self, ScriptError> {
        let script: Script = serde_json::from_str(text).map_err(ScriptError::NotJson)?;
        let steps = script
            .steps
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                tools.parse_value(v).map_err(|source| ScriptError::Step {
                    step: i + 1,
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if steps.is_empty() {
            return Err(ScriptError::NoSteps);
        }
        Ok(Self { steps, at: 0 })
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl Model for ScriptedModel {
    async fn next_step(&mut self, _t: &Transcript) -> Result<Step, String> {
        // Running off the end is `Done`, not an error. The loop then applies
        // its own rule: a script that stops without a passing check does not
        // converge, which is exactly what it should demonstrate.
        let s = self.steps.get(self.at).cloned().unwrap_or(Step::Done {
            summary: "script ended".into(),
        });
        self.at += 1;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_parses_every_tool() {
        let m = ScriptedModel::from_json(
            r#"{"steps":[
                {"tool":"blast_radius","symbol":"f"},
                {"tool":"read_file","path":"a.py"},
                {"tool":"edit","path":"a.py","old":"x","new":"y"},
                {"tool":"run_checks","paths":["a.py"],"kinds":["typecheck","tests"]},
                {"tool":"read_skill","name":"rust-unsafe"},
                {"tool":"done","summary":"ok"}
            ]}"#,
        )
        .expect("valid script");
        assert_eq!(m.len(), 6);
    }

    #[test]
    fn an_unknown_check_kind_is_refused() {
        let e = ScriptedModel::from_json(r#"{"steps":[{"tool":"run_checks","kinds":["lint"]}]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown check kind"), "{e}");
    }

    #[test]
    fn an_unknown_tool_is_refused_rather_than_skipped() {
        // A skipped step would make the loop look better behaved than it is.
        let e = ScriptedModel::from_json(r#"{"steps":[{"tool":"sudo","path":"x"}]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not valid JSON") || e.contains("unknown"), "{e}");
    }

    #[test]
    fn a_script_calls_a_repository_tool_only_where_the_run_offers_it() {
        let lint = crate::registry::CustomTool {
            name: "lint".into(),
            description: "Lint one file.".into(),
            command: vec!["x".into()],
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        };
        let tools = ToolSet::build(crate::registry::ToolSelection {
            custom: vec![lint],
            ..Default::default()
        })
        .unwrap();
        let text = r#"{"steps":[{"tool":"lint","path":"a.py"}]}"#;
        assert_eq!(
            ScriptedModel::from_json_with(text, &tools).unwrap().len(),
            1
        );
        // Without the tool set there is no `lint`, and a script calling it is
        // refused at load rather than failing mid-run.
        assert!(ScriptedModel::from_json(text).is_err());
        // A built-in the run does not enable is refused the same way.
        let e = ScriptedModel::from_json_with(
            r#"{"steps":[{"tool":"bash","command":"ls"}]}"#,
            &ToolSet::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("step 1") && e.contains("bash"), "{e}");
    }

    #[test]
    fn an_empty_script_is_refused() {
        assert!(ScriptedModel::from_json(r#"{"steps":[]}"#).is_err());
    }

    #[tokio::test]
    async fn running_past_the_end_yields_done() {
        let mut m =
            ScriptedModel::from_json(r#"{"steps":[{"tool":"read_file","path":"a"}]}"#).unwrap();
        let t = Transcript {
            task: "t".into(),
            turns: vec![],
            final_notice: false,
            prior: None,
            repo: crate::agent::RepoContext::default(),
        };
        assert!(matches!(m.next_step(&t).await, Ok(Step::Call(_))));
        assert!(matches!(m.next_step(&t).await, Ok(Step::Done { .. })));
    }
}
