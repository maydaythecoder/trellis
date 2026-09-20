//! The model adapter: a serving runtime driving the loop.
//!
//! Tier 2 hands the tool-call JSON Schema to the runtime's guided-decoding
//! parameter, so output is well-formed **by construction**. Tier 4 sends no
//! schema and validates what comes back, retrying on malformed output.
//!
//! The difference is not cosmetic and is why `trellis doctor` reports the
//! active tier: on tier 4 a run can fail on *format alone*, which is the
//! failure mode quantization makes common — quantization degrades format
//! reliability faster than it degrades reasoning, so a model that reasons
//! adequately about the code may still be unable to emit valid JSON reliably.
//!
//! **Retries live here, not in the loop.** `Model::next_step` returning `Err`
//! ends a run, so a malformed call — which has not failed at anything except
//! formatting — must be retried inside this call or a recoverable hiccup would
//! end work that was going fine.

use crate::agent::{Model, Step, Transcript};
use crate::wire::ToolCallWire;
use trellis_model::{AdapterTier, Dialect, RuntimeClient};

/// How many times a malformed response is retried before the run ends.
///
/// Small on purpose. A model that cannot produce a valid call in a handful of
/// attempts is below the floor for this harness, and burning a budget to
/// discover that slowly helps nobody (S7).
const MAX_FORMAT_RETRIES: u32 = 4;

pub struct RuntimeModel {
    client: RuntimeClient,
    tier: AdapterTier,
    max_tokens: u32,
    tokens: u64,
}

impl RuntimeModel {
    /// `constrained` selects tier 2 over tier 4.
    ///
    /// It is a caller's decision rather than something guessed here, because
    /// only a probe can tell whether the runtime honours guided decoding — and
    /// a runtime silently ignores parameters it does not recognise, so
    /// guessing wrong yields unconstrained output that looks like success.
    pub fn new(client: RuntimeClient, constrained: bool) -> Self {
        Self {
            client,
            tier: if constrained {
                AdapterTier::RuntimeConstrained
            } else {
                AdapterTier::ValidateAndRetry
            },
            max_tokens: 512,
            tokens: 0,
        }
    }

    pub fn tier(&self) -> AdapterTier {
        self.tier
    }

    pub fn dialect(&self) -> Dialect {
        self.client.dialect()
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Renders the transcript into a prompt.
    ///
    /// Observations are truncated because a check's output can run to
    /// thousands of lines and the window is the scarce resource on local
    /// weights (S5). The *tail* is kept: a type checker's summary and its
    /// first real error are what a reader acts on, and both sit at the end.
    fn prompt(&self, t: &Transcript) -> String {
        let mut p = String::new();
        p.push_str(
            "You are editing a Python repository. Reply with exactly one JSON tool call.\n\n\
             Tools: blast_radius{symbol}, read_file{path}, edit{path,old,new}, \
             run_checks{paths,kinds}, done{summary}.\n\n\
             `edit` requires `old` to appear exactly once in the file. Only `run_checks` \
             can establish that the code is correct; saying you are done does not.\n\n",
        );
        p.push_str("Task: ");
        p.push_str(&t.task);
        p.push_str("\n\n");

        for (i, turn) in t.turns.iter().enumerate() {
            p.push_str(&format!(
                "--- step {} ---\ncalled: {:?}\n",
                i + 1,
                turn.call
            ));
            let obs = &turn.observation;
            let shown: String = if obs.len() > 2000 {
                let tail: String = obs
                    .chars()
                    .skip(obs.chars().count().saturating_sub(2000))
                    .collect();
                format!("...(truncated)...\n{tail}")
            } else {
                obs.clone()
            };
            p.push_str(&shown);
            p.push('\n');
        }

        if t.final_notice {
            p.push_str(
                "\nNOTICE: the budget is exhausted. This is your last step. Leave the \
                 repository in a state you can describe, then emit done.\n",
            );
        }
        p.push_str("\nNext tool call (JSON only):\n");
        p
    }
}

/// Pulls the first JSON object out of a completion.
///
/// Tier 4 output is unconstrained, so a model will wrap its call in prose or a
/// code fence however firmly it is asked not to. Recovering the object is not
/// leniency about correctness — the parsed call still has to satisfy the same
/// schema — it is leniency about packaging, which is the only part tier 4
/// cannot guarantee.
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

impl Model for RuntimeModel {
    async fn next_step(&mut self, t: &Transcript) -> Result<Step, String> {
        let prompt = self.prompt(t);
        let schema = (self.tier == AdapterTier::RuntimeConstrained).then(ToolCallWire::schema);
        let mut last = String::new();

        for attempt in 1..=MAX_FORMAT_RETRIES {
            let completion = self
                .client
                .complete(&prompt, schema.as_ref(), self.max_tokens)
                .await
                // A transport failure is terminal: retrying a refused
                // connection just spends the budget discovering it again.
                .map_err(|e| format!("runtime call failed: {e}"))?;
            self.tokens += completion.tokens;

            let Some(json) = extract_json(&completion.text) else {
                last = format!("no JSON object in output: {}", truncate(&completion.text));
                tracing::warn!(attempt, "malformed tool call; retrying");
                continue;
            };
            match serde_json::from_str::<ToolCallWire>(json) {
                Ok(w) => match w.into_step() {
                    Ok(s) => return Ok(s),
                    Err(e) => {
                        // A well-formed call naming something the loop cannot
                        // do. Retrying is right: the model may pick a tool it
                        // can actually use.
                        last = e;
                    }
                },
                Err(e) => last = format!("{e}; text was {}", truncate(json)),
            }
            tracing::warn!(attempt, error = %last, "invalid tool call; retrying");
        }

        Err(format!(
            "no valid tool call after {MAX_FORMAT_RETRIES} attempts on tier {:?}. \
             Last problem: {last}",
            self.tier
        ))
    }

    fn tokens_used(&self) -> u64 {
        self.tokens
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_object_is_extracted() {
        assert_eq!(
            extract_json(r#"{"tool":"done"}"#),
            Some(r#"{"tool":"done"}"#)
        );
    }

    #[test]
    fn an_object_wrapped_in_prose_or_a_fence_is_recovered() {
        // Tier 4 has no grammar, so this is the normal case rather than the
        // exceptional one.
        let fenced =
            "Sure!\n```json\n{\"tool\":\"read_file\",\"path\":\"a.py\"}\n```\nHope that helps.";
        let got = extract_json(fenced).expect("should recover the object");
        let w: ToolCallWire = serde_json::from_str(got).unwrap();
        assert!(matches!(w, ToolCallWire::ReadFile { .. }));
    }

    #[test]
    fn nested_objects_and_braces_in_strings_do_not_confuse_it() {
        let text = r#"prefix {"tool":"edit","path":"a.py","old":"if x: {","new":"}"} suffix"#;
        let got = extract_json(text).expect("should span the whole object");
        let w: ToolCallWire = serde_json::from_str(got).unwrap();
        match w {
            ToolCallWire::Edit { old, new, .. } => {
                assert_eq!(old, "if x: {");
                assert_eq!(new, "}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unterminated_object_is_not_extracted() {
        assert_eq!(extract_json(r#"{"tool":"done""#), None);
        assert_eq!(extract_json("no braces here"), None);
    }

    #[test]
    fn tier_is_chosen_by_the_caller_not_guessed() {
        let c = RuntimeClient::new("http://127.0.0.1:8080", Dialect::LlamaCpp, None).unwrap();
        assert_eq!(
            RuntimeModel::new(c.clone(), true).tier(),
            AdapterTier::RuntimeConstrained
        );
        assert_eq!(
            RuntimeModel::new(c, false).tier(),
            AdapterTier::ValidateAndRetry
        );
    }

    #[test]
    fn the_prompt_carries_the_task_the_transcript_and_the_final_notice() {
        let c = RuntimeClient::new("http://127.0.0.1:8080", Dialect::LlamaCpp, None).unwrap();
        let m = RuntimeModel::new(c, true);
        let t = Transcript {
            task: "fix the type error".into(),
            turns: vec![crate::agent::Turn {
                call: crate::agent::ToolCall::ReadFile {
                    path: "a.py".into(),
                },
                observation: "file contents".into(),
                verdict_passed: None,
            }],
            final_notice: true,
        };
        let p = m.prompt(&t);
        assert!(p.contains("fix the type error"));
        assert!(p.contains("file contents"));
        assert!(
            p.contains("last step"),
            "the model must be told it is the last turn"
        );
        assert!(
            p.contains("saying you are done does not"),
            "the prompt must not imply the model can declare success"
        );
    }

    #[test]
    fn a_long_observation_is_truncated_from_the_front() {
        // The tail holds the checker's summary and its first real error, which
        // is what a reader acts on.
        let c = RuntimeClient::new("http://127.0.0.1:8080", Dialect::LlamaCpp, None).unwrap();
        let m = RuntimeModel::new(c, true);
        let mut obs = "X".repeat(3000);
        obs.push_str("THE-IMPORTANT-TAIL");
        let t = Transcript {
            task: "t".into(),
            turns: vec![crate::agent::Turn {
                call: crate::agent::ToolCall::ReadFile {
                    path: "a.py".into(),
                },
                observation: obs,
                verdict_passed: None,
            }],
            final_notice: false,
        };
        let p = m.prompt(&t);
        assert!(p.contains("THE-IMPORTANT-TAIL"));
        assert!(p.contains("truncated"));
        assert!(
            p.len() < 3500,
            "prompt should not carry the whole observation"
        );
    }
}
