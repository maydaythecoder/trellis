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

use crate::image::Image;
use crate::retry::{MAX_ATTEMPTS, is_context_overflow, retry_delay, retryable, status_retry_delay};
use crate::runtime::RuntimeError;
use crate::secret::ApiKey;
use crate::transport::{self, Auth, Endpoint, Proxy, SendError};
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
    /// `trellis run --image`, carried in the first user message on every
    /// request (ADR 0041). Empty for every other run, whose bodies are then
    /// exactly what they were before images existed.
    pub images: Vec<Image>,
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
    /// Output tokens as the provider reported them; `None` when the reply
    /// carried no usage figure. Not `0`: an unreported reply is not free,
    /// and reading it as free let budgets never trip.
    pub tokens: Option<u64>,
    /// Tokens the prompt occupied, as the provider reported them; `None`
    /// when the reply did not say. Cached tokens are included on every
    /// protocol, so the figure means the same thing whichever one answered
    /// (ADR 0027, #106). Not `0` when unreported, for the reason `tokens`
    /// is not.
    pub prompt_tokens: Option<u64>,
    /// The assistant message to replay with this call's result.
    pub replay: Value,
    /// How long colibri held the request in its admission queue, from its
    /// `x-colibri-queue-wait-ms` header (ADR 0047 decision 8); `None` when
    /// the header is absent or its value is not a non-negative integer.
    /// Set by `HostedClient::send`, which has the headers; `parse_reply`
    /// sees only the body and leaves it `None`.
    pub queue_wait_ms: Option<u64>,
}

/// The thinking a request is sent with, given whether every assistant turn
/// in it is the provider's own recorded reply (ADR 0024 §4).
///
/// Anthropic refuses, with a 400, a thinking-enabled request whose history
/// has an assistant `tool_use` that does not start with a thinking block,
/// and a turn with no recorded reply can only be rebuilt as a bare
/// `tool_use`. So on Anthropic such a request goes without thinking. The
/// reply to it then has no thinking block either, and the unreplayed turn
/// is still in the history, so once withheld it stays withheld for that
/// transcript: turning it back on would draw the same 400. OpenAI's
/// `reasoning_effort` has no such rule and is left as configured.
pub fn thinking_sent(
    protocol: Protocol,
    thinking: Thinking,
    every_turn_replayed: bool,
) -> Thinking {
    match protocol {
        Protocol::Anthropic if !every_turn_replayed => Thinking::Off,
        _ => thinking,
    }
}

/// Whether every exchange in `conv` goes back as the provider's own reply:
/// the one fact [`thinking_sent`] needs, decided here and nowhere else, so
/// a caller that reports withheld thinking agrees with the body sent.
pub fn every_turn_replayed(conv: &Conversation) -> bool {
    conv.exchanges.iter().all(|x| x.replay.is_some())
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
    body_for(protocol, model, conv, tools, max_tokens, thinking, false)
}

/// [`request_body`], with colibri's request profile when `colibri` (ADR 0047
/// decision 5). Without it the body is exactly `request_body`'s.
fn body_for(
    protocol: Protocol,
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
    colibri: bool,
) -> Value {
    let thinking = thinking_sent(protocol, thinking, every_turn_replayed(conv));
    match protocol {
        Protocol::OpenAiChat => openai_body(model, conv, tools, max_tokens, thinking, colibri),
        Protocol::Anthropic => anthropic_body(model, conv, tools, max_tokens, thinking, colibri),
    }
}

fn openai_body(
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
    colibri: bool,
) -> Value {
    // A plain string when there is no image, so every other body is byte
    // for byte what it was; a parts array, the text first, when there is.
    let user = if conv.images.is_empty() {
        json!(conv.user)
    } else {
        let mut parts = vec![json!({ "type": "text", "text": conv.user })];
        parts.extend(conv.images.iter().map(|i| {
            json!({ "type": "image_url", "image_url": {
                "url": format!("data:{};base64,{}", i.media_type(), i.base64())
            }})
        }));
        Value::Array(parts)
    };
    let mut messages = vec![
        json!({ "role": "system", "content": conv.system }),
        json!({ "role": "user", "content": user }),
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
    // No `temperature` (colibri excepted, below): reasoning models reject a
    // non-default one, and `max_completion_tokens` because they reject
    // `max_tokens`.
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
    // colibri is no reasoning API: with no `temperature` it samples at 0.7,
    // and with no thinking field its Qwen3.8 thinks at `xhigh` (ADR 0047
    // decision 5). `enable_thinking: false` asks for the least thinking the
    // family's template allows.
    if colibri {
        b["temperature"] = json!(0);
        if thinking.effort().is_none() {
            b["enable_thinking"] = json!(false);
        }
    }
    b
}

fn anthropic_body(
    model: &str,
    conv: &Conversation,
    tools: &[NativeTool],
    max_tokens: u32,
    thinking: Thinking,
    colibri: bool,
) -> Value {
    // Images before the text, the order Anthropic's documentation asks for.
    let mut first: Vec<Value> = conv
        .images
        .iter()
        .map(|i| {
            json!({ "type": "image", "source": {
                "type": "base64", "media_type": i.media_type(), "data": i.base64()
            }})
        })
        .collect();
    first.push(json!({ "type": "text", "text": conv.user }));
    let mut messages = vec![json!({ "role": "user", "content": first })];
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
    if colibri {
        // colibri thinks on this path only for `enabled`, reading `adaptive`
        // as off, and takes no effort (ADR 0047 decision 5). Withheld
        // thinking arrives here as `Off` and is stated as `disabled`.
        b["temperature"] = json!(0);
        let kind = if thinking.effort().is_some() {
            "enabled"
        } else {
            "disabled"
        };
        b["thinking"] = json!({ "type": kind });
    } else if let Some(e) = thinking.effort() {
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
            let tokens = v["usage"]["completion_tokens"].as_u64();
            // Already counts cached prompt tokens; Anthropic's figure below is
            // summed to match it.
            let prompt_tokens = v["usage"]["prompt_tokens"].as_u64();
            let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
            // A reply cut at the room the prompt leaves loses its call, or
            // ends inside it; naming the limit points the fix at the context
            // window, not at formatting, as `max_tokens` does below (#254).
            let length_limited = v.pointer("/choices/0/finish_reason") == Some(&json!("length"));
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
                        Err(e) if length_limited => Err(format!(
                            "the reply stopped at its length limit inside a tool call to \
                             `{name}`; its arguments are not JSON ({e}): {}",
                            cut(raw)
                        )),
                        Err(e) => Err(format!(
                            "the arguments of `{name}` are not JSON ({e}): {}",
                            cut(raw)
                        )),
                    }
                }
                [] => {
                    let why = no_tool_call_error(msg["content"].as_str().unwrap_or(""));
                    Err(if length_limited {
                        format!("the reply stopped at its length limit before a tool call. {why}")
                    } else {
                        why
                    })
                }
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
                prompt_tokens,
                queue_wait_ms: None,
                replay: Value::Object(replay),
            })
        }
        Protocol::Anthropic => {
            let content = v["content"]
                .as_array()
                .ok_or_else(|| shape("expected a `content` array"))?;
            let tokens = v["usage"]["output_tokens"].as_u64();
            let prompt_tokens = anthropic_prompt_tokens(&v["usage"]);
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
                    let why = no_tool_call_error(&text.join(" "));
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
                prompt_tokens,
                queue_wait_ms: None,
                replay: json!({ "role": "assistant", "content": content }),
            })
        }
    }
}

/// Anthropic's prompt figure: `input_tokens` plus the cache-creation and
/// cache-read tokens it leaves out, so the sum is what OpenAI's
/// `prompt_tokens` reports (ADR 0027, #106).
///
/// Gated on `input_tokens`: a cache field counts as 0 only beside it, since
/// without it the sum would be a part of the prompt presented as all of it,
/// and a low number reads as a cheap run.
fn anthropic_prompt_tokens(usage: &Value) -> Option<u64> {
    let input = usage["input_tokens"].as_u64()?;
    let cached = |k: &str| usage[k].as_u64().unwrap_or(0);
    Some(
        input
            .saturating_add(cached("cache_creation_input_tokens"))
            .saturating_add(cached("cache_read_input_tokens")),
    )
}

fn no_tool_call_error(text: &str) -> String {
    format!(
        "the reply called no tool; its text was: {}",
        cut(text.trim())
    )
}

/// A malformed reply as a shape error quotes it.
fn cut(s: &str) -> String {
    transport::excerpt(s, transport::SHAPE_QUOTE_CHARS)
}

/// colibri's `x-colibri-queue-wait-ms` value as a count of milliseconds.
///
/// The value is server input: only ASCII digits, after trimming, that fit a
/// `u64` are read. A sign, a fraction, a unit or an overflow is `None`, not
/// an error, so a malformed figure never fails a reply that is otherwise
/// good. (`u64::from_str` alone would accept a leading `+`.)
fn queue_wait_ms(value: Option<&str>) -> Option<u64> {
    let v = value?.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}

/// A hosted endpoint, with its key.
#[derive(Clone)]
pub struct HostedClient {
    protocol: Protocol,
    endpoint: Endpoint,
    /// Resolved once, at construction, so a proxy URL that is refused
    /// fails before a run's snapshot rather than on its first turn.
    proxy: Option<Proxy>,
    base: String,
    model: String,
    key: Option<ApiKey>,
    thinking: Thinking,
    timeout: Duration,
    /// ADR 0047 decision 5: the endpoint was identified as colibri, so its
    /// bodies carry `temperature: 0` and state thinking explicitly.
    colibri: bool,
}

impl std::fmt::Debug for HostedClient {
    /// By hand, so nothing about the key but its source can be printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedClient")
            .field("protocol", &self.protocol)
            .field("base", &self.base)
            .field("model", &self.model)
            .field("proxy", &self.proxy.as_ref().map(ToString::to_string))
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
    /// the process environment for it. The proxy is still read from the
    /// environment (`HTTPS_PROXY`, `NO_PROXY`), which for the loopback
    /// endpoints tests use is never consulted.
    pub fn with_key(
        protocol: Protocol,
        base: &str,
        model: &str,
        key_env: &str,
        key: Option<ApiKey>,
    ) -> Result<Self, RuntimeError> {
        Self::with_key_and_env(protocol, base, model, key_env, key, &|k| {
            std::env::var(k).ok()
        })
    }

    fn with_key_and_env(
        protocol: Protocol,
        base: &str,
        model: &str,
        key_env: &str,
        key: Option<ApiKey>,
        env: &dyn Fn(&str) -> Option<String>,
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
        let proxy = transport::proxy_for(&endpoint, env)?;
        Ok(Self {
            protocol,
            endpoint,
            proxy,
            base: base.to_string(),
            model: model.to_string(),
            key,
            thinking: Thinking::Off,
            timeout: Duration::from_secs(300),
            colibri: false,
        })
    }

    #[must_use]
    pub fn with_thinking(mut self, t: Thinking) -> Self {
        self.thinking = t;
        self
    }

    /// Sends colibri's request profile (ADR 0047 decision 5) when `on`: for
    /// an endpoint identified as colibri, which otherwise samples at 0.7 and
    /// reads an absent thinking setting by its own family default.
    #[must_use]
    pub fn with_colibri_profile(mut self, on: bool) -> Self {
        self.colibri = on;
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

    /// The request body for this client's protocol, model and thinking, with
    /// colibri's request profile when it is on.
    pub fn body(&self, conv: &Conversation, tools: &[NativeTool], max_tokens: u32) -> Value {
        body_for(
            self.protocol,
            &self.model,
            conv,
            tools,
            max_tokens,
            self.thinking,
            self.colibri,
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
        self.send_via(|| {
            transport::post(
                &self.endpoint,
                self.proxy.as_ref(),
                path,
                &headers,
                self.key.as_ref().map(|k| (auth, k)),
                &text,
                self.timeout,
            )
        })
        .await
    }

    /// The retry policy around `attempt`, which makes one request. Split
    /// from `send` so a test can make one attempt fail in a way a loopback
    /// socket cannot be made to on demand (an OS connect timeout).
    ///
    /// Every retry goes to the same endpoint through the same `attempt`, so
    /// where the key is sent does not change with the attempt number
    /// (ADR 0024 §3).
    async fn send_via<F, Fut>(&self, mut attempt_once: F) -> Result<Reply, RuntimeError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<transport::Response, SendError>>,
    {
        let mut attempt = 1;
        loop {
            let sent = attempt_once().await;
            let wait = match sent {
                Ok(r) if (200..300).contains(&r.status) => {
                    let v: Value = serde_json::from_str(&r.body).map_err(|e| {
                        RuntimeError::Shape(format!(
                            "response is not JSON: {e}; body: {}",
                            cut(&self.scrub(&r.body))
                        ))
                    })?;
                    let mut reply = parse_reply(self.protocol, &v)?;
                    reply.queue_wait_ms = queue_wait_ms(r.header("x-colibri-queue-wait-ms"));
                    return Ok(reply);
                }
                // Before the context-overflow arm: a 401 or 403 body may echo
                // a masked form of the key, and that arm quotes the body.
                Ok(r) if matches!(r.status, 401 | 403) => {
                    return Err(transport::status_error(&r, self.key.as_ref()));
                }
                Ok(r) if is_context_overflow(r.status, &r.body) => {
                    return Err(RuntimeError::ContextOverflow(cut(&self.scrub(&r.body))));
                }
                Ok(r) if retryable(r.status) && attempt < MAX_ATTEMPTS => {
                    let d = status_retry_delay(r.status, r.header("retry-after"), attempt);
                    // One event per wait, so a run idle for up to a minute
                    // says why (#613).
                    tracing::warn!(
                        status = r.status,
                        attempt,
                        wait_s = d.as_secs_f64(),
                        wait_ms = d.as_millis() as u64,
                        "provider asked for a retry"
                    );
                    d
                }
                // Retries spent: say so, as a timed-out connect does, so an
                // overload that outlasted every wait is not read as a
                // one-off refusal (#613).
                Ok(r) if retryable(r.status) => {
                    return Err(match transport::status_error(&r, self.key.as_ref()) {
                        RuntimeError::Http { status, body } => RuntimeError::Http {
                            status,
                            body: format!("{body} (after {MAX_ATTEMPTS} attempts)"),
                        },
                        other => other,
                    });
                }
                Ok(r) => {
                    return Err(transport::status_error(&r, self.key.as_ref()));
                }
                Err(SendError::Broken(e)) if attempt < MAX_ATTEMPTS => {
                    let d = retry_delay(None, attempt);
                    tracing::warn!(attempt, error = %e, "request failed mid-flight; retrying");
                    d
                }
                // #210: a run against a loopback stub ended on `os error
                // 10060` while the host was loaded, because every connect
                // failure was treated as a refusal. A handshake the OS timed
                // out gets the policy a broken request gets: at most
                // MAX_ATTEMPTS (4) attempts, 1, 2, 4 s apart. Each attempt's
                // connect is bounded by the OS and then by `self.timeout`, so
                // an endpoint that never answers now fails after up to four
                // OS connect timeouts and 7 s of waits, not one timeout.
                // `e` is the transport's message, the address and the OS
                // error: it holds no key and no request body.
                Err(SendError::ConnectTimedOut(e)) if attempt < MAX_ATTEMPTS => {
                    let d = retry_delay(None, attempt);
                    tracing::warn!(
                        attempt,
                        wait_ms = d.as_millis() as u64,
                        error = %e,
                        "connect timed out; retrying"
                    );
                    d
                }
                Err(SendError::ConnectTimedOut(e)) => {
                    return Err(RuntimeError::Connect {
                        base: self.base.clone(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("{e} (after {MAX_ATTEMPTS} attempts)"),
                        ),
                    });
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
            images: Vec::new(),
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
    fn an_image_rides_in_the_first_user_message_in_each_protocol_s_format() {
        let img = Image::from_bytes(b"GIF89a-pixels").unwrap();
        let data = img.base64().to_string();
        let mut c = conv();
        c.images = vec![img];
        let b = request_body(Protocol::Anthropic, "m", &c, &tools(), 100, Thinking::Off);
        let first = &b["messages"][0]["content"];
        assert_eq!(first[0]["type"], "image");
        assert_eq!(first[0]["source"]["type"], "base64");
        assert_eq!(first[0]["source"]["media_type"], "image/gif");
        assert_eq!(first[0]["source"]["data"], data.as_str());
        assert_eq!(first[1]["text"], "Task: fix it");
        let b = request_body(Protocol::OpenAiChat, "m", &c, &tools(), 100, Thinking::Off);
        let user = &b["messages"][1]["content"];
        assert_eq!(user[0]["text"], "Task: fix it");
        assert_eq!(user[1]["type"], "image_url");
        assert_eq!(
            user[1]["image_url"]["url"],
            format!("data:image/gif;base64,{data}").as_str()
        );
        // Without one, the user content is the plain string it always was.
        let b = request_body(
            Protocol::OpenAiChat,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::Off,
        );
        assert_eq!(b["messages"][1]["content"], "Task: fix it");
    }

    #[test]
    fn thinking_maps_to_each_protocol_s_own_field() {
        // Every Anthropic turn replayed as the provider sent it: a rebuilt
        // bare `tool_use` would make it refuse thinking (see below).
        let mut replayed = conv();
        replayed.exchanges[0].replay = Some(json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig"},
            {"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a.py"}}]}));
        let b = request_body(
            Protocol::Anthropic,
            "m",
            &replayed,
            &tools(),
            100,
            Thinking::High,
        );
        assert_eq!(b["thinking"]["type"], "adaptive");
        assert_eq!(b["output_config"]["effort"], "high");
        // A turn with no recorded reply: Anthropic answers a thinking
        // request whose tool_use has no thinking block with a 400, so the
        // request goes without thinking (ADR 0024 §4).
        let b = request_body(
            Protocol::Anthropic,
            "m",
            &conv(),
            &tools(),
            100,
            Thinking::High,
        );
        assert!(b.get("thinking").is_none() && b.get("output_config").is_none());
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
    fn config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else() {
        // The risk: a `[[providers]] protocol` or `[model] thinking` word
        // read as another, or an unknown one accepted instead of refused.
        for (w, p) in [
            ("openai", Protocol::OpenAiChat),
            ("OpenAI-Chat", Protocol::OpenAiChat),
            ("Anthropic", Protocol::Anthropic),
        ] {
            assert_eq!(Protocol::parse_config(w), Some(p), "{w}");
        }
        assert_eq!(Protocol::parse_config("gemini"), None);
        for (w, t) in [
            ("off", Thinking::Off),
            ("low", Thinking::Low),
            ("Medium", Thinking::Medium),
            ("HIGH", Thinking::High),
        ] {
            assert_eq!(Thinking::parse(w), Some(t), "{w}");
        }
        assert_eq!(Thinking::parse("max"), None);
    }

    #[test]
    fn each_protocol_names_its_own_key_variable_and_its_api() {
        // The variable is where the key is read from (ADR 0010, ADR 0024
        // §3); the wrong one reads another provider's key.
        assert_eq!(Protocol::OpenAiChat.default_key_env(), "OPENAI_API_KEY");
        assert_eq!(Protocol::Anthropic.default_key_env(), "ANTHROPIC_API_KEY");
        assert_eq!(
            Protocol::OpenAiChat.describe(),
            "OpenAI-compatible chat completions"
        );
        assert_eq!(Protocol::Anthropic.describe(), "Anthropic Messages");
    }

    #[test]
    fn a_client_reports_what_it_was_built_with_and_builds_its_body_from_it() {
        // `doctor` and the run banner print these; the key's source, never
        // the key.
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "gpt-x",
            "OPENAI_API_KEY",
            Some(ApiKey::for_test("sk-test-key-1", "OPENAI_API_KEY")),
        )
        .unwrap()
        .with_thinking(Thinking::Medium);
        assert_eq!(c.base(), "http://127.0.0.1:9/v1");
        assert_eq!(c.model(), "gpt-x");
        assert_eq!(c.thinking(), Thinking::Medium);
        assert_eq!(c.key_source(), Some("OPENAI_API_KEY"));
        assert_eq!(
            c.body(&conv(), &tools(), 100),
            request_body(
                Protocol::OpenAiChat,
                "gpt-x",
                &conv(),
                &tools(),
                100,
                Thinking::Medium
            )
        );
        let keyless = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "m",
            "K",
            None,
        )
        .unwrap();
        assert_eq!(keyless.key_source(), None);
    }

    fn colibri(protocol: Protocol, thinking: Thinking) -> HostedClient {
        HostedClient::with_key(protocol, "http://127.0.0.1:9/v1", "m", "K", None)
            .unwrap()
            .with_thinking(thinking)
            .with_colibri_profile(true)
    }

    /// `conv()` with its one turn replayed as the provider sent it, so
    /// Anthropic thinking is not withheld.
    fn replayed() -> Conversation {
        let mut c = conv();
        c.exchanges[0].replay = Some(json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"sig"},
            {"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a.py"}}]}));
        c
    }

    #[test]
    fn colibri_profile_openai_with_thinking_off_sends_temperature_0_and_enable_thinking_false() {
        // The risk: colibri samples at 0.7 with no `temperature`, and its
        // Qwen3.8 reads an absent thinking setting as `xhigh` (ADR 0047 d5).
        let b = colibri(Protocol::OpenAiChat, Thinking::Off).body(&conv(), &tools(), 100);
        assert_eq!(b["temperature"], json!(0));
        assert_eq!(b["enable_thinking"], json!(false));
        assert!(b.get("reasoning_effort").is_none());
    }

    #[test]
    fn colibri_profile_openai_with_thinking_on_sends_the_effort_and_no_enable_thinking() {
        let b = colibri(Protocol::OpenAiChat, Thinking::Low).body(&conv(), &tools(), 100);
        assert_eq!(b["temperature"], json!(0));
        assert_eq!(b["reasoning_effort"], "low");
        assert!(b.get("enable_thinking").is_none());
    }

    #[test]
    fn colibri_profile_anthropic_with_thinking_off_sends_thinking_disabled() {
        let b = colibri(Protocol::Anthropic, Thinking::Off).body(&replayed(), &tools(), 100);
        assert_eq!(b["temperature"], json!(0));
        assert_eq!(b["thinking"], json!({"type": "disabled"}));
        assert!(b.get("output_config").is_none());
    }

    #[test]
    fn colibri_profile_anthropic_with_thinking_on_sends_thinking_enabled_not_adaptive() {
        // colibri thinks on the Anthropic path only for `enabled`; the
        // `adaptive` sent to Anthropic itself would read as off.
        let b = colibri(Protocol::Anthropic, Thinking::High).body(&replayed(), &tools(), 100);
        assert_eq!(b["temperature"], json!(0));
        assert_eq!(b["thinking"], json!({"type": "enabled"}));
        assert!(b.get("output_config").is_none());
    }

    #[test]
    fn colibri_profile_anthropic_with_thinking_withheld_sends_thinking_disabled() {
        // A turn with no recorded reply withholds thinking (ADR 0024 §4);
        // with the profile that is stated, not left to colibri's default.
        let b = colibri(Protocol::Anthropic, Thinking::High).body(&conv(), &tools(), 100);
        assert_eq!(b["temperature"], json!(0));
        assert_eq!(b["thinking"], json!({"type": "disabled"}));
        assert!(b.get("output_config").is_none());
    }

    #[test]
    fn colibri_profile_off_leaves_every_other_body_byte_for_byte() {
        // The risk: the profile leaking into bodies for hosted providers,
        // whose reasoning models reject a `temperature`. The literals are
        // the bodies as sent before the profile existed.
        let openai = concat!(
            r#"{"model":"m","messages":[{"role":"system","content":"SYSTEM"},"#,
            r#"{"role":"user","content":"Task: fix it"},{"role":"assistant","content":null,"#,
            r#""tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","#,
            r#""arguments":"{\"path\":\"a.py\"}"}}]},"#,
            r#"{"role":"tool","tool_call_id":"c1","content":"file text"},"#,
            r#"{"role":"user","content":"last step"}],"#,
            r#""tools":[{"type":"function","function":{"name":"read_file","#,
            r#""description":"Read a file","parameters":{"type":"object","#,
            r#""properties":{"path":{"type":"string"}},"required":["path"]}}}],"#,
            r#""tool_choice":"required","parallel_tool_calls":false,"max_completion_tokens":100"#,
        );
        let anthropic = concat!(
            r#"{"model":"m","max_tokens":100,"system":"SYSTEM","messages":["#,
            r#"{"role":"user","content":[{"type":"text","text":"Task: fix it"}]},"#,
            r#"{"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"sig"},"#,
            r#"{"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a.py"}}]},"#,
            r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"file text"},"#,
            r#"{"type":"text","text":"last step"}]}],"#,
            r#""tools":[{"name":"read_file","description":"Read a file","input_schema":{"#,
            r#""type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}],"#,
            r#""tool_choice":{"type":"auto","disable_parallel_tool_use":true}"#,
        );
        let cases = [
            (
                Protocol::OpenAiChat,
                Thinking::Off,
                conv(),
                format!("{openai}}}"),
            ),
            (
                Protocol::OpenAiChat,
                Thinking::High,
                conv(),
                format!(r#"{openai},"reasoning_effort":"high"}}"#),
            ),
            (
                Protocol::Anthropic,
                Thinking::Off,
                replayed(),
                format!("{anthropic}}}"),
            ),
            (
                Protocol::Anthropic,
                Thinking::High,
                replayed(),
                format!(
                    r#"{anthropic},"thinking":{{"type":"adaptive"}},"output_config":{{"effort":"high"}}}}"#
                ),
            ),
        ];
        for (protocol, thinking, conv, golden) in cases {
            for c in [
                HostedClient::with_key(protocol, "http://127.0.0.1:9/v1", "m", "K", None)
                    .unwrap()
                    .with_thinking(thinking),
                colibri(protocol, thinking).with_colibri_profile(false),
            ] {
                assert_eq!(
                    c.body(&conv, &tools(), 100).to_string(),
                    golden,
                    "{protocol:?} {thinking:?}"
                );
            }
        }
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
        assert_eq!(r.tokens, Some(9));
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
        assert_eq!(r.tokens, Some(5));
        assert_eq!(r.call.unwrap().name, "read_file");
        assert_eq!(r.replay["content"][0]["text"], "I think");

        // Prose instead of a call is a format failure, retried by the adapter.
        let v = json!({"content":[{"type":"text","text":"Sure!"}],"stop_reason":"end_turn"});
        let e = parse_reply(Protocol::Anthropic, &v)
            .unwrap()
            .call
            .unwrap_err();
        assert!(e.contains("called no tool") && e.contains("Sure!"), "{e}");

        // Why there is no call, when the provider says: a reply cut at the
        // token limit and a refusal each need a different fix from the user.
        for (stop, says) in [
            ("max_tokens", "stopped at max_tokens"),
            ("refusal", "provider refused"),
        ] {
            let v = json!({"content":[{"type":"text","text":"Sure!"}],"stop_reason":stop});
            let e = parse_reply(Protocol::Anthropic, &v)
                .unwrap()
                .call
                .unwrap_err();
            assert!(e.contains(says) && e.contains("Sure!"), "{stop}: {e}");
        }
    }

    /// An openai-chat reply with one call whose arguments are `raw`, or with
    /// no call when `raw` is `None`, and `finish` as its finish_reason.
    fn openai_reply(raw: Option<&str>, finish: Option<&str>) -> Value {
        let mut msg = json!({"content":"Sure!"});
        if let Some(raw) = raw {
            msg["tool_calls"] = json!([
                {"id":"x","type":"function","function":{"name":"read_file","arguments":raw}}]);
        }
        let mut choice = json!({ "message": msg });
        if let Some(f) = finish {
            choice["finish_reason"] = json!(f);
        }
        json!({ "choices": [choice] })
    }

    #[test]
    fn a_no_call_reply_cut_at_its_length_limit_says_so() {
        // colibri cuts a reply at the room the prompt leaves; a call cut off
        // arrives as no call, and the fix is the context window, not format.
        let e = parse_reply(Protocol::OpenAiChat, &openai_reply(None, Some("length")))
            .unwrap()
            .call
            .unwrap_err();
        assert!(
            e.starts_with("the reply stopped at its length limit before a tool call")
                && e.contains("Sure!"),
            "{e}"
        );
    }

    #[test]
    fn a_call_cut_at_its_length_limit_says_so_and_names_the_tool() {
        // A server that returns the partial call: the arguments are not JSON
        // because they were cut, not because the model formatted them badly.
        let v = openai_reply(Some("{\"path\":\"a.p"), Some("length"));
        let e = parse_reply(Protocol::OpenAiChat, &v)
            .unwrap()
            .call
            .unwrap_err();
        assert!(
            e.starts_with("the reply stopped at its length limit inside a tool call")
                && e.contains("`read_file`"),
            "{e}"
        );
    }

    #[test]
    fn a_whole_call_at_its_length_limit_is_still_accepted() {
        // The limit can land just after a complete call; that call stands.
        let v = openai_reply(Some("{\"path\":\"a.py\"}"), Some("length"));
        let call = parse_reply(Protocol::OpenAiChat, &v).unwrap().call.unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args, json!({"path":"a.py"}));
    }

    #[test]
    fn only_finish_reason_length_is_called_a_length_limit() {
        // Any other reason, or none, keeps the format-failure message.
        for finish in [None, Some("stop"), Some("tool_calls")] {
            let e = parse_reply(Protocol::OpenAiChat, &openai_reply(None, finish))
                .unwrap()
                .call
                .unwrap_err();
            assert!(e.starts_with("the reply called no tool"), "{finish:?}: {e}");
            let v = openai_reply(Some("{\"path\":"), finish);
            let e = parse_reply(Protocol::OpenAiChat, &v)
                .unwrap()
                .call
                .unwrap_err();
            assert!(
                e.starts_with("the arguments of `read_file` are not JSON"),
                "{finish:?}: {e}"
            );
        }
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
    fn openai_chat_prompt_tokens_are_read_from_usage() {
        // Recorded shape: OpenAI's `prompt_tokens` already includes the
        // cached part, which `prompt_tokens_details` breaks out.
        let v = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"done","arguments":"{}"}}]}}],
            "usage":{"prompt_tokens":1200,"completion_tokens":9,"total_tokens":1209,
                "prompt_tokens_details":{"cached_tokens":1024}}});
        let r = parse_reply(Protocol::OpenAiChat, &v).unwrap();
        assert_eq!((r.prompt_tokens, r.tokens), (Some(1200), Some(9)));
    }

    #[test]
    fn anthropic_prompt_tokens_include_cache_creation_and_cache_read() {
        // The risk: reading `input_tokens` alone reports only the uncached
        // tail of a mostly-cached prompt, a fraction of what OpenAI's figure
        // would say for the same request.
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}],
            "usage":{"input_tokens":12,"cache_creation_input_tokens":300,
                "cache_read_input_tokens":4000,"output_tokens":5}});
        let r = parse_reply(Protocol::Anthropic, &v).unwrap();
        assert_eq!((r.prompt_tokens, r.tokens), (Some(4312), Some(5)));

        // An absent cache field is 0 beside a present `input_tokens`.
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}],
            "usage":{"input_tokens":12,"output_tokens":5}});
        assert_eq!(
            parse_reply(Protocol::Anthropic, &v).unwrap().prompt_tokens,
            Some(12)
        );
    }

    #[test]
    fn anthropic_prompt_tokens_without_input_tokens_are_unknown_not_the_cache_part() {
        // The cache fields alone are part of the prompt; summing them would
        // present that part as the whole.
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}],
            "usage":{"cache_creation_input_tokens":300,"cache_read_input_tokens":4000,
                "output_tokens":5}});
        let r = parse_reply(Protocol::Anthropic, &v).unwrap();
        assert_eq!((r.prompt_tokens, r.tokens), (None, Some(5)));
    }

    #[test]
    fn a_reply_without_usage_has_unknown_prompt_tokens_not_zero() {
        let v = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"done","arguments":"{}"}}]}}],
            "usage":{"completion_tokens":9}});
        assert_eq!(
            parse_reply(Protocol::OpenAiChat, &v).unwrap().prompt_tokens,
            None
        );
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}]});
        assert_eq!(
            parse_reply(Protocol::Anthropic, &v).unwrap().prompt_tokens,
            None
        );
        // A reported zero is a figure, and stays one.
        let v = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"done","arguments":"{}"}}]}}],
            "usage":{"prompt_tokens":0}});
        assert_eq!(
            parse_reply(Protocol::OpenAiChat, &v).unwrap().prompt_tokens,
            Some(0)
        );
    }

    #[test]
    fn a_reply_without_usage_has_an_unknown_token_count_not_zero() {
        let v = json!({"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"done","arguments":"{}"}}]}}]});
        assert_eq!(parse_reply(Protocol::OpenAiChat, &v).unwrap().tokens, None);
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}]});
        assert_eq!(parse_reply(Protocol::Anthropic, &v).unwrap().tokens, None);
        // A reported zero is a figure, and stays one.
        let v = json!({"content":[{"type":"tool_use","id":"t","name":"done","input":{}}],
            "usage":{"output_tokens":0}});
        assert_eq!(
            parse_reply(Protocol::Anthropic, &v).unwrap().tokens,
            Some(0)
        );
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
    fn a_refused_proxy_fails_at_construction_before_any_request() {
        // The risk: a proxy URL with credentials refused only on the first
        // turn, after the run's snapshot, instead of before it.
        // Built at run time so no `user:pass@` literal reaches a secret
        // scanner (#185).
        let pw = "p";
        let env =
            |k: &str| (k == "HTTPS_PROXY").then(|| format!("http://u:{pw}@proxy.example:3128"));
        let e = HostedClient::with_key_and_env(
            Protocol::OpenAiChat,
            "https://api.openai.com/v1",
            "m",
            "OPENAI_API_KEY",
            Some(ApiKey::for_test("sk-x", "OPENAI_API_KEY")),
            &env,
        )
        .unwrap_err()
        .to_string();
        if cfg!(feature = "hosted") {
            assert!(e.contains("credentials"), "{e}");
        }
        // A local endpoint never reads it.
        assert!(
            HostedClient::with_key_and_env(
                Protocol::OpenAiChat,
                "http://127.0.0.1:9/v1",
                "m",
                "OPENAI_API_KEY",
                None,
                &env,
            )
            .is_ok()
        );
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
            // With a Retry-After of its own: a bare 529 now waits 15 s (#613),
            // past this stub's accept timeout. That schedule is held by
            // an_overloaded_529_or_503_without_retry_after_waits_15_30_60_s.
            ("529 Overloaded", "Retry-After: 0\r\n", "{}"),
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
        match &e {
            RuntimeError::Http { status: 503, body } => {
                assert_eq!(body, "down (after 4 attempts)");
            }
            other => panic!("expected the 503, got {other:?}"),
        }
        assert_eq!(h.await.unwrap().len(), MAX_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn a_status_not_worth_retrying_is_reported_at_once_with_the_key_scrubbed() {
        // The risk: a 404 retried as if the provider were overloaded, and a
        // body that echoes the key quoted in the error (ADR 0010).
        let (base, h) = stub(vec![(
            "404 Not Found",
            "",
            "no model `m` for key sk-test-key-1, sorry",
        )])
        .await;
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            &base,
            "m",
            "K",
            Some(ApiKey::for_test("sk-test-key-1", "K")),
        )
        .unwrap();
        match c.send(&json!({})).await {
            Err(RuntimeError::Http { status: 404, body }) => {
                assert_eq!(body, "no model `m` for key <key from K>, sorry");
            }
            other => panic!("expected the 404, got {other:?}"),
        }
        assert_eq!(h.await.unwrap().len(), 1);
    }

    const OPENAI_OK: &str = r#"{"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"c","type":"function",
        "function":{"name":"done","arguments":"{}"}}]}}]}"#;

    /// What `send` puts in `queue_wait_ms` for each of `extras`, the extra
    /// headers of a `200 OK` the stub answers each request with in turn.
    async fn queue_waits(extras: Vec<&'static str>) -> Vec<Option<u64>> {
        let n = extras.len();
        let (base, h) = stub(
            extras
                .into_iter()
                .map(|e| ("200 OK", e, OPENAI_OK))
                .collect(),
        )
        .await;
        let c = HostedClient::with_key(Protocol::OpenAiChat, &base, "m", "K", None).unwrap();
        let mut got = Vec::new();
        for _ in 0..n {
            let r = c.send(&json!({})).await.expect("a 200 with a call");
            got.push(r.queue_wait_ms);
        }
        assert_eq!(h.await.unwrap().len(), n);
        got
    }

    #[tokio::test]
    async fn colibri_queue_wait_header_is_read() {
        // The risk (ADR 0047 decision 8): on a colibri server shared with
        // other clients, time spent queued for admission is indistinguishable
        // from time spent generating unless the reply carries the figure.
        // Header names are case-insensitive (RFC 9110 §5.1).
        assert_eq!(
            queue_waits(vec![
                "x-colibri-queue-wait-ms: 1234\r\n",
                "X-Colibri-Queue-Wait-Ms: 0\r\n",
                "X-COLIBRI-QUEUE-WAIT-MS:  42 \r\n",
            ])
            .await,
            vec![Some(1234), Some(0), Some(42)]
        );
    }

    #[tokio::test]
    async fn colibri_queue_wait_header_absent_or_malformed_is_none() {
        // The risk: the header is server input. A value that is not a plain
        // non-negative integer must read as "not reported", never as a
        // number it does not say, and never fail a reply that is otherwise
        // good.
        assert_eq!(
            queue_waits(vec![
                "",
                "x-colibri-queue-wait-ms: -5\r\n",
                "x-colibri-queue-wait-ms: +5\r\n",
                "x-colibri-queue-wait-ms: 1.5\r\n",
                "x-colibri-queue-wait-ms: 12ms\r\n",
                "x-colibri-queue-wait-ms: abc\r\n",
                "x-colibri-queue-wait-ms: \r\n",
                "x-colibri-queue-wait-ms: 99999999999999999999999\r\n",
            ])
            .await,
            vec![None; 8]
        );
    }

    /// `c.send_via` where attempt `n` (from 1) fails with `fail(n)` when that
    /// returns one, and otherwise makes the real request `send` makes.
    /// Returns the result and how many attempts were made.
    async fn send_failing(
        c: &HostedClient,
        fail: impl Fn(u32) -> Option<SendError>,
    ) -> (Result<Reply, RuntimeError>, u32) {
        let text = json!({}).to_string();
        let n = std::cell::Cell::new(0u32);
        let r = c
            .send_via(|| {
                n.set(n.get() + 1);
                let injected = fail(n.get());
                let text = &text;
                async move {
                    match injected {
                        Some(e) => Err(e),
                        None => {
                            transport::post(
                                &c.endpoint,
                                None,
                                "/chat/completions",
                                &[],
                                None,
                                text,
                                c.timeout,
                            )
                            .await
                        }
                    }
                }
            })
            .await;
        (r, n.get())
    }

    fn timed_out() -> SendError {
        SendError::ConnectTimedOut(
            "cannot reach 127.0.0.1:9: A connection attempt failed (os error 10060)".into(),
        )
    }

    #[tokio::test]
    async fn connect_retry_a_timed_out_connect_is_retried_and_the_second_attempt_succeeds() {
        // The risk (#210): an OS connect timeout on a loaded host was
        // treated as a refusal, so one slow handshake ended the whole run.
        let ok = r#"{"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"c","type":"function",
            "function":{"name":"done","arguments":"{}"}}]}}]}"#;
        let (base, h) = stub(vec![("200 OK", "", ok)]).await;
        let c = HostedClient::with_key(Protocol::OpenAiChat, &base, "m", "K", None).unwrap();
        let (r, attempts) = send_failing(&c, |n| (n == 1).then(timed_out)).await;
        assert_eq!(
            r.expect("the second attempt succeeds").call.unwrap().name,
            "done"
        );
        assert_eq!(attempts, 2);
        assert_eq!(
            h.await.unwrap().len(),
            1,
            "one connection reached the endpoint"
        );
    }

    #[tokio::test]
    async fn connect_retry_stops_at_the_cap_and_says_how_many_attempts() {
        // The risk: an endpoint that never answers retried without bound.
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "m",
            "K",
            None,
        )
        .unwrap();
        // Real waits (tokio's paused clock needs its `test-util` feature,
        // which this workspace does not enable): 7 s.
        let started = std::time::Instant::now();
        let (r, attempts) = send_failing(&c, |_| Some(timed_out())).await;
        assert_eq!(attempts, MAX_ATTEMPTS);
        // 1, 2 and 4 s between the four attempts.
        assert!(
            started.elapsed() >= Duration::from_secs(7),
            "{:?}",
            started.elapsed()
        );
        match r {
            Err(RuntimeError::Connect { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::TimedOut);
                let m = source.to_string();
                assert!(m.contains("10060") && m.contains("after 4 attempts"), "{m}");
            }
            other => panic!("expected a connect error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further() {
        // The risk: a dropped connection either ending the run on its first
        // occurrence or retried without bound. Attempts past the cap would
        // make a real request to a closed port, so a fifth shows in the
        // count rather than as a hang. Real waits: 7 s, as above.
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "m",
            "K",
            None,
        )
        .unwrap();
        let (r, attempts) = send_failing(&c, |n| {
            (n <= MAX_ATTEMPTS).then(|| SendError::Broken("connection reset".into()))
        })
        .await;
        assert_eq!(attempts, MAX_ATTEMPTS);
        match r {
            Err(RuntimeError::Connect { source, .. }) => {
                assert!(source.to_string().contains("connection reset"), "{source}");
            }
            other => panic!("expected a connect error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_retry_does_not_apply_to_a_refused_connection() {
        // The risk: retrying a refusal, which is nothing listening and only
        // repeats, adding 7 s to every mistyped port (ADR 0024 §4).
        let c = HostedClient::with_key(
            Protocol::OpenAiChat,
            "http://127.0.0.1:9/v1",
            "m",
            "K",
            None,
        )
        .unwrap();
        let (r, attempts) = send_failing(&c, |_| {
            Some(SendError::Connect(
                "cannot reach 127.0.0.1:9: refused".into(),
            ))
        })
        .await;
        assert_eq!(attempts, 1);
        assert!(matches!(r, Err(RuntimeError::Connect { .. })), "{r:?}");
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
