//! Tier 1: a hosted model's own tool-calling API (ADR 0024).
//!
//! Two wire protocols cover most providers: OpenAI-compatible chat
//! completions and Anthropic Messages. The mapping from a conversation to a
//! request, and from a reply to a tool call, is pure and tested on fixture
//! JSON; the client around it adds the key, the transport and the retry
//! policy.
//!
//! **Exactly one call per reply.** The loop is one call per turn (ADR 0005),
//! so both protocols are asked not to call tools in parallel, and a reply
//! carrying more than one is treated as malformed. Answering one of two
//! call ids would be a 400 on both APIs.

use crate::runtime::RuntimeError;
use crate::secret::ApiKey;
use crate::transport::{self, Auth, Endpoint, SendError};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// Which wire protocol a hosted endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `POST <base>/chat/completions` with `tools` and `tool_calls`.
    OpenAiChat,
    /// `POST <base>/v1/messages` with `tool_use` and `tool_result` blocks.
    Anthropic,
}

impl Protocol {
    /// The spec scheme, so a spec round-trips. `openai:` is taken by the vLLM
    /// completions alias, which is why this one is `openai-chat`.
    pub fn scheme(self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "openai-chat",
            Protocol::Anthropic => "anthropic",
        }
    }

    /// `[[providers]] protocol = ...`.
    pub fn parse_config(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai" | "openai-chat" => Some(Protocol::OpenAiChat),
            "anthropic" => Some(Protocol::Anthropic),
            _ => None,
        }
    }

    /// The variable a key is read from when a spec names no provider.
    pub fn default_key_env(self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "OPENAI_API_KEY",
            Protocol::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    /// Human name for `doctor` and the run banner.
    pub fn describe(self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "OpenAI-compatible chat completions",
            Protocol::Anthropic => "Anthropic Messages",
        }
    }
}

/// `[model] thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Thinking {
    #[default]
    Off,
    Low,
    Medium,
    High,
}

impl Thinking {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" => Some(Thinking::Off),
            "low" => Some(Thinking::Low),
            "medium" => Some(Thinking::Medium),
            "high" => Some(Thinking::High),
            _ => None,
        }
    }

    fn effort(self) -> Option<&'static str> {
        match self {
            Thinking::Off => None,
            Thinking::Low => Some("low"),
            Thinking::Medium => Some("medium"),
            Thinking::High => Some("high"),
        }
    }
}

/// One tool as a native tool definition.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeTool {
    pub name: String,
    pub description: String,
    /// A JSON Schema object for the arguments.
    pub parameters: Value,
}

/// One completed turn: the call the model made and what came back.
#[derive(Debug, Clone)]
pub struct Exchange {
    /// The call id, as the provider issued it or as synthesised for a turn
    /// the provider never saw.
    pub id: String,
    pub name: String,
    /// The arguments object, without `tool`.
    pub args: Value,
    /// The observation as the model is shown it.
    pub result: String,
    /// The provider's own assistant message for this call, replayed verbatim
    /// when present so reasoning blocks tied to it go back unchanged.
    pub replay: Option<Value>,
}

/// What one request carries.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub system: String,
    /// The first user message: the task and any previous-run fence.
    pub user: String,
    pub exchanges: Vec<Exchange>,
    /// Text after the last result, e.g. the budget notice.
    pub trailing: Option<String>,
}

/// A tool call read out of a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// A reply, reduced to what the adapter needs.
#[derive(Debug, Clone)]
pub struct Reply {
    /// `Ok` with the one call, or `Err` saying why there is not exactly one.
    pub call: Result<NativeCall, String>,
    /// Output tokens, where reported; `0` means not reported.
    pub tokens: u64,
    /// The assistant message to replay with this call's result.
    pub replay: Value,
}

/// Builds the request body. Pure: no key, no transport.
pub fn request_body(
    protocol: Protocol,
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
) -> Value {
    match protocol {
        Protocol::OpenAiChat => openai_body(model, conv, tools, max_tokens, thinking),
        Protocol::Anthropic => anthropic_body(model, conv, tools, max_tokens, thinking),
    }
}

fn openai_body(
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
) -> Value {
    let mut messages = vec![
        json!({ "role": "system", "content": conv.system }),
        json!({ "role": "user", "content": conv.user }),
    ];
    for x in &conv.exchanges {
        messages.push(x.replay.clone().unwrap_or_else(|| {
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": x.id,
                    "type": "function",
                    "function": { "name": x.name, "arguments": x.args.to_string() }
                }]
            })
        }));
        messages.push(json!({ "role": "tool", "tool_call_id": x.id, "content": x.result }));
    }
    if let Some(t) = &conv.trailing {
        messages.push(json!({ "role": "user", "content": t }));
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    // No `temperature`: reasoning models reject a non-default one, and
    // `max_completion_tokens` because they reject `max_tokens`.
    let mut b = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "required",
        "parallel_tool_calls": false,
        "max_completion_tokens": max_tokens,
    });
    if let Some(e) = thinking.effort() {
        b["reasoning_effort"] = json!(e);
    }
    b
}

fn anthropic_body(
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
) -> Value {
    let mut messages = vec![json!({ "role": "user", "content": [
        { "type": "text", "text": conv.user }
    ]})];
    for x in &conv.exchanges {
        messages.push(x.replay.clone().unwrap_or_else(|| {
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": x.id, "name": x.name, "input": x.args }
            ]})
        }));
        messages.push(json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": x.id, "content": x.result }
        ]}));
    }
    if let Some(t) = &conv.trailing {
        // Into the last user message: roles must alternate.
        if let Some(Value::Array(content)) = messages.last_mut().and_then(|m| m.get_mut("content"))
        {
            content.push(json!({ "type": "text", "text": t }));
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();
    // `auto`, not `any`: current models reject a forced tool choice. A reply
    // with no call is malformed and retried like any other.
    let mut b = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": conv.system,
        "messages": messages,
        "tools": tools,
        "tool_choice": { "type": "auto", "disable_parallel_tool_use": true },
    });
    if let Some(e) = thinking.effort() {
        b["thinking"] = json!({ "type": "adaptive" });
        b["output_config"] = json!({ "effort": e });
    }
    b
}

/// Reads a reply. A reply that is not the expected shape is an error; a
/// reply of the right shape with no single call is a `Reply` whose `call`
/// says why, so the adapter can retry it as a format failure.
pub fn parse_reply(protocol: Protocol, v: &Value) -> Result<Reply, RuntimeError> {
    let shape = |what: &str| RuntimeError::Shape(format!("{what}; got {}", cut(&v.to_string())));
    match protocol {
        Protocol::OpenAiChat => {
            let msg = v
                .pointer("/choices/0/message")
                .ok_or_else(|| shape("expected `choices[0].message`"))?;
            let tokens = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
            let call = match calls.as_slice() {
                [one] => {
                    let name = one
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let raw = one
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    match serde_json::from_str::<Value>(raw) {
                        Ok(args) => Ok(NativeCall {
                            id: one["id"].as_str().unwrap_or("call_0").to_string(),
                            name,
                            args,
                        }),
                        Err(e) => Err(format!(
                            "the arguments of `{name}` are not JSON ({e}): {}",
                            cut(raw)
                        )),
                    }
                }
                [] => Err(no_call(msg["content"].as_str().unwrap_or(""))),
                many => Err(format!(
                    "{} tool calls in one reply; the loop takes exactly one per turn",
                    many.len()
                )),
            };
            // Only the parts a later request may carry: the provider may add
            // fields (refusal, annotations) a request would reject.
            let mut replay = Map::new();
            replay.insert("role".into(), json!("assistant"));
            replay.insert("content".into(), msg["content"].clone());
            if let Some(tc) = msg.get("tool_calls") {
                replay.insert("tool_calls".into(), tc.clone());
            }
            Ok(Reply {
                call,
                tokens,
                replay: Value::Object(replay),
            })
        }
        Protocol::Anthropic => {
            let content = v["content"]
                .as_array()
                .ok_or_else(|| shape("expected a `content` array"))?;
            let tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
            let uses: Vec<&Value> = content.iter().filter(|b| b["type"] == "tool_use").collect();
            let call = match uses.as_slice() {
                [one] => Ok(NativeCall {
                    id: one["id"].as_str().unwrap_or("toolu_0").to_string(),
                    name: one["name"].as_str().unwrap_or("").to_string(),
                    args: one["input"].clone(),
                }),
                [] => {
                    let text: Vec<&str> = content
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect();
                    let why = no_call(&text.join(" "));
                    Err(match v["stop_reason"].as_str() {
                        Some("max_tokens") => {
                            format!("the reply stopped at max_tokens before a tool call. {why}")
                        }
                        Some("refusal") => format!("the provider refused the request. {why}"),
                        _ => why,
                    })
                }
                many => Err(format!(
                    "{} tool calls in one reply; the loop takes exactly one per turn",
                    many.len()
                )),
            };
            // The whole content, thinking blocks and their signatures
            // included: a later request must carry them back unchanged.
            Ok(Reply {
                call,
                tokens,
                replay: json!({ "role": "assistant", "content": content }),
            })
        }
    }
}

fn no_call(text: &str) -> String {
    format!(
        "the reply called no tool; its text was: {}",
        cut(text.trim())
    )
}

fn cut(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Whether an error body is the provider saying the request exceeded the
/// context window. Matched on the documented markers only: OpenAI's
/// `context_length_exceeded` code, Anthropic's `prompt is too long`.
pub fn is_context_overflow(status: u16, body: &str) -> bool {
    status == 413
        || (status == 400
            && (body.contains("context_length_exceeded")
                || body.contains("prompt is too long")
                || body.contains("maximum context length")))
}

/// Statuses worth retrying: rate limits and the provider's own failures.
/// 529 is Anthropic's "overloaded".
fn retryable(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504 | 529)
}

/// How many attempts one model call gets, the first included.
pub const MAX_ATTEMPTS: u32 = 4;

/// The longest wait honoured from `Retry-After`. A provider asking for more
/// is asking a run to sit idle for longer than a turn is worth.
pub const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

/// The wait before attempt `attempt + 1`, `attempt` counting from 1.
///
/// `Retry-After` in seconds when present, capped; otherwise 1, 2, 4 s. An
/// HTTP-date `Retry-After` is not parsed and falls back to the fixed
/// backoff (ADR 0024, "Not built").
pub fn retry_delay(retry_after: Option<&str>, attempt: u32) -> Duration {
    retry_after
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| Duration::from_secs_f64(s).min(MAX_RETRY_WAIT))
        .unwrap_or_else(|| Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5)))
}

/// A hosted endpoint, with its key.
#[derive(Clone)]
pub struct HostedClient {
    protocol: Protocol,
    endpoint: Endpoint,
    base: String,
    model: String,
    key: Option<ApiKey>,
    thinking: Thinking,
    timeout: Duration,
}

impl std::fmt::Debug for HostedClient {
    /// By hand, so nothing about the key but its source can be printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedClient")
            .field("protocol", &self.protocol)
            .field("base", &self.base)
            .field("model", &self.model)
            .field("key", &self.key.as_ref().map(ApiKey::source))
            .finish()
    }
}

impl HostedClient {
    /// Reads the key from `key_env` and builds the client.
    ///
    /// Fails before any request, and so before a run's snapshot, when an
    /// https endpoint has no key or the build has no TLS.
    pub fn new(
        protocol: Protocol,
        base: &str,
        model: &str,
        key_env: &str,
    ) -> Result<Self, RuntimeError> {
        Self::with_key(protocol, base, model, key_env, ApiKey::from_env(key_env))
    }

    /// As [`new`](Self::new) with the key supplied, so tests need not touch
    /// the process environment.
    pub fn with_key(
        protocol: Protocol,
        base: &str,
        model: &str,
        key_env: &str,
        key: Option<ApiKey>,
    ) -> Result<Self, RuntimeError> {
        let endpoint = Endpoint::parse(base)?;
        if endpoint.tls && !cfg!(feature = "hosted") {
            return Err(RuntimeError::Config(transport::NO_TLS.to_string()));
        }
        if endpoint.tls && key.is_none() {
            return Err(RuntimeError::Config(format!(
                "{} at {base} needs a key, and {key_env} is not set. Keys are read from \
                 the environment only, never from a file (ADR 0010, ADR 0024): \
                 set {key_env} and run again.",
                protocol.describe()
            )));
        }
        Ok(Self {
            protocol,
            endpoint,
            base: base.to_string(),
            model: model.to_string(),
            key,
            thinking: Thinking::Off,
            timeout: Duration::from_secs(300),
        })
    }

    #[must_use]
    pub fn with_thinking(mut self, t: Thinking) -> Self {
        self.thinking = t;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn thinking(&self) -> Thinking {
        self.thinking
    }

    /// Where the key came from, for `doctor` and the banner.
    pub fn key_source(&self) -> Option<&str> {
        self.key.as_ref().map(ApiKey::source)
    }

    /// The request body for this client's protocol, model and thinking.
    pub fn body(&self, conv: &Conversation, tools: &[NativeTool], max_tokens: u32) -> Value {
        request_body(
            self.protocol,
            &self.model,
            conv,
            tools,
            max_tokens,
            self.thinking,
        )
    }

    /// Sends a body and reads the reply, retrying 429 and 5xx with backoff.
    ///
    /// `skip_all`: the body is the whole transcript, and nothing about the
    /// key should be near a span even by accident.
    #[tracing::instrument(skip_all, fields(protocol = ?self.protocol))]
    pub async fn send(&self, body: &Value) -> Result<Reply, RuntimeError> {
        let text = body.to_string();
        let (path, auth, headers): (&str, Auth, Vec<(&str, &str)>) = match self.protocol {
            Protocol::OpenAiChat => ("/chat/completions", Auth::Bearer, Vec::new()),
            Protocol::Anthropic => (
                "/v1/messages",
                Auth::XApiKey,
                vec![("anthropic-version", "2023-06-01")],
            ),
        };
        let mut attempt = 1;
        loop {
            let sent = transport::post(
                &self.endpoint,
                path,
                &headers,
                self.key.as_ref().map(|k| (auth, k)),
                &text,
                self.timeout,
            )
            .await;
            let wait = match sent {
                Ok(r) if (200..300).contains(&r.status) => {
                    let v: Value = serde_json::from_str(&r.body).map_err(|e| {
                        RuntimeError::Shape(format!(
                            "response is not JSON: {e}; body: {}",
                            cut(&self.scrub(&r.body))
                        ))
                    })?;
                    return parse_reply(self.protocol, &v);
                }
                Ok(r) if matches!(r.status, 401 | 403) => {
                    // Not the body: OpenAI's echoes a masked form of the key.
                    return Err(RuntimeError::Auth {
                        status: r.status,
                        source_var: self
                            .key
                            .as_ref()
                            .map_or_else(|| "no key".to_string(), |k| k.source().to_string()),
                    });
                }
                Ok(r) if is_context_overflow(r.status, &r.body) => {
                    return Err(RuntimeError::ContextOverflow(cut(&self.scrub(&r.body))));
                }
                Ok(r) if retryable(r.status) && attempt < MAX_ATTEMPTS => {
                    let d = retry_delay(r.header("retry-after"), attempt);
                    tracing::warn!(
                        status = r.status,
                        attempt,
                        wait_ms = d.as_millis() as u64,
                        "provider asked for a retry"
                    );
                    d
                }
                Ok(r) => {
                    return Err(RuntimeError::Http {
                        status: r.status,
                        body: self.scrub(&r.body).chars().take(400).collect(),
                    });
                }
                Err(SendError::Broken(e)) if attempt < MAX_ATTEMPTS => {
                    let d = retry_delay(None, attempt);
                    tracing::warn!(attempt, error = %e, "request failed mid-flight; retrying");
                    d
                }
                Err(SendError::Broken(e)) | Err(SendError::Connect(e)) => {
                    return Err(RuntimeError::Connect {
                        base: self.base.clone(),
                        source: std::io::Error::other(e),
                    });
                }
                Err(SendError::Timeout) => return Err(RuntimeError::Timeout(self.timeout)),
                Err(SendError::Config(e)) => return Err(RuntimeError::Config(e)),
            };
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    fn scrub(&self, s: &str) -> String {
        match &self.key {
            Some(k) => k.scrub(s),
            None => s.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn conv() -> Conversation {
        Conversation {
            system: "SYSTEM".into(),
            user: "Task: fix it".into(),
            exchanges: vec![Exchange {
                id: "c1".into(),
                name: "read_file".into(),
                args: json!({"path": "a.py"}),
                result: "file text".into(),
                replay: None,
            }],
            trailing: Some("last step".into()),
        }
    }

    fn tools() -> Vec<NativeTool> {
        vec![NativeTool {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        }]
    }

    #[test]
    fn openai_carries_tools_one_call_and_the_history_as_tool_messages() {
        let b = request_body(
            Protocol::OpenAiChat,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::Off,
        );
        assert_eq!(b["tool_choice"], "required");
        assert_eq!(b["parallel_tool_calls"], false);
        assert_eq!(b["tools"][0]["function"]["name"], "read_file");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["role"], "user");
        assert_eq!(m[2]["tool_calls"][0]["id"], "c1");
        // A JSON string, not an object: the API's own shape.
        assert_eq!(
            m[2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a.py\"}"
        );
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "c1");
        assert_eq!(m[3]["content"], "file text");
        assert_eq!(m[4]["content"], "last step");
        assert!(b.get("temperature").is_none(), "reasoning models reject it");
        assert!(b.get("reasoning_effort").is_none(), "off sends nothing");
    }

    #[test]
    fn anthropic_carries_system_top_level_and_results_as_blocks() {
        let b = request_body(
            Protocol::Anthropic,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::Off,
        );
        assert_eq!(b["system"], "SYSTEM");
        assert_eq!(b["max_tokens"], 100);
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert_eq!(b["tool_choice"]["disable_parallel_tool_use"], true);
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3, "user, assistant, user: roles alternate");
        assert_eq!(m[1]["content"][0]["type"], "tool_use");
        assert_eq!(m[1]["content"][0]["input"]["path"], "a.py");
        assert_eq!(m[2]["content"][0]["type"], "tool_result");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(m[2]["content"][1]["text"], "last step");
        assert!(b.get("thinking").is_none());
    }

    #[test]
    fn thinking_maps_to_each_protocol_s_own_field() {
        let b = request_body(
            Protocol::Anthropic,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::High,
        );
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["output_config"]["effort"], "high");
        let b = request_body(
            Protocol::OpenAiChat,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::Low,
        );
        assert_eq!(b["reasoning_effort"], "low");
    }

    #[test]
    fn a_replayed_assistant_message_goes_back_unchanged() {
        // Anthropic thinking blocks carry signatures and must be returned as
        // they came; rebuilding the message would drop them.
        let mut c = conv();
        let replay = json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig"},
            {"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a.py"}}]});
        c.exchanges[0].replay = Some(replay.clone());
        let b = request_body(Protocol::Anthropic, "m", &c, &tools(), 100, Thinking::High);
        assert_eq!(b["messages"][1], replay);
    }

    #[test]
    fn a_reply_is_read_into_one_call_or_a_reason_there_is_not_one() {
        let v = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"done","arguments":"{\"summary\":\"ok\"}"}}]}}],
            "usage":{"completion_tokens":9}});
        let r = parse_reply(Protocol::OpenAiChat, &v).unwrap();
        assert_eq!(r.tokens, 9);
        assert_eq!(
            r.call.unwrap(),
            NativeCall {
                id: "x".into(),
                name: "done".into(),
                args: json!({"summary":"ok"})
            }
        );

        let v = json!({"content":[{"type":"text","text":"I think"},
            {"type":"tool_use","id":"t","name":"read_file","input":{"path":"a.py"}}],
            "usage":{"output_tokens":5}});
        let r = parse_reply(Protocol::Anthropic, &v).unwrap();
        assert_eq!(r.call.unwrap().name, "read_file");
        assert_eq!(r.replay["content"][0]["text"], "I think");

        // Prose instead of a call is a format failure, retried by the adapter.
        let v = json!({"content":[{"type":"text","text":"Sure!"}],"stop_reason":"end_turn"});
        let e = parse_reply(Protocol::Anthropic, &v)
            .unwrap()
            .call
            .unwrap_err();
        assert!(e.contains("called no tool") && e.contains("Sure!"), "{e}");
    }

    #[test]
    fn two_calls_in_one_reply_are_malformed_not_half_answered() {
        // Answering one of two ids is a 400 on both APIs, and running both
        // breaks the one-call-per-turn loop (ADR 0005).
        let v = json!({"content":[
            {"type":"tool_use","id":"a","name":"read_file","input":{}},
            {"type":"tool_use","id":"b","name":"read_file","input":{}}]});
        let e = parse_reply(Protocol::Anthropic, &v)
            .unwrap()
            .call
            .unwrap_err();
        assert!(e.contains("2 tool calls"), "{e}");
    }

    #[test]
    fn an_unexpected_shape_is_an_error() {
        assert!(parse_reply(Protocol::OpenAiChat, &json!({"x":1})).is_err());
        assert!(parse_reply(Protocol::Anthropic, &json!({"x":1})).is_err());
    }

    #[test]
    fn retry_after_is_honoured_and_capped() {
        assert_eq!(retry_delay(Some("3"), 1), Duration::from_secs(3));
        assert_eq!(retry_delay(Some("0.5"), 1), Duration::from_millis(500));
        assert_eq!(retry_delay(Some("86400"), 1), MAX_RETRY_WAIT);
        // Missing or an HTTP date: the fixed backoff.
        assert_eq!(retry_delay(None, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(None, 3), Duration::from_secs(4));
        assert_eq!(
            retry_delay(Some("Wed, 21 Oct 2026 07:28:00 GMT"), 2),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn overflow_is_recognised_by_the_documented_markers_only() {
        assert!(is_context_overflow(
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#
        ));
        assert!(is_context_overflow(
            400,
            r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
        ));
        assert!(is_context_overflow(413, ""));
        assert!(!is_context_overflow(
            400,
            r#"{"error":"roles must alternate"}"#
        ));
    }

    #[test]
    fn an_https_endpoint_without_a_key_is_refused_naming_the_variable() {
        let e = HostedClient::with_key(
            Protocol::Anthropic,
            "https://api.anthropic.com",
            "m",
            "ANTHROPIC_API_KEY",
            None,
        )
        .unwrap_err()
        .to_string();
        // Without the feature the refusal is about TLS instead; both happen
        // before any request.
        if cfg!(feature = "hosted") {
            assert!(e.contains("ANTHROPIC_API_KEY"), "{e}");
        } else {
            assert!(e.contains("hosted"), "{e}");
        }
    }

    #[test]
    fn debug_of_a_client_shows_where_the_key_came_from_not_the_key() {
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "m",
            "OPENAI_API_KEY",
            Some(ApiKey::for_test("sk-sekrit-123", "OPENAI_API_KEY")),
        )
        .unwrap();
        let d = format!("{c:?}");
        assert!(d.contains("OPENAI_API_KEY") && !d.contains("sekrit"), "{d}");
    }

    /// A loopback stub answering each request in turn with `(status, extra
    /// headers, body)`, returning what it received.
    async fn stub(
        replies: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let mut got = Vec::new();
            for (status, extra, body) in replies {
                let Ok(Ok((mut s, _))) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
                else {
                    break;
                };
                let mut buf = vec![0u8; 65536];
                let n = s.read(&mut buf).await.unwrap();
                got.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let out = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(out.as_bytes()).await.unwrap();
                s.shutdown().await.ok();
            }
            got
        });
        (format!("http://{addr}"), h)
    }

    #[tokio::test]
    async fn a_429_is_retried_after_the_wait_it_asked_for() {
        let ok =
            r#"{"content":[{"type":"tool_use","id":"t","name":"done","input":{"summary":"x"}}]}"#;
        let (base, h) = stub(vec![
            ("429 Too Many Requests", "Retry-After: 0\r\n", "{}"),
            ("529 Overloaded", "", "{}"),
            ("200 OK", "", ok),
        ])
        .await;
        let c = HostedClient::with_key(
            Protocol::Anthropic,
            &base,
            "m",
            "K",
            Some(ApiKey::for_test("sk-test-key-1", "K")),
        )
        .unwrap();
        let r = c
            .send(&json!({}))
            .await
            .expect("the third attempt succeeds");
        assert_eq!(r.call.unwrap().name, "done");
        let got = h.await.unwrap();
        assert_eq!(got.len(), 3);
        assert!(
            got[0].starts_with("POST /v1/messages HTTP/1.1"),
            "{}",
            got[0]
        );
        assert!(got[0].contains("anthropic-version: 2023-06-01"));
        assert!(got[0].contains("x-api-key: sk-test-key-1"));
    }

    #[tokio::test]
    async fn retries_stop_at_the_cap_and_report_the_status() {
        let (base, h) = stub(vec![
            ("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            ("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            ("503 Service Unavailable", "Retry-After: 0\r\n", "{}"),
            ("503 Service Unavailable", "Retry-After: 0\r\n", "down"),
        ])
        .await;
        let c = HostedClient::with_key(Protocol::OpenAiChat, &base, "m", "K", None).unwrap();
        let e = c.send(&json!({})).await.unwrap_err();
        assert!(matches!(e, RuntimeError::Http { status: 503, .. }), "{e}");
        assert_eq!(h.await.unwrap().len(), MAX_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn a_refused_key_names_its_variable_and_not_the_provider_s_echo_of_it() {
        // OpenAI's 401 body echoes a masked form of the key it was sent.
        let (base, h) = stub(vec![(
            "401 Unauthorized",
            "",
            r#"{"error":{"message":"Incorrect API key provided: sk-tes*****ey-1"}}"#,
        )])
        .await;
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            &base,
            "m",
            "OPENAI_API_KEY",
            Some(ApiKey::for_test("sk-test-key-1", "OPENAI_API_KEY")),
        )
        .unwrap();
        let e = c.send(&json!({})).await.unwrap_err().to_string();
        assert!(e.contains("OPENAI_API_KEY"), "{e}");
        assert!(!e.contains("sk-tes"), "{e}");
        let got = h.await.unwrap();
        assert!(got[0].contains("Authorization: Bearer sk-test-key-1"));
        assert!(got[0].starts_with("POST /chat/completions"), "{}", got[0]);
    }

    #[tokio::test]
    async fn an_overflow_is_its_own_error_so_the_adapter_can_compact() {
        let (base, _h) = stub(vec![(
            "400 Bad Request",
            "",
            r#"{"error":{"code":"context_length_exceeded"}}"#,
        )])
        .await;
        let c = HostedClient::with_key(Protocol::OpenAiChat, &base, "m", "K", None).unwrap();
        let e = c.send(&json!({})).await.unwrap_err();
        assert!(matches!(e, RuntimeError::ContextOverflow(_)), "{e}");
    }
}
