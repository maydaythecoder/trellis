//! Tier 1: a hosted provider's own tool-calling API (ADR 0024), split out of
//! `adapter.rs` so the hosted path, the one that carries the user's key, reads
//! on its own. The observation shaping and compaction it shares with
//! [`super::RuntimeModel`] stay in the parent module.

use super::{
    CompactRequests, Compacted, Compaction, MAX_FORMAT_RETRIES, Reported, RequestedCompaction,
    observation_at, prior_block, repo_block, steer_block,
};
use crate::agent::{Model, Step, ToolCall, Transcript};
use trellis_model::{
    AdapterTier, Conversation, Exchange, HostedClient, NativeTool, Protocol, RuntimeError,
};

/// The per-reply ceiling for a hosted model. Larger than the local one
/// because a hosted reasoning model spends its thinking against it; it is a
/// cap, so a model that stops early pays nothing for it.
pub(crate) const HOSTED_MAX_REPLY_TOKENS: u32 = 16_000;

/// How many times a context-overflow refusal shrinks the budget and retries
/// (ADR 0024 §5). Each retry costs a request; two is enough to learn whether
/// eliding helps at all.
const MAX_OVERFLOW_RETRIES: u32 = 2;

/// Tier 1: a hosted provider's own tool-calling API (ADR 0024).
///
/// The transcript becomes a message list; the run's tool set becomes the
/// native tool list; the one call that comes back goes through
/// `ToolSet::parse_value`, the parser every other tier uses.
pub struct HostedModel {
    client: HostedClient,
    max_tokens: u32,
    /// Output tokens the provider reported.
    tokens: u64,
    /// Prompt tokens as reported, and the replies that reported none.
    prompt: Reported,
    /// Replies that carried no usage figure. Each is charged to the in-run
    /// ceiling at the `max_tokens` it was requested with, the most it can
    /// have cost; none of them reaches the trail as a number.
    unreported: u64,
    /// Refuse a reply with no usage figure: set when the run executes under
    /// an agent profile with a `[budget]` (ADR 0026 §5), whose ledger counts
    /// only measured spend.
    usage_required: bool,
    /// Bytes of the most recent request body, tools included.
    last_prompt_bytes: Option<usize>,
    compaction: Compaction,
    /// Per turn index, the provider's own call id and assistant message, so
    /// history goes back as the provider sent it (reasoning blocks and their
    /// signatures included) rather than rebuilt.
    issued: Vec<Option<IssuedCall>>,
    /// The call returned by the last `next_step`, waiting to be matched to
    /// the turn the loop appends for it.
    pending: Option<(usize, IssuedCall)>,
    /// Set, and said once, when thinking was withheld from a request.
    thinking_withheld: bool,
    /// `trellis run --image`, sent with the first user message of every
    /// request (ADR 0041).
    images: Vec<trellis_model::Image>,
    /// Output tokens and queue wait over the replies the last `next_step`
    /// read; `None` until it reads one, so a step with no reply reports
    /// nothing rather than a sum of 0.
    step: Option<(Reported, Reported)>,
    /// The words the reply behind the last `next_step`'s call carried
    /// beside it, cut to [`SAID_CAP`]; `None` when it carried none.
    said: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct IssuedCall {
    pub(super) call: ToolCall,
    pub(super) id: String,
    pub(super) replay: serde_json::Value,
}

impl HostedModel {
    pub fn new(client: HostedClient) -> Self {
        Self {
            client,
            max_tokens: HOSTED_MAX_REPLY_TOKENS,
            tokens: 0,
            prompt: Reported::default(),
            unreported: 0,
            usage_required: false,
            last_prompt_bytes: None,
            compaction: Compaction::default(),
            issued: Vec::new(),
            pending: None,
            thinking_withheld: false,
            images: Vec::new(),
            step: None,
            said: None,
        }
    }

    /// Images for the first user message (ADR 0041). Their base64 is part
    /// of every body, so the context budget counts it like any other byte.
    #[must_use]
    pub fn with_images(mut self, images: Vec<trellis_model::Image>) -> Self {
        self.images = images;
        self
    }

    /// Ends the run at the first reply with no usage figure, rather than
    /// letting a budgeted profile spend what its ledger cannot count.
    pub fn require_usage(&mut self, required: bool) {
        self.usage_required = required;
    }

    /// Output tokens as the provider reported them, for the trail. `None`
    /// once any reply carried no usage figure: the sum would then be a
    /// lower bound presented as a count, and [`Model::tokens_used`]'s upper
    /// bound is an estimate, not a measurement.
    pub fn tokens_measured(&self) -> Option<u64> {
        (self.unreported == 0).then_some(self.tokens)
    }

    /// `[model] context_bytes` (ADR 0024 §5).
    #[must_use]
    pub fn with_context_bytes(mut self, budget: Option<usize>) -> Self {
        self.compaction.budget = budget;
        self
    }

    /// Answers a client's `compact` at the next request (ADR 0025 §5).
    pub fn compact_on_request(&mut self, requests: std::sync::Arc<CompactRequests>) {
        self.compaction.requests = Some(requests);
    }

    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    pub fn client(&self) -> &HostedClient {
        &self.client
    }

    /// Whether a request of this run went without the configured thinking
    /// because a turn in it had no recorded reply (ADR 0024 §4).
    pub fn thinking_withheld(&self) -> bool {
        self.thinking_withheld
    }

    pub fn tier(&self) -> AdapterTier {
        AdapterTier::Native
    }

    /// Files the previous call under the turn the loop made of it. Only when
    /// the turn's call is the one returned: the loop may have appended a
    /// turn of its own (the governance audit), which the provider never saw.
    fn record_pending(&mut self, t: &Transcript) {
        if let Some((idx, iss)) = self.pending.take()
            && t.turns.get(idx).is_some_and(|x| x.call == iss.call)
        {
            if self.issued.len() <= idx {
                self.issued.resize(idx + 1, None);
            }
            self.issued[idx] = Some(iss);
        }
    }
}

/// The system message for a hosted model: the same repository fences as
/// the completion prompt, with the JSON-format instructions replaced by the
/// one rule a native API needs.
fn hosted_system(t: &Transcript) -> String {
    // The same three ADR 0025 hooks as the completion prompt: a hosted run
    // must not be the one path where `.trellis/system.md` and
    // `append_system.md` are silently ignored.
    let mut p = t.repo.extra.hosted_opening();
    p.push_str(
        "`kinds` are any of: typecheck, tests, import, governance. No other value is a \
         check.\n\n\
         `edit` requires `old` to appear exactly once in the file, copied from the file \
         rather than reconstructed. Only `run_checks` can establish that the code is \
         correct; saying you are done does not. Call `done` when you are finished.\n\n",
    );
    p.push_str(&t.repo.extra.after_protocol());
    repo_block(t, &mut p);
    p.trim_end().to_string()
}

/// The provider's own record of turn `i`, when there is one and it is for
/// this very call. The one decider of which turns are replayed: the
/// conversation is built from it, and so is [`every_turn_replayed`], which
/// the thinking warning reads without building a conversation.
fn issued_for<'a>(
    issued: &'a [Option<IssuedCall>],
    i: usize,
    turn: &crate::agent::Turn,
) -> Option<&'a IssuedCall> {
    issued
        .get(i)
        .and_then(Option::as_ref)
        .filter(|x| x.call == turn.call)
}

/// Whether every turn goes back as the provider's own reply: what
/// `trellis_model::every_turn_replayed` would say of [`conversation`]'s
/// result, decided by the same [`issued_for`], without building one.
pub(super) fn every_turn_replayed(t: &Transcript, issued: &[Option<IssuedCall>]) -> bool {
    t.turns
        .iter()
        .enumerate()
        .all(|(i, turn)| issued_for(issued, i, turn).is_some())
}

/// The transcript as a conversation, with the `elided` oldest observations
/// replaced by their one-line notes.
pub(super) fn conversation(
    t: &Transcript,
    issued: &[Option<IssuedCall>],
    protocol: Protocol,
    elided: usize,
    images: &[trellis_model::Image],
) -> Conversation {
    let mut user = format!("Task: {}\n\n", t.task);
    prior_block(t, &mut user);
    let exchanges = t
        .turns
        .iter()
        .enumerate()
        .map(|(i, turn)| {
            let mine = issued_for(issued, i, turn);
            let mut args = crate::registry::call_json(&turn.call);
            if let Some(m) = args.as_object_mut() {
                m.remove("tool");
            }
            Exchange {
                id: mine.map_or_else(
                    || match protocol {
                        Protocol::OpenAiChat => format!("call_trellis_{}", i + 1),
                        Protocol::Anthropic => format!("toolu_trellis_{}", i + 1),
                    },
                    |x| x.id.clone(),
                ),
                name: crate::gate::tool_name(&turn.call).to_string(),
                args,
                result: observation_at(turn, i, elided),
                replay: mine.map(|x| x.replay.clone()),
            }
        })
        .collect();
    Conversation {
        system: hosted_system(t),
        user: user.trim_end().to_string(),
        exchanges,
        trailing: hosted_trailing(t),
        images: images.to_vec(),
    }
}

/// User text after the last tool result: delivered steers, then the budget
/// notice. `trellis_model::Conversation` has one place for user text after
/// the history, and both dialects render it as user content, never as a tool
/// result, so a steer does not read as an observation. `None` when there is
/// neither, which is the conversation exactly as it was before #113.
fn hosted_trailing(t: &Transcript) -> Option<String> {
    let mut text = steer_block(t).trim().to_string();
    if t.final_notice {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(
            "NOTICE: the budget is exhausted. This is your last step. Leave the repository \
             in a state you can describe, then call done.",
        );
    }
    (!text.is_empty()).then_some(text)
}

/// The run's tool set as native tools: one per branch of the composed
/// schema, named by its `tool` constant, with `tool` itself removed from
/// the parameters. `read_skill` only when there is a skill, as in the
/// prompt's tool line.
pub(crate) fn native_tools(t: &Transcript) -> Vec<NativeTool> {
    let schema = t.repo.tools.schema();
    let branches = schema
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    branches
        .into_iter()
        .filter_map(|b| {
            let name = b.pointer("/properties/tool/const")?.as_str()?.to_string();
            if name == "read_skill" && t.repo.skills.is_empty() {
                return None;
            }
            let description = b
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&name)
                .to_string();
            let mut props = b
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();
            props.remove("tool");
            let required: Vec<serde_json::Value> = b
                .get("required")
                .and_then(serde_json::Value::as_array)
                .map(|r| r.iter().filter(|x| *x != "tool").cloned().collect())
                .unwrap_or_default();
            Some(NativeTool {
                name,
                description,
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": props,
                    "required": required,
                    "additionalProperties": false,
                }),
            })
        })
        .collect()
}

impl Model for HostedModel {
    async fn next_step(&mut self, t: &Transcript) -> Result<Step, String> {
        self.record_pending(t);
        self.step = None;
        self.said = None;
        let tools = native_tools(t);
        let protocol = self.client.protocol();
        let mut last = String::new();
        let mut overflows = 0;
        let mut attempt = 0;
        // The body the provider last refused as too long. Eliding can run
        // out of observations to elide; resending the same bytes then buys
        // the same refusal.
        let mut refused: Option<serde_json::Value> = None;
        let configured = self.client.thinking();
        // Decided by `issued_for`, which also decides each exchange's
        // `replay` in the conversation the body is built from, so the
        // warning and the request cannot disagree; without building that
        // conversation once more per call to find out. Elision changes only
        // observations, never which turns are replayed.
        let replayed = every_turn_replayed(t, &self.issued);
        if !self.thinking_withheld
            && configured != trellis_model::Thinking::Off
            && trellis_model::thinking_sent(protocol, configured, replayed)
                == trellis_model::Thinking::Off
        {
            self.thinking_withheld = true;
            tracing::warn!(
                "thinking: withheld for the rest of this run: a turn in the history has no \
                 recorded reply to send back with its thinking block, and {} refuses a \
                 tool call without one",
                protocol.describe()
            );
        }
        while attempt < MAX_FORMAT_RETRIES {
            let (client, issued, max, images) =
                (&self.client, &self.issued, self.max_tokens, &self.images);
            // Each observation is one JSON string in the body, so its cost
            // is its escaped length; `fit_by` checks that against a real
            // render and falls back to the scan if it is ever wrong.
            let body = self.compaction.fit_by(
                t,
                |k| client.body(&conversation(t, issued, protocol, k, images), &tools, max),
                |b| b.to_string().len(),
                |s| serde_json::to_string(s).map_or(s.len(), |j| j.len()),
            );
            if refused.as_ref() == Some(&body) {
                return Err(format!(
                    "hosted call failed: the prompt without any elidable observation still \
                     exceeds the provider's window ({} bytes; the newest observation, the \
                     system prompt and the tool list are never elided). Last refusal: {last}",
                    body.to_string().len()
                ));
            }
            let bytes = self
                .compaction
                .last_size
                .unwrap_or_else(|| body.to_string().len());
            self.last_prompt_bytes = Some(bytes);
            tracing::debug!(prompt_bytes = bytes, turns = t.turns.len());
            // The key is not in the body; it is added by the transport. An
            // image's bytes are, and a log is not where they go (ADR 0041).
            tracing::trace!(request = %trellis_model::redact_images(&body), "request as sent");
            let reply = match self.client.send(&body).await {
                Ok(r) => r,
                Err(RuntimeError::ContextOverflow(why)) if overflows < MAX_OVERFLOW_RETRIES => {
                    // The provider's refusal is the only measurement of its
                    // window trellis has in bytes (ADR 0024 §5).
                    overflows += 1;
                    last = why.clone();
                    refused = Some(body);
                    let shrunk = bytes * 3 / 4;
                    self.compaction.budget =
                        Some(self.compaction.budget.map_or(shrunk, |b| b.min(shrunk)));
                    tracing::warn!(bytes, budget = shrunk, error = %why, "context overflow; eliding and retrying");
                    continue;
                }
                Err(e) => return Err(format!("hosted call failed: {e}")),
            };
            attempt += 1;
            // Every reply, one retried as a format failure too. A missing
            // prompt figure does not end a budgeted run the way a missing
            // output figure does: the ledger counts output tokens only.
            self.prompt.add(reply.prompt_tokens);
            let (tokens, wait) = self.step.get_or_insert_default();
            tokens.add(reply.tokens);
            wait.add(reply.queue_wait_ms);
            match reply.tokens {
                Some(n) => self.tokens += n,
                None => {
                    self.unreported += 1;
                    if self.usage_required {
                        return Err(format!(
                            "the provider's reply carried no usage figure ({} has no \
                             output-token count in it), and this run executes under an \
                             agent profile with a [budget], whose ledger counts only \
                             measured spend. Use a provider that reports usage, or \
                             remove [budget] from the profile (ADR 0026 §5)",
                            self.client.protocol().describe()
                        ));
                    }
                }
            }
            let call = match reply.call {
                Ok(c) => c,
                Err(why) => {
                    last = why;
                    tracing::warn!(attempt, error = %last, "no usable tool call; retrying");
                    continue;
                }
            };
            let mut obj = match call.args {
                serde_json::Value::Object(m) => m,
                serde_json::Value::Null => serde_json::Map::new(),
                other => {
                    last = format!(
                        "the arguments of `{}` are not an object: {other}",
                        call.name
                    );
                    continue;
                }
            };
            obj.insert("tool".into(), serde_json::Value::String(call.name.clone()));
            match t.repo.tools.parse_value(serde_json::Value::Object(obj)) {
                Ok(step) => {
                    if let Step::Call(c) = &step {
                        // This reply's alone: a retried reply's words went
                        // with a call the loop never ran.
                        self.said = said_in(&reply.replay);
                        self.pending = Some((
                            t.turns.len(),
                            IssuedCall {
                                call: c.clone(),
                                id: call.id,
                                replay: reply.replay,
                            },
                        ));
                    }
                    return Ok(step);
                }
                Err(e) => {
                    last = format!("{e}; the call was `{}`", call.name);
                    tracing::warn!(attempt, error = %last, "invalid tool call; retrying");
                }
            }
        }
        Err(format!(
            "no valid tool call after {MAX_FORMAT_RETRIES} attempts on tier Native. \
             Last problem: {last}"
        ))
    }

    /// For the in-run ceiling: what was reported, plus each unreported
    /// reply at its request's `max_tokens`, so a silent provider trips the
    /// budget early rather than never.
    fn tokens_used(&self) -> Option<u64> {
        Some(
            self.tokens
                .saturating_add(self.unreported.saturating_mul(u64::from(self.max_tokens))),
        )
    }

    fn prompt_tokens(&self) -> Option<u64> {
        self.prompt.total()
    }

    fn prompt_bytes(&self) -> Option<usize> {
        self.last_prompt_bytes
    }

    /// The budget in force: as built, until a provider's context-overflow
    /// refusal lowers it, which the `compacted` event's `budget` reports.
    fn context_bytes(&self) -> Option<usize> {
        self.compaction.budget
    }

    fn compactions(&self) -> &[Compacted] {
        &self.compaction.events
    }

    fn requested_compactions(&self) -> &[RequestedCompaction] {
        &self.compaction.requested
    }

    fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }

    fn step_usage(&self) -> crate::agent::StepUsage {
        match self.step {
            Some((tokens, wait)) => crate::agent::StepUsage {
                completion_tokens: tokens.total(),
                queue_wait_ms: wait.total(),
            },
            None => crate::agent::StepUsage::default(),
        }
    }
}

/// The most of the model's words a turn event carries: the cap a command's
/// output is kept to, since the words travel on the same event stream.
pub(crate) const SAID_CAP: usize = crate::exec::OUTPUT_CAP;

/// The words an assistant message carried beside its call: OpenAI's string
/// `content`, or Anthropic's `text` blocks joined. Thinking blocks are the
/// model's reasoning, not what it said, and are left out. Trimmed, and
/// `None` when nothing is left, so a reply with no words is never sent as
/// an empty string a client would read as words.
///
/// Model output (CLAUDE.md rule 10): kept as text and never read as a
/// verdict or an instruction.
pub(super) fn said_in(replay: &serde_json::Value) -> Option<String> {
    let text = match &replay["content"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| cut_said(text))
}

/// `text` whole when it fits [`SAID_CAP`], else cut at the last whole
/// character within it with a last line saying how many bytes were cut: a
/// silently cut reply reads as all the model said.
pub(crate) fn cut_said(text: &str) -> String {
    if text.len() <= SAID_CAP {
        return text.to_string();
    }
    let mut end = SAID_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n...({} more bytes cut at {SAID_CAP} bytes)...",
        &text[..end],
        text.len() - end
    )
}

/// The first 200 characters of model output, for an error message: enough
/// to recognise the reply without putting all of it on one line.
pub(super) fn excerpt_for_error(s: &str) -> String {
    s.chars().take(200).collect()
}
