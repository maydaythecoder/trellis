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
use std::collections::BTreeMap;

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
    /// A step in a `targets` section, numbered from 1 within it.
    #[error("targets.{target} step {step}: {source}")]
    TargetStep {
        target: String,
        step: usize,
        source: ToolCallError,
    },
    #[error("targets.{0} has no steps")]
    TargetNoSteps(String),
    /// A section for a symbol the run was not given: a typo there would
    /// otherwise fall back to `steps` and do less than the script says.
    #[error("targets.{0} names no --target of this run (its targets: {1})")]
    UnknownTarget(String, String),
    /// One agent plays one list of steps, and two of its targets each have
    /// their own; running one and dropping the other would do less than the
    /// script says.
    #[error(
        "targets `{0}` and `{1}` are one partition, so one agent works on both, and the \
         script gives each its own steps; give them one section between them, or use \
         `steps` (`trellis partitions` shows which targets share a partition)"
    )]
    SharedPartition(String, String),
}

/// Where under `.trellis/` a run writes its replay recording (#620, ADR 0056).
pub const REPLAYS_DIR: &str = "replays";

/// The `.gitignore` trellis writes in `.trellis/replays/` unasked. Public so
/// the trust digest can tell this file from one a person wrote (ADR 0023 §4,
/// amended 2026-10-12).
///
/// Its own file rather than a line in `.trellis/.gitignore`: that one is
/// written once and never rewritten, so a line added there would never reach
/// a repository that already has one, and its first replay would dirty the
/// worktree the next run needs clean (ADR 0009).
pub const REPLAY_IGNORE: &str = r#"# Written by TRELLIS. Every run records the steps its model took here, as a
# script `trellis run --model scripted:<file>` plays back (ADR 0056). They
# are per-machine records, like the index: never committed, and a run
# refuses to start while git tracks anything here.
#
# This file ignores itself and everything beside it, so a recording never
# makes `git status` dirty, which would block the next run (ADR 0009).
*
"#;

/// What one run's model decided, as the script that plays it back (#620).
///
/// Holds steps, never verdicts: a replay runs the checks again, so nothing
/// read back from a recording can stand for a pass (rules 1 and 2). It is
/// read back through [`ScriptedModel`], as data: parsed against the
/// replaying run's own tools, with an unknown tool refused at load (rule 10).
#[derive(Debug, Default, Clone)]
pub struct Recording {
    pub run_id: String,
    /// The commit the run started from; a replay means the same tree.
    pub base_commit: String,
    pub task: String,
    /// A single-agent run's steps, in the order the model gave them.
    pub steps: Vec<Step>,
    /// A multi-agent run's steps, one section per agent, keyed by the first
    /// of its targets, which is how [`ScriptedModel::from_json_for`] finds an
    /// agent's section.
    pub targets: BTreeMap<String, Vec<Step>>,
}

impl Recording {
    /// The recording as a script. `run_id`, `base_commit` and `task` are for
    /// the person reading it; the loader reads only `steps` and `targets`.
    pub fn to_json(&self) -> serde_json::Value {
        let steps = |s: &[Step]| -> Vec<serde_json::Value> {
            s.iter().map(crate::registry::step_json).collect()
        };
        let mut v = serde_json::json!({
            "recording": 1,
            "run_id": self.run_id,
            "base_commit": self.base_commit,
            "task": self.task,
        });
        if self.targets.is_empty() {
            v["steps"] = steps(&self.steps).into();
        } else {
            let sections: serde_json::Map<String, serde_json::Value> = self
                .targets
                .iter()
                .map(|(t, s)| (t.clone(), serde_json::json!({ "steps": steps(s) })))
                .collect();
            v["targets"] = sections.into();
        }
        v
    }
}

#[derive(Debug, Deserialize)]
struct Script {
    /// What every agent plays, unless `targets` gives it its own.
    #[serde(default)]
    steps: Vec<serde_json::Value>,
    /// Steps for the agent working on one `--target` of a multi-agent run,
    /// keyed by the symbol as it was typed (#466).
    #[serde(default)]
    targets: BTreeMap<String, Section>,
}

#[derive(Debug, Deserialize)]
struct Section {
    steps: Vec<serde_json::Value>,
}

/// Parses each step through the run's own parser; `err` numbers a failure.
fn parse_steps(
    values: Vec<serde_json::Value>,
    tools: &ToolSet,
    err: impl Fn(usize, ToolCallError) -> ScriptError,
) -> Result<Vec<Step>, ScriptError> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| tools.parse_value(v).map_err(|source| err(i + 1, source)))
        .collect()
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
    ///
    /// Plays `steps`. A `targets` section is for one agent of a multi-agent
    /// run, and only [`from_json_for`](Self::from_json_for) reads it.
    pub fn from_json_with(text: &str, tools: &ToolSet) -> Result<Self, ScriptError> {
        let script: Script = serde_json::from_str(text).map_err(ScriptError::NotJson)?;
        Self::fallback(script.steps, tools)
    }

    /// The script for one agent of a multi-agent run (#466): the `targets`
    /// section of the one target in `mine` that has one, else `steps`.
    ///
    /// `all` is every target of the run, as typed: a section naming none of
    /// them is refused, as is a partition two of whose targets each have a
    /// section. Both are refused at load, before the run's snapshot.
    pub fn from_json_for(
        text: &str,
        tools: &ToolSet,
        all: &[String],
        mine: &[String],
    ) -> Result<Self, ScriptError> {
        let mut script: Script = serde_json::from_str(text).map_err(ScriptError::NotJson)?;
        if let Some(stray) = script.targets.keys().find(|k| !all.contains(k)) {
            return Err(ScriptError::UnknownTarget(stray.clone(), all.join(", ")));
        }
        let with: Vec<&String> = mine
            .iter()
            .filter(|t| script.targets.contains_key(*t))
            .collect();
        match with.as_slice() {
            [] => Self::fallback(script.steps, tools),
            [target] => {
                let section = script
                    .targets
                    .remove(*target)
                    .expect("filtered on contains_key above");
                let steps = parse_steps(section.steps, tools, |step, source| {
                    ScriptError::TargetStep {
                        target: (*target).clone(),
                        step,
                        source,
                    }
                })?;
                if steps.is_empty() {
                    return Err(ScriptError::TargetNoSteps((*target).clone()));
                }
                Ok(Self { steps, at: 0 })
            }
            [a, b, ..] => Err(ScriptError::SharedPartition((*a).clone(), (*b).clone())),
        }
    }

    /// `steps`, which every agent without a section of its own plays.
    fn fallback(values: Vec<serde_json::Value>, tools: &ToolSet) -> Result<Self, ScriptError> {
        let steps = parse_steps(values, tools, |step, source| ScriptError::Step {
            step,
            source,
        })?;
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
        assert!(
            e.contains("step 1") && e.contains("unknown tool `sudo`"),
            "{e}"
        );
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
            timeout_s: None,
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

    /// Every target of the run, and the ones one agent holds.
    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    const TARGETED: &str = r#"{
        "steps": [{"tool":"read_file","path":"x.py"}],
        "targets": {
            "fa": {"steps": [{"tool":"read_file","path":"a.py"},{"tool":"done","summary":"a"}]},
            "fb": {"steps": [{"tool":"read_file","path":"b.py"}]}
        }
    }"#;

    #[test]
    fn an_agent_plays_its_targets_section_and_falls_back_to_steps() {
        // Option (a) on #466: each partition's agent plays the section of
        // the target it holds, and one with no section plays `steps`.
        let all = names(&["fa", "fb", "fc"]);
        let tools = ToolSet::every_builtin();
        let a = ScriptedModel::from_json_for(TARGETED, &tools, &all, &names(&["fa"])).unwrap();
        assert_eq!(a.len(), 2);
        let b = ScriptedModel::from_json_for(TARGETED, &tools, &all, &names(&["fb"])).unwrap();
        assert_eq!(b.len(), 1);
        let c = ScriptedModel::from_json_for(TARGETED, &tools, &all, &names(&["fc"])).unwrap();
        assert_eq!(c.len(), 1, "fc has no section and plays steps");
        // A single-agent run plays `steps`, as before #466.
        assert_eq!(ScriptedModel::from_json(TARGETED).unwrap().len(), 1);
    }

    #[test]
    fn a_section_for_a_symbol_the_run_was_not_given_is_refused() {
        // A typo in a key would otherwise fall back to `steps`, and the
        // script would silently do less than it says.
        let e = ScriptedModel::from_json_for(
            TARGETED,
            &ToolSet::every_builtin(),
            &names(&["fa", "fc"]),
            &names(&["fa"]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("targets.fb") && e.contains("fa, fc"), "{e}");
    }

    #[test]
    fn two_sections_for_one_agent_are_refused_rather_than_one_dropped() {
        let e = ScriptedModel::from_json_for(
            TARGETED,
            &ToolSet::every_builtin(),
            &names(&["fa", "fb"]),
            &names(&["fa", "fb"]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("`fa` and `fb`"), "{e}");
    }

    #[test]
    fn a_bad_step_in_a_section_is_refused_naming_the_section() {
        let text = r#"{"targets":{"fa":{"steps":[{"tool":"sudo"}]}}}"#;
        let e = ScriptedModel::from_json_for(
            text,
            &ToolSet::every_builtin(),
            &names(&["fa"]),
            &names(&["fa"]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("targets.fa step 1"), "{e}");
        let text = r#"{"targets":{"fa":{"steps":[]}}}"#;
        assert!(
            ScriptedModel::from_json_for(
                text,
                &ToolSet::every_builtin(),
                &names(&["fa"]),
                &names(&["fa"])
            )
            .is_err()
        );
        // With no `steps` and no section, an agent has nothing to play.
        assert!(
            ScriptedModel::from_json_for(
                text,
                &ToolSet::every_builtin(),
                &names(&["fa", "fb"]),
                &names(&["fb"])
            )
            .is_err()
        );
    }

    #[test]
    fn a_recording_plays_back_the_steps_it_holds() {
        // #620: a recording is a script. A single-agent one loads through
        // `from_json_with`, one agent's section of a multi-agent one through
        // `from_json_for`, and either holds the same steps it was given.
        let tools = ToolSet::every_builtin();
        let steps = vec![
            tools
                .parse(r#"{"tool":"edit","path":"a.py","old":"1","new":"2"}"#)
                .unwrap(),
            Step::Done {
                summary: "ok".into(),
            },
        ];
        let one = Recording {
            run_id: "r".into(),
            steps: steps.clone(),
            ..Default::default()
        };
        let m = ScriptedModel::from_json_with(&one.to_json().to_string(), &tools).unwrap();
        assert_eq!(m.steps, steps);
        let many = Recording {
            targets: [("fa".to_string(), steps.clone())].into(),
            ..Default::default()
        };
        let text = many.to_json().to_string();
        let all = names(&["fa", "fb"]);
        let m = ScriptedModel::from_json_for(&text, &tools, &all, &names(&["fa"])).unwrap();
        assert_eq!(m.steps, steps);
        // An agent the recording has no section for has nothing to play,
        // and is refused at load rather than given another agent's steps.
        assert!(ScriptedModel::from_json_for(&text, &tools, &all, &names(&["fb"])).is_err());
    }

    #[tokio::test]
    async fn running_past_the_end_yields_done() {
        let mut m =
            ScriptedModel::from_json(r#"{"steps":[{"tool":"read_file","path":"a"}]}"#).unwrap();
        let t = Transcript {
            task: "t".into(),
            turns: vec![],
            final_notice: false,
            steering: Default::default(),
            prior: None,
            repo: crate::agent::RepoContext::default(),
        };
        assert!(matches!(m.next_step(&t).await, Ok(Step::Call(_))));
        assert!(matches!(m.next_step(&t).await, Ok(Step::Done { .. })));
    }
}
